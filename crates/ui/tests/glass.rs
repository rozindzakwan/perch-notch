//! Glass components: shapes, hover, avatars, sliders, toggles.

use perch_core::view::{Action, HitKind, SliderTarget};
use perch_ui::effects::Effects;
use perch_ui::glass::{self, Ctx, Leading};
use perch_ui::text::TextRenderer;
use perch_ui::theme::Palette;
use tiny_skia::Pixmap;

fn alpha(pm: &Pixmap, x: u32, y: u32) -> f32 {
    pm.pixel(x, y)
        .map(|p| f32::from(p.alpha()) / 255.0)
        .unwrap_or(0.0)
}

fn rgba(pm: &Pixmap, x: u32, y: u32) -> [u8; 4] {
    pm.pixel(x, y)
        .map(|p| [p.red(), p.green(), p.blue(), p.alpha()])
        .unwrap_or([0; 4])
}

fn text() -> TextRenderer {
    TextRenderer::new().unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn surface_is_clipped_to_its_rounded_shape() {
    let pal = Palette::dark(false);
    let mut t = text();
    let mut fx = Effects::default();
    for s in [1.0_f32, 1.25, 1.5, 2.0] {
        let (w, h) = ((220.0 * s) as u32, (140.0 * s) as u32);
        let mut pm = Pixmap::new(w, h).unwrap_or_else(|| panic!("pixmap"));
        let (x, y, rw, rh) = (10.0 * s, 10.0 * s, w as f32 - 20.0 * s, h as f32 - 20.0 * s);
        {
            let mut ctx = Ctx::new(&mut pm, &mut t, &mut fx, pal, s);
            glass::surface(&mut ctx, (x, y, rw, rh), 26.0 * s, None, 0.0);
        }
        // Just inside the bounding box but outside the rounded corner.
        assert_eq!(
            alpha(&pm, (x + 1.0) as u32, (y + 1.0) as u32),
            0.0,
            "corner at {s}x"
        );
        // The middle is the glass fill.
        let mid = alpha(&pm, w / 2, h / 2);
        assert!(
            (mid - pal.surface.alpha()).abs() <= 0.02,
            "fill alpha {mid} at {s}x"
        );
        // No holes just inside the straight edges.
        for (px, py) in [
            (w as f32 / 2.0, y + 2.0),
            (w as f32 / 2.0, y + rh - 3.0),
            (x + 2.0, h as f32 / 2.0),
            (x + rw - 3.0, h as f32 / 2.0),
        ] {
            assert!(
                alpha(&pm, px as u32, py as u32) > 0.0,
                "hole at ({px},{py}) at {s}x"
            );
        }
    }
}

#[test]
fn row_hover_changes_only_when_cursor_inside() {
    let pal = Palette::dark(false);
    let mut t = text();
    let mut fx = Effects::default();
    let render = |t: &mut TextRenderer, fx: &mut Effects, cursor: Option<(f32, f32)>| {
        let mut pm = Pixmap::new(320, 60).unwrap_or_else(|| panic!("pixmap"));
        {
            let mut ctx = Ctx::new(&mut pm, t, fx, pal, 1.0);
            ctx.cursor = cursor;
            glass::row(
                &mut ctx,
                (10.0, 10.0, 300.0, 42.0),
                &Leading::None,
                "Ann Lee",
                Some("Quarterly report"),
                Some("2m"),
                None,
                None,
                false,
            );
        }
        rgba(&pm, 300, 14)
    };
    let none = render(&mut t, &mut fx, None);
    let outside = render(&mut t, &mut fx, Some((5.0, 5.0)));
    let inside = render(&mut t, &mut fx, Some((150.0, 30.0)));
    assert_eq!(none, outside, "cursor elsewhere changes nothing");
    assert_ne!(none, inside, "hovering shows a highlight");
}

#[test]
fn avatar_colour_is_stable_for_a_seed() {
    let pal = Palette::dark(false);
    let mut t = text();
    let mut fx = Effects::default();
    let mut draw = |seed: u64| {
        let mut pm = Pixmap::new(40, 40).unwrap_or_else(|| panic!("pixmap"));
        {
            let mut ctx = Ctx::new(&mut pm, &mut t, &mut fx, pal, 1.0);
            glass::avatar(&mut ctx, 20.0, 20.0, 32.0, "AL", seed);
        }
        rgba(&pm, 10, 20)
    };
    assert_eq!(draw(42), draw(42));
    assert_ne!(
        draw(42),
        draw(43),
        "neighbouring seeds pick different gradients"
    );
}

#[test]
fn slider_hit_spans_the_track() {
    let pal = Palette::dark(false);
    let mut t = text();
    let mut fx = Effects::default();
    let mut pm = Pixmap::new(300, 40).unwrap_or_else(|| panic!("pixmap"));
    let mut ctx = Ctx::new(&mut pm, &mut t, &mut fx, pal, 1.0);
    let target = SliderTarget::Volume("spotify".into());
    glass::slider(
        &mut ctx,
        (20.0, 9.0, 260.0, 22.0),
        0.5,
        true,
        target.clone(),
    );
    let hit = ctx
        .hits
        .iter()
        .find(|h| h.kind == HitKind::Slider(target.clone()))
        .unwrap_or_else(|| panic!("no slider hit"));
    assert!(hit.x <= 20.0 && hit.x + hit.w >= 280.0, "{hit:?}");
}

#[test]
fn toggle_on_and_off_differ_visibly() {
    let pal = Palette::dark(false);
    let mut t = text();
    let mut fx = Effects::default();
    let mut draw = |on: bool| {
        let mut pm = Pixmap::new(60, 30).unwrap_or_else(|| panic!("pixmap"));
        {
            let mut ctx = Ctx::new(&mut pm, &mut t, &mut fx, pal, 1.0);
            glass::toggle(&mut ctx, (10.0, 5.0, 40.0, 20.0), on, Action::OpenSettings);
        }
        (rgba(&pm, 18, 15), rgba(&pm, 42, 15))
    };
    assert_ne!(draw(true), draw(false));
}
