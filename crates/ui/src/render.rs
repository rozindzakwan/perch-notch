//! Draws one frame of the pill into a `tiny-skia` pixmap.
//!
//! The renderer is deliberately stateless apart from the glyph cache: it takes
//! a [`Frame`] describing what to show and paints it. That keeps the drawing
//! code testable off-screen (the UI smoke test renders to a pixmap and checks
//! pixels) and means a frame is only ever produced when something changed.

use perch_core::platform::Track;
use tiny_skia::{
    BlendMode, Color, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Transform,
};

use crate::art;
use crate::text::TextRenderer;
use crate::theme::Palette;

/// What the collapsed pill shows when nothing else is going on.
#[derive(Debug, Clone, PartialEq)]
pub enum Collapsed {
    /// Just the mascot.
    Idle,
    /// A short status string, e.g. the Pomodoro's remaining time.
    Status(String),
    /// A scrolling-free marquee-less now-playing label.
    NowPlaying(String),
}

/// Everything the renderer needs for one frame.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Pixmap width in physical pixels.
    pub width: u32,
    /// Pixmap height in physical pixels.
    pub height: u32,
    /// HiDPI scale, so text and padding grow with the display.
    pub scale: f32,
    /// Animation progress, 0.0 collapsed .. 1.0 expanded.
    pub progress: f32,
    /// Colours.
    pub palette: Palette,
    /// Corner radius override in logical pixels.
    pub corner_radius: Option<f32>,
    /// What the collapsed pill shows.
    pub collapsed: Collapsed,
    /// Current track, when the media feature is on and a player is present.
    pub track: Option<Track>,
    /// Decoded cover art as RGBA8, with its side length.
    pub cover: Option<(Vec<u8>, u32)>,
    /// Playback fraction for the seek bar, when known.
    pub progress_fraction: Option<f64>,
    /// Elapsed / total labels for the seek bar.
    pub time_labels: Option<(String, String)>,
    /// The currently-sung lyric line, when lyrics are enabled.
    pub lyric: Option<String>,
}

impl Frame {
    /// A blank frame of the given size; callers fill in what they have.
    pub fn new(width: u32, height: u32, scale: f32, palette: Palette) -> Self {
        Self {
            width,
            height,
            scale: if scale.is_finite() && scale > 0.0 {
                scale
            } else {
                1.0
            },
            progress: 0.0,
            palette,
            corner_radius: None,
            collapsed: Collapsed::Idle,
            track: None,
            cover: None,
            progress_fraction: None,
            time_labels: None,
            lyric: None,
        }
    }
}

/// Paints [`Frame`]s. Holds the glyph cache.
pub struct Renderer {
    text: TextRenderer,
}

impl Renderer {
    /// Build a renderer, loading the embedded font.
    pub fn new() -> perch_core::Result<Self> {
        Ok(Self {
            text: TextRenderer::new()?,
        })
    }

    /// Paint a frame. Returns false only when the pixmap is degenerate.
    pub fn draw(&mut self, pm: &mut Pixmap, frame: &Frame) -> bool {
        if pm.width() == 0 || pm.height() == 0 {
            return false;
        }
        pm.fill(Color::TRANSPARENT);

        let w = pm.width() as f32;
        let h = pm.height() as f32;
        let s = frame.scale;
        let radius = frame.corner_radius.map(|r| r * s);

        // The pill body.
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(frame.palette.bg);
        if let Some(path) = art::pill(0.0, 0.0, w, h, radius) {
            pm.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );

            // Hairline border, drawn as a thin stroke of the same path.
            let mut border = Paint {
                anti_alias: true,
                ..Default::default()
            };
            border.set_color(frame.palette.border);
            let stroke = tiny_skia::Stroke {
                width: 1.0_f32.max(s * 0.75),
                ..Default::default()
            };
            pm.stroke_path(&path, &border, &stroke, Transform::identity(), None);
        }

        if frame.progress < 0.5 {
            self.draw_collapsed(pm, frame, w, h);
        } else {
            self.draw_expanded(pm, frame, w, h);
        }
        true
    }

    fn draw_collapsed(&mut self, pm: &mut Pixmap, frame: &Frame, w: f32, h: f32) {
        let s = frame.scale;
        let pad = 10.0 * s;
        let icon = (h - 2.0 * pad).clamp(8.0 * s, 22.0 * s);
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };

        // Mascot on the left.
        paint.set_color(frame.palette.fg);
        if let Some(p) = art::mascot(pad, (h - icon) / 2.0, icon) {
            pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
        }
        paint.set_color(frame.palette.accent);
        if let Some(p) = art::mascot_eye(pad, (h - icon) / 2.0, icon) {
            pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
        }

        let label = match &frame.collapsed {
            Collapsed::Idle => None,
            Collapsed::Status(s) => Some(s.clone()),
            Collapsed::NowPlaying(s) => Some(s.clone()),
        };
        if let Some(label) = label {
            let px = (h * 0.42).clamp(9.0, 20.0 * s);
            let x = pad + icon + 8.0 * s;
            let avail = (w - x - pad).max(0.0);
            let shown = self.text.elide(&label, px, avail);
            let m = self.text.measure(&shown, px);
            let baseline = (h + m.ascent - m.descent) / 2.0;
            let color = match &frame.collapsed {
                Collapsed::Status(_) => frame.palette.accent,
                _ => frame.palette.fg,
            };
            self.text.draw(pm, &shown, x, baseline, px, color, avail);
        }
    }

    fn draw_expanded(&mut self, pm: &mut Pixmap, frame: &Frame, w: f32, h: f32) {
        let s = frame.scale;
        let pad = 14.0 * s;
        let art_side = (h - 2.0 * pad).max(8.0 * s);

        // Cover art, or the mascot as a placeholder.
        let mut drew_cover = false;
        if let Some((rgba, side)) = &frame.cover {
            if *side > 0 && rgba.len() as u32 >= side * side * 4 {
                if let Some(mut cover) = Pixmap::new(*side, *side) {
                    copy_rgba(&mut cover, rgba, *side);
                    let scale = art_side / *side as f32;
                    // Clip the cover to a rounded square by drawing it through
                    // a mask-shaped path: draw the rounded rect, then the image
                    // with SourceIn-like behaviour via a clip mask.
                    if let Some(mask_path) =
                        art::rounded_rect(pad, pad, art_side, art_side, 6.0 * s)
                    {
                        let mut mask = tiny_skia::Mask::new(pm.width(), pm.height());
                        if let Some(mask) = mask.as_mut() {
                            mask.fill_path(
                                &mask_path,
                                FillRule::Winding,
                                true,
                                Transform::identity(),
                            );
                            pm.draw_pixmap(
                                0,
                                0,
                                cover.as_ref(),
                                &PixmapPaint::default(),
                                Transform::from_scale(scale, scale).post_translate(pad, pad),
                                Some(mask),
                            );
                            drew_cover = true;
                        }
                    }
                }
            }
        }
        if !drew_cover {
            let mut paint = Paint {
                anti_alias: true,
                ..Default::default()
            };
            paint.set_color(frame.palette.muted);
            if let Some(p) = art::rounded_rect(pad, pad, art_side, art_side, 6.0 * s) {
                pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
            }
            paint.set_color(frame.palette.bg);
            let inset = art_side * 0.18;
            if let Some(p) = art::mascot(pad + inset, pad + inset, art_side - 2.0 * inset) {
                pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
            }
        }

        let tx = pad + art_side + 14.0 * s;
        let avail = (w - tx - pad).max(0.0);
        let title_px = (16.0 * s).clamp(10.0, 28.0);
        let sub_px = (12.5 * s).clamp(9.0, 22.0);

        let (title, artist) = match &frame.track {
            Some(t) => (t.title.clone(), t.artist.clone()),
            None => ("Nothing playing".to_string(), String::new()),
        };

        let shown_title = self.text.elide(&title, title_px, avail);
        let tm = self.text.measure(&shown_title, title_px);
        let mut y = pad + tm.ascent;
        self.text
            .draw(pm, &shown_title, tx, y, title_px, frame.palette.fg, avail);

        if !artist.is_empty() {
            let shown = self.text.elide(&artist, sub_px, avail);
            let sm = self.text.measure(&shown, sub_px);
            y += sm.ascent + 4.0 * s;
            self.text
                .draw(pm, &shown, tx, y, sub_px, frame.palette.muted, avail);
        }

        if let Some(lyric) = &frame.lyric {
            if !lyric.is_empty() {
                let shown = self.text.elide(lyric, sub_px, avail);
                let sm = self.text.measure(&shown, sub_px);
                y += sm.ascent + 6.0 * s;
                self.text
                    .draw(pm, &shown, tx, y, sub_px, frame.palette.accent, avail);
            }
        }

        // Seek bar along the bottom of the text column.
        if let Some(fraction) = frame.progress_fraction {
            let bar_h = 3.0 * s;
            let bar_y = h - pad - bar_h - 14.0 * s;
            let mut paint = Paint {
                anti_alias: true,
                ..Default::default()
            };
            paint.set_color(frame.palette.border);
            if let Some(r) = Rect::from_xywh(tx, bar_y, avail, bar_h) {
                pm.fill_rect(r, &paint, Transform::identity(), None);
            }
            paint.set_color(frame.palette.accent);
            let played = (avail * fraction.clamp(0.0, 1.0) as f32).max(0.0);
            if let Some(r) = Rect::from_xywh(tx, bar_y, played, bar_h) {
                pm.fill_rect(r, &paint, Transform::identity(), None);
            }

            if let Some((elapsed, total)) = &frame.time_labels {
                let px = (10.5 * s).clamp(8.0, 18.0);
                let m = self.text.measure(elapsed, px);
                let baseline = bar_y + bar_h + m.ascent + 4.0 * s;
                self.text
                    .draw(pm, elapsed, tx, baseline, px, frame.palette.muted, avail);
                let tw = self.text.measure(total, px).width;
                self.text.draw(
                    pm,
                    total,
                    tx + avail - tw,
                    baseline,
                    px,
                    frame.palette.muted,
                    avail,
                );
            }
        }

        // Transport controls, bottom-right of the art column.
        let playing = frame.track.as_ref().is_some_and(|t| t.playing);
        let icon = 15.0 * s;
        let gap = 12.0 * s;
        let total_w = icon * 3.0 + gap * 2.0;
        let cx = tx + (avail - total_w).max(0.0) / 2.0;
        let cy = h - pad - icon;
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(frame.palette.fg);
        paint.blend_mode = BlendMode::SourceOver;
        if frame.track.is_some() {
            if let Some(p) = art::icon_prev(cx, cy, icon) {
                pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
            }
            let mid = cx + icon + gap;
            let p = if playing {
                art::icon_pause(mid, cy, icon)
            } else {
                art::icon_play(mid, cy, icon)
            };
            if let Some(p) = p {
                pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
            }
            if let Some(p) = art::icon_next(cx + (icon + gap) * 2.0, cy, icon) {
                pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
            }
        }
        let _ = PathBuilder::new();
    }
}

/// Copy straight RGBA8 bytes into a premultiplied pixmap.
fn copy_rgba(pm: &mut Pixmap, rgba: &[u8], side: u32) {
    let pixels = pm.pixels_mut();
    for i in 0..(side as usize * side as usize) {
        let o = i * 4;
        let (r, g, b, a) = match (
            rgba.get(o),
            rgba.get(o + 1),
            rgba.get(o + 2),
            rgba.get(o + 3),
        ) {
            (Some(r), Some(g), Some(b), Some(a)) => (*r, *g, *b, *a),
            _ => continue,
        };
        let af = f32::from(a) / 255.0;
        if let Some(p) = tiny_skia::PremultipliedColorU8::from_rgba(
            (f32::from(r) * af).round() as u8,
            (f32::from(g) * af).round() as u8,
            (f32::from(b) * af).round() as u8,
            a,
        ) {
            if let Some(slot) = pixels.get_mut(i) {
                *slot = p;
            }
        }
    }
}
