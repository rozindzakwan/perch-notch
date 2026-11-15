//! The glass surface (panel, pill, banner background) and the banner.

use tiny_skia::{Color, PathBuilder};

use super::{widgets, Ctx, Leading};
use crate::art::rounded_rect;
use crate::tokens::{radius, text, SECONDARY_ALPHA};

/// A glass surface: optional soft shadow (`shadow` = blur in logical px, 0 =
/// none), the translucent fill, an optional colour glow (album tint), a
/// hairline border and a one-pixel highlight along the top inside edge.
pub fn surface(
    ctx: &mut Ctx<'_>,
    rect: (f32, f32, f32, f32),
    r: f32,
    tint: Option<(Color, Color)>,
    shadow: f32,
) {
    let (x, y, w, h) = rect;
    if w < 1.0 || h < 1.0 {
        return;
    }
    let r = r.min(w / 2.0).min(h / 2.0);
    if shadow > 0.0 {
        ctx.fx
            .shadow(ctx.pm, x, y, w, h, r, shadow * ctx.s, 0.40 * ctx.alpha);
    }
    let body = rounded_rect(x, y, w, h, r);
    ctx.fill(body.clone(), ctx.pal.surface);
    if let Some((a, b)) = tint {
        let (a, b) = (ctx.fade(a, 1.0), ctx.fade(b, 1.0));
        ctx.fx.glow(ctx.pm, rect, r, (a, b));
    }
    let hair = 1.0_f32.max(ctx.s * 0.75);
    // Border inset by half its width so it stays inside the shape.
    let half = hair / 2.0;
    ctx.stroke(
        rounded_rect(x + half, y + half, w - hair, h - hair, (r - half).max(0.0)),
        ctx.pal.border,
        hair,
    );
    // Top highlight: a short line under the top edge, between the corners.
    if w > 2.0 * r + 2.0 {
        let mut pb = PathBuilder::new();
        pb.move_to(x + r, y + hair * 1.5);
        pb.line_to(x + w - r, y + hair * 1.5);
        ctx.stroke(pb.finish(), ctx.pal.highlight, hair);
    }
}

/// A notification banner, the notch opened halfway: glass surface, a leading
/// avatar/icon, an optional caption (the app it came from), a bold title and
/// up to two lines of text.
pub fn banner(
    ctx: &mut Ctx<'_>,
    rect: (f32, f32, f32, f32),
    leading: &Leading,
    app: Option<&str>,
    title: &str,
    body: &str,
) {
    let (x, y, w, h) = rect;
    let s = ctx.s;
    surface(ctx, rect, radius::BANNER * s, None, 8.0);
    let pad = 14.0 * s;
    let d = 36.0 * s;
    let caption_h = 14.0 * s;
    let title_h = 20.0 * s;
    let line_h = 16.0 * s;
    let tx_probe = x + pad + d + 12.0 * s;
    let tw = (x + w - pad - tx_probe).max(0.0);
    let lines = wrap_lines(ctx, body, text::SECONDARY, tw, 2);
    // With a caption the rows sit at fixed places from the top, so a short
    // and a long message line up; without one the block is centred.
    let block = title_h + lines.len() as f32 * line_h;
    let mut ty = if app.is_some() {
        y + 11.0 * s
    } else {
        y + ((h - block) / 2.0).max(6.0 * s)
    };
    let lead_y = if lines.is_empty() && app.is_none() {
        y + (h - d) / 2.0
    } else {
        ty + 2.0 * s
    };
    let lead_w = widgets::leading(ctx, leading, x + pad, lead_y, d);
    let tx = x + pad + if lead_w > 0.0 { lead_w + 12.0 * s } else { 0.0 };
    let tw = (x + w - pad - tx).max(0.0);
    let muted = ctx.fade(ctx.pal.fg, SECONDARY_ALPHA);
    if let Some(app) = app {
        ctx.text(app, tx, ty, caption_h, text::CAPTION, muted, tw);
        ty += caption_h;
    }
    ctx.text(
        title,
        tx,
        ty,
        title_h,
        (text::BODY.0, crate::tokens::Weight::Bold),
        ctx.pal.fg,
        tw,
    );
    ty += title_h;
    for line in lines {
        ctx.text(&line, tx, ty, line_h, text::SECONDARY, muted, tw);
        ty += line_h;
    }
}

/// Word-wrap `s` into at most `max` lines of width `w` (physical px); the
/// last line is elided by the text renderer when it still overflows.
fn wrap_lines(
    ctx: &mut Ctx<'_>,
    s: &str,
    style: (f32, crate::tokens::Weight),
    w: f32,
    max: usize,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let words: Vec<&str> = s.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        let candidate = if cur.is_empty() {
            (*word).to_string()
        } else {
            format!("{cur} {word}")
        };
        if !cur.is_empty() && ctx.measure(&candidate, style) > w {
            lines.push(std::mem::take(&mut cur));
            if lines.len() == max - 1 {
                // The rest goes on the last line (elided when drawn).
                cur = words[i..].join(" ");
                break;
            }
            cur = (*word).to_string();
        } else {
            cur = candidate;
        }
    }
    if !cur.is_empty() && lines.len() < max {
        lines.push(cur);
    }
    lines
}
