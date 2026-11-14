//! Camera mirror window: a small, borderless, always-on-top window under the
//! notch that shows the latest camera frame, flipped horizontally like a
//! mirror. Frames are pushed by the capture thread into a shared slot; the
//! window only paints when told a new frame arrived. Click or Esc closes it.

use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use perch_core::error::{Error, Result};
use softbuffer::{Context, Surface};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

/// The latest frame: straight RGB8 (3 bytes per pixel), width, height.
pub type FrameSlot = Arc<Mutex<Option<(Vec<u8>, u32, u32)>>>;

/// Scale `src` (RGB8, `sw` x `sh`) into a `dw` x `dh` 0RGB buffer, mirrored
/// horizontally and cropped to fill (no letterboxing). Pure.
pub fn mirror_fill(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u32> {
    let mut out = vec![0u32; (dw * dh) as usize];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || src.len() < (sw * sh * 3) as usize {
        return out;
    }
    // Crop the source to the destination aspect ratio, centred.
    let (cw, ch) = if u64::from(sw) * u64::from(dh) > u64::from(sh) * u64::from(dw) {
        ((u64::from(sh) * u64::from(dw) / u64::from(dh)) as u32, sh)
    } else {
        (sw, (u64::from(sw) * u64::from(dh) / u64::from(dw)) as u32)
    };
    let (ox, oy) = ((sw - cw) / 2, (sh - ch) / 2);
    for y in 0..dh {
        let sy = oy + y * ch / dh;
        for x in 0..dw {
            let sx = ox + (cw - 1 - x * cw / dw.max(1)).min(cw - 1);
            let i = ((sy * sw + sx) * 3) as usize;
            let (r, g, b) = (src[i], src[i + 1], src[i + 2]);
            out[(y * dw + x) as usize] = (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
        }
    }
    out
}

/// The mirror window.
pub struct MirrorWindow {
    window: Rc<Window>,
    surface: Surface<Rc<Window>, Rc<Window>>,
    slot: FrameSlot,
}

impl MirrorWindow {
    /// Open a `w` x `h` (physical) mirror with its top-left at `(x, y)`.
    pub fn open(
        el: &ActiveEventLoop,
        slot: FrameSlot,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    ) -> Result<Self> {
        let attrs = Window::default_attributes()
            .with_title("Perch mirror")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(PhysicalPosition::new(x, y))
            .with_inner_size(PhysicalSize::new(w.max(64), h.max(48)));
        let window = Rc::new(
            el.create_window(attrs)
                .map_err(|e| Error::backend(format!("mirror window: {e}")))?,
        );
        let ctx =
            Context::new(window.clone()).map_err(|e| Error::backend(format!("softbuffer: {e}")))?;
        let surface = Surface::new(&ctx, window.clone())
            .map_err(|e| Error::backend(format!("softbuffer: {e}")))?;
        Ok(Self {
            window,
            surface,
            slot,
        })
    }

    /// Window id.
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// A new frame is in the slot.
    pub fn frame_ready(&self) {
        self.window.request_redraw();
    }

    /// Handle a window event; `true` means "close the mirror".
    pub fn handle(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CloseRequested => true,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } => true,
            WindowEvent::KeyboardInput { event, .. } => {
                event.state == ElementState::Pressed
                    && event.logical_key == Key::Named(NamedKey::Escape)
            }
            WindowEvent::RedrawRequested => {
                self.paint();
                false
            }
            _ => false,
        }
    }

    fn paint(&mut self) {
        let size = self.window.inner_size();
        let (Some(nw), Some(nh)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        let frame = self.slot.lock().ok().and_then(|f| f.clone());
        let pixels = match frame {
            Some((rgb, w, h)) => mirror_fill(&rgb, w, h, size.width, size.height),
            None => vec![0x0016_171c; (size.width * size.height) as usize],
        };
        if self.surface.resize(nw, nh).is_err() {
            return;
        }
        let Ok(mut buf) = self.surface.buffer_mut() else {
            return;
        };
        for (dst, src) in buf.iter_mut().zip(pixels.iter()) {
            *dst = *src;
        }
        let _ = buf.present();
    }
}
