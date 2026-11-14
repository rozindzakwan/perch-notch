//! Draws one frame of the notch into a `tiny-skia` pixmap.
//!
//! The renderer takes plain data (a [`Frame`] for geometry/colours and a
//! [`PanelView`] for content), paints it, and returns the [`Hit`] regions that
//! clicks resolve against. It holds no application state besides the glyph
//! and shadow caches, so it can be exercised entirely off-screen
//! (`tests/render.rs`).

use perch_core::platform::TransportCommand;
use perch_core::view::{
    Action, BadgeKind, Body, CollapsedView, Glyph, Hit, HitKind, MediaView, PanelView, Row,
    SliderRow, TasksView, UsageBar,
};
use tiny_skia::{Color, FillRule, Mask, Paint, Pixmap, PixmapPaint, Stroke, Transform};

use crate::art;
use crate::effects::Effects;
use crate::glass::{self, Ctx};
use crate::icons::Icon;
use crate::text::TextRenderer;
use crate::theme::Palette;
use crate::tokens::{radius, text, Weight};

/// Kept for API compatibility with older callers/tests: what the collapsed
/// pill shows. New code builds a [`PanelView`] instead.
pub type Collapsed = CollapsedView;

/// Panel header: padding + tab bar + gap, in logical px.
const HEADER: f32 = 12.0 + 30.0 + 10.0;
/// Content becomes clickable once at least this visible.
const CLICKABLE_ALPHA: f32 = 0.5;

/// Geometry and colours for one frame.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Pixmap width in physical pixels.
    pub width: u32,
    /// Pixmap height in physical pixels.
    pub height: u32,
    /// Monitor scale.
    pub scale: f32,
    /// Open-ness from the notch (0 collapsed, 1 expanded; may slightly exceed 1).
    pub progress: f32,
    /// Colours.
    pub palette: Palette,
    /// Corner radius override in logical px.
    pub corner_radius: Option<f32>,
    /// No compositor: draw an opaque surface (the shell clips with XShape).
    pub opaque: bool,
    /// Opacity of the panel content (fades in after the panel opens).
    pub content_alpha: f32,
    /// Animated position of the active tab highlight, in tab units; negative
    /// = use the active tab's index.
    pub tab_x: f32,
    /// Cursor position in pixmap pixels, for hover states.
    pub cursor: Option<(f32, f32)>,
    /// The OS blurs what is behind the window.
    pub blur: bool,
    /// Transparent margin (left, top, right, bottom) around the surface,
    /// holding its shadow. Physical px.
    pub inset: (f32, f32, f32, f32),
    /// Size the panel content is laid out at (physical px), normally the
    /// final expanded size, so content never reflows while the panel grows.
    pub layout: Option<(f32, f32)>,
    /// Shadow blur in logical px (0 = none).
    pub shadow: f32,
}

impl Frame {
    /// A frame with sensible defaults.
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
            content_alpha: 1.0,
            tab_x: -1.0,
            cursor: None,
            blur: false,
            inset: (0.0, 0.0, 0.0, 0.0),
            layout: None,
            shadow: 0.0,
        }
    }
}

/// Paints frames. Holds the glyph and shadow caches.
pub struct Renderer {
    text: TextRenderer,
    fx: Effects,
}

/// Per-frame drawing context for the screens not yet moved to `glass`.
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
    /// Build a renderer, loading the embedded fonts.
    pub fn new() -> perch_core::Result<Self> {
        Ok(Self {
            text: TextRenderer::new()?,
            fx: Effects::default(),
        })
    }

    /// Measure text (used by the form windows for layout).
    pub fn text_mut(&mut self) -> &mut TextRenderer {
        &mut self.text
    }

    /// Logical width the collapsed pill wants for its content (0 = no
    /// preference; the shell clamps to its configured range).
    pub fn pill_width(&mut self, collapsed: &CollapsedView, scale: f32) -> u32 {
        let s = scale.max(0.1);
        let w = |t: &mut TextRenderer, str: &str, px: f32, wt: Weight| {
            t.measure_w(str, px * s, wt).width / s
        };
        let width = match collapsed {
            CollapsedView::Idle => 0.0,
            CollapsedView::Message(m) => 44.0 + w(&mut self.text, m, text::BODY.0, Weight::Medium),
            CollapsedView::Pill { text, badges, .. } => {
                let mut total = 8.0 + 20.0 + 10.0 + 12.0;
                if !text.is_empty() {
                    total += w(&mut self.text, text, text::BODY.0, Weight::Medium);
                }
                for b in badges {
                    total += 8.0 + badge_width(&mut self.text, b.count, 1.0);
                }
                total
            }
        };
        width.ceil() as u32
    }

    /// Paint a frame and return its hit regions. Returns an empty list (and
    /// paints nothing) when the pixmap is degenerate.
    pub fn draw(&mut self, pm: &mut Pixmap, frame: &Frame, view: &PanelView) -> Vec<Hit> {
        if pm.width() == 0 || pm.height() == 0 {
            return Vec::new();
        }
        pm.fill(Color::TRANSPARENT);
        let (w, h) = (pm.width() as f32, pm.height() as f32);
        let s = frame.scale;
        let mut pal = frame.palette;
        if frame.opaque {
            pal.surface.set_alpha(1.0);
            pal.bg.set_alpha(1.0);
        }
        let (il, it, ir, ib) = frame.inset;
        let panel = (il, it, (w - il - ir).max(1.0), (h - it - ib).max(1.0));
        let open = frame.progress >= 0.5;
        let r = frame.corner_radius.map(|r| r * s).unwrap_or(if open {
            radius::PANEL * s
        } else {
            panel.3 / 2.0
        });
        let mut ctx = Ctx::new(pm, &mut self.text, &mut self.fx, pal, s);
        ctx.cursor = frame.cursor;
        glass::surface(&mut ctx, panel, r, None, 0.0);

        if open {
            if frame.content_alpha > 0.0 {
                ctx.alpha = frame.content_alpha.clamp(0.0, 1.0);
                draw_panel(&mut ctx, view, panel, frame);
            }
        } else {
            // The pill's own content fades out as the panel starts to grow.
            ctx.alpha = (1.0 - frame.progress / 0.3).clamp(0.0, 1.0);
            if ctx.alpha > 0.0 {
                draw_pill(&mut ctx, &view.collapsed, panel);
            }
            // The collapsed pill is hover-only: nothing to click.
            ctx.hits.clear();
        }
        let mut hits = std::mem::take(&mut ctx.hits);
        drop(ctx);

        // Content is laid out at the final size; while the panel is still
        // growing, cut everything outside its rounded shape.
        let settled = frame
            .layout
            .map(|(lw, lh)| (lw - panel.2).abs() < 0.5 && (lh - panel.3).abs() < 0.5);
        if open && settled == Some(false) {
            clip_outside(pm, panel, r);
        }
        if open && frame.content_alpha < CLICKABLE_ALPHA {
            hits.clear();
        }
        hits.retain(|hit| {
            hit.x >= panel.0 - 0.5
                && hit.y >= panel.1 - 0.5
                && hit.x + hit.w <= panel.0 + panel.2 + 0.5
                && hit.y + hit.h <= panel.1 + panel.3 + 0.5
        });
        if frame.shadow > 0.0 && !frame.opaque {
            self.fx.shadow(
                pm,
                panel.0,
                panel.1,
                panel.2,
                panel.3,
                r,
                frame.shadow * s,
                0.40,
            );
        }
        hits
    }
}

/// Zero every pixel outside the rounded rect.
fn clip_outside(pm: &mut Pixmap, (x, y, w, h): (f32, f32, f32, f32), r: f32) {
    let Some(mut mask) = Mask::new(pm.width(), pm.height()) else {
        return;
    };
    let Some(path) = art::rounded_rect(x, y, w, h, r) else {
        return;
    };
    mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
    for (px, m) in pm.pixels_mut().iter_mut().zip(mask.data()) {
        match *m {
            255 => {}
            0 => *px = tiny_skia::PremultipliedColorU8::TRANSPARENT,
            m => {
                let k = |c: u8| ((u16::from(c) * u16::from(m) + 127) / 255) as u8;
                if let Some(c) = tiny_skia::PremultipliedColorU8::from_rgba(
                    k(px.red()),
                    k(px.green()),
                    k(px.blue()),
                    k(px.alpha()),
                ) {
                    *px = c;
                }
            }
        }
    }
}

fn glyph_icon(g: Glyph) -> Icon {
    match g {
        Glyph::Music => Icon::Music,
        Glyph::Timer => Icon::Clock,
        Glyph::Mail => Icon::Mail,
        Glyph::Clock => Icon::Clock,
        Glyph::Warning => Icon::Warning,
    }
}

/// Logical width of one count badge (mail icon + number).
fn badge_width(t: &mut TextRenderer, count: u32, s: f32) -> f32 {
    let label = count_label(count);
    (8.0 + 12.0 + 4.0) * s
        + t.measure_tabular(&label, text::CAPTION.0 * s, Weight::Bold)
            .width
        + 8.0 * s
}

fn count_label(count: u32) -> String {
    if count > 99 {
        "99+".into()
    } else {
        count.to_string()
    }
}

/// The collapsed pill: mascot when idle; otherwise cover/glyph, text and
/// badges.
fn draw_pill(ctx: &mut Ctx<'_>, collapsed: &CollapsedView, (x, y, w, h): (f32, f32, f32, f32)) {
    let s = ctx.s;
    let inner = 8.0 * s;
    match collapsed {
        CollapsedView::Idle => {
            let icon = (h - 2.0 * 6.0 * s).clamp(8.0 * s, 22.0 * s);
            let (mx, my) = (x + 10.0 * s, y + (h - icon) / 2.0);
            let fg = ctx.pal.fg;
            let eye = ctx.pal.accent.from;
            ctx.fill(art::mascot(mx, my, icon), fg);
            ctx.fill(art::mascot_eye(mx, my, icon), eye);
        }
        CollapsedView::Message(m) => {
            let tw = ctx.measure(m, text::BODY).min(w - 2.0 * inner);
            let accent = ctx.pal.accent.from;
            ctx.text(m, x + (w - tw) / 2.0, y, h, text::BODY, accent, tw);
        }
        CollapsedView::Pill {
            thumb,
            glyph,
            text: label,
            badges,
        } => {
            let lead = (h - 12.0 * s).clamp(10.0 * s, 20.0 * s);
            let lx = x + inner;
            let ly = y + (h - lead) / 2.0;
            let mut tx = lx;
            if let Some((px, side)) = thumb {
                glass::image(ctx, px, *side, (lx, ly, lead, lead), lead * 0.3);
                tx += lead + 10.0 * s;
            } else if let Some(g) = glyph {
                let accent = ctx.pal.accent.from;
                ctx.icon(glyph_icon(*g), lx, ly, lead, accent);
                tx += lead + 10.0 * s;
            }
            // Badges from the right.
            let mut right = x + w - inner;
            let bh = (h - 12.0 * s).clamp(12.0 * s, 20.0 * s);
            for b in badges.iter().rev() {
                let bw = badge_width(ctx.text, b.count, s);
                let bx = right - bw;
                let by = y + (h - bh) / 2.0;
                let (bg, fg) = match b.kind {
                    BadgeKind::Mail => (
                        Color::from_rgba8(120, 170, 255, 64),
                        Color::from_rgba8(188, 212, 255, 255),
                    ),
                };
                ctx.rrect((bx, by, bw, bh), bh / 2.0, bg);
                let isz = 12.0 * s;
                ctx.icon(Icon::Mail, bx + 8.0 * s, by + (bh - isz) / 2.0, isz, fg);
                let label = count_label(b.count);
                let px = text::CAPTION.0 * s;
                let m = ctx.text.measure_tabular(&label, px, Weight::Bold);
                let base = by + (bh + m.ascent - m.descent) / 2.0;
                let c = ctx.fade(fg, 1.0);
                ctx.text.draw_tabular(
                    ctx.pm,
                    &label,
                    bx + 24.0 * s,
                    base,
                    px,
                    c,
                    m.width + 2.0,
                    Weight::Bold,
                );
                right = bx - 8.0 * s;
            }
            if !label.is_empty() {
                let fg = ctx.pal.fg;
                ctx.text(label, tx, y, h, text::BODY, fg, (right - tx).max(0.0));
            }
        }
    }
}

/// Panel: icon tabs, pin and settings, then the body, laid out at the final
/// expanded size.
fn draw_panel(ctx: &mut Ctx<'_>, view: &PanelView, panel: (f32, f32, f32, f32), frame: &Frame) {
    let s = ctx.s;
    let (lw, lh) = frame.layout.unwrap_or((panel.2, panel.3));
    // Music glows in the cover's colours.
    if let Body::Media(m) = &view.body {
        if m.has_player {
            let g = media_accent(ctx, m);
            let (a, b) = (ctx.fade(g.from, 1.0), ctx.fade(g.to, 1.0));
            let r = frame
                .corner_radius
                .map(|r| r * s)
                .unwrap_or(radius::PANEL * s);
            ctx.fx.glow(ctx.pm, panel, r, (a, b));
        }
    }
    // Centre the layout horizontally, anchor it to the top.
    let x = panel.0 + (panel.2 - lw) / 2.0;
    let y = panel.1;
    let pad = 12.0 * s;
    let bar = 30.0 * s;
    let btn = 26.0 * s;

    // Pin + settings on the right.
    let gear_x = x + lw - pad - btn;
    let pin_x = gear_x - btn - 4.0 * s;
    let by = y + pad + (bar - btn) / 2.0;
    for (bx, icon, action, active) in [
        (pin_x, Icon::Pin, Action::PinToggle, view.pinned),
        (gear_x, Icon::Gear, Action::OpenSettings, false),
    ] {
        let rect = (bx, by, btn, btn);
        if ctx.hovered(rect) {
            ctx.rrect(rect, btn / 2.0, ctx.pal.raised[1]);
        }
        let color = if active {
            ctx.pal.accent.from
        } else {
            ctx.fade(ctx.pal.fg, 0.7)
        };
        let isz = 15.0 * s;
        ctx.icon(
            icon,
            bx + (btn - isz) / 2.0,
            by + (btn - isz) / 2.0,
            isz,
            color,
        );
        ctx.button(rect, action);
    }

    // Tabs.
    let items: Vec<(Icon, Action)> = view
        .tabs
        .iter()
        .map(|t| (Icon::for_tab(*t), Action::SelectTab(*t)))
        .collect();
    if !items.is_empty() {
        let active = view
            .tab
            .and_then(|t| view.tabs.iter().position(|x| *x == t))
            .unwrap_or(0);
        let hx = if frame.tab_x >= 0.0 {
            frame.tab_x
        } else {
            active as f32
        };
        let tw = (pin_x - 8.0 * s - (x + pad)).max(0.0);
        glass::tabs(ctx, (x + pad, y + pad, tw, bar), &items, active, hx);
    }

    let body = (
        x + pad,
        y + HEADER * s,
        lw - 2.0 * pad,
        (lh - HEADER * s - pad).max(0.0),
    );
    match &view.body {
        Body::Media(m) => return music(ctx, m, body),
        Body::Tasks(t) => return tasks(ctx, t, body),
        Body::List {
            rows,
            empty,
            header,
        } => return list(ctx, rows, empty, header.as_ref(), body),
        _ => {}
    }
    // Screens not yet restyled go through the older painter.
    let mut pal = ctx.pal;
    let a = ctx.alpha;
    for c in [&mut pal.fg, &mut pal.muted, &mut pal.surface, &mut pal.bg] {
        c.apply_opacity(a);
    }
    pal.accent.from.apply_opacity(a);
    let mut p = Painter {
        pm: ctx.pm,
        text: ctx.text,
        pal,
        s,
        hits: Vec::new(),
    };
    draw_body(&mut p, view, body);
    let hits = std::mem::take(&mut p.hits);
    ctx.hits.extend(hits);
}

fn draw_body(p: &mut Painter<'_>, view: &PanelView, (bx, by, bw, bh): (f32, f32, f32, f32)) {
    let s = p.s;
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
    let accent = p.pal.accent.from;
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
    let accent = p.pal.accent.from;
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
                let accent = p.pal.accent.from;
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
        let accent = p.pal.accent.from;
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
    let accent = p.pal.accent.from;
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

/// Album colours as a gradient, or the coral fallback.
fn media_accent(ctx: &Ctx<'_>, m: &MediaView) -> crate::theme::Gradient {
    match m.accent {
        Some((a, b)) => crate::theme::Gradient {
            from: Color::from_rgba8(a[0], a[1], a[2], 255),
            to: Color::from_rgba8(b[0], b[1], b[2], 255),
        },
        None => ctx.pal.accent,
    }
}

/// Music: cover, title/artist, lyrics, gradient seek bar, transport.
fn music(ctx: &mut Ctx<'_>, m: &MediaView, (x, y, w, _h): (f32, f32, f32, f32)) {
    let s = ctx.s;
    let muted = ctx.fade(ctx.pal.fg, crate::tokens::SECONDARY_ALPHA);
    if !m.has_player {
        let isz = 34.0 * s;
        let cy = y + 30.0 * s;
        let c = ctx.fade(ctx.pal.fg, 0.35);
        ctx.icon(Icon::Music, x + (w - isz) / 2.0, cy, isz, c);
        let tw = ctx.measure(&m.title, text::TITLE);
        let fg = ctx.pal.fg;
        ctx.text(
            &m.title,
            x + (w - tw) / 2.0,
            cy + isz + 10.0 * s,
            22.0 * s,
            text::TITLE,
            fg,
            w,
        );
        let hint = "Play something in Spotify, a browser or any media app";
        let hw = ctx.measure(hint, text::SECONDARY).min(w);
        ctx.text(
            hint,
            x + (w - hw) / 2.0,
            cy + isz + 34.0 * s,
            18.0 * s,
            text::SECONDARY,
            muted,
            w,
        );
        return;
    }
    let g = media_accent(ctx, m);
    let side = 72.0 * s;
    let (cx, cy) = (x, y + 2.0 * s);
    match &m.cover {
        Some((px, n)) => glass::image(ctx, px, *n, (cx, cy, side, side), radius::COVER * s),
        None => {
            let p = art::rounded_rect(cx, cy, side, side, radius::COVER * s);
            ctx.fill_gradient(p, g, cx, cy, cx + side, cy + side);
            let isz = side * 0.42;
            ctx.icon(
                Icon::Music,
                cx + (side - isz) / 2.0,
                cy + (side - isz) / 2.0,
                isz,
                Color::WHITE,
            );
        }
    }
    let tx = cx + side + 12.0 * s;
    let tw = (x + w - tx).max(0.0);
    let fg = ctx.pal.fg;
    ctx.text(&m.title, tx, cy + 2.0 * s, 20.0 * s, text::TITLE, fg, tw);
    if !m.artist.is_empty() {
        ctx.text(
            &m.artist,
            tx,
            cy + 22.0 * s,
            16.0 * s,
            text::SECONDARY,
            muted,
            tw,
        );
    }
    if let Some(l) = &m.lyric {
        ctx.text(
            l,
            tx,
            cy + 42.0 * s,
            16.0 * s,
            (text::BODY.0, Weight::Bold),
            fg,
            tw,
        );
        if let Some(n) = &m.next_lyric {
            ctx.text(n, tx, cy + 58.0 * s, 14.0 * s, text::CAPTION, muted, tw);
        }
    }
    // Seek bar with times.
    let by = cy + side + 14.0 * s;
    let bh = 5.0 * s;
    let frac = m.fraction.unwrap_or(0.0) as f32;
    glass::progress(ctx, (x, by, w, bh), frac, g);
    if m.fraction.is_some() {
        ctx.hit(
            (x, by - 6.0 * s, w, bh + 12.0 * s),
            HitKind::Slider(perch_core::view::SliderTarget::Seek),
        );
    }
    if let Some((a, b)) = &m.times {
        let px = text::CAPTION.0 * s;
        let base = by + bh + 14.0 * s;
        let c = ctx.fade(muted, 1.0);
        ctx.text
            .draw_tabular(ctx.pm, a, x, base, px, c, w / 2.0, Weight::Medium);
        let bw = ctx.text.measure_tabular(b, px, Weight::Medium).width;
        ctx.text
            .draw_tabular(ctx.pm, b, x + w - bw, base, px, c, bw + 2.0, Weight::Medium);
    }
    // Transport.
    let ty = by + bh + 40.0 * s;
    let mid = x + w / 2.0;
    let small = 34.0 * s;
    let big = 44.0 * s;
    let gap = 14.0 * s;
    let play_icon = if m.playing { Icon::Pause } else { Icon::Play };
    glass::round_button(
        ctx,
        mid - big / 2.0 - gap - small / 2.0,
        ty,
        small,
        Icon::Prev,
        false,
        Action::Transport(TransportCommand::Previous),
    );
    glass::round_button(
        ctx,
        mid,
        ty,
        big,
        play_icon,
        true,
        Action::Transport(TransportCommand::PlayPause),
    );
    glass::round_button(
        ctx,
        mid + big / 2.0 + gap + small / 2.0,
        ty,
        small,
        Icon::Next,
        false,
        Action::Transport(TransportCommand::Next),
    );
}

/// Tasks: Pomodoro ring + controls, reminder line, round-check to-dos.
fn tasks(ctx: &mut Ctx<'_>, t: &TasksView, (x, y, w, h): (f32, f32, f32, f32)) {
    let s = ctx.s;
    let muted = ctx.fade(ctx.pal.fg, crate::tokens::SECONDARY_ALPHA);
    let mut top = y;
    if let Some(p) = &t.pomodoro {
        let r = 30.0 * s;
        let (rcx, rcy) = (x + r + 2.0 * s, top + r + 2.0 * s);
        glass::ring(ctx, rcx, rcy, r, p.fraction, &p.remaining);
        ctx.button(
            (
                rcx - r - 4.0 * s,
                rcy - r - 4.0 * s,
                2.0 * r + 8.0 * s,
                2.0 * r + 8.0 * s,
            ),
            Action::PomodoroToggle,
        );
        let tx = rcx + r + 14.0 * s;
        let tw = (x + w - tx).max(0.0);
        let title = p
            .label
            .split_whitespace()
            .next()
            .unwrap_or("Focus")
            .to_string();
        let fg = ctx.pal.fg;
        ctx.text(&title, tx, top + 6.0 * s, 20.0 * s, text::TITLE, fg, tw);
        ctx.text(
            &p.session,
            tx,
            top + 26.0 * s,
            16.0 * s,
            text::SECONDARY,
            muted,
            tw,
        );
        let d = 28.0 * s;
        let byy = top + 44.0 * s + d / 2.0;
        let icon = if p.running { Icon::Pause } else { Icon::Play };
        glass::round_button(
            ctx,
            tx + d / 2.0,
            byy,
            d,
            icon,
            false,
            Action::PomodoroToggle,
        );
        glass::round_button(
            ctx,
            tx + d * 1.5 + 8.0 * s,
            byy,
            d,
            Icon::Next,
            false,
            Action::PomodoroSkip,
        );
        glass::round_button(
            ctx,
            tx + d * 2.5 + 16.0 * s,
            byy,
            d,
            Icon::Reset,
            false,
            Action::PomodoroReset,
        );
        if let Some(rem) = &t.reminder {
            let rw = ctx.measure(rem, text::CAPTION).min(w * 0.4);
            ctx.text(
                rem,
                x + w - rw,
                top + 6.0 * s,
                16.0 * s,
                text::CAPTION,
                muted,
                rw,
            );
        }
        top += 2.0 * r + 14.0 * s;
    } else if let Some(rem) = &t.reminder {
        ctx.text(rem, x, top, 18.0 * s, text::CAPTION, muted, w);
        top += 22.0 * s;
    }
    if let Some(rows) = &t.todos {
        let rh = crate::tokens::space::ROW1 * s;
        for row in rows {
            if top + rh > y + h {
                break;
            }
            let done = row.check.unwrap_or(false);
            glass::row(
                ctx,
                (x - 6.0 * s, top, w + 12.0 * s, rh),
                &glass::Leading::Check(done),
                &row.text,
                None,
                (!row.detail.is_empty()).then_some(row.detail.as_str()),
                None,
                row.remove.clone(),
                row.primary.clone(),
                row.dim,
            );
            top += rh;
        }
        if top + rh <= y + h + 1.0 {
            let rect = (x - 6.0 * s, top, w + 12.0 * s, rh);
            if ctx.hovered(rect) {
                ctx.rrect(rect, radius::ROW * s, ctx.pal.raised[0]);
            }
            let isz = 14.0 * s;
            ctx.icon(Icon::Plus, x + 7.0 * s, top + (rh - isz) / 2.0, isz, muted);
            let label = if rows.is_empty() {
                "Nothing to do. Add a task"
            } else {
                "Add a task"
            };
            ctx.text(
                label,
                x + 38.0 * s,
                top,
                rh,
                text::BODY,
                muted,
                w - 40.0 * s,
            );
            ctx.button(rect, Action::TodoAdd);
        }
    }
}

fn kind_icon(k: perch_core::view::RowKind) -> Icon {
    use perch_core::view::RowKind;
    match k {
        RowKind::Text => Icon::Text,
        RowKind::Image => Icon::Image,
        RowKind::Link => Icon::Link,
        RowKind::File => Icon::Text,
        RowKind::Folder => Icon::Shelf,
        RowKind::Warning => Icon::Warning,
    }
}

/// The leading element a row asks for (a check wins when the row has one).
fn row_leading(r: &Row) -> glass::Leading {
    use perch_core::view::RowLeading;
    if let Some(done) = r.check {
        return glass::Leading::Check(done);
    }
    match &r.leading {
        RowLeading::None => glass::Leading::None,
        RowLeading::Avatar { initials, seed } => glass::Leading::Avatar {
            initials: initials.clone(),
            seed: *seed,
        },
        RowLeading::Kind(k) => glass::Leading::Icon(kind_icon(*k), None),
        RowLeading::Thumb(px, side) => glass::Leading::Thumb(px.clone(), *side),
    }
}

/// A list body: "N items" + header chip, then rows (two lines when they
/// have a subtitle), or the empty text.
fn list(
    ctx: &mut Ctx<'_>,
    rows: &[Row],
    empty: &str,
    header: Option<&(String, Action)>,
    (x, y, w, h): (f32, f32, f32, f32),
) {
    let s = ctx.s;
    let muted = ctx.fade(ctx.pal.fg, crate::tokens::SECONDARY_ALPHA);
    let mut top = y;
    if let Some((label, action)) = header {
        let hh = 22.0 * s;
        let count = format!(
            "{} item{}",
            rows.len(),
            if rows.len() == 1 { "" } else { "s" }
        );
        ctx.text(&count, x, top, hh, text::CAPTION, muted, w / 2.0);
        glass::chip(ctx, x + w, top, label, action.clone());
        top += hh + 6.0 * s;
    }
    if rows.is_empty() {
        let tw = ctx.measure(empty, text::BODY).min(w);
        ctx.text(
            empty,
            x + (w - tw) / 2.0,
            top,
            (y + h - top).max(24.0 * s),
            text::BODY,
            muted,
            w,
        );
        return;
    }
    for r in rows {
        let rh = if r.subtitle.is_some() {
            crate::tokens::space::ROW2
        } else {
            crate::tokens::space::ROW1
        } * s;
        if top + rh > y + h + 1.0 {
            break;
        }
        let chip = r.chip.as_ref().map(|(l, a)| (l.as_str(), a.clone()));
        glass::row(
            ctx,
            (x - 6.0 * s, top, w + 12.0 * s, rh),
            &row_leading(r),
            &r.text,
            r.subtitle.as_deref(),
            (!r.detail.is_empty()).then_some(r.detail.as_str()),
            chip,
            r.remove.clone(),
            r.primary.clone(),
            r.dim,
        );
        top += rh;
    }
}
