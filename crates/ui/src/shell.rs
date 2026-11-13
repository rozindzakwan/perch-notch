//! The window shell: one transparent pill window plus one invisible hot-zone
//! window, both created through `winit` and then given their OS-specific style
//! bits by the platform crates.
//!
//! Hover detection is entirely event-driven: the hot-zone window is 1-2 px tall
//! and reports `CursorEntered` / `CursorLeft`. Nothing here polls the cursor.

use std::num::NonZeroU32;
use std::rc::Rc;

use perch_core::error::{Error, Result};
use perch_core::geometry::Rect;
use softbuffer::{Context, Surface};
use tiny_skia::Pixmap;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId, WindowLevel};

/// What the shell reports upwards; the app maps these onto notch events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellEvent {
    /// Cursor entered the hover strip.
    HotZoneEntered,
    /// Cursor left the hover strip.
    HotZoneLeft,
    /// Cursor entered the pill itself.
    PillEntered,
    /// Cursor left the pill.
    PillLeft,
    /// The pill was clicked at a position relative to its top-left, in physical
    /// pixels.
    PillClicked(i32, i32),
    /// The pill needs repainting.
    Redraw,
}

/// The pair of windows that make up the notch.
pub struct NotchWindows {
    pill: Rc<Window>,
    hot: Rc<Window>,
    surface: Surface<Rc<Window>, Rc<Window>>,
    pill_id: WindowId,
    hot_id: WindowId,
    /// No compositor (X11 without ARGB compositing): paint opaque and clip
    /// the window to the pill shape instead.
    opaque: bool,
}

fn pill_attributes(rect: Rect) -> winit::window::WindowAttributes {
    let attrs = Window::default_attributes()
        .with_title("Perch")
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_window_level(WindowLevel::AlwaysOnTop)
        .with_visible(false)
        .with_inner_size(PhysicalSize::new(rect.w.max(1), rect.h.max(1)))
        .with_position(PhysicalPosition::new(rect.x, rect.y));
    #[cfg(windows)]
    let attrs = {
        use winit::platform::windows::WindowAttributesExtWindows;
        attrs
            .with_skip_taskbar(true)
            .with_undecorated_shadow(false)
            .with_class_name("PerchNotch")
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let attrs = {
        use winit::platform::x11::WindowAttributesExtX11;
        // DOCK keeps Muffin from giving us decorations or a task-list entry,
        // and we never set a strut, so windows are not pushed around.
        attrs
            .with_x11_window_type(vec![winit::platform::x11::WindowType::Dock])
            .with_x11_screen(0)
    };
    attrs
}

fn hot_attributes(rect: Rect) -> winit::window::WindowAttributes {
    let attrs = Window::default_attributes()
        .with_title("Perch hot zone")
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_window_level(WindowLevel::AlwaysOnTop)
        .with_visible(false)
        .with_inner_size(PhysicalSize::new(rect.w.max(1), rect.h.max(1)))
        .with_position(PhysicalPosition::new(rect.x, rect.y));
    #[cfg(windows)]
    let attrs = {
        use winit::platform::windows::WindowAttributesExtWindows;
        attrs
            .with_skip_taskbar(true)
            .with_undecorated_shadow(false)
            .with_class_name("PerchHotZone")
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let attrs = {
        use winit::platform::x11::WindowAttributesExtX11;
        attrs.with_x11_window_type(vec![winit::platform::x11::WindowType::Dock])
    };
    attrs
}

impl NotchWindows {
    /// Create both windows and apply the OS style bits.
    pub fn create(el: &ActiveEventLoop, pill_rect: Rect, hot_rect: Rect) -> Result<Self> {
        let pill = el
            .create_window(pill_attributes(pill_rect))
            .map_err(|e| Error::backend(format!("creating the pill window: {e}")))?;
        let pill = Rc::new(pill);

        let ctx = Context::new(pill.clone())
            .map_err(|e| Error::backend(format!("softbuffer context: {e}")))?;
        let surface = Surface::new(&ctx, pill.clone())
            .map_err(|e| Error::backend(format!("softbuffer surface: {e}")))?;

        let hot = el
            .create_window(hot_attributes(hot_rect))
            .map_err(|e| Error::backend(format!("creating the hot-zone window: {e}")))?;
        let hot = Rc::new(hot);

        let opaque = Self::detect_opaque();
        let me = Self {
            pill_id: pill.id(),
            hot_id: hot.id(),
            pill,
            hot,
            surface,
            opaque,
        };
        me.apply_platform_styles()?;
        Ok(me)
    }

    #[cfg(windows)]
    fn apply_platform_styles(&self) -> Result<()> {
        use perch_platform_windows::window as win;
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let raw = |w: &Window| -> Result<isize> {
            let handle = w
                .window_handle()
                .map_err(|e| Error::backend(format!("window handle: {e}")))?;
            match handle.as_raw() {
                RawWindowHandle::Win32(h) => Ok(isize::from(h.hwnd)),
                other => Err(Error::backend(format!("unexpected handle: {other:?}"))),
            }
        };

        let pill = win::hwnd_from_raw(raw(&self.pill)?);
        let before = win::ex_style(pill);
        let style = win::apply_notch_styles(pill)?;
        log::debug!("pill ex-style 0x{before:x} -> 0x{style:x}");
        if !win::has_notch_styles(style) {
            return Err(Error::backend(format!(
                "pill window did not take the notch styles (ex-style 0x{style:x})"
            )));
        }

        let hot = win::hwnd_from_raw(raw(&self.hot)?);
        win::apply_notch_styles(hot)?;
        // NOTE: the hover strip is deliberately *not* WS_EX_TRANSPARENT.
        // Click-through and hover detection are mutually exclusive on Windows:
        // a transparent window is skipped by hit-testing, so it never receives
        // WM_MOUSEMOVE and therefore never produces CursorEntered. Since the
        // strip is 1-2 physical pixels on the screen edge, the cost is that a
        // click landing exactly on that line is swallowed; the benefit is
        // event-driven hover with no cursor polling anywhere. (An earlier
        // version applied WS_EX_TRANSPARENT and appeared to work only because
        // winit's set_visible was silently resetting the style.)
        Ok(())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn apply_platform_styles(&self) -> Result<()> {
        use perch_platform_linux::x11;
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let raw = |w: &Window| -> Option<u32> {
            let handle = w.window_handle().ok()?;
            match handle.as_raw() {
                RawWindowHandle::Xlib(h) => Some(h.window as u32),
                RawWindowHandle::Xcb(h) => Some(h.window.get()),
                _ => None,
            }
        };
        match (raw(&self.pill), raw(&self.hot)) {
            (Some(pill), Some(hot)) => x11::apply_notch_properties(pill, hot),
            _ => {
                // Wayland: no X11 properties to set. The layer-shell path is a
                // documented best-effort limitation.
                log::info!("not an X11 session; skipping X11 window properties");
                Ok(())
            }
        }
    }

    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    fn apply_platform_styles(&self) -> Result<()> {
        Ok(())
    }

    /// The pill window's id.
    pub fn pill_id(&self) -> WindowId {
        self.pill_id
    }

    /// The hot-zone window's id.
    pub fn hot_id(&self) -> WindowId {
        self.hot_id
    }

    /// Classify a `winit` window id.
    pub fn classify(&self, id: WindowId) -> Option<ShellEvent> {
        if id == self.pill_id {
            Some(ShellEvent::Redraw)
        } else if id == self.hot_id {
            Some(ShellEvent::HotZoneEntered)
        } else {
            None
        }
    }

    /// Move/resize the pill without activating it.
    ///
    /// On Windows the OS calls come *after* the `winit` calls on purpose:
    /// `set_visible` and `request_inner_size` rewrite the window's extended
    /// style from `winit`'s own cached value, which drops the notch style bits
    /// we added behind its back. Re-asserting them here is what keeps the pill
    /// out of the taskbar and Alt-Tab for the whole session, not just at
    /// startup. (Found by scripts/smoke-windows.ps1, which reads the live
    /// ex-style back out of the running process.)
    pub fn place_pill(&self, rect: Rect, visible: bool) -> Result<()> {
        self.pill
            .set_outer_position(PhysicalPosition::new(rect.x, rect.y));
        let _ = self
            .pill
            .request_inner_size(PhysicalSize::new(rect.w.max(1), rect.h.max(1)));
        self.pill.set_visible(visible);
        #[cfg(windows)]
        {
            use perch_platform_windows::window as win;
            let hwnd = win::hwnd_from_raw(self.raw_pill()?);
            win::apply_notch_styles(hwnd)?;
            // SetWindowPos with NOACTIVATE is the only way to be sure the move
            // does not steal focus.
            win::place_topmost(hwnd, rect, visible)?;
        }
        Ok(())
    }

    /// Move/resize the hover strip.
    pub fn place_hot_zone(&self, rect: Rect, visible: bool) -> Result<()> {
        self.hot
            .set_outer_position(PhysicalPosition::new(rect.x, rect.y));
        let _ = self
            .hot
            .request_inner_size(PhysicalSize::new(rect.w.max(1), rect.h.max(1)));
        self.hot.set_visible(visible);
        #[cfg(windows)]
        {
            use perch_platform_windows::window as win;
            let hwnd = win::hwnd_from_raw(self.raw_hot()?);
            // Deliberately NOT click-through: see `create`'s note.
            win::apply_notch_styles(hwnd)?;
            win::place_topmost(hwnd, rect, visible)?;
        }
        Ok(())
    }

    /// The pill's raw window handle as an integer, for the platform helpers.
    #[cfg(windows)]
    fn raw_pill(&self) -> Result<isize> {
        Self::raw_handle(&self.pill)
    }

    /// The hover strip's raw window handle as an integer.
    #[cfg(windows)]
    fn raw_hot(&self) -> Result<isize> {
        Self::raw_handle(&self.hot)
    }

    #[cfg(windows)]
    fn raw_handle(win: &Window) -> Result<isize> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = win
            .window_handle()
            .map_err(|e| Error::backend(format!("window handle: {e}")))?;
        match handle.as_raw() {
            RawWindowHandle::Win32(h) => Ok(isize::from(h.hwnd)),
            other => Err(Error::backend(format!("unexpected handle: {other:?}"))),
        }
    }

    /// Live extended style of the pill window, so the smoke test and the
    /// `--verbose` log can prove the style bits really are applied.
    #[cfg(windows)]
    pub fn pill_ex_style(&self) -> isize {
        match self.raw_pill() {
            Ok(raw) => perch_platform_windows::window::ex_style(
                perch_platform_windows::window::hwnd_from_raw(raw),
            ),
            Err(_) => 0,
        }
    }

    /// Ask for a repaint.
    pub fn request_redraw(&self) {
        self.pill.request_redraw();
    }

    /// Current pill size in physical pixels.
    pub fn pill_size(&self) -> (u32, u32) {
        let s = self.pill.inner_size();
        (s.width, s.height)
    }

    /// Present a pixmap into the pill window.
    pub fn present(&mut self, pm: &Pixmap) -> Result<()> {
        let (w, h) = (pm.width(), pm.height());
        let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else {
            return Ok(());
        };
        self.surface
            .resize(nw, nh)
            .map_err(|e| Error::backend(format!("surface resize: {e}")))?;
        let mut buffer = self
            .surface
            .buffer_mut()
            .map_err(|e| Error::backend(format!("surface buffer: {e}")))?;
        for (dst, src) in buffer.iter_mut().zip(pm.pixels()) {
            // softbuffer wants 0xAARRGGBB with premultiplied alpha, which is
            // exactly how tiny-skia stores pixels.
            *dst = (u32::from(src.alpha()) << 24)
                | (u32::from(src.red()) << 16)
                | (u32::from(src.green()) << 8)
                | u32::from(src.blue());
        }
        buffer
            .present()
            .map_err(|e| Error::backend(format!("surface present: {e}")))
    }
}

impl NotchWindows {
    /// Whether frames must be painted opaque (no compositor).
    pub fn needs_opaque(&self) -> bool {
        self.opaque
    }
}

impl NotchWindows {
    #[cfg(all(unix, not(target_os = "macos")))]
    fn detect_opaque() -> bool {
        // ARGB transparency needs a compositing manager (Muffin is one). Without
        // it the transparent corners would render black, so paint opaque and
        // clip the window to the pill shape with XShape instead.
        !perch_platform_linux::x11::compositor_running()
    }

    #[cfg(not(all(unix, not(target_os = "macos"))))]
    fn detect_opaque() -> bool {
        false
    }
}

impl NotchWindows {
    /// Clip the pill window to its rounded shape. Only needed (and only does
    /// anything) on X11 without a compositor, where transparency is not
    /// available; elsewhere the alpha channel already gives round corners.
    pub fn clip_to_pill(&self, w: u32, h: u32, radius: u32) {
        if !self.opaque {
            return;
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let id = match self.pill.window_handle().map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Xlib(h)) => Some(h.window as u32),
                Ok(RawWindowHandle::Xcb(h)) => Some(h.window.get()),
                _ => None,
            };
            if let Some(id) = id {
                if let Err(e) = perch_platform_linux::x11::set_rounded_shape(id, w, h, radius) {
                    log::debug!("XShape clip failed: {e}");
                }
            }
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let _ = (w, h, radius);
    }
}

impl NotchWindows {
    /// The X11 window ids of the pill and the hover strip, when running on X11.
    pub fn x11_ids(&self) -> Option<(u32, u32)> {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let id = |w: &Window| match w.window_handle().map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Xlib(h)) => Some(h.window as u32),
                Ok(RawWindowHandle::Xcb(h)) => Some(h.window.get()),
                _ => None,
            };
            Some((id(&self.pill)?, id(&self.hot)?))
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        None
    }
}
