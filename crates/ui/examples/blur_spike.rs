//! Throwaway spike for ADR-018: can a borderless, per-pixel-transparent
//! winit + softbuffer window get the Windows 11 acrylic backdrop, confined to
//! its rounded shape, without idle CPU?
//!
//! It opens two windows of its own: an opaque checkerboard (something sharp
//! to blur) and, over it, the "notch" window drawing a translucent rounded
//! rectangle. Screenshots for the ADR only ever contain these two windows.
//!
//! Usage: `cargo run -p perch-ui --example blur_spike -- <none|backdrop|backdrop-rgn> [seconds]`
//! It prints both windows' physical rects, then exits after `seconds` (8).

#[cfg(not(windows))]
fn main() {
    eprintln!("the blur spike is Windows-only");
}

#[cfg(windows)]
fn main() {
    spike::run();
}

#[cfg(windows)]
mod spike {
    use std::num::NonZeroU32;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use softbuffer::{Context, Surface};
    use tiny_skia::{Color, FillRule, Paint, Pixmap, Transform};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_TRANSIENTWINDOW,
        DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    };
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};
    use windows::Win32::UI::Controls::MARGINS;
    use winit::application::ApplicationHandler;
    use winit::dpi::{PhysicalPosition, PhysicalSize};
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::window::{Window, WindowId, WindowLevel};

    const CHECKER: (i32, i32, u32, u32) = (200, 200, 560, 440);
    const GLASS: (i32, i32, u32, u32) = (290, 270, 380, 300);
    const RADIUS: f32 = 26.0;

    struct Win {
        window: Rc<Window>,
        surface: Surface<Rc<Window>, Rc<Window>>,
        glass: bool,
    }

    struct App {
        mode: String,
        until: Instant,
        wins: Vec<Win>,
    }

    fn hwnd(w: &Window) -> Option<HWND> {
        match w.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(HWND(isize::from(h.hwnd) as *mut std::ffi::c_void)),
            _ => None,
        }
    }

    fn open(el: &ActiveEventLoop, rect: (i32, i32, u32, u32), glass: bool) -> Option<Win> {
        let attrs = Window::default_attributes()
            .with_title(if glass {
                "spike glass"
            } else {
                "spike checker"
            })
            .with_decorations(false)
            .with_resizable(false)
            .with_active(false)
            .with_transparent(glass)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(PhysicalPosition::new(rect.0, rect.1))
            .with_inner_size(PhysicalSize::new(rect.2, rect.3));
        let window = Rc::new(el.create_window(attrs).ok()?);
        let ctx = Context::new(window.clone()).ok()?;
        let surface = Surface::new(&ctx, window.clone()).ok()?;
        if let Some(h) = hwnd(&window) {
            let _ = perch_platform_windows::window::apply_notch_styles(h);
        }
        Some(Win {
            window,
            surface,
            glass,
        })
    }

    fn apply_backdrop(h: HWND, rgn: bool, w: u32, hgt: u32) {
        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        let dark: u32 = 1;
        let backdrop = DWMSBT_TRANSIENTWINDOW.0;
        // SAFETY: `h` is a live window this process created; the attribute
        // pointers reference correctly sized locals.
        unsafe {
            let r = DwmExtendFrameIntoClientArea(h, &margins);
            println!("DwmExtendFrameIntoClientArea: {r:?}");
            let r = DwmSetWindowAttribute(
                h,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const u32 as *const std::ffi::c_void,
                4,
            );
            println!("dark mode: {r:?}");
            let r = DwmSetWindowAttribute(
                h,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &backdrop as *const i32 as *const std::ffi::c_void,
                4,
            );
            println!("backdrop: {r:?}");
            if rgn {
                let d = (2.0 * RADIUS) as i32;
                let region = CreateRoundRectRgn(0, 0, w as i32 + 1, hgt as i32 + 1, d, d);
                let r = SetWindowRgn(h, region, true);
                println!("SetWindowRgn: {r}");
            }
        }
    }

    fn paint(win: &mut Win) {
        let size = win.window.inner_size();
        let (Some(nw), Some(nh)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        let Some(mut pm) = Pixmap::new(size.width, size.height) else {
            return;
        };
        if win.glass {
            if let Some(p) =
                perch_ui::art::rounded_rect(0.0, 0.0, size.width as f32, size.height as f32, RADIUS)
            {
                let mut paint = Paint {
                    anti_alias: true,
                    ..Default::default()
                };
                paint.set_color(Color::from_rgba8(18, 20, 28, 148)); // 0.58
                pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
            }
        } else {
            for y in 0..size.height {
                for x in 0..size.width {
                    let on = ((x / 10) + (y / 10)) % 2 == 0;
                    let c = if on { 255 } else { 0 };
                    let i = (y * size.width + x) as usize;
                    if let Some(px) = tiny_skia::PremultipliedColorU8::from_rgba(c, c, c, 255) {
                        pm.pixels_mut()[i] = px;
                    }
                }
            }
        }
        if win.surface.resize(nw, nh).is_err() {
            return;
        }
        let Ok(mut buf) = win.surface.buffer_mut() else {
            return;
        };
        for (dst, src) in buf.iter_mut().zip(pm.pixels()) {
            *dst = (u32::from(src.alpha()) << 24)
                | (u32::from(src.red()) << 16)
                | (u32::from(src.green()) << 8)
                | u32::from(src.blue());
        }
        let _ = buf.present();
    }

    impl ApplicationHandler for App {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            if !self.wins.is_empty() {
                return;
            }
            for (rect, glass) in [(CHECKER, false), (GLASS, true)] {
                let Some(win) = open(el, rect, glass) else {
                    eprintln!("cannot open a window");
                    el.exit();
                    return;
                };
                if glass && self.mode.starts_with("backdrop") {
                    if let Some(h) = hwnd(&win.window) {
                        apply_backdrop(h, self.mode == "backdrop-rgn", rect.2, rect.3);
                    }
                }
                let pos = win.window.outer_position().unwrap_or_default();
                let size = win.window.inner_size();
                println!(
                    "{} at {},{} size {}x{} scale {}",
                    if glass { "glass" } else { "checker" },
                    pos.x,
                    pos.y,
                    size.width,
                    size.height,
                    win.window.scale_factor()
                );
                win.window.request_redraw();
                self.wins.push(win);
            }
            println!("pid {}", std::process::id());
            el.set_control_flow(ControlFlow::WaitUntil(self.until));
        }

        fn window_event(&mut self, _el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
            if let WindowEvent::RedrawRequested = event {
                if let Some(w) = self.wins.iter_mut().find(|w| w.window.id() == id) {
                    paint(w);
                }
            }
        }

        fn about_to_wait(&mut self, el: &ActiveEventLoop) {
            if Instant::now() >= self.until {
                el.exit();
            } else {
                el.set_control_flow(ControlFlow::WaitUntil(self.until));
            }
        }
    }

    pub fn run() {
        let mut args = std::env::args().skip(1);
        let mode = args.next().unwrap_or_else(|| "backdrop".into());
        let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(8);
        let Ok(el) = EventLoop::new() else {
            eprintln!("no event loop");
            return;
        };
        let mut app = App {
            mode,
            until: Instant::now() + Duration::from_secs(secs),
            wins: Vec::new(),
        };
        if let Err(e) = el.run_app(&mut app) {
            eprintln!("{e}");
        }
    }
}
