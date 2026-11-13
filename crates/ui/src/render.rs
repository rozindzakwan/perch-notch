//! Draws one frame of the notch into a `tiny-skia` pixmap.
//!
//! The renderer takes plain data (a [`Frame`] for geometry/colours and a
//! [`PanelView`] for content), paints it, and returns the [`Hit`] regions that
//! clicks resolve against. It holds no application state besides the glyph
//! cache, so it can be exercised entirely off-screen (`tests/render.rs`).

use perch_core::platform::TransportCommand;
use perch_core::view::{
    Action, Body, CollapsedView, Hit, HitKind, MediaView, PanelView, Row, SliderRow, TasksView,
    UsageBar,
};
use tiny_skia::{Color, FillRule, Paint, Pixmap, PixmapPaint, Stroke, Transform};

use crate::art;
use crate::text::TextRenderer;
use crate::theme::Palette;

/// Kept for API compatibility with older callers/tests: what the collapsed
/// pill shows. New code builds a [`PanelView`] instead.
pub type Collapsed = CollapsedView;

/// Geometry and colours for one frame.
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
    /// Paint the pill fully opaque (no compositor on X11: the window is
    /// clipped to the pill shape with XShape instead of alpha).
    pub opaque: bool,
}

impl Frame {
    /// A frame of the given size.
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
            opaque: false,
        }
    }
}

/// Paints frames. Holds the glyph cache.
pub struct Renderer {
    text: TextRenderer,
}

/// Per-frame drawing context.
struct Painter<'a> {
    pm: &'a mut Pixmap,
    text: &'a mut TextRenderer,
    pal: Palette,
    s: f32,
    hits: Vec<Hit>,
}

fn paint(color: Color) -> Paint<'static> {
    let mut p = Paint {
        anti_alias: true,
        ..Default::default()
    };
    p.set_color(color);
    p
}

fn with_alpha(c: Color, a: f32) -> Color {
    let mut c = c;
    c.apply_opacity(a);
    c
}

impl Painter<'_> {
    fn fill(&mut self, path: Option<tiny_skia::Path>, color: Color) {
        if let Some(p) = path {
            self.pm.fill_path(
                &p,
                &paint(color),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: Color) {
        self.fill(art::rounded_rect(x, y, w, h, r), color);
    }

    fn stroke(&mut self, path: Option<tiny_skia::Path>, color: Color, width: f32) {
        if let Some(p) = path {
            let st = Stroke {
                width,
                line_cap: tiny_skia::LineCap::Round,
                ..Default::default()
            };
            self.pm
                .stroke_path(&p, &paint(color), &st, Transform::identity(), None);
        }
    }

    /// Text vertically centred in a band `[top, top + band_h)`.
    fn text_in(
        &mut self,
        s: &str,
        x: f32,
        top: f32,
        band_h: f32,
        px: f32,
        color: Color,
        max_w: f32,
    ) -> f32 {
        if max_w <= 0.0 || s.is_empty() {
            return 0.0;
        }
        let shown = self.text.elide(s, px, max_w);
        let m = self.text.measure(&shown, px);
        let baseline = top + (band_h + m.ascent - m.descent) / 2.0;
        self.text
            .draw(self.pm, &shown, x, baseline, px, color, max_w)
    }

    fn measure(&mut self, s: &str, px: f32) -> f32 {
        self.text.measure(s, px).width
    }

    fn hit(&mut self, x: f32, y: f32, w: f32, h: f32, kind: HitKind) {
        if w > 0.0 && h > 0.0 {
            self.hits.push(Hit { x, y, w, h, kind });
        }
    }

    fn button(&mut self, x: f32, y: f32, w: f32, h: f32, action: Action) {
        self.hit(x, y, w, h, HitKind::Button(action));
    }

    /// A small text button; returns its width.
    fn text_button(&mut self, label: &str, right: f32, y: f32, h: f32, action: Action) -> f32 {
        let s = self.s;
        let px = 11.5 * s;
        let w = self.measure(label, px) + 14.0 * s;
        let x = right - w;
        self.rrect(x, y, w, h, h / 2.0, with_alpha(self.pal.fg, 0.10));
        self.text_in(label, x + 7.0 * s, y, h, px, self.pal.fg, w);
        self.button(x, y, w, h, action);
        w
    }
}

impl Renderer {
    /// Build a renderer, loading the embedded font.
    pub fn new() -> perch_core::Result<Self> {
        Ok(Self {
            text: TextRenderer::new()?,
        })
    }

    /// Measure text (used by the form windows for layout).
    pub fn text_mut(&mut self) -> &mut TextRenderer {
        &mut self.text
    }

    /// Paint a frame and return its hit regions. Returns an empty list (and
    /// paints nothing) when the pixmap is degenerate.
    pub fn draw(&mut self, pm: &mut Pixmap, frame: &Frame, view: &PanelView) -> Vec<Hit> {
        if pm.width() == 0 || pm.height() == 0 {
            return Vec::new();
        }
        pm.fill(Color::TRANSPARENT);
        let w = pm.width() as f32;
        let h = pm.height() as f32;
        let s = frame.scale;
        let mut pal = frame.palette;
        if frame.opaque {
            pal.bg.set_alpha(1.0);
        }
        let mut p = Painter {
            pm,
            text: &mut self.text,
            pal,
            s,
            hits: Vec::new(),
        };

        // Pill body + hairline border.
        let radius = frame
            .corner_radius
            .map(|r| r * s)
            .or(Some(if frame.progress >= 0.5 {
                18.0 * s
            } else {
                h / 2.0
            }));
        let body = art::pill(0.0, 0.0, w, h, radius);
        p.fill(body.clone(), pal.bg);
        p.stroke(body, pal.border, 1.0_f32.max(s * 0.75));

        if frame.progress < 0.5 {
            draw_collapsed(&mut p, &view.collapsed, w, h);
        } else {
            draw_expanded(&mut p, view, w, h);
        }
        p.hits
    }
}

fn draw_collapsed(p: &mut Painter<'_>, collapsed: &CollapsedView, w: f32, h: f32) {
    let s = p.s;
    let pad = 10.0 * s;
    let icon = (h - 2.0 * pad).clamp(8.0 * s, 22.0 * s);
    let fg = p.pal.fg;
    let accent = p.pal.accent;
    p.fill(art::mascot(pad, (h - icon) / 2.0, icon), fg);
    p.fill(art::mascot_eye(pad, (h - icon) / 2.0, icon), accent);

    let (label, color) = match collapsed {
        CollapsedView::Idle => return,
        CollapsedView::Status(t) | CollapsedView::Message(t) => (t.as_str(), accent),
        CollapsedView::NowPlaying(t) => (t.as_str(), fg),
    };
    let px = (h * 0.42).clamp(9.0, 20.0 * s);
    let x = pad + icon + 8.0 * s;
    p.text_in(label, x, 0.0, h, px, color, (w - x - pad).max(0.0));
}

fn draw_expanded(p: &mut Painter<'_>, view: &PanelView, w: f32, h: f32) {
    let s = p.s;
    let pad = 14.0 * s;
    let strip_h = 22.0 * s;
    let top = 10.0 * s;

    // Tab strip.
    let mut x = pad;
    let px = 12.0 * s;
    let icons_w = 2.0 * strip_h + 6.0 * s;
    for tab in &view.tabs {
        let label = tab.label();
        let tw = p.measure(label, px) + 16.0 * s;
        if x + tw > w - pad - icons_w {
            break;
        }
        let selected = view.tab == Some(*tab);
        if selected {
            p.rrect(
                x,
                top,
                tw,
                strip_h,
                strip_h / 2.0,
                with_alpha(p.pal.fg, 0.14),
            );
        }
        let color = if selected { p.pal.fg } else { p.pal.muted };
        p.text_in(label, x + 8.0 * s, top, strip_h, px, color, tw);
        p.button(x, top, tw, strip_h, Action::SelectTab(*tab));
        x += tw + 2.0 * s;
    }

    // Pin + settings icons, right-aligned.
    let gx = w - pad - strip_h;
    let gear_color = p.pal.muted;
    p.stroke(
        art::gear(gx + strip_h * 0.2, top + strip_h * 0.2, strip_h * 0.6),
        gear_color,
        1.4 * s,
    );
    p.button(gx, top, strip_h, strip_h, Action::OpenSettings);
    let pin_x = gx - strip_h - 6.0 * s;
    let pin_color = if view.pinned {
        p.pal.accent
    } else {
        p.pal.muted
    };
    p.fill(
        art::pin(pin_x + strip_h * 0.22, top + strip_h * 0.18, strip_h * 0.56),
        pin_color,
    );
    p.button(pin_x, top, strip_h, strip_h, Action::PinToggle);

    // Body.
    let by = top + strip_h + 8.0 * s;
    let bh = (h - by - 12.0 * s).max(0.0);
    let bx = pad;
    let bw = (w - 2.0 * pad).max(0.0);
    match &view.body {
        Body::Media(m) => draw_media(p, m, bx, by, bw, bh),
        Body::Tasks(t) => draw_tasks(p, t, bx, by, bw, bh),
        Body::List {
            rows,
            empty,
            header,
        } => {
            let mut y = by;
            if let Some((label, action)) = header {
                let hh = 18.0 * s;
                p.text_button(label, bx + bw, y, hh, action.clone());
                let count = format!(
                    "{} item{}",
                    rows.len(),
                    if rows.len() == 1 { "" } else { "s" }
                );
                let muted = p.pal.muted;
                p.text_in(&count, bx, y, hh, 11.0 * s, muted, bw / 2.0);
                y += hh + 4.0 * s;
            }
            draw_rows(p, rows, empty, bx, y, bw, by + bh - y);
        }
        Body::Sliders { rows, empty } => draw_sliders(p, rows, empty, bx, by, bw, bh),
        Body::Usage { total, bars } => draw_usage(p, total, bars, bx, by, bw, bh),
        Body::Buttons(buttons) => draw_buttons(p, buttons, bx, by, bw, bh),
        Body::Empty(text) => {
            let muted = p.pal.muted;
            p.text_in(text, bx, by, bh, 12.0 * s, muted, bw);
        }
    }
}

fn draw_media(p: &mut Painter<'_>, m: &MediaView, x: f32, y: f32, w: f32, h: f32) {
    let s = p.s;
    let art_side = h.min(w * 0.3).max(8.0 * s);
    // Cover art or a placeholder with the mascot.
    let mut drew = false;
    if let Some((rgba, side)) = &m.cover {
        if *side > 0 && rgba.len() as u32 >= side * side * 4 {
            if let Some(mut cover) = Pixmap::new(*side, *side) {
                copy_rgba(&mut cover, rgba, *side);
                let k = art_side / *side as f32;
                if let (Some(mask_path), Some(mut mask)) = (
                    art::rounded_rect(x, y, art_side, art_side, 8.0 * s),
                    tiny_skia::Mask::new(p.pm.width(), p.pm.height()),
                ) {
                    mask.fill_path(&mask_path, FillRule::Winding, true, Transform::identity());
                    p.pm.draw_pixmap(
                        0,
                        0,
                        cover.as_ref(),
                        &PixmapPaint {
                            quality: tiny_skia::FilterQuality::Bilinear,
                            ..Default::default()
                        },
                        Transform::from_scale(k, k).post_translate(x, y),
                        Some(&mask),
                    );
                    drew = true;
                }
            }
        }
    }
    if !drew {
        let muted = with_alpha(p.pal.fg, 0.12);
        p.rrect(x, y, art_side, art_side, 8.0 * s, muted);
        let inset = art_side * 0.2;
        let fg = with_alpha(p.pal.fg, 0.55);
        p.fill(
            art::mascot(x + inset, y + inset, art_side - 2.0 * inset),
            fg,
        );
    }

    let tx = x + art_side + 14.0 * s;
    let tw = (x + w - tx).max(0.0);
    let title_px = 15.0 * s;
    let sub_px = 12.0 * s;
    let fg = p.pal.fg;
    let muted = p.pal.muted;
    let accent = p.pal.accent;
    let mut ty = y;
    p.text_in(&m.title, tx, ty, 20.0 * s, title_px, fg, tw);
    ty += 20.0 * s;
    if !m.artist.is_empty() {
        p.text_in(&m.artist, tx, ty, 16.0 * s, sub_px, muted, tw);
    }
    ty += 18.0 * s;
    if let Some(l) = &m.lyric {
        p.text_in(l, tx, ty, 16.0 * s, sub_px, accent, tw);
    }

    if !m.has_player {
        return;
    }
    // Controls + seek bar along the bottom of the text column.
    let icon = 16.0 * s;
    let gap = 16.0 * s;
    let cy = y + h - icon;
    let total = icon * 3.0 + gap * 2.0;
    let cx = tx;
    p.fill(art::icon_prev(cx, cy, icon), fg);
    p.button(
        cx - 4.0 * s,
        cy - 4.0 * s,
        icon + 8.0 * s,
        icon + 8.0 * s,
        Action::Transport(TransportCommand::Previous),
    );
    let mid = cx + icon + gap;
    let play = if m.playing {
        art::icon_pause(mid, cy, icon)
    } else {
        art::icon_play(mid, cy, icon)
    };
    p.fill(play, fg);
    p.button(
        mid - 4.0 * s,
        cy - 4.0 * s,
        icon + 8.0 * s,
        icon + 8.0 * s,
        Action::Transport(TransportCommand::PlayPause),
    );
    let nx = cx + 2.0 * (icon + gap);
    p.fill(art::icon_next(nx, cy, icon), fg);
    p.button(
        nx - 4.0 * s,
        cy - 4.0 * s,
        icon + 8.0 * s,
        icon + 8.0 * s,
        Action::Transport(TransportCommand::Next),
    );

    if let Some(f) = m.fraction {
        let bar_x = cx + total + 14.0 * s;
        let bar_w = (x + w - bar_x).max(0.0);
        let bar_y = cy + icon / 2.0 - 1.5 * s;
        if bar_w > 20.0 * s {
            let border = with_alpha(p.pal.fg, 0.18);
            p.rrect(bar_x, bar_y, bar_w, 3.0 * s, 1.5 * s, border);
            p.rrect(
                bar_x,
                bar_y,
                bar_w * f.clamp(0.0, 1.0) as f32,
                3.0 * s,
                1.5 * s,
                accent,
            );
            p.hit(
                bar_x,
                bar_y - 8.0 * s,
                bar_w,
                19.0 * s,
                HitKind::Slider(perch_core::view::SliderTarget::Seek),
            );
            if let Some((a, b)) = &m.times {
                let px = 10.0 * s;
                p.text_in(a, bar_x, bar_y - 16.0 * s, 12.0 * s, px, muted, bar_w / 2.0);
                let bw = p.measure(b, px);
                p.text_in(
                    b,
                    bar_x + bar_w - bw,
                    bar_y - 16.0 * s,
                    12.0 * s,
                    px,
                    muted,
                    bw + 1.0,
                );
            }
        }
    }
}

fn draw_tasks(p: &mut Painter<'_>, t: &TasksView, x: f32, y: f32, w: f32, h: f32) {
    let s = p.s;
    let row_h = 20.0 * s;
    let mut cy = y;
    let fg = p.pal.fg;
    let muted = p.pal.muted;
    let accent = p.pal.accent;
    if let Some(pomo) = &t.pomodoro {
        // [play/pause] Focus 24:13   [skip] [reset]          Water in 12 min
        let icon = 14.0 * s;
        let iy = cy + (row_h - icon) / 2.0;
        let shape = if pomo.running {
            art::icon_pause(x, iy, icon)
        } else {
            art::icon_play(x, iy, icon)
        };
        p.fill(shape, accent);
        p.button(
            x - 3.0 * s,
            cy,
            icon + 6.0 * s,
            row_h,
            Action::PomodoroToggle,
        );
        let lw = p.text_in(
            &pomo.label,
            x + icon + 8.0 * s,
            cy,
            row_h,
            13.0 * s,
            fg,
            w * 0.45,
        );
        let mut bx = x + icon + 16.0 * s + lw;
        p.fill(art::icon_next(bx, iy, icon * 0.9), muted);
        p.button(
            bx - 3.0 * s,
            cy,
            icon + 6.0 * s,
            row_h,
            Action::PomodoroSkip,
        );
        bx += icon + 10.0 * s;
        p.stroke(art::reset(bx, iy, icon * 0.9), muted, 1.4 * s);
        p.button(
            bx - 3.0 * s,
            cy,
            icon + 6.0 * s,
            row_h,
            Action::PomodoroReset,
        );
    }
    if let Some(r) = &t.reminder {
        let rw = p.measure(r, 11.0 * s);
        p.text_in(r, x + w - rw, cy, row_h, 11.0 * s, muted, rw + 1.0);
    }
    if t.pomodoro.is_some() || t.reminder.is_some() {
        cy += row_h + 4.0 * s;
    }
    if let Some(rows) = &t.todos {
        let hh = 18.0 * s;
        p.text_button("+ Add", x + w, cy, hh, Action::TodoAdd);
        let pending = rows.iter().filter(|r| r.check == Some(false)).count();
        p.text_in(
            &format!("To-do  ({pending} open)"),
            x,
            cy,
            hh,
            11.0 * s,
            muted,
            w / 2.0,
        );
        cy += hh + 4.0 * s;
        draw_rows(p, rows, "Nothing to do. Nice.", x, cy, w, y + h - cy);
    }
}

fn draw_rows(p: &mut Painter<'_>, rows: &[Row], empty: &str, x: f32, y: f32, w: f32, h: f32) {
    let s = p.s;
    let row_h = 20.0 * s;
    let muted = p.pal.muted;
    if rows.is_empty() {
        p.text_in(empty, x, y, row_h, 12.0 * s, muted, w);
        return;
    }
    let mut cy = y;
    for row in rows {
        if cy + row_h > y + h + 0.5 {
            break;
        }
        let color = if row.dim { muted } else { p.pal.fg };
        let mut tx = x;
        if let Some(checked) = row.check {
            let b = 12.0 * s;
            let by = cy + (row_h - b) / 2.0;
            p.stroke(art::rounded_rect(tx, by, b, b, 3.0 * s), muted, 1.2 * s);
            if checked {
                let accent = p.pal.accent;
                p.stroke(
                    art::check(tx + 2.0 * s, by + 2.0 * s, b - 4.0 * s),
                    accent,
                    1.6 * s,
                );
            }
            tx += b + 8.0 * s;
        }
        let close = 14.0 * s;
        let detail_w = if row.detail.is_empty() {
            0.0
        } else {
            p.measure(&row.detail, 10.5 * s) + 8.0 * s
        };
        let text_w = (x + w - tx - detail_w - close - 6.0 * s).max(0.0);
        p.text_in(&row.text, tx, cy, row_h, 12.5 * s, color, text_w);
        if detail_w > 0.0 {
            p.text_in(
                &row.detail,
                x + w - close - 6.0 * s - detail_w,
                cy,
                row_h,
                10.5 * s,
                muted,
                detail_w,
            );
        }
        if let Some(a) = &row.primary {
            p.button(x, cy, w - close - 4.0 * s, row_h, a.clone());
        }
        if let Some(a) = &row.remove {
            let cx = x + w - close;
            p.stroke(
                art::cross(
                    cx + 3.0 * s,
                    cy + (row_h - close) / 2.0 + 3.0 * s,
                    close - 6.0 * s,
                ),
                muted,
                1.3 * s,
            );
            p.button(cx - 2.0 * s, cy, close + 4.0 * s, row_h, a.clone());
        }
        cy += row_h;
    }
}

fn draw_sliders(
    p: &mut Painter<'_>,
    rows: &[SliderRow],
    empty: &str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) {
    let s = p.s;
    let row_h = 24.0 * s;
    let muted = p.pal.muted;
    if rows.is_empty() {
        p.text_in(empty, x, y, row_h, 12.0 * s, muted, w);
        return;
    }
    let label_w = (w * 0.32).min(150.0 * s);
    let mut cy = y;
    for row in rows {
        if cy + row_h > y + h + 0.5 {
            break;
        }
        let dot = 9.0 * s;
        let accent = p.pal.accent;
        let dot_color = if row.on {
            accent
        } else {
            with_alpha(p.pal.fg, 0.25)
        };
        p.rrect(x, cy + (row_h - dot) / 2.0, dot, dot, dot / 2.0, dot_color);
        let color = if row.on { p.pal.fg } else { muted };
        p.text_in(
            &row.label,
            x + dot + 8.0 * s,
            cy,
            row_h,
            12.5 * s,
            color,
            label_w - dot - 8.0 * s,
        );
        p.button(x - 2.0 * s, cy, label_w, row_h, row.toggle.clone());

        let pct = format!("{:>3}%", (row.value * 100.0).round() as i32);
        let pct_w = 34.0 * s;
        let sx = x + label_w + 6.0 * s;
        let sw = (x + w - sx - pct_w).max(0.0);
        let track_y = cy + row_h / 2.0 - 2.0 * s;
        let track = with_alpha(p.pal.fg, 0.18);
        p.rrect(sx, track_y, sw, 4.0 * s, 2.0 * s, track);
        let fill = if row.on { accent } else { muted };
        p.rrect(
            sx,
            track_y,
            sw * row.value.clamp(0.0, 1.0),
            4.0 * s,
            2.0 * s,
            fill,
        );
        let knob = 10.0 * s;
        let fg = p.pal.fg;
        p.rrect(
            sx + sw * row.value.clamp(0.0, 1.0) - knob / 2.0,
            cy + (row_h - knob) / 2.0,
            knob,
            knob,
            knob / 2.0,
            fg,
        );
        p.hit(sx, cy, sw, row_h, HitKind::Slider(row.target.clone()));
        p.text_in(
            &pct,
            x + w - pct_w + 4.0 * s,
            cy,
            row_h,
            11.0 * s,
            muted,
            pct_w,
        );
        cy += row_h;
    }
}

fn draw_usage(p: &mut Painter<'_>, total: &str, bars: &[UsageBar], x: f32, y: f32, w: f32, h: f32) {
    let s = p.s;
    let fg = p.pal.fg;
    let muted = p.pal.muted;
    let accent = p.pal.accent;
    p.text_in(total, x, y, 18.0 * s, 13.0 * s, fg, w);
    let mut cy = y + 22.0 * s;
    let row_h = 18.0 * s;
    if bars.is_empty() {
        p.text_in(
            "No activity recorded yet today.",
            x,
            cy,
            row_h,
            12.0 * s,
            muted,
            w,
        );
        return;
    }
    let name_w = (w * 0.3).min(140.0 * s);
    for bar in bars {
        if cy + row_h > y + h + 0.5 {
            break;
        }
        p.text_in(&bar.app, x, cy, row_h, 11.5 * s, fg, name_w - 6.0 * s);
        let label_w = p.measure(&bar.label, 10.5 * s) + 6.0 * s;
        let bx = x + name_w;
        let bw = (w - name_w - label_w).max(0.0);
        let track = with_alpha(p.pal.fg, 0.10);
        p.rrect(bx, cy + row_h / 2.0 - 3.0 * s, bw, 6.0 * s, 3.0 * s, track);
        p.rrect(
            bx,
            cy + row_h / 2.0 - 3.0 * s,
            (bw * bar.fraction.clamp(0.0, 1.0)).max(6.0 * s),
            6.0 * s,
            3.0 * s,
            accent,
        );
        p.text_in(
            &bar.label,
            x + w - label_w + 6.0 * s,
            cy,
            row_h,
            10.5 * s,
            muted,
            label_w,
        );
        cy += row_h + 2.0 * s;
    }
}

fn draw_buttons(p: &mut Painter<'_>, buttons: &[(String, Action)], x: f32, y: f32, w: f32, h: f32) {
    let s = p.s;
    if buttons.is_empty() {
        let muted = p.pal.muted;
        p.text_in("No tools enabled.", x, y, 20.0 * s, 12.0 * s, muted, w);
        return;
    }
    let n = buttons.len() as f32;
    let gap = 10.0 * s;
    let bw = ((w - gap * (n - 1.0)) / n).max(0.0);
    let bh = (h * 0.6).clamp(28.0 * s, 56.0 * s);
    let by = y + (h - bh) / 2.0;
    for (i, (label, action)) in buttons.iter().enumerate() {
        let bx = x + i as f32 * (bw + gap);
        let bg = with_alpha(p.pal.fg, 0.10);
        p.rrect(bx, by, bw, bh, 10.0 * s, bg);
        let px = 13.0 * s;
        let lw = p.measure(label, px);
        let fg = p.pal.fg;
        p.text_in(label, bx + (bw - lw) / 2.0, by, bh, px, fg, bw);
        p.button(bx, by, bw, bh, action.clone());
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
        if let Some(px) = tiny_skia::PremultipliedColorU8::from_rgba(
            (f32::from(r) * af).round() as u8,
            (f32::from(g) * af).round() as u8,
            (f32::from(b) * af).round() as u8,
            a,
        ) {
            if let Some(slot) = pixels.get_mut(i) {
                *slot = px;
            }
        }
    }
}
