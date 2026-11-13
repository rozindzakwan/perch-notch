//! Glyph rasterisation and a tiny text layout, built on `fontdue`.
//!
//! There is no system font enumeration here on purpose: Perch embeds one UI
//! font so the pill looks identical on Windows and Linux and so startup does no
//! font-config work. Rasterised glyphs are cached per (char, px) pair, which is
//! what keeps animated frames cheap.

use std::collections::HashMap;

use fontdue::{Font, FontSettings};
use perch_core::error::{Error, Result};
use tiny_skia::{Color, Pixmap, PremultipliedColorU8};

/// An embedded font plus a glyph cache.
pub struct TextRenderer {
    font: Font,
    cache: HashMap<(char, u32), Glyph>,
}

struct Glyph {
    width: usize,
    height: usize,
    /// Coverage bitmap, one byte per pixel.
    coverage: Vec<u8>,
    advance: f32,
    /// Offset from the baseline-left origin to the bitmap's top-left.
    left: f32,
    top: f32,
}

/// Where a laid-out string ended up, so callers can centre and clip it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMetrics {
    /// Total advance width in pixels.
    pub width: f32,
    /// Distance from the top of the line box to the baseline.
    pub ascent: f32,
    /// Distance from the baseline to the bottom of the line box.
    pub descent: f32,
}

impl TextMetrics {
    /// Full line height.
    pub fn line_height(&self) -> f32 {
        self.ascent + self.descent
    }
}

impl TextRenderer {
    /// Load the embedded UI font.
    pub fn new() -> Result<Self> {
        // DejaVu Sans is bundled by the build (see build.rs); it covers Latin,
        // Cyrillic, Greek and a wide punctuation range.
        const FONT: &[u8] = include_bytes!("../assets/PerchUI.ttf");
        let font = Font::from_bytes(FONT, FontSettings::default())
            .map_err(|e| Error::backend(format!("loading the embedded UI font: {e}")))?;
        Ok(Self {
            font,
            cache: HashMap::new(),
        })
    }

    fn glyph(&mut self, ch: char, px: f32) -> &Glyph {
        let key = (ch, px.to_bits());
        let key = (key.0, px as u32);
        if !self.cache.contains_key(&key) {
            let (metrics, coverage) = self.font.rasterize(ch, px);
            self.cache.insert(
                key,
                Glyph {
                    width: metrics.width,
                    height: metrics.height,
                    coverage,
                    advance: metrics.advance_width,
                    left: metrics.xmin as f32,
                    top: metrics.ymin as f32 + metrics.height as f32,
                },
            );
        }
        // The entry was just inserted when missing, so this cannot fail; the
        // match keeps us free of unwrap on a runtime path.
        match self.cache.get(&key) {
            Some(g) => g,
            None => unreachable_glyph(),
        }
    }

    /// Measure a string without drawing it.
    pub fn measure(&mut self, s: &str, px: f32) -> TextMetrics {
        let mut width = 0.0;
        for ch in s.chars() {
            width += self.glyph(ch, px).advance;
        }
        let line = self.font.horizontal_line_metrics(px);
        let (ascent, descent) = match line {
            Some(m) => (m.ascent, -m.descent),
            None => (px * 0.8, px * 0.2),
        };
        TextMetrics {
            width,
            ascent,
            descent,
        }
    }

    /// Draw a string with its baseline at `(x, baseline)`, clipped to `max_w`.
    /// Returns the advance actually used.
    pub fn draw(
        &mut self,
        pm: &mut Pixmap,
        s: &str,
        x: f32,
        baseline: f32,
        px: f32,
        color: Color,
        max_w: f32,
    ) -> f32 {
        let mut pen = x;
        for ch in s.chars() {
            let (gw, gh, adv, gl, gt, coverage) = {
                let g = self.glyph(ch, px);
                (
                    g.width,
                    g.height,
                    g.advance,
                    g.left,
                    g.top,
                    g.coverage.clone(),
                )
            };
            if pen - x + adv > max_w {
                break;
            }
            blit(pm, &coverage, gw, gh, pen + gl, baseline - gt, color);
            pen += adv;
        }
        pen - x
    }

    /// Truncate a string with a trailing ellipsis so it fits `max_w`.
    pub fn elide(&mut self, s: &str, px: f32, max_w: f32) -> String {
        if self.measure(s, px).width <= max_w {
            return s.to_string();
        }
        let ell = "\u{2026}";
        let ell_w = self.measure(ell, px).width;
        let mut out = String::new();
        let mut w = 0.0;
        for ch in s.chars() {
            let adv = self.glyph(ch, px).advance;
            if w + adv + ell_w > max_w {
                break;
            }
            out.push(ch);
            w += adv;
        }
        out.push_str(ell);
        out
    }
}

/// Composite an 8-bit coverage bitmap onto a pixmap with a solid colour.
fn blit(pm: &mut Pixmap, coverage: &[u8], gw: usize, gh: usize, x: f32, y: f32, color: Color) {
    if gw == 0 || gh == 0 {
        return;
    }
    let pw = pm.width() as i32;
    let ph = pm.height() as i32;
    let ox = x.round() as i32;
    let oy = y.round() as i32;
    let (sr, sg, sb, sa) = (
        color.red() * 255.0,
        color.green() * 255.0,
        color.blue() * 255.0,
        color.alpha(),
    );
    let data = pm.pixels_mut();
    for gy in 0..gh {
        let py = oy + gy as i32;
        if py < 0 || py >= ph {
            continue;
        }
        for gx in 0..gw {
            let px = ox + gx as i32;
            if px < 0 || px >= pw {
                continue;
            }
            let cov = match coverage.get(gy * gw + gx) {
                Some(c) => f32::from(*c) / 255.0,
                None => continue,
            };
            if cov <= 0.0 {
                continue;
            }
            let a = cov * sa;
            let idx = (py * pw + px) as usize;
            let dst = match data.get(idx) {
                Some(d) => *d,
                None => continue,
            };
            // Source-over, premultiplied.
            let inv = 1.0 - a;
            let nr = sr * a + f32::from(dst.red()) * inv;
            let ng = sg * a + f32::from(dst.green()) * inv;
            let nb = sb * a + f32::from(dst.blue()) * inv;
            let na = a * 255.0 + f32::from(dst.alpha()) * inv;
            if let Some(p) = PremultipliedColorU8::from_rgba(
                nr.round().clamp(0.0, 255.0) as u8,
                ng.round().clamp(0.0, 255.0) as u8,
                nb.round().clamp(0.0, 255.0) as u8,
                na.round().clamp(0.0, 255.0) as u8,
            ) {
                if let Some(slot) = data.get_mut(idx) {
                    *slot = p;
                }
            }
        }
    }
}

/// Only reachable if `HashMap::insert` followed by `get` of the same key fails,
/// which cannot happen; this keeps the hot path free of `unwrap`.
fn unreachable_glyph() -> &'static Glyph {
    static EMPTY: Glyph = Glyph {
        width: 0,
        height: 0,
        coverage: Vec::new(),
        advance: 0.0,
        left: 0.0,
        top: 0.0,
    };
    &EMPTY
}
