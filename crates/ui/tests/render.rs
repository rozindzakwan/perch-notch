//! Off-screen rendering tests: every panel paints without panicking at
//! several scales and registers the hit regions the shell needs.

use std::sync::Arc;

use perch_core::noise::NoiseKind;
use perch_core::view::{
    resolve_click, Action, Badge, BadgeKind, Body, CollapsedView, MediaView, PanelView,
    PomodoroView, Row, SliderRow, SliderTarget, Tab, TasksView, UsageBar,
};
use perch_ui::render::{Frame, Renderer};
use perch_ui::theme::Palette;
use tiny_skia::Pixmap;

fn render(
    view: &PanelView,
    w: u32,
    h: u32,
    scale: f32,
    progress: f32,
) -> (Pixmap, Vec<perch_core::view::Hit>) {
    let mut r = Renderer::new().unwrap_or_else(|e| panic!("renderer: {e}"));
    let mut pm = Pixmap::new(w, h).unwrap_or_else(|| panic!("pixmap"));
    let mut frame = Frame::new(w, h, scale, Palette::dark(false));
    frame.progress = progress;
    let hits = r.draw(&mut pm, &frame, view);
    (pm, hits)
}

fn bodies() -> Vec<Body> {
    vec![
        Body::Media(MediaView {
            title: "A very long title that will definitely need eliding somewhere".into(),
            artist: "Artist".into(),
            has_player: true,
            playing: true,
            fraction: Some(0.4),
            times: Some(("1:00".into(), "2:30".into())),
            lyric: Some("la la la".into()),
            cover: Some((Arc::new(vec![200u8; 16 * 16 * 4]), 16)),
            accent: None,
            next_lyric: None,
        }),
        Body::Tasks(TasksView {
            pomodoro: Some(PomodoroView {
                label: "Focus 24:59".into(),
                running: true,
            }),
            todos: Some(vec![Row {
                text: "Write tests".into(),
                detail: "in 5 min".into(),
                check: Some(false),
                primary: Some(Action::TodoAdd),
                remove: Some(Action::OpenSettings),
                dim: false,
            }]),
            reminder: Some("Water in 12 min".into()),
        }),
        Body::List {
            rows: vec![],
            empty: "Nothing here".into(),
            header: None,
        },
        Body::Sliders {
            rows: vec![SliderRow {
                label: "Rain".into(),
                value: 0.6,
                on: true,
                target: SliderTarget::Noise(NoiseKind::Rain),
                toggle: Action::NoiseToggle(NoiseKind::Rain),
            }],
            empty: String::new(),
        },
        Body::Usage {
            total: "Today 2 h 03 min".into(),
            bars: vec![UsageBar {
                app: "editor".into(),
                label: "1 h".into(),
                fraction: 1.0,
            }],
        },
        Body::Buttons(vec![("Screenshot".into(), Action::Screenshot)]),
        Body::Empty("Nothing enabled".into()),
    ]
}

#[test]
fn every_panel_renders_at_several_scales() {
    for scale in [1.0, 1.25, 2.0] {
        for body in bodies() {
            let view = PanelView {
                collapsed: CollapsedView::Idle,
                tabs: vec![Tab::Media, Tab::Tasks, Tab::Sounds],
                tab: Some(Tab::Media),
                body,
                pinned: false,
            };
            let w = (480.0 * scale) as u32;
            let h = (200.0 * scale) as u32;
            let (pm, hits) = render(&view, w, h, scale, 1.0);
            assert!(
                pm.pixels().iter().any(|p| p.alpha() > 0),
                "something was painted"
            );
            assert!(
                hits.iter().any(|h| matches!(
                    h.kind,
                    perch_core::view::HitKind::Button(Action::OpenSettings)
                )),
                "settings gear is always clickable"
            );
        }
    }
}

#[test]
fn tabs_are_clickable_and_slider_clicks_carry_a_value() {
    let view = PanelView {
        collapsed: CollapsedView::Idle,
        tabs: vec![Tab::Media, Tab::Sounds],
        tab: Some(Tab::Sounds),
        body: bodies().remove(3),
        pinned: true,
    };
    let (_, hits) = render(&view, 480, 200, 1.0, 1.0);
    let tab = hits
        .iter()
        .find(|h| h.kind == perch_core::view::HitKind::Button(Action::SelectTab(Tab::Media)))
        .unwrap_or_else(|| panic!("media tab hit"));
    assert_eq!(
        resolve_click(&hits, tab.x + 1.0, tab.y + 1.0),
        Some(Action::SelectTab(Tab::Media))
    );
    let slider = hits
        .iter()
        .find(|h| matches!(h.kind, perch_core::view::HitKind::Slider(_)))
        .unwrap_or_else(|| panic!("slider"));
    let click = resolve_click(&hits, slider.x + slider.w * 0.5, slider.y + slider.h * 0.5);
    match click {
        Some(Action::Slider(SliderTarget::Noise(NoiseKind::Rain), v)) => {
            assert!((v - 0.5).abs() < 0.02)
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn collapsed_pill_has_no_hits_and_degenerate_sizes_do_not_panic() {
    let view = PanelView {
        collapsed: pill("Artist - Title", 0),
        ..PanelView::default()
    };
    let (_, hits) = render(&view, 200, 32, 1.0, 0.0);
    assert!(hits.is_empty());
    for (w, h) in [(1, 1), (3, 200), (480, 2)] {
        let _ = render(&view, w, h, 1.0, 1.0);
    }
}

#[test]
fn opaque_frames_have_no_translucent_body_pixels() {
    let mut r = Renderer::new().unwrap_or_else(|e| panic!("{e}"));
    let mut pm = Pixmap::new(200, 32).unwrap_or_else(|| panic!("pixmap"));
    let mut frame = Frame::new(200, 32, 1.0, Palette::dark(false));
    frame.opaque = true;
    r.draw(&mut pm, &frame, &PanelView::default());
    let centre = pm.pixel(100, 16).unwrap_or_else(|| panic!("pixel"));
    assert_eq!(centre.alpha(), 255);
}

fn pill(text: &str, mail: u32) -> CollapsedView {
    CollapsedView::Pill {
        thumb: Some((Arc::new(vec![220u8; 16 * 16 * 4]), 16)),
        glyph: None,
        text: text.into(),
        badges: if mail > 0 {
            vec![Badge {
                kind: BadgeKind::Mail,
                count: mail,
            }]
        } else {
            Vec::new()
        },
    }
}

fn collapsed_view(c: CollapsedView) -> PanelView {
    PanelView {
        collapsed: c,
        ..PanelView::default()
    }
}

#[test]
fn collapsed_pill_shows_thumb_text_and_badge_inside_bounds() {
    let (pm, hits) = render(
        &collapsed_view(pill("The Weeknd - Die For You", 3)),
        300,
        32,
        1.0,
        0.0,
    );
    assert!(hits.is_empty(), "the collapsed pill is hover-only");
    // Thumbnail near the left, badge near the right, both painted.
    assert!(
        pm.pixel(22, 16).map(|p| p.alpha()).unwrap_or(0) > 200,
        "thumb"
    );
    let right_painted = (250..290).any(|x| pm.pixel(x, 16).map(|p| p.red() > 120).unwrap_or(false));
    assert!(right_painted, "badge drawn near the right edge");
    // Nothing outside the pill's rounded ends.
    assert_eq!(pm.pixel(0, 0).map(|p| p.alpha()).unwrap_or(0), 0);
}

#[test]
fn long_track_title_is_ellipsized_in_the_pill() {
    let long = "A ".repeat(150);
    let (short_pm, _) = render(&collapsed_view(pill("Hi", 7)), 300, 32, 1.0, 0.0);
    let (long_pm, _) = render(&collapsed_view(pill(&long, 7)), 300, 32, 1.0, 0.0);
    // The badge region (right 48 px) is identical: the text never runs into it.
    for x in 252..300 {
        for y in 0..32 {
            assert_eq!(
                short_pm.pixel(x, y).map(|p| p.alpha()),
                long_pm.pixel(x, y).map(|p| p.alpha()),
                "text spilled into the badge at ({x},{y})"
            );
        }
    }
}

#[test]
fn pill_width_follows_content() {
    let mut r = Renderer::new().unwrap_or_else(|e| panic!("{e}"));
    let short = r.pill_width(&pill("Hi", 0), 1.0);
    let long = r.pill_width(&pill("The Weeknd - Die For You (Extended Mix)", 3), 1.0);
    assert!(long > short, "{long} vs {short}");
    assert!(r.pill_width(&CollapsedView::Idle, 1.0) <= short);
}

#[test]
fn tabs_use_icons_and_report_one_hit_each() {
    let view = PanelView {
        collapsed: CollapsedView::Idle,
        tabs: vec![Tab::Media, Tab::Tasks, Tab::Mail, Tab::Clipboard],
        tab: Some(Tab::Mail),
        body: Body::Empty(String::new()),
        pinned: false,
    };
    let (_, hits) = render(&view, 380, 300, 1.0, 1.0);
    let tab_hits = hits
        .iter()
        .filter(|h| {
            matches!(
                h.kind,
                perch_core::view::HitKind::Button(Action::SelectTab(_))
            )
        })
        .count();
    assert_eq!(tab_hits, 4);
}

#[test]
fn content_alpha_zero_draws_no_body_text() {
    let view = |body: Body| PanelView {
        collapsed: CollapsedView::Idle,
        tabs: vec![Tab::Media],
        tab: Some(Tab::Media),
        body,
        pinned: false,
    };
    let draw = |v: &PanelView, alpha: f32| {
        let mut r = Renderer::new().unwrap_or_else(|e| panic!("{e}"));
        let mut pm = Pixmap::new(380, 300).unwrap_or_else(|| panic!("pixmap"));
        let mut frame = Frame::new(380, 300, 1.0, Palette::dark(false));
        frame.progress = 1.0;
        frame.content_alpha = alpha;
        let hits = r.draw(&mut pm, &frame, v);
        (pm, hits)
    };
    let a = view(bodies().remove(0));
    let b = view(Body::Empty("nothing here at all".into()));
    let (pa, ha) = draw(&a, 0.0);
    let (pb, _) = draw(&b, 0.0);
    assert_eq!(
        pa.data(),
        pb.data(),
        "with content hidden only the surface is drawn"
    );
    assert!(ha.is_empty(), "hidden content is not clickable");
    let (pa1, _) = draw(&a, 1.0);
    assert_ne!(pa1.data(), pb.data());
}
