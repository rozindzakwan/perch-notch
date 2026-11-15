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
                fraction: 0.1,
                remaining: "24:59".into(),
                session: "Session 1 of 4".into(),
            }),
            todos: Some(vec![Row {
                text: "Write tests".into(),
                detail: "in 5 min".into(),
                check: Some(false),
                primary: Some(Action::TodoAdd),
                remove: Some(Action::OpenSettings),
                dim: false,
                ..Row::default()
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
                leading: perch_core::view::RowLeading::Kind(perch_core::view::RowKind::Sound),
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
                banner: None,
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
        banner: None,
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
        banner: None,
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
        banner: None,
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

fn panel_render(body: Body, tab: Tab, pal: Palette) -> (Pixmap, Vec<perch_core::view::Hit>) {
    let view = PanelView {
        collapsed: CollapsedView::Idle,
        tabs: vec![Tab::Media, Tab::Tasks],
        tab: Some(tab),
        body,
        pinned: false,
        banner: None,
    };
    let mut r = Renderer::new().unwrap_or_else(|e| panic!("{e}"));
    let mut pm = Pixmap::new(380, 300).unwrap_or_else(|| panic!("pixmap"));
    let mut frame = Frame::new(380, 300, 1.0, pal);
    frame.progress = 1.0;
    let hits = r.draw(&mut pm, &frame, &view);
    (pm, hits)
}

fn media(accent: Option<([u8; 3], [u8; 3])>, player: bool) -> Body {
    Body::Media(MediaView {
        title: if player {
            "Die For You".into()
        } else {
            "Nothing playing".into()
        },
        artist: if player {
            "The Weeknd".into()
        } else {
            String::new()
        },
        has_player: player,
        playing: player,
        fraction: player.then_some(0.5),
        times: player.then(|| ("2:10".into(), "4:20".into())),
        lyric: None,
        cover: None,
        accent,
        next_lyric: None,
    })
}

fn seek_fill_rgb(hits: &[perch_core::view::Hit], pm: &Pixmap) -> (u8, u8, u8) {
    let seek = hits
        .iter()
        .find(|h| h.kind == perch_core::view::HitKind::Slider(SliderTarget::Seek))
        .unwrap_or_else(|| panic!("seek bar"));
    let p = pm
        .pixel((seek.x + 3.0) as u32, (seek.y + seek.h / 2.0) as u32)
        .unwrap_or_else(|| panic!("pixel"));
    let a = f32::from(p.alpha().max(1)) / 255.0;
    let un = |c: u8| (f32::from(c) / a).round().min(255.0) as u8;
    (un(p.red()), un(p.green()), un(p.blue()))
}

#[test]
fn music_uses_the_album_accent_or_the_coral_fallback() {
    let (pm, hits) = panel_render(
        media(Some(([20, 200, 90], [10, 90, 200])), true),
        Tab::Media,
        Palette::dark(false),
    );
    let (r, g, _) = seek_fill_rgb(&hits, &pm);
    assert!(g > r + 60, "album green expected, got r={r} g={g}");
    let (pm, hits) = panel_render(media(None, true), Tab::Media, Palette::dark(false));
    let (r, g, b) = seek_fill_rgb(&hits, &pm);
    assert!(
        r > 200 && g < 160 && b > 100,
        "coral fallback expected, got {r},{g},{b}"
    );
}

#[test]
fn music_without_player_shows_the_empty_state() {
    for pal in [Palette::dark(false), Palette::light(false)] {
        let (pm, hits) = panel_render(media(None, false), Tab::Media, pal);
        assert!(
            !hits.iter().any(|h| matches!(
                h.kind,
                perch_core::view::HitKind::Button(Action::Transport(_))
            )),
            "no transport controls without a player"
        );
        let body_painted = (60..240).any(|y| {
            (20..360).any(|x| {
                pm.pixel(x, y)
                    .map(|p| p.alpha() == 255 && (p.red() > 150 || p.red() < 80))
                    .unwrap_or(false)
            })
        });
        assert!(body_painted, "the empty state says something");
    }
}

#[test]
fn tasks_ring_reflects_the_focus_fraction() {
    let body = Body::Tasks(TasksView {
        pomodoro: Some(PomodoroView {
            label: "Focus 12:30".into(),
            running: true,
            fraction: 0.5,
            remaining: "12:30".into(),
            session: "Session 1 of 4".into(),
        }),
        todos: Some(Vec::new()),
        reminder: None,
    });
    let (pm, hits) = panel_render(body, Tab::Tasks, Palette::dark(false));
    let ring = hits
        .iter()
        .find(|h| h.kind == perch_core::view::HitKind::Button(Action::PomodoroToggle))
        .unwrap_or_else(|| panic!("ring toggles the timer"));
    let (cx, cy, r) = (
        ring.x + ring.w / 2.0,
        ring.y + ring.h / 2.0,
        ring.w / 2.0 - 5.0,
    );
    let at = |deg: f32| {
        let a = (deg - 90.0).to_radians();
        let p = pm
            .pixel((cx + r * a.cos()) as u32, (cy + r * a.sin()) as u32)
            .unwrap_or_else(|| panic!("px"));
        p.red()
    };
    assert!(
        at(60.0) > at(240.0) + 40,
        "first half filled (red {}), second half track (red {})",
        at(60.0),
        at(240.0)
    );
}

#[test]
fn todo_rows_have_round_checks_and_an_add_row() {
    let todo = |id: u64, text: &str| Row {
        text: text.into(),
        detail: String::new(),
        check: Some(false),
        primary: Some(Action::TodoToggle(perch_core::todo::TodoId(id))),
        remove: Some(Action::TodoRemove(perch_core::todo::TodoId(id))),
        dim: false,
        ..Row::default()
    };
    let body = Body::Tasks(TasksView {
        pomodoro: None,
        todos: Some(vec![
            todo(1, "Send the report"),
            todo(2, "Call the dentist"),
        ]),
        reminder: None,
    });
    let (_, hits) = panel_render(body, Tab::Tasks, Palette::dark(false));
    let has = |a: Action| {
        hits.iter()
            .any(|h| h.kind == perch_core::view::HitKind::Button(a.clone()))
    };
    assert!(has(Action::TodoToggle(perch_core::todo::TodoId(1))));
    assert!(has(Action::TodoToggle(perch_core::todo::TodoId(2))));
    assert!(has(Action::TodoAdd), "an add-a-task row");
}

fn list_render(
    rows: Vec<Row>,
    scale: f32,
    cursor: Option<(f32, f32)>,
) -> Vec<perch_core::view::Hit> {
    let view = PanelView {
        collapsed: CollapsedView::Idle,
        tabs: vec![Tab::Mail],
        tab: Some(Tab::Mail),
        body: Body::List {
            rows,
            empty: "No unread mail.".into(),
            header: Some(("Accounts".into(), Action::OpenSettings)),
        },
        pinned: false,
        banner: None,
    };
    let (w, h) = ((380.0 * scale) as u32, (300.0 * scale) as u32);
    let mut r = Renderer::new().unwrap_or_else(|e| panic!("{e}"));
    let mut pm = Pixmap::new(w, h).unwrap_or_else(|| panic!("pixmap"));
    let mut frame = Frame::new(w, h, scale, Palette::dark(false));
    frame.progress = 1.0;
    frame.cursor = cursor;
    r.draw(&mut pm, &frame, &view)
}

fn mail_row(sender: &str, subject: &str) -> Row {
    Row {
        text: sender.into(),
        subtitle: Some(subject.into()),
        detail: "2m".into(),
        leading: perch_core::view::RowLeading::Avatar {
            initials: perch_core::view::initials(sender),
            seed: perch_core::view::seed(sender),
        },
        primary: Some(Action::TodoAdd),
        chip: Some(("Open".into(), Action::OpenSettings)),
        ..Row::default()
    }
}

#[test]
fn long_subject_and_emoji_never_leave_the_row() {
    let subject = "😀 Привет ".repeat(5) + &"very long subject ".repeat(18);
    for scale in [1.0_f32, 1.5] {
        let probe = list_render(vec![mail_row("Владимир Иванов", &subject)], scale, None);
        let row = probe
            .iter()
            .find(|h| h.kind == perch_core::view::HitKind::Button(Action::TodoAdd))
            .unwrap_or_else(|| panic!("row hit"))
            .clone();
        let hits = list_render(
            vec![mail_row("Владимир Иванов", &subject)],
            scale,
            Some((row.x + row.w / 2.0, row.y + row.h / 2.0)),
        );
        let chip = hits
            .iter()
            .find(|h| {
                h.kind == perch_core::view::HitKind::Button(Action::OpenSettings) && h.y >= row.y
            })
            .unwrap_or_else(|| panic!("hover chip"));
        assert!(
            chip.x >= row.x
                && chip.x + chip.w <= row.x + row.w + 0.5
                && chip.y >= row.y
                && chip.y + chip.h <= row.y + row.h + 0.5,
            "chip {chip:?} inside row {row:?} at {scale}x"
        );
    }
}

#[test]
fn chip_appears_only_on_hover() {
    let rows = || vec![mail_row("Ann Lee", "Quarterly report")];
    let probe = list_render(rows(), 1.0, None);
    let row = probe
        .iter()
        .find(|h| h.kind == perch_core::view::HitKind::Button(Action::TodoAdd))
        .unwrap_or_else(|| panic!("row hit"))
        .clone();
    let chips = |hits: &[perch_core::view::Hit]| {
        hits.iter()
            .filter(|h| {
                h.kind == perch_core::view::HitKind::Button(Action::OpenSettings) && h.y >= row.y
            })
            .count()
    };
    assert_eq!(chips(&probe), 0, "no chip without hover");
    let hovered = list_render(rows(), 1.0, Some((row.x + 20.0, row.y + row.h / 2.0)));
    assert_eq!(chips(&hovered), 1, "chip while hovered");
}

fn render_body(
    body: Body,
    tab: Tab,
    pal: Palette,
    scale: f32,
) -> (Pixmap, Vec<perch_core::view::Hit>) {
    let view = PanelView {
        collapsed: CollapsedView::Idle,
        tabs: vec![tab],
        tab: Some(tab),
        body,
        pinned: false,
        banner: None,
    };
    let (w, h) = ((380.0 * scale) as u32, (300.0 * scale) as u32);
    let mut r = Renderer::new().unwrap_or_else(|e| panic!("{e}"));
    let mut pm = Pixmap::new(w, h).unwrap_or_else(|| panic!("pixmap"));
    let mut frame = Frame::new(w, h, scale, pal);
    frame.progress = 1.0;
    let hits = r.draw(&mut pm, &frame, &view);
    (pm, hits)
}

#[test]
fn mixer_rows_have_coloured_discs_and_wide_slider_hits() {
    let body = Body::Sliders {
        rows: vec![SliderRow {
            label: "Spotify".into(),
            value: 0.8,
            on: true,
            target: SliderTarget::Volume("spotify".into()),
            toggle: Action::MuteToggle("spotify".into()),
            leading: perch_core::view::RowLeading::Disc {
                initials: "S".into(),
                rgb: perch_core::view::app_colour("Spotify").unwrap_or([0, 0, 0]),
            },
        }],
        empty: String::new(),
    };
    let (pm, hits) = render_body(body, Tab::Mixer, Palette::dark(false), 1.0);
    let mute = hits
        .iter()
        .find(|h| h.kind == perch_core::view::HitKind::Button(Action::MuteToggle("spotify".into())))
        .unwrap_or_else(|| panic!("the disc mutes"));
    let p = pm
        .pixel((mute.x + 4.0) as u32, (mute.y + mute.h / 2.0) as u32)
        .unwrap_or_else(|| panic!("pixel"));
    assert!(
        p.green() > 150 && p.red() < 80,
        "Spotify green disc, got {:?}",
        (p.red(), p.green(), p.blue())
    );
    let slider = hits
        .iter()
        .find(|h| matches!(h.kind, perch_core::view::HitKind::Slider(_)))
        .unwrap_or_else(|| panic!("slider"));
    assert!(slider.w >= 0.55 * 356.0, "slider is wide: {}", slider.w);
}

#[test]
fn every_body_renders_in_light_and_dark_with_empty_data() {
    let empties = vec![
        (Tab::Media, media(None, false)),
        (
            Tab::Tasks,
            Body::Tasks(TasksView {
                pomodoro: None,
                todos: Some(Vec::new()),
                reminder: None,
            }),
        ),
        (
            Tab::Mail,
            Body::List {
                rows: Vec::new(),
                empty: "No unread mail.".into(),
                header: None,
            },
        ),
        (
            Tab::Mixer,
            Body::Sliders {
                rows: Vec::new(),
                empty: "No app is playing sound.".into(),
            },
        ),
        (
            Tab::Time,
            Body::Usage {
                total: "Nothing yet".into(),
                bars: Vec::new(),
            },
        ),
        (Tab::Tools, Body::Buttons(Vec::new())),
        (Tab::Media, Body::Empty("Nothing enabled".into())),
    ];
    for pal in [Palette::dark(false), Palette::light(false)] {
        for scale in [1.0_f32, 1.5] {
            for (tab, body) in empties.clone() {
                let (pm, _) = render_body(body, tab, pal, scale);
                // Compare with an empty body: any difference below the header
                // is the empty-state text (secondary text is translucent, so
                // look for change rather than for opaque pixels).
                let (blank, _) = render_body(Body::Empty(String::new()), tab, pal, scale);
                let header_end = (60.0 * scale) as u32;
                let text_like = (header_end..pm.height())
                    .any(|y| (0..pm.width()).any(|x| pm.pixel(x, y) != blank.pixel(x, y)));
                let expects_text = !matches!(tab, Tab::Tools);
                assert!(
                    !expects_text || text_like,
                    "{tab:?} empty state shows text ({scale}x)"
                );
            }
        }
    }
}

#[test]
fn time_bars_are_rounded_gradients() {
    let body = Body::Usage {
        total: "3h 12m today".into(),
        bars: vec![UsageBar {
            app: "Code".into(),
            label: "1h 40m".into(),
            fraction: 1.0,
        }],
    };
    let (pm, _) = render_body(body, Tab::Time, Palette::dark(false), 1.0);
    // Find the bar: the reddest pixel row in the body.
    let mut best = (0u32, 0u32, 0u8);
    for y in 60..200 {
        for x in 20..360 {
            if let Some(p) = pm.pixel(x, y) {
                if p.red() > best.2 && p.green() < 160 {
                    best = (x, y, p.red());
                }
            }
        }
    }
    assert!(best.2 > 180, "a coral gradient bar is drawn");
}

/// Brightest pixel (max channel) in the rows `[y0, y1)` between x0 and x1.
fn brightest(pm: &Pixmap, x0: u32, x1: u32, y0: u32, y1: u32) -> u8 {
    let mut best = 0;
    for y in y0..y1.min(pm.height()) {
        for x in x0..x1.min(pm.width()) {
            if let Some(p) = pm.pixel(x, y) {
                best = best.max(p.red()).max(p.green()).max(p.blue());
            }
        }
    }
    best
}

#[test]
fn a_mirrored_notification_banner_shows_the_app_and_two_lines_of_text() {
    let view = |body: &str, app: Option<&str>| PanelView {
        banner: Some(perch_core::view::BannerView {
            leading: perch_core::view::RowLeading::Avatar {
                initials: "WA".into(),
                seed: 7,
            },
            title: "Ann".into(),
            body: body.into(),
            tab: None,
            app: app.map(str::to_string),
        }),
        ..PanelView::default()
    };
    let long = "See you at eight at the usual place, and please bring the charger \
                and the blue folder from my desk";
    let (two, _) = render(&view(long, Some("WhatsApp")), 360, 88, 1.0, 0.0);
    let (one, _) = render(&view("Short", Some("WhatsApp")), 360, 88, 1.0, 0.0);
    // Body text sits in the lower part; the long one fills a second line
    // that the short one leaves empty.
    let second_line = |pm: &Pixmap| brightest(pm, 70, 340, 62, 80);
    assert!(
        second_line(&two) > second_line(&one) + 40,
        "wrapped onto a second line: {} vs {}",
        second_line(&two),
        second_line(&one)
    );
    // The caption (app name) is drawn above the title.
    let (bare, _) = render(&view("Short", None), 360, 88, 1.0, 0.0);
    let caption_row = |pm: &Pixmap| brightest(pm, 70, 200, 8, 20);
    assert!(
        caption_row(&one) > caption_row(&bare) + 40,
        "caption shown: {} vs {}",
        caption_row(&one),
        caption_row(&bare)
    );
}

#[test]
fn a_stripe_shows_only_the_bar_not_the_pill_content() {
    let with = |collapsed: CollapsedView| PanelView {
        collapsed,
        ..PanelView::default()
    };
    let pill = CollapsedView::Pill {
        thumb: None,
        glyph: Some(perch_core::view::Glyph::Music),
        text: "Die For You".into(),
        badges: vec![],
    };
    let (stripe, _) = render(&with(pill.clone()), 120, 5, 1.0, 0.0);
    let (bare, _) = render(&with(CollapsedView::Idle), 120, 5, 1.0, 0.0);
    assert_eq!(
        stripe.data(),
        bare.data(),
        "no text or icons squeezed into the stripe"
    );
    let (full, _) = render(&with(pill), 200, 32, 1.0, 0.0);
    let (bare, _) = render(&with(CollapsedView::Idle), 200, 32, 1.0, 0.0);
    assert_ne!(
        full.data(),
        bare.data(),
        "the normal pill still shows its content"
    );
}
