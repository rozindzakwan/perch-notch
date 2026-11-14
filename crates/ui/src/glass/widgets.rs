//! Interactive pieces: tabs, rows, avatars, buttons, sliders, toggles, rings.

use std::sync::Arc;

use perch_core::view::{Action, HitKind, SliderTarget};
use tiny_skia::{
    Color, FillRule, IntSize, Paint, PathBuilder, Pattern, Pixmap, PremultipliedColorU8,
    SpreadMode, Transform,
};

use super::{avatar_gradient, Ctx, Leading};
use crate::art::rounded_rect;
use crate::icons::Icon;
use crate::theme::Gradient;
use crate::tokens::{radius, text, Weight, SECONDARY_ALPHA};

fn circle(cx: f32, cy: f32, r: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, r.max(0.0));
    pb.finish()
}

/// Segmented icon tabs inside a capsule. `highlight_x` is the (animated)
/// position of the active capsule in segment units (0.0 = first tab).
pub fn tabs(
    ctx: &mut Ctx<'_>,
    rect: (f32, f32, f32, f32),
    items: &[(Icon, Action)],
    active: usize,
    highlight_x: f32,
) {
    let (x, y, w, h) = rect;
    if items.is_empty() || w <= 0.0 {
        return;
    }
    ctx.rrect(rect, h / 2.0, ctx.pal.raised[0]);
    let pad = 3.0 * ctx.s;
    let seg = (w - 2.0 * pad) / items.len() as f32;
    let hx = x + pad + seg * highlight_x.clamp(0.0, (items.len() - 1) as f32);
    let hh = h - 2.0 * pad;
    ctx.rrect((hx, y + pad, seg, hh), hh / 2.0, ctx.pal.raised[2]);
    let icon = 15.0 * ctx.s;
    for (i, (ic, action)) in items.iter().enumerate() {
        let sx = x + pad + seg * i as f32;
        let color = if i == active {
            ctx.pal.fg
        } else {
            ctx.fade(ctx.pal.fg, 0.55)
        };
        ctx.icon(
            *ic,
            sx + (seg - icon) / 2.0,
            y + (h - icon) / 2.0,
            icon,
            color,
        );
        ctx.button((sx, y, seg, h), action.clone());
    }
}

/// Draw a row's leading element at `(x, y)` in a `d`-sized square; returns
/// the width used (0 for none).
/// Public alias of [`leading`] for screens drawing their own rows.
pub fn leading_at(ctx: &mut Ctx<'_>, lead: &Leading, x: f32, y: f32, d: f32) -> f32 {
    leading(ctx, lead, x, y, d)
}

pub(crate) fn leading(ctx: &mut Ctx<'_>, lead: &Leading, x: f32, y: f32, d: f32) -> f32 {
    match lead {
        Leading::None => 0.0,
        Leading::Avatar { initials, seed } => {
            avatar(ctx, x + d / 2.0, y + d / 2.0, d, initials, *seed);
            d
        }
        Leading::Icon(icon, bg) => {
            let r = d * 0.3;
            match bg {
                Some(g) => {
                    let p = rounded_rect(x, y, d, d, r);
                    ctx.fill_gradient(p, *g, x, y, x + d, y + d);
                }
                None => ctx.rrect((x, y, d, d), r, ctx.pal.raised[1]),
            }
            let s = d * 0.55;
            ctx.icon(*icon, x + (d - s) / 2.0, y + (d - s) / 2.0, s, ctx.pal.fg);
            d
        }
        Leading::Disc { initials, color } => {
            let r = d / 2.0;
            let (cx, cy) = (x + r, y + r);
            ctx.fill(circle(cx, cy, r), *color);
            let px = (d * 0.38) / ctx.s;
            let tw = ctx.measure(initials, (px, Weight::Bold));
            ctx.text(
                initials,
                cx - tw / 2.0,
                cy - r,
                d,
                (px, Weight::Bold),
                Color::WHITE,
                d,
            );
            d
        }
        Leading::Check(done) => {
            let r = d * 0.34;
            let (cx, cy) = (x + d / 2.0, y + d / 2.0);
            if *done {
                let g = ctx.pal.tasks;
                ctx.fill_gradient(circle(cx, cy, r), g, cx - r, cy - r, cx + r, cy + r);
                let s = r * 1.3;
                ctx.icon(Icon::Check, cx - s / 2.0, cy - s / 2.0, s, Color::WHITE);
            } else {
                let c = ctx.fade(ctx.pal.fg, 0.45);
                ctx.stroke(circle(cx, cy, r), c, 1.5 * ctx.s);
            }
            d
        }
        Leading::Thumb(px, side) => {
            image(ctx, px, *side, (x, y, d, d), d * 0.28);
            d
        }
    }
}

/// A list row: leading element, title, optional second line, trailing text,
/// and a chip that replaces the trailing text while hovered.
#[allow(clippy::too_many_arguments)]
pub fn row(
    ctx: &mut Ctx<'_>,
    rect: (f32, f32, f32, f32),
    lead: &Leading,
    title: &str,
    subtitle: Option<&str>,
    trailing: Option<&str>,
    chip_label: Option<(&str, Action)>,
    remove: Option<Action>,
    primary: Option<Action>,
    dim: bool,
) {
    let (x, y, w, h) = rect;
    let s = ctx.s;
    let saved = ctx.alpha;
    if dim {
        ctx.alpha *= 0.55;
    }
    let hovered = ctx.hovered(rect);
    if hovered {
        ctx.rrect(rect, radius::ROW * s, ctx.pal.raised[0]);
    }
    let pad = 6.0 * s;
    let d = 28.0 * s;
    let lw = leading(ctx, lead, x + pad, y + (h - d) / 2.0, d);
    let tx = x + pad + if lw > 0.0 { lw + 10.0 * s } else { 4.0 * s };
    let mut right = x + w - pad;
    if hovered && (chip_label.is_some() || remove.is_some()) {
        if let Some(a) = remove {
            let d = 22.0 * s;
            let (bx, by) = (right - d, y + (h - d) / 2.0);
            ctx.rrect((bx, by, d, d), d / 2.0, ctx.pal.raised[1]);
            let isz = 10.0 * s;
            let c = ctx.fade(ctx.pal.fg, 0.8);
            ctx.icon(
                Icon::Close,
                bx + (d - isz) / 2.0,
                by + (d - isz) / 2.0,
                isz,
                c,
            );
            ctx.button((bx, by, d, d), a);
            right -= d + 6.0 * s;
        }
        if let Some((label, action)) = chip_label {
            let cw = chip(ctx, right, y + (h - 22.0 * s) / 2.0, label, action);
            right -= cw + 8.0 * s;
        }
    } else if let Some(t) = trailing {
        let tw = ctx.measure(t, text::CAPTION).min(w * 0.4);
        let muted = ctx.fade(ctx.pal.fg, SECONDARY_ALPHA);
        ctx.text(t, right - tw, y, h, text::CAPTION, muted, tw);
        right -= tw + 8.0 * s;
    }
    let tw = (right - tx).max(0.0);
    match subtitle {
        Some(sub) if !sub.is_empty() => {
            let line = h / 2.0 - 1.0 * s;
            ctx.text(
                title,
                tx,
                y + 2.0 * s,
                line,
                (text::BODY.0, Weight::Bold),
                ctx.pal.fg,
                tw,
            );
            let muted = ctx.fade(ctx.pal.fg, SECONDARY_ALPHA);
            ctx.text(
                sub,
                tx,
                y + h / 2.0 - 1.0 * s,
                line,
                text::SECONDARY,
                muted,
                tw,
            );
        }
        _ => {
            ctx.text(title, tx, y, h, text::BODY, ctx.pal.fg, tw);
        }
    }
    if let Some(a) = primary {
        // The chip's own hit (recorded first) wins over the row.
        ctx.button(rect, a);
    }
    ctx.alpha = saved;
}

/// Initials on a gradient disc.
pub fn avatar(ctx: &mut Ctx<'_>, cx: f32, cy: f32, d: f32, initials: &str, seed: u64) {
    let g = avatar_gradient(seed);
    let r = d / 2.0;
    ctx.fill_gradient(circle(cx, cy, r), g, cx - r, cy - r, cx + r, cy + r);
    let px = (d * 0.38) / ctx.s;
    let tw = ctx.measure(initials, (px, Weight::Bold));
    ctx.text(
        initials,
        cx - tw / 2.0,
        cy - r,
        d,
        (px, Weight::Bold),
        Color::WHITE,
        d,
    );
}

/// A round icon button; `primary` = white disc with a dark icon.
pub fn round_button(
    ctx: &mut Ctx<'_>,
    cx: f32,
    cy: f32,
    d: f32,
    icon: Icon,
    primary: bool,
    action: Action,
) {
    let r = d / 2.0;
    let hovered = ctx.hovered((cx - r, cy - r, d, d));
    if primary {
        let c = if hovered {
            Color::from_rgba8(235, 236, 240, 255)
        } else {
            Color::WHITE
        };
        ctx.fill(circle(cx, cy, r), c);
    } else {
        let c = if hovered {
            ctx.pal.raised[2]
        } else {
            ctx.pal.raised[1]
        };
        ctx.fill(circle(cx, cy, r), c);
        ctx.stroke(circle(cx, cy, r - 0.5), ctx.pal.border, 1.0);
    }
    let fg = if primary {
        Color::from_rgba8(17, 17, 17, 255)
    } else {
        ctx.pal.fg
    };
    let s = d * 0.42;
    ctx.icon(icon, cx - s / 2.0, cy - s / 2.0, s, fg);
    ctx.button((cx - r, cy - r, d, d), action);
}

/// A thick slider you can grab anywhere along its track.
pub fn slider(
    ctx: &mut Ctx<'_>,
    rect: (f32, f32, f32, f32),
    value: f32,
    on: bool,
    target: SliderTarget,
) {
    let (x, y, w, h) = rect;
    ctx.rrect(rect, h / 2.0, ctx.pal.raised[0]);
    let v = value.clamp(0.0, 1.0);
    let fw = (w * v).max(h);
    let fill = if on {
        ctx.fade(ctx.pal.fg, 0.32)
    } else {
        ctx.fade(ctx.pal.fg, 0.14)
    };
    ctx.rrect((x, y, fw, h), h / 2.0, fill);
    ctx.hit(rect, HitKind::Slider(target));
}

/// An on/off switch.
pub fn toggle(ctx: &mut Ctx<'_>, rect: (f32, f32, f32, f32), on: bool, action: Action) {
    let (x, y, w, h) = rect;
    let track = rounded_rect(x, y, w, h, h / 2.0);
    if on {
        let g = ctx.pal.mail;
        ctx.fill_gradient(track, g, x, y, x + w, y);
    } else {
        ctx.fill(track, ctx.fade(ctx.pal.fg, 0.18));
    }
    let r = h / 2.0 - 2.0 * ctx.s;
    let kx = if on { x + w - h / 2.0 } else { x + h / 2.0 };
    ctx.fill(circle(kx, y + h / 2.0, r), Color::WHITE);
    ctx.button(rect, action);
}

/// A ring progress (Pomodoro): track, gradient arc from 12 o'clock, label.
pub fn ring(ctx: &mut Ctx<'_>, cx: f32, cy: f32, r: f32, fraction: f32, label: &str) {
    let width = 6.0 * ctx.s;
    ctx.stroke(circle(cx, cy, r), ctx.pal.raised[1], width);
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        let mut pb = PathBuilder::new();
        let steps = (64.0 * f).ceil().max(2.0) as usize;
        for i in 0..=steps {
            let a = -std::f32::consts::FRAC_PI_2
                + std::f32::consts::TAU * f * (i as f32 / steps as f32);
            let (sn, cs) = a.sin_cos();
            let (px, py) = (cx + r * cs, cy + r * sn);
            if i == 0 {
                pb.move_to(px, py);
            } else {
                pb.line_to(px, py);
            }
        }
        let accent = ctx.pal.accent;
        stroke_gradient(ctx, pb.finish(), accent, (cx - r, cy, cx + r, cy), width);
    }
    let tw = ctx
        .text
        .measure_tabular(label, 13.0 * ctx.s, Weight::Bold)
        .width;
    let m = ctx.text.measure_tabular(label, 13.0 * ctx.s, Weight::Bold);
    let baseline = cy + (m.ascent - m.descent) / 2.0;
    let c = ctx.fade(ctx.pal.fg, 1.0);
    ctx.text.draw_tabular(
        ctx.pm,
        label,
        cx - tw / 2.0,
        baseline,
        13.0 * ctx.s,
        c,
        tw + 2.0,
        Weight::Bold,
    );
}

fn stroke_gradient(
    ctx: &mut Ctx<'_>,
    path: Option<tiny_skia::Path>,
    g: Gradient,
    (x0, y0, x1, y1): (f32, f32, f32, f32),
    width: f32,
) {
    let Some(p) = path else { return };
    let (from, to) = (ctx.fade(g.from, 1.0), ctx.fade(g.to, 1.0));
    let Some(shader) = tiny_skia::LinearGradient::new(
        tiny_skia::Point::from_xy(x0, y0),
        tiny_skia::Point::from_xy(x1, y1),
        vec![
            tiny_skia::GradientStop::new(0.0, from),
            tiny_skia::GradientStop::new(1.0, to),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) else {
        return;
    };
    let paint = Paint {
        shader,
        anti_alias: true,
        ..Default::default()
    };
    let st = tiny_skia::Stroke {
        width,
        line_cap: tiny_skia::LineCap::Round,
        line_join: tiny_skia::LineJoin::Round,
        ..Default::default()
    };
    ctx.pm
        .stroke_path(&p, &paint, &st, Transform::identity(), None);
}

/// A progress bar with a gradient fill.
pub fn progress(ctx: &mut Ctx<'_>, rect: (f32, f32, f32, f32), fraction: f32, g: Gradient) {
    let (x, y, w, h) = rect;
    ctx.rrect(rect, h / 2.0, ctx.pal.raised[1]);
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        let fw = (w * f).max(h);
        let p = rounded_rect(x, y, fw, h, h / 2.0);
        ctx.fill_gradient(p, g, x, y, x + w, y);
    }
}

/// A small pill button right-aligned at `right`; returns its width.
pub fn chip(ctx: &mut Ctx<'_>, right: f32, y: f32, label: &str, action: Action) -> f32 {
    let h = 22.0 * ctx.s;
    let w = ctx.measure(label, text::CAPTION) + 16.0 * ctx.s;
    let x = right - w;
    ctx.rrect((x, y, w, h), h / 2.0, ctx.pal.raised[2]);
    ctx.text(label, x + 8.0 * ctx.s, y, h, text::CAPTION, ctx.pal.fg, w);
    ctx.button((x, y, w, h), action);
    w
}

/// A square image (straight RGBA8, `side` × `side`) scaled into `rect` with
/// rounded corners.
pub fn image(
    ctx: &mut Ctx<'_>,
    rgba: &Arc<Vec<u8>>,
    side: u32,
    rect: (f32, f32, f32, f32),
    r: f32,
) {
    let (x, y, w, h) = rect;
    let n = (side * side * 4) as usize;
    if side == 0 || rgba.len() < n {
        return;
    }
    let mut data = Vec::with_capacity(n);
    for px in rgba[..n].chunks_exact(4) {
        let a = u16::from(px[3]);
        let pm = |c: u8| ((u16::from(c) * a + 127) / 255) as u8;
        let c = PremultipliedColorU8::from_rgba(pm(px[0]), pm(px[1]), pm(px[2]), px[3])
            .unwrap_or(PremultipliedColorU8::TRANSPARENT);
        data.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    let Some(size) = IntSize::from_wh(side, side) else {
        return;
    };
    let Some(src) = Pixmap::from_vec(data, size) else {
        return;
    };
    let sx = w / side as f32;
    let sy = h / side as f32;
    let paint = Paint {
        shader: Pattern::new(
            src.as_ref(),
            SpreadMode::Pad,
            tiny_skia::FilterQuality::Bilinear,
            ctx.alpha,
            Transform::from_row(sx, 0.0, 0.0, sy, x, y),
        ),
        anti_alias: true,
        ..Default::default()
    };
    if let Some(p) = rounded_rect(x, y, w, h, r) {
        ctx.pm
            .fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
    }
}
