//! Original vector artwork: the pill shape, Perch's bird mascot and the
//! transport icons. Everything here is drawn from first principles with
//! `tiny-skia` paths; no third-party assets are used.

use tiny_skia::{Path, PathBuilder, Rect};

/// A rounded rectangle. `r` is clamped so it can never exceed half the shorter
/// side, which is what makes a fully-rounded pill fall out of the same code.
pub fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r <= f32::EPSILON {
        return Rect::from_xywh(x, y, w, h).map(PathBuilder::from_rect);
    }
    // Circle-approximating cubic handle length.
    let k = r * 0.5522847;
    let (x1, y1) = (x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x1 - r, y);
    pb.cubic_to(x1 - r + k, y, x1, y + r - k, x1, y + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    pb.line_to(x + r, y1);
    pb.cubic_to(x + r - k, y1, x, y1 - r + k, x, y1 - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// The pill itself: a rounded rect whose radius is half its height unless the
/// config overrides it.
pub fn pill(x: f32, y: f32, w: f32, h: f32, radius: Option<f32>) -> Option<Path> {
    rounded_rect(x, y, w, h, radius.unwrap_or(h / 2.0))
}

/// Perch's mascot: a minimal side-on bird silhouette sitting on a ledge.
/// Original artwork, drawn as a closed path inside the unit box `0..1`, scaled
/// to `size` and placed at `(x, y)`.
pub fn mascot(x: f32, y: f32, size: f32) -> Option<Path> {
    if size <= 0.0 {
        return None;
    }
    let s = |v: f32| v * size;
    let mut pb = PathBuilder::new();
    // Back of the head, over the crown, down to the beak.
    pb.move_to(x + s(0.30), y + s(0.34));
    pb.cubic_to(
        x + s(0.30),
        y + s(0.14),
        x + s(0.62),
        y + s(0.12),
        x + s(0.66),
        y + s(0.30),
    );
    // Beak: a short wedge pointing right.
    pb.line_to(x + s(0.95), y + s(0.38));
    pb.line_to(x + s(0.66), y + s(0.46));
    // Breast curving down to the tail.
    pb.cubic_to(
        x + s(0.70),
        y + s(0.66),
        x + s(0.58),
        y + s(0.80),
        x + s(0.40),
        y + s(0.80),
    );
    // Tail sweeping back and up.
    pb.line_to(x + s(0.04), y + s(0.72));
    pb.line_to(x + s(0.26), y + s(0.60));
    // Back up to the nape.
    pb.cubic_to(
        x + s(0.20),
        y + s(0.50),
        x + s(0.22),
        y + s(0.38),
        x + s(0.30),
        y + s(0.34),
    );
    pb.close();
    pb.finish()
}

/// The mascot's eye, drawn in the accent colour on top of the silhouette.
pub fn mascot_eye(x: f32, y: f32, size: f32) -> Option<Path> {
    let r = size * 0.055;
    if r <= 0.0 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.push_circle(x + size * 0.50, y + size * 0.30, r);
    pb.finish()
}

/// Play triangle inside the given square.
pub fn icon_play(x: f32, y: f32, size: f32) -> Option<Path> {
    if size <= 0.0 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(x + size * 0.28, y + size * 0.18);
    pb.line_to(x + size * 0.82, y + size * 0.50);
    pb.line_to(x + size * 0.28, y + size * 0.82);
    pb.close();
    pb.finish()
}

/// Pause bars inside the given square.
pub fn icon_pause(x: f32, y: f32, size: f32) -> Option<Path> {
    if size <= 0.0 {
        return None;
    }
    let bw = size * 0.17;
    let mut pb = PathBuilder::new();
    if let Some(r) = Rect::from_xywh(x + size * 0.26, y + size * 0.18, bw, size * 0.64) {
        pb.push_rect(r);
    }
    if let Some(r) = Rect::from_xywh(x + size * 0.57, y + size * 0.18, bw, size * 0.64) {
        pb.push_rect(r);
    }
    pb.finish()
}

/// Next-track icon: a triangle plus a bar.
pub fn icon_next(x: f32, y: f32, size: f32) -> Option<Path> {
    if size <= 0.0 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(x + size * 0.22, y + size * 0.20);
    pb.line_to(x + size * 0.66, y + size * 0.50);
    pb.line_to(x + size * 0.22, y + size * 0.80);
    pb.close();
    if let Some(r) = Rect::from_xywh(x + size * 0.70, y + size * 0.20, size * 0.12, size * 0.60) {
        pb.push_rect(r);
    }
    pb.finish()
}

/// Previous-track icon: the mirror of [`icon_next`].
pub fn icon_prev(x: f32, y: f32, size: f32) -> Option<Path> {
    if size <= 0.0 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(x + size * 0.78, y + size * 0.20);
    pb.line_to(x + size * 0.34, y + size * 0.50);
    pb.line_to(x + size * 0.78, y + size * 0.80);
    pb.close();
    if let Some(r) = Rect::from_xywh(x + size * 0.18, y + size * 0.20, size * 0.12, size * 0.60) {
        pb.push_rect(r);
    }
    pb.finish()
}

/// Settings gear, drawn as a stroke: a ring with six short spokes.
pub fn gear(x: f32, y: f32, size: f32) -> Option<Path> {
    if size <= 0.0 {
        return None;
    }
    let c = (x + size / 2.0, y + size / 2.0);
    let r = size * 0.28;
    let mut pb = PathBuilder::new();
    pb.push_circle(c.0, c.1, r);
    for i in 0..6 {
        let a = i as f32 * std::f32::consts::PI / 3.0;
        let (sa, ca) = a.sin_cos();
        pb.move_to(c.0 + ca * r * 1.25, c.1 + sa * r * 1.25);
        pb.line_to(c.0 + ca * size * 0.5, c.1 + sa * size * 0.5);
    }
    pb.finish()
}

/// Pin (thumbtack) silhouette: a round head over a pointed needle.
pub fn pin(x: f32, y: f32, size: f32) -> Option<Path> {
    if size <= 0.0 {
        return None;
    }
    let s = |v: f32| v * size;
    let mut pb = PathBuilder::new();
    pb.push_circle(x + s(0.5), y + s(0.32), s(0.28));
    pb.move_to(x + s(0.36), y + s(0.5));
    pb.line_to(x + s(0.64), y + s(0.5));
    pb.line_to(x + s(0.5), y + s(1.0));
    pb.close();
    pb.finish()
}

/// Reset: an open circular arrow, drawn as a stroke.
pub fn reset(x: f32, y: f32, size: f32) -> Option<Path> {
    if size <= 0.0 {
        return None;
    }
    let c = (x + size / 2.0, y + size / 2.0);
    let r = size * 0.42;
    let mut pb = PathBuilder::new();
    let steps = 14;
    for i in 0..=steps {
        let a = -0.5 + i as f32 / steps as f32 * 5.0;
        let p = (c.0 + a.cos() * r, c.1 + a.sin() * r);
        if i == 0 {
            pb.move_to(p.0, p.1);
        } else {
            pb.line_to(p.0, p.1);
        }
    }
    // Arrow head at the start of the arc.
    let a0: f32 = -0.5;
    let tip = (c.0 + a0.cos() * r, c.1 + a0.sin() * r);
    pb.move_to(tip.0, tip.1);
    pb.line_to(tip.0 - size * 0.05, tip.1 - size * 0.28);
    pb.move_to(tip.0, tip.1);
    pb.line_to(tip.0 - size * 0.28, tip.1 + size * 0.02);
    pb.finish()
}

/// Check mark, drawn as a stroke.
pub fn check(x: f32, y: f32, size: f32) -> Option<Path> {
    if size <= 0.0 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(x, y + size * 0.55);
    pb.line_to(x + size * 0.38, y + size * 0.9);
    pb.line_to(x + size, y + size * 0.1);
    pb.finish()
}

/// An "x", drawn as a stroke.
pub fn cross(x: f32, y: f32, size: f32) -> Option<Path> {
    if size <= 0.0 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(x, y);
    pb.line_to(x + size, y + size);
    pb.move_to(x + size, y);
    pb.line_to(x, y + size);
    pb.finish()
}
