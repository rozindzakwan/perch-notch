//! Off-screen rendering tests: every panel paints without panicking at
//! several scales and registers the hit regions the shell needs.

use std::sync::Arc;

use perch_core::noise::NoiseKind;
use perch_core::view::{
    resolve_click, Action, Body, CollapsedView, MediaView, PanelView, PomodoroView, Row, SliderRow,
    SliderTarget, Tab, TasksView, UsageBar,
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
        collapsed: CollapsedView::NowPlaying("Artist - Title".into()),
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
