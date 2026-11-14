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

/// A notification banner: glass surface with a leading avatar/icon, a bold
/// title and one line of detail.
pub fn banner(
    ctx: &mut Ctx<'_>,
    rect: (f32, f32, f32, f32),
    leading: &Leading,
    title: &str,
    body: &str,
) {
    let (x, y, w, h) = rect;
    let s = ctx.s;
    surface(ctx, rect, radius::BANNER * s, None, 8.0);
    let d = 32.0 * s;
    let pad = 12.0 * s;
    let lead_w = widgets::leading(ctx, leading, x + pad, y + (h - d) / 2.0, d);
    let tx = x + pad + if lead_w > 0.0 { lead_w + 10.0 * s } else { 0.0 };
    let tw = (x + w - pad - tx).max(0.0);
    if body.is_empty() {
        ctx.text(
            title,
            tx,
            y,
            h,
            (text::BODY.0, crate::tokens::Weight::Bold),
            ctx.pal.fg,
            tw,
        );
    } else {
        let line = h / 2.0 - 2.0 * s;
        ctx.text(
            title,
            tx,
            y + 4.0 * s,
            line,
            (text::BODY.0, crate::tokens::Weight::Bold),
            ctx.pal.fg,
            tw,
        );
        let muted = ctx.fade(ctx.pal.fg, SECONDARY_ALPHA);
        ctx.text(
            body,
            tx,
            y + h / 2.0 - 2.0 * s,
            line,
            text::SECONDARY,
            muted,
            tw,
        );
    }
}
