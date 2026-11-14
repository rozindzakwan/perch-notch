//! Camera mirror window: a small, borderless, always-on-top window under the
//! notch that shows the latest camera frame, flipped horizontally like a
//! mirror, inside a rounded frame. Frames are pushed by the capture thread
//! into a shared slot; the window only paints when told a new frame arrived.
//! Click or Esc closes it.

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

/// [`mirror_fill`] inside a rounded frame: premultiplied `0xAARRGGBB` with
/// transparent corners (radius in pixels) and a faint hairline edge, for a
/// transparent window. With no frame yet (`src` empty) the frame is a dark
/// glass fill. Pure.
pub fn mirror_frame(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32, radius: f32) -> Vec<u32> {
    let n = (dw * dh) as usize;
    let Some(mut pm) = tiny_skia::Pixmap::new(dw.max(1), dh.max(1)) else {
        return vec![0; n];
    };
    let has_frame = sw > 0 && sh > 0 && src.len() >= (sw * sh * 3) as usize;
    let mut content = vec![0u32; n];
    if has_frame {
        content = mirror_fill(src, sw, sh, dw, dh);
    }
    let Some(path) = crate::art::rounded_rect(0.0, 0.0, dw as f32, dh as f32, radius) else {
        return vec![0; n];
    };
    // Coverage mask of the rounded shape.
    let mut paint = tiny_skia::Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color(tiny_skia::Color::WHITE);
    pm.fill_path(
        &path,
        &paint,
        tiny_skia::FillRule::Winding,
        tiny_skia::Transform::identity(),
        None,
    );
    let empty = 0x0012_141C;
    let mut out: Vec<u32> = pm
        .pixels()
        .iter()
        .zip(content.iter())
        .map(|(cov, px)| {
            let a = u32::from(cov.alpha());
            let px = if has_frame { *px } else { empty };
            let ch = |shift: u32| (((px >> shift) & 0xFF) * a / 255) << shift;
            (a << 24) | ch(16) | ch(8) | ch(0)
        })
        .collect();
    // Hairline: blend white at 16 % along the inside edge.
    pm.fill(tiny_skia::Color::TRANSPARENT);
    if let Some(edge) =
        crate::art::rounded_rect(0.5, 0.5, dw as f32 - 1.0, dh as f32 - 1.0, radius - 0.5)
    {
        let stroke = tiny_skia::Stroke {
            width: 1.0,
            ..Default::default()
        };
        pm.stroke_path(
            &edge,
            &paint,
            &stroke,
            tiny_skia::Transform::identity(),
            None,
        );
        for (o, e) in out.iter_mut().zip(pm.pixels()) {
            let k = u32::from(e.alpha()) * 41 / 255; // 0.16 of the coverage
            if k == 0 || *o >> 24 == 0 {
                continue;
            }
            let a = *o >> 24;
            let ch = |shift: u32| {
                let c = (*o >> shift) & 0xFF;
                ((c * (255 - k) + a * k) / 255) << shift
            };
            *o = (a << 24) | ch(16) | ch(8) | ch(0);
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
            .with_transparent(true)
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
        let radius = crate::tokens::radius::COVER * self.window.scale_factor() as f32;
        let pixels = match frame {
            Some((rgb, w, h)) => mirror_frame(&rgb, w, h, size.width, size.height, radius),
            None => mirror_frame(&[], 0, 0, size.width, size.height, radius),
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
