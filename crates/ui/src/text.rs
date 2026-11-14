//! Glyph rasterisation and a tiny text layout, built on `fontdue`.
//!
//! There is no system font enumeration here on purpose: Perch embeds its UI
//! fonts so the notch looks identical on Windows and Linux and startup does no
//! font-config work. Inter (Regular, Medium, Bold) is the UI face; DejaVu Sans
//! is the fallback for glyphs Inter lacks (symbols, emoji). Rasterised glyphs
//! are cached per (font, char, size), which keeps animated frames cheap.

use std::collections::HashMap;

use fontdue::{Font, FontSettings};
use perch_core::error::{Error, Result};
use tiny_skia::{Color, Pixmap, PremultipliedColorU8};

use crate::tokens::Weight;

const FONT_REGULAR: usize = 0;
const FONT_MEDIUM: usize = 1;
const FONT_BOLD: usize = 2;
const FONT_FALLBACK: usize = 3;
const FONT_SCRIPTS: usize = 4;

/// The embedded fonts plus a glyph cache.
pub struct TextRenderer {
    /// Inter Regular, Medium, Bold.
    fonts: Vec<Font>,
    /// DejaVu Sans in two parts, each parsed on the first character that
    /// needs it: symbols and emoji (`FONT_FALLBACK`), other scripts
    /// plus rare symbols (`FONT_SCRIPTS`). fontdue builds every outline up
    /// front, so an unused fallback would cost memory for nothing.
    fallback: [std::cell::OnceCell<Option<Font>>; 2],
    cache: HashMap<(usize, char, u32), Glyph>,
    /// Widest digit advance per (font, size), for tabular numbers.
    digit_w: HashMap<(usize, u32), f32>,
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

fn load(bytes: &'static [u8], name: &str) -> Result<Font> {
    Font::from_bytes(bytes, FontSettings::default())
        .map_err(|e| Error::backend(format!("loading the embedded font {name}: {e}")))
}

impl TextRenderer {
    /// Load the embedded UI fonts.
    pub fn new() -> Result<Self> {
        let fonts = vec![
            load(
                include_bytes!("../assets/Inter-Regular.ttf"),
                "Inter Regular",
            )?,
            load(include_bytes!("../assets/Inter-Medium.ttf"), "Inter Medium")?,
            load(include_bytes!("../assets/Inter-Bold.ttf"), "Inter Bold")?,
        ];
        Ok(Self {
            fonts,
            fallback: [std::cell::OnceCell::new(), std::cell::OnceCell::new()],
            cache: HashMap::new(),
            digit_w: HashMap::new(),
        })
    }

    /// Font by index; the fallback is loaded on first use.
    fn font(&self, i: usize) -> Option<&Font> {
        // DejaVu Sans, subset to what Inter lacks (scripts/subset-fonts.sh).
        let (slot, bytes, name): (usize, &'static [u8], &str) = match i {
            FONT_FALLBACK => (
                0,
                include_bytes!("../assets/PerchSymbols.ttf"),
                "DejaVu Sans symbols",
            ),
            FONT_SCRIPTS => (
                1,
                include_bytes!("../assets/PerchScripts.ttf"),
                "DejaVu Sans scripts",
            ),
            _ => return self.fonts.get(i),
        };
        self.fallback[slot]
            .get_or_init(|| match load(bytes, name) {
                Ok(f) => Some(f),
                Err(e) => {
                    log::warn!("{e}");
                    None
                }
            })
            .as_ref()
    }

    /// Whether the symbols / other-scripts fallbacks have been loaded (tests,
    /// diagnostics).
    pub fn fallbacks_loaded(&self) -> (bool, bool) {
        (
            self.fallback[0].get().is_some(),
            self.fallback[1].get().is_some(),
        )
    }

    /// Which fallback could hold `ch`, by the ranges each was subset to.
    fn fallback_for(ch: char) -> usize {
        match u32::from(ch) {
            0x0530..=0x1DFF
            | 0x1F00..=0x1FFF
            | 0x2500..=0x259F
            | 0x27C0..=0x28FF
            | 0x2C00..=0x2E7F
            | 0xA640..=0xA6FF
            | 0xFB13..=0xFB4F
            | 0xFE20..=0xFE2F
            | 0x1D400..=0x1D7FF => FONT_SCRIPTS,
            _ => FONT_FALLBACK,
        }
    }

    /// Which font draws `ch` at `weight`: the Inter weight if it has the
    /// glyph, otherwise the fallback.
    fn font_for(&self, ch: char, weight: Weight) -> usize {
        let idx = match weight {
            Weight::Regular => FONT_REGULAR,
            Weight::Medium => FONT_MEDIUM,
            Weight::Bold => FONT_BOLD,
        };
        let has = |i: usize| {
            self.font(i)
                .map(|f| f.lookup_glyph_index(ch) != 0)
                .unwrap_or(false)
        };
        let fallback = Self::fallback_for(ch);
        if ch.is_whitespace() || has(idx) {
            idx
        } else if has(fallback) {
            fallback
        } else {
            idx
        }
    }

    fn glyph(&mut self, ch: char, px: f32, weight: Weight) -> &Glyph {
        let font = self.font_for(ch, weight);
        let key = (font, ch, px.to_bits());
        if !self.cache.contains_key(&key) {
            let (metrics, coverage) = match self.font(font) {
                Some(f) => f.rasterize(ch, px),
                None => return unreachable_glyph(),
            };
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

    fn line(&self, px: f32, weight: Weight) -> (f32, f32) {
        let idx = self.font_for('A', weight);
        match self.font(idx).and_then(|f| f.horizontal_line_metrics(px)) {
            Some(m) => (m.ascent, -m.descent),
            None => (px * 0.8, px * 0.2),
        }
    }

    /// Advance of the widest ASCII digit (tabular numbers).
    fn digit_width(&mut self, px: f32, weight: Weight) -> f32 {
        let font = self.font_for('0', weight);
        let key = (font, px.to_bits());
        if let Some(w) = self.digit_w.get(&key) {
            return *w;
        }
        let w = ('0'..='9')
            .map(|d| self.glyph(d, px, weight).advance)
            .fold(0.0_f32, f32::max);
        self.digit_w.insert(key, w);
        w
    }

    fn advance(&mut self, ch: char, px: f32, weight: Weight, tabular: bool) -> f32 {
        if tabular && ch.is_ascii_digit() {
            self.digit_width(px, weight)
        } else {
            self.glyph(ch, px, weight).advance
        }
    }

    fn measure_impl(&mut self, s: &str, px: f32, weight: Weight, tabular: bool) -> TextMetrics {
        let mut width = 0.0;
        for ch in s.chars() {
            width += self.advance(ch, px, weight, tabular);
        }
        let (ascent, descent) = self.line(px, weight);
        TextMetrics {
            width,
            ascent,
            descent,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_impl(
        &mut self,
        pm: &mut Pixmap,
        s: &str,
        x: f32,
        baseline: f32,
        px: f32,
        color: Color,
        max_w: f32,
        weight: Weight,
        tabular: bool,
    ) -> f32 {
        let mut pen = x;
        for ch in s.chars() {
            let cell = self.advance(ch, px, weight, tabular);
            let (gw, gh, adv, gl, gt, coverage) = {
                let g = self.glyph(ch, px, weight);
                (
                    g.width,
                    g.height,
                    g.advance,
                    g.left,
                    g.top,
                    g.coverage.clone(),
                )
            };
            if pen - x + cell > max_w {
                break;
            }
            // Tabular digits sit centred in their fixed-width cell.
            let shift = (cell - adv) / 2.0;
            blit(
                pm,
                &coverage,
                gw,
                gh,
                pen + shift + gl,
                baseline - gt,
                color,
            );
            pen += cell;
        }
        pen - x
    }

    /// Measure a string in Inter Medium (the body weight).
    pub fn measure(&mut self, s: &str, px: f32) -> TextMetrics {
        self.measure_impl(s, px, Weight::Medium, false)
    }

    /// Measure a string at a weight.
    pub fn measure_w(&mut self, s: &str, px: f32, weight: Weight) -> TextMetrics {
        self.measure_impl(s, px, weight, false)
    }

    /// Measure with tabular (equal-width) digits.
    pub fn measure_tabular(&mut self, s: &str, px: f32, weight: Weight) -> TextMetrics {
        self.measure_impl(s, px, weight, true)
    }

    /// Draw a string in Inter Medium with its baseline at `(x, baseline)`,
    /// clipped to `max_w`. Returns the advance actually used.
    #[allow(clippy::too_many_arguments)]
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
        self.draw_impl(pm, s, x, baseline, px, color, max_w, Weight::Medium, false)
    }

    /// Draw at a weight.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_w(
        &mut self,
        pm: &mut Pixmap,
        s: &str,
        x: f32,
        baseline: f32,
        px: f32,
        color: Color,
        max_w: f32,
        weight: Weight,
    ) -> f32 {
        self.draw_impl(pm, s, x, baseline, px, color, max_w, weight, false)
    }

    /// Draw with tabular (equal-width) digits.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_tabular(
        &mut self,
        pm: &mut Pixmap,
        s: &str,
        x: f32,
        baseline: f32,
        px: f32,
        color: Color,
        max_w: f32,
        weight: Weight,
    ) -> f32 {
        self.draw_impl(pm, s, x, baseline, px, color, max_w, weight, true)
    }

    /// Truncate a string (Inter Medium) with a trailing ellipsis to fit `max_w`.
    pub fn elide(&mut self, s: &str, px: f32, max_w: f32) -> String {
        self.elide_w(s, px, max_w, Weight::Medium)
    }

    /// Truncate a string at a weight with a trailing ellipsis to fit `max_w`.
    pub fn elide_w(&mut self, s: &str, px: f32, max_w: f32, weight: Weight) -> String {
        if self.measure_w(s, px, weight).width <= max_w {
            return s.to_string();
        }
        let ell = "\u{2026}";
        let ell_w = self.measure_w(ell, px, weight).width;
        let mut out = String::new();
        let mut w = 0.0;
        for ch in s.chars() {
            let adv = self.glyph(ch, px, weight).advance;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::Weight;

    #[test]
    fn fallback_is_parsed_only_when_a_character_needs_it() {
        let mut t = TextRenderer::new().unwrap_or_else(|e| panic!("{e}"));
        let _ = t.measure("Perch \u{2022} \u{2014} J\u{fc}rgen 12:30", 14.0);
        assert_eq!(t.fallbacks_loaded(), (false, false), "Inter covers Latin");
        let _ = t.measure("Halo \u{1F600}", 14.0);
        assert_eq!(
            t.fallbacks_loaded(),
            (true, false),
            "an emoji loads only the symbols part"
        );
        let _ = t.measure("\u{5e9}\u{5dc}\u{5d5}\u{5dd}", 14.0); // Hebrew
        assert_eq!(t.fallbacks_loaded(), (true, true));
        assert_eq!(t.font_for('\u{5e9}', Weight::Medium), FONT_SCRIPTS);
    }

    #[test]
    fn latin_uses_inter_and_emoji_falls_back() {
        let t = TextRenderer::new().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(t.font_for('A', Weight::Regular), FONT_REGULAR);
        assert_eq!(t.font_for('A', Weight::Bold), FONT_BOLD);
        assert_eq!(t.font_for('😀', Weight::Regular), FONT_FALLBACK);
    }

    #[test]
    fn bold_is_wider_than_regular() {
        let mut t = TextRenderer::new().unwrap_or_else(|e| panic!("{e}"));
        let r = t.measure_w("Perch", 14.0, Weight::Regular).width;
        let b = t.measure_w("Perch", 14.0, Weight::Bold).width;
        assert!(b > r, "bold {b} vs regular {r}");
    }

    #[test]
    fn tabular_digits_have_equal_width() {
        let mut t = TextRenderer::new().unwrap_or_else(|e| panic!("{e}"));
        let ones = t.measure_tabular("1111", 13.0, Weight::Medium).width;
        let eights = t.measure_tabular("8888", 13.0, Weight::Medium).width;
        assert!((ones - eights).abs() < 0.01, "{ones} vs {eights}");
        // Proportional digits differ, which is why tabular exists.
        let p1 = t.measure_w("1111", 13.0, Weight::Medium).width;
        let p8 = t.measure_w("8888", 13.0, Weight::Medium).width;
        assert!((p1 - p8).abs() > 0.5);
    }

    #[test]
    fn elide_never_exceeds_the_box() {
        let mut t = TextRenderer::new().unwrap_or_else(|e| panic!("{e}"));
        let long = "Привет 😀 ".repeat(10) + &"quarterly report ".repeat(15);
        let mut max_w = 20.0;
        while max_w < 300.0 {
            let shown = t.elide_w(&long, 12.5, max_w, Weight::Medium);
            let w = t.measure_w(&shown, 12.5, Weight::Medium).width;
            assert!(w <= max_w + 0.5, "{w} > {max_w} for {shown:?}");
            assert!(shown.ends_with('\u{2026}'), "{shown:?}");
            max_w += 7.0;
        }
    }
}
