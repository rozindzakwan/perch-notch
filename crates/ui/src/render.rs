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
use tiny_skia::{Color, FillRule, Mask, Pixmap, Transform};

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
    /// Opacity of hover highlights (fades in when the hovered item changes).
    pub hover_alpha: f32,
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
            hover_alpha: 1.0,
        }
    }
}

/// Paints frames. Holds the glyph and shadow caches.
pub struct Renderer {
    text: TextRenderer,
    fx: Effects,
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
        ctx.hover = frame.hover_alpha.clamp(0.0, 1.0);
        let banner = view.banner.as_ref().filter(|_| !open);
        if let Some(b) = banner {
            // The pill has stretched into a notification banner.
            glass::banner(&mut ctx, panel, &leading_of(&b.leading), &b.title, &b.body);
            ctx.button(panel, Action::BannerOpen);
            let hits = std::mem::take(&mut ctx.hits);
            return hits;
        }
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
            let c = ctx.hover_fill(ctx.pal.raised[1]);
            ctx.rrect(rect, btn / 2.0, c);
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
        Body::Media(m) => music(ctx, m, body),
        Body::Tasks(t) => tasks(ctx, t, body),
        Body::List {
            rows,
            empty,
            header,
        } => list(ctx, rows, empty, header.as_ref(), body),
        Body::Sliders { rows, empty } => sliders(ctx, rows, empty, body),
        Body::Usage { total, bars } => usage(ctx, total, bars, body),
        Body::Buttons(items) => buttons(ctx, items, body),
        Body::Empty(msg) => {
            let muted = ctx.fade(ctx.pal.fg, crate::tokens::SECONDARY_ALPHA);
            let (bx, by, bw, _) = body;
            let tw = ctx.measure(msg, text::BODY).min(bw);
            ctx.text(
                msg,
                bx + (bw - tw) / 2.0,
                by,
                40.0 * s,
                text::BODY,
                muted,
                bw,
            );
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
                let c = ctx.hover_fill(ctx.pal.raised[0]);
                ctx.rrect(rect, radius::ROW * s, c);
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
        RowKind::Sound => Icon::Waves,
        RowKind::Mail => Icon::Mail,
    }
}

/// The leading element a row asks for (a check wins when the row has one).
fn row_leading(r: &Row) -> glass::Leading {
    if let Some(done) = r.check {
        return glass::Leading::Check(done);
    }
    leading_of(&r.leading)
}

fn leading_of(l: &perch_core::view::RowLeading) -> glass::Leading {
    use perch_core::view::RowLeading;
    match l {
        RowLeading::None => glass::Leading::None,
        RowLeading::Avatar { initials, seed } => glass::Leading::Avatar {
            initials: initials.clone(),
            seed: *seed,
        },
        RowLeading::Kind(k) => glass::Leading::Icon(kind_icon(*k), None),
        RowLeading::Disc { initials, rgb } => glass::Leading::Disc {
            initials: initials.clone(),
            color: Color::from_rgba8(rgb[0], rgb[1], rgb[2], 255),
        },
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

fn slider_leading(r: &SliderRow) -> glass::Leading {
    use perch_core::view::RowLeading;
    if !r.on {
        return glass::Leading::Icon(Icon::Mute, None);
    }
    match &r.leading {
        RowLeading::Disc { initials, rgb } => glass::Leading::Disc {
            initials: initials.clone(),
            color: Color::from_rgba8(rgb[0], rgb[1], rgb[2], 255),
        },
        RowLeading::Avatar { initials, seed } => glass::Leading::Avatar {
            initials: initials.clone(),
            seed: *seed,
        },
        RowLeading::Kind(k) => glass::Leading::Icon(kind_icon(*k), None),
        RowLeading::Thumb(px, side) => glass::Leading::Thumb(px.clone(), *side),
        RowLeading::None => glass::Leading::None,
    }
}

/// Mixer / sounds: disc (click to mute), name, thick slider.
fn sliders(ctx: &mut Ctx<'_>, rows: &[SliderRow], empty: &str, (x, y, w, h): (f32, f32, f32, f32)) {
    let s = ctx.s;
    let muted = ctx.fade(ctx.pal.fg, crate::tokens::SECONDARY_ALPHA);
    if rows.is_empty() {
        let tw = ctx.measure(empty, text::BODY).min(w);
        ctx.text(empty, x + (w - tw) / 2.0, y, 40.0 * s, text::BODY, muted, w);
        return;
    }
    let rh = 38.0 * s;
    let d = 28.0 * s;
    let label_w = 76.0 * s;
    let mut top = y;
    for r in rows {
        if top + rh > y + h + 1.0 {
            break;
        }
        let saved = ctx.alpha;
        if !r.on {
            ctx.alpha *= 0.6;
        }
        let ly = top + (rh - d) / 2.0;
        glass::leading_at(ctx, &slider_leading(r), x, ly, d);
        ctx.button((x, ly, d, d), r.toggle.clone());
        let fg = ctx.pal.fg;
        ctx.text(&r.label, x + d + 10.0 * s, top, rh, text::BODY, fg, label_w);
        let sx = x + d + 10.0 * s + label_w + 8.0 * s;
        let sh = 22.0 * s;
        glass::slider(
            ctx,
            (sx, top + (rh - sh) / 2.0, (x + w - sx).max(0.0), sh),
            r.value,
            r.on,
            r.target.clone(),
        );
        ctx.alpha = saved;
        top += rh;
    }
}

/// Screen time: today's total, then a rounded gradient bar per app.
fn usage(ctx: &mut Ctx<'_>, total: &str, bars: &[UsageBar], (x, y, w, h): (f32, f32, f32, f32)) {
    let s = ctx.s;
    let muted = ctx.fade(ctx.pal.fg, crate::tokens::SECONDARY_ALPHA);
    let fg = ctx.pal.fg;
    ctx.text(total, x, y, 22.0 * s, text::TITLE, fg, w);
    let mut top = y + 30.0 * s;
    if bars.is_empty() {
        ctx.text(
            "Use your apps for a while and they show up here.",
            x,
            top,
            20.0 * s,
            text::SECONDARY,
            muted,
            w,
        );
        return;
    }
    let g = ctx.pal.accent;
    let rh = 32.0 * s;
    for b in bars {
        if top + rh > y + h + 1.0 {
            break;
        }
        let lw = ctx
            .text
            .measure_tabular(&b.label, text::CAPTION.0 * s, Weight::Medium)
            .width;
        ctx.text(
            &b.app,
            x,
            top,
            16.0 * s,
            text::BODY,
            fg,
            (w - lw - 8.0 * s).max(0.0),
        );
        let base = top + 12.0 * s;
        let c = ctx.fade(muted, 1.0);
        ctx.text.draw_tabular(
            ctx.pm,
            &b.label,
            x + w - lw,
            base,
            text::CAPTION.0 * s,
            c,
            lw + 2.0,
            Weight::Medium,
        );
        glass::progress(ctx, (x, top + 19.0 * s, w, 6.0 * s), b.fraction, g);
        top += rh;
    }
}

fn tool_icon(a: &Action) -> Icon {
    match a {
        Action::Screenshot => Icon::Screenshot,
        Action::CameraToggle => Icon::Camera,
        Action::DictateToggle => Icon::Mic,
        _ => Icon::Tools,
    }
}

/// Tools: big round icon buttons with a caption.
fn buttons(ctx: &mut Ctx<'_>, items: &[(String, Action)], (x, y, w, _h): (f32, f32, f32, f32)) {
    let s = ctx.s;
    if items.is_empty() {
        return;
    }
    let d = 56.0 * s;
    let gap = 24.0 * s;
    let total = items.len() as f32 * d + (items.len() - 1) as f32 * gap;
    let mut cx = x + (w - total) / 2.0 + d / 2.0;
    let cy = y + 8.0 * s + d / 2.0;
    let muted = ctx.fade(ctx.pal.fg, crate::tokens::SECONDARY_ALPHA);
    for (label, action) in items {
        glass::round_button(ctx, cx, cy, d, tool_icon(action), false, action.clone());
        let tw = ctx.measure(label, text::CAPTION);
        ctx.text(
            label,
            cx - tw / 2.0,
            cy + d / 2.0 + 6.0 * s,
            16.0 * s,
            text::CAPTION,
            muted,
            tw + 2.0,
        );
        cx += d + gap;
    }
}
