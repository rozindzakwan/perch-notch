//! Soft shadows and colour glows for glass surfaces.
//!
//! tiny-skia has no blur filter, so a shadow is an alpha mask of the rounded
//! shape blurred with three box-blur passes, built once per (size, radius,
//! blur) and cached. Drawing a cached shadow is a single masked composite.
//! Glows (the album-colour tint inside a panel) are two radial gradients
//! clipped to the surface's rounded shape; the clip mask is cached the same
//! way.

use tiny_skia::{
    Color, FillRule, GradientStop, Mask, Paint, PathBuilder, Pixmap, Point, PremultipliedColorU8,
    RadialGradient, SpreadMode, Transform,
};

const CACHE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Key {
    w: u32,
    h: u32,
    r: u32,
    blur: u32,
}

struct ShadowMask {
    key: Key,
    width: u32,
    height: u32,
    /// Alpha 0..=255, row-major.
    alpha: Vec<u8>,
}

/// Cache of shadow masks and the panel glow.
#[derive(Default)]
pub struct Effects {
    shadows: Vec<ShadowMask>,
    glow: Option<(GlowKey, Pixmap)>,
    misses: usize,
}

fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    crate::art::rounded_rect(x, y, w, h, r)
}

/// Sliding-window box blur of radius `r` along rows (`horizontal`) or columns.
fn box_blur(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (outer, inner) = if horizontal { (h, w) } else { (w, h) };
    let at = |o: usize, i: usize| if horizontal { o * w + i } else { i * w + o };
    let win = (2 * r + 1) as u32;
    for o in 0..outer {
        let mut sum: u32 = 0;
        // Window centred on i = 0 (indices below 0 count as 0).
        for i in 0..=r.min(inner.saturating_sub(1)) {
            sum += u32::from(src[at(o, i)]);
        }
        for i in 0..inner {
            dst[at(o, i)] = (sum / win) as u8;
            let add = i + r + 1;
            if add < inner {
                sum += u32::from(src[at(o, add)]);
            }
            if i >= r {
                sum -= u32::from(src[at(o, i - r)]);
            }
        }
    }
}

impl Effects {
    /// How many masks had to be built (tests and diagnostics).
    pub fn cache_misses(&self) -> usize {
        self.misses
    }

    /// The nine-slice base: a blurred rounded square just big enough to hold
    /// the corners, with the shape itself cut out (shadows only show outside
    /// the surface, so they never darken translucent glass from below).
    fn shadow_mask(&mut self, key: Key) -> Option<usize> {
        if let Some(i) = self.shadows.iter().position(|m| m.key == key) {
            let m = self.shadows.remove(i);
            self.shadows.push(m);
            return Some(self.shadows.len() - 1);
        }
        self.misses += 1;
        let pad = key.blur * 2;
        // Core big enough for both corners plus a 2 px straight middle.
        let core = key.r * 2 + 2;
        let width = core + pad * 2;
        let height = width;
        let mut pm = Pixmap::new(width, height)?;
        let path = rounded(
            pad as f32,
            pad as f32,
            core as f32,
            core as f32,
            key.r as f32,
        )?;
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(Color::BLACK);
        pm.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        let (w, h) = (width as usize, height as usize);
        let mut a: Vec<u8> = pm.pixels().iter().map(|p| p.alpha()).collect();
        // The cut-out sits where the surface really is: the shadow is drawn
        // blur/2 lower, so the hole is blur/2 higher in mask coordinates.
        let off = (key.blur / 2) as f32;
        let mut hole = Pixmap::new(width, height)?;
        let hole_path = rounded(
            pad as f32,
            pad as f32 - off,
            core as f32,
            core as f32,
            key.r as f32,
        )?;
        hole.fill_path(
            &hole_path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        let shape: Vec<u8> = hole.pixels().iter().map(|p| p.alpha()).collect();
        let mut b = vec![0u8; a.len()];
        let r = ((key.blur as f32) / 3.0).round().max(1.0) as usize;
        for _ in 0..3 {
            box_blur(&a, &mut b, w, h, r, true);
            box_blur(&b, &mut a, w, h, r, false);
        }
        for (v, s) in a.iter_mut().zip(&shape) {
            *v = (u32::from(*v) * (255 - u32::from(*s)) / 255) as u8;
        }
        if self.shadows.len() >= CACHE {
            self.shadows.remove(0);
        }
        self.shadows.push(ShadowMask {
            key,
            width,
            height,
            alpha: a,
        });
        Some(self.shadows.len() - 1)
    }

    /// Draw a soft black shadow around the rounded rect `(x, y, w, h,
    /// radius)`, spreading about `blur` pixels, offset down by `blur / 2`,
    /// at peak opacity `alpha`. Any size reuses the same cached base, so it
    /// stays cheap while a panel animates.
    #[allow(clippy::too_many_arguments)]
    pub fn shadow(
        &mut self,
        pm: &mut Pixmap,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        blur: f32,
        alpha: f32,
    ) {
        if w < 1.0 || h < 1.0 || blur <= 0.0 || alpha <= 0.0 {
            return;
        }
        let r = radius.round().max(0.0).min((w.min(h) / 2.0).floor()) as u32;
        let key = Key {
            w: 0,
            h: 0,
            r,
            blur: blur.round().max(1.0) as u32,
        };
        let Some(i) = self.shadow_mask(key) else {
            return;
        };
        let Some(m) = self.shadows.get(i) else {
            return;
        };
        let pad = (key.blur * 2) as i32;
        let (tw, th) = (w.round() as i32 + pad * 2, h.round() as i32 + pad * 2);
        let ox = x.round() as i32 - pad;
        let oy = y.round() as i32 - pad + (key.blur as i32) / 2;
        let (mw, mh) = (m.width as i32, m.height as i32);
        let (half_w, half_h) = (mw / 2, mh / 2);
        // Target -> base coordinate: corners map 1:1, the middle stretches.
        let map = |t: i32, total: i32, base: i32, half: i32| -> i32 {
            if t < half {
                t
            } else if t >= total - (base - half) {
                t - total + base
            } else {
                half
            }
        };
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        let peak = alpha.clamp(0.0, 1.0);
        let data = pm.pixels_mut();
        for ty in 0..th {
            let py = oy + ty;
            if py < 0 || py >= ph {
                continue;
            }
            let my = map(ty, th, mh, half_h);
            for tx in 0..tw {
                let px = ox + tx;
                if px < 0 || px >= pw {
                    continue;
                }
                let mx = map(tx, tw, mw, half_w);
                let cov = m.alpha[(my * mw + mx) as usize];
                if cov == 0 {
                    continue;
                }
                let a = f32::from(cov) / 255.0 * peak;
                let idx = (py * pw + px) as usize;
                let dst = data[idx];
                let inv = 1.0 - a;
                // Black source-over, premultiplied.
                if let Some(c) = PremultipliedColorU8::from_rgba(
                    (f32::from(dst.red()) * inv).round() as u8,
                    (f32::from(dst.green()) * inv).round() as u8,
                    (f32::from(dst.blue()) * inv).round() as u8,
                    (a * 255.0 + f32::from(dst.alpha()) * inv)
                        .round()
                        .min(255.0) as u8,
                ) {
                    data[idx] = c;
                }
            }
        }
    }

    /// Paint two soft colour blobs (top-left and bottom-right) inside the
    /// rounded rect, for the album-colour tint behind a panel's content.
    ///
    /// The result is cached as a rect-sized pixmap keyed by geometry and
    /// colours, so a settled panel only blits it (radial gradients through
    /// a mask cost ~9 ms per frame at 2x).
    pub fn glow(
        &mut self,
        pm: &mut Pixmap,
        rect: (f32, f32, f32, f32),
        radius: f32,
        colors: (Color, Color),
    ) {
        let (x, y, w, h) = rect;
        if w < 1.0 || h < 1.0 || !(x.is_finite() && y.is_finite()) {
            return;
        }
        let (x0, y0) = (x.floor(), y.floor());
        let key = GlowKey {
            rect: [x, y, w, h, radius].map(f32::to_bits),
            colors: [colors.0, colors.1].map(|c| {
                let c = c.to_color_u8();
                u32::from_be_bytes([c.red(), c.green(), c.blue(), c.alpha()])
            }),
        };
        if self.glow.as_ref().map(|g| g.0) != Some(key) {
            self.misses += 1;
            let Some(img) = render_glow(rect, (x0, y0), radius, colors) else {
                return;
            };
            self.glow = Some((key, img));
        }
        if let Some((_, img)) = &self.glow {
            pm.draw_pixmap(
                x0 as i32,
                y0 as i32,
                img.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GlowKey {
    rect: [u32; 5],
    colors: [u32; 2],
}

/// The glow for `rect`, drawn into a pixmap whose top-left is `origin`.
fn render_glow(
    (x, y, w, h): (f32, f32, f32, f32),
    (x0, y0): (f32, f32),
    radius: f32,
    colors: (Color, Color),
) -> Option<Pixmap> {
    let (lx, ly) = (x - x0, y - y0);
    let mut img = Pixmap::new((lx + w).ceil() as u32, (ly + h).ceil() as u32)?;
    let mut mask = Mask::new(img.width(), img.height())?;
    mask.fill_path(
        &rounded(lx, ly, w, h, radius)?,
        FillRule::Winding,
        true,
        Transform::identity(),
    );
    let blobs = [
        (lx + w * 0.15, ly + h * 0.2, w.max(h) * 0.65, colors.0, 0.45),
        (lx + w * 0.9, ly + h * 0.9, w.max(h) * 0.7, colors.1, 0.35),
    ];
    let path = PathBuilder::from_rect(tiny_skia::Rect::from_xywh(lx, ly, w, h)?);
    for (cx, cy, r, c, a) in blobs {
        let mut inner = c;
        inner.apply_opacity(a);
        let mut outer = c;
        outer.set_alpha(0.0);
        let Some(shader) = RadialGradient::new(
            Point::from_xy(cx, cy),
            Point::from_xy(cx, cy),
            r,
            vec![GradientStop::new(0.0, inner), GradientStop::new(1.0, outer)],
            SpreadMode::Pad,
            Transform::identity(),
        ) else {
            continue;
        };
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Default::default()
        };
        img.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            Some(&mask),
        );
    }
    Some(img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tiny_skia::{Color, Pixmap};

    fn alpha(pm: &Pixmap, x: u32, y: u32) -> u8 {
        pm.pixel(x, y).map(|p| p.alpha()).unwrap_or(0)
    }

    #[test]
    fn glow_is_cached_while_settled_and_stays_inside_the_shape() {
        let mut fx = Effects::default();
        let mut pm = Pixmap::new(200, 160).unwrap_or_else(|| panic!("pixmap"));
        let colors = (
            Color::from_rgba8(255, 122, 89, 255),
            Color::from_rgba8(196, 49, 122, 255),
        );
        let rect = (10.0, 10.0, 180.0, 140.0);
        fx.glow(&mut pm, rect, 26.0, colors);
        let first = fx.cache_misses();
        fx.glow(&mut pm, rect, 26.0, colors);
        assert_eq!(fx.cache_misses(), first, "same panel: reused");
        fx.glow(&mut pm, (10.0, 10.0, 180.0, 150.0), 26.0, colors);
        assert_eq!(fx.cache_misses(), first + 1, "resized: rebuilt");
        assert!(alpha(&pm, 40, 40) > 0, "tinted inside");
        assert_eq!(alpha(&pm, 11, 11), 0, "nothing outside the rounded corner");
    }

    #[test]
    fn shadow_falls_off_outward() {
        let mut fx = Effects::default();
        let mut pm = Pixmap::new(200, 200).unwrap_or_else(|| panic!("pixmap"));
        fx.shadow(&mut pm, 50.0, 50.0, 100.0, 80.0, 20.0, 12.0, 0.4);
        // Below the shape's bottom edge (y = 130), middle column.
        let edge = alpha(&pm, 100, 131);
        let six = alpha(&pm, 100, 137);
        let far = alpha(&pm, 100, 160);
        assert!(
            edge > six && six > far,
            "edge {edge}, 6 px {six}, far {far}"
        );
        assert!(far <= 2, "shadow reaches too far: {far}");
    }

    #[test]
    fn resizing_reuses_the_cached_shadow() {
        let mut fx = Effects::default();
        let mut pm = Pixmap::new(300, 300).unwrap_or_else(|| panic!("pixmap"));
        fx.shadow(&mut pm, 20.0, 20.0, 200.0, 150.0, 26.0, 12.0, 0.4);
        fx.shadow(&mut pm, 60.0, 40.0, 200.0, 150.0, 26.0, 12.0, 0.4);
        assert_eq!(fx.cache_misses(), 1, "same size and radius reuse the mask");
        // A new size (every frame of an animation) must not rebuild it...
        fx.shadow(&mut pm, 20.0, 20.0, 210.0, 160.0, 26.0, 12.0, 0.4);
        assert_eq!(fx.cache_misses(), 1, "resizing reuses the nine-slice base");
        // ...but a different corner radius needs its own base.
        fx.shadow(&mut pm, 20.0, 20.0, 210.0, 160.0, 16.0, 12.0, 0.4);
        assert_eq!(fx.cache_misses(), 2);
    }

    #[test]
    fn glow_stays_inside_its_rounded_rect() {
        let mut fx = Effects::default();
        let mut pm = Pixmap::new(120, 100).unwrap_or_else(|| panic!("pixmap"));
        let c = (
            Color::from_rgba8(255, 110, 90, 255),
            Color::from_rgba8(196, 49, 122, 255),
        );
        fx.glow(&mut pm, (10.0, 10.0, 100.0, 80.0), 26.0, c);
        assert_eq!(alpha(&pm, 11, 11), 0, "rounded corner stays clear");
        assert_eq!(alpha(&pm, 5, 50), 0, "outside stays clear");
        assert!(alpha(&pm, 30, 30) > 0, "the glow shows inside");
    }

    #[test]
    #[cfg_attr(debug_assertions, ignore = "timing only meaningful in release")]
    fn cached_shadow_is_cheap() {
        let mut fx = Effects::default();
        let mut pm = Pixmap::new(900, 750).unwrap_or_else(|| panic!("pixmap"));
        let t = Instant::now();
        fx.shadow(&mut pm, 24.0, 24.0, 760.0, 600.0, 52.0, 24.0, 0.4);
        let first = t.elapsed();
        let t = Instant::now();
        fx.shadow(&mut pm, 24.0, 24.0, 760.0, 600.0, 52.0, 24.0, 0.4);
        let cached = t.elapsed();
        assert!(first.as_millis() < 15, "first build {first:?}");
        assert!(cached.as_millis() < 2, "cached draw {cached:?}");
    }
}
