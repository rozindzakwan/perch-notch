//! Render the pill and every panel to PNG files, off-screen, for eyeballing
//! the design without running the app.
//!
//! Usage: `cargo run -p perch-ui --example preview -- <out-dir> [scale] [light]`

use std::path::PathBuf;
use std::sync::Arc;

use perch_core::noise::NoiseKind;
use perch_core::view::{
    Action, Badge, BadgeKind, Body, CollapsedView, MediaView, PanelView, PomodoroView, Row,
    SliderRow, SliderTarget, Tab, TasksView, UsageBar,
};
use perch_ui::render::{Frame, Renderer};
use perch_ui::theme::Palette;
use tiny_skia::{Color, Pixmap};

fn cover(side: u32) -> Arc<Vec<u8>> {
    let mut v = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let t = (x + y) as f32 / (2 * side) as f32;
            let r = (255.0 * (1.0 - t) + 196.0 * t) as u8;
            let g = (122.0 * (1.0 - t) + 49.0 * t) as u8;
            let b = (89.0 * (1.0 - t) + 122.0 * t) as u8;
            v.extend_from_slice(&[r, g, b, 255]);
        }
    }
    Arc::new(v)
}

fn row(text: &str, detail: &str) -> Row {
    Row {
        text: text.into(),
        detail: detail.into(),
        check: None,
        primary: Some(Action::OpenSettings),
        remove: None,
        dim: false,
        ..Row::default()
    }
}

fn mail(sender: &str, subject: &str, age: &str) -> Row {
    Row {
        text: sender.into(),
        subtitle: Some(subject.into()),
        detail: age.into(),
        leading: perch_core::view::RowLeading::Avatar {
            initials: perch_core::view::initials(sender),
            seed: perch_core::view::seed(sender),
        },
        ..Row::default()
    }
}

fn panels() -> Vec<(&'static str, Tab, Body)> {
    vec![
        (
            "music",
            Tab::Media,
            Body::Media(MediaView {
                title: "Die For You".into(),
                artist: "The Weeknd".into(),
                has_player: true,
                playing: true,
                fraction: Some(0.42),
                times: Some(("1:47".into(), "4:20".into())),
                lyric: Some("'Cause I'm right for you, babe".into()),
                cover: Some((cover(128), 128)),
                accent: Some(([0xFF, 0x7A, 0x59], [0xC4, 0x31, 0x7A])),
                next_lyric: Some("I think it's time that you knew".into()),
            }),
        ),
        (
            "tasks",
            Tab::Tasks,
            Body::Tasks(TasksView {
                pomodoro: Some(PomodoroView {
                    label: "Focus 16:12".into(),
                    running: true,
                    fraction: 0.35,
                    remaining: "16:12".into(),
                    session: "Session 2 of 4 - break next".into(),
                }),
                todos: Some(vec![
                    row("Send the quarterly report", "14:30"),
                    row("Call the dentist", ""),
                ]),
                reminder: None,
            }),
        ),
        (
            "mail",
            Tab::Mail,
            Body::List {
                rows: vec![
                    mail("Ann Lee", "Quarterly report", "2m"),
                    mail("J\u{fc}rgen M.", "Halo \u{1F600}", "14m"),
                    mail("Build Bot", "Nightly build passed", "1h"),
                ],
                empty: "No unread mail.".into(),
                header: Some(("Accounts".into(), Action::OpenSettings)),
            },
        ),
        (
            "mixer",
            Tab::Mixer,
            Body::Sliders {
                rows: vec![
                    SliderRow {
                        label: "Spotify".into(),
                        value: 0.8,
                        on: true,
                        target: SliderTarget::Volume("spotify".into()),
                        toggle: Action::MuteToggle("spotify".into()),
                        leading: perch_core::view::RowLeading::Disc {
                            initials: "S".into(),
                            rgb: [0x1D, 0xB9, 0x54],
                        },
                    },
                    SliderRow {
                        label: "Chrome".into(),
                        value: 0.55,
                        on: true,
                        target: SliderTarget::Volume("chrome".into()),
                        toggle: Action::MuteToggle("chrome".into()),
                        leading: perch_core::view::RowLeading::Disc {
                            initials: "C".into(),
                            rgb: [0x42, 0x85, 0xF4],
                        },
                    },
                ],
                empty: "No app is playing sound.".into(),
            },
        ),
        (
            "sounds",
            Tab::Sounds,
            Body::Sliders {
                rows: vec![SliderRow {
                    label: "Rain".into(),
                    value: 0.5,
                    on: true,
                    target: SliderTarget::Noise(NoiseKind::Rain),
                    toggle: Action::OpenSettings,
                    leading: perch_core::view::RowLeading::Kind(perch_core::view::RowKind::Sound),
                }],
                empty: String::new(),
            },
        ),
        (
            "time",
            Tab::Time,
            Body::Usage {
                total: "3h 12m today".into(),
                bars: vec![
                    UsageBar {
                        app: "Code".into(),
                        label: "1h 40m".into(),
                        fraction: 1.0,
                    },
                    UsageBar {
                        app: "Chrome".into(),
                        label: "52m".into(),
                        fraction: 0.52,
                    },
                ],
            },
        ),
    ]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    let scale: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let light = args.next().is_some_and(|s| s == "light");
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("cannot create {}: {e}", out.display());
        return;
    }
    let pal = if light {
        Palette::light(false)
    } else {
        Palette::dark(false)
    };
    let Ok(mut r) = Renderer::new() else {
        eprintln!("renderer failed");
        return;
    };
    let wall = if light {
        Color::from_rgba8(210, 214, 222, 255)
    } else {
        Color::from_rgba8(46, 56, 84, 255)
    };
    let save = |name: &str, w: u32, h: u32, frame: &Frame, view: &PanelView, r: &mut Renderer| {
        let Some(mut pm) = Pixmap::new(w, h) else {
            return;
        };
        r.draw(&mut pm, frame, view);
        // Composite over a wallpaper-ish colour so translucency is visible.
        let Some(mut bg) = Pixmap::new(w, h) else {
            return;
        };
        bg.fill(wall);
        bg.draw_pixmap(
            0,
            0,
            pm.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            tiny_skia::Transform::identity(),
            None,
        );
        let path = out.join(format!("{name}.png"));
        if let Err(e) = bg.save_png(&path) {
            eprintln!("{}: {e}", path.display());
        } else {
            println!("{}", path.display());
        }
    };
    let tabs = vec![
        Tab::Media,
        Tab::Tasks,
        Tab::Mail,
        Tab::Clipboard,
        Tab::Mixer,
        Tab::Time,
    ];
    // Pill.
    let pill = PanelView {
        collapsed: CollapsedView::Pill {
            thumb: Some((cover(64), 64)),
            glyph: None,
            text: "Die For You".into(),
            badges: vec![Badge {
                kind: BadgeKind::Mail,
                count: 3,
            }],
        },
        ..PanelView::default()
    };
    let pw = (r.pill_width(&pill.collapsed, scale).max(200) as f32 * scale) as u32;
    let ph = (32.0 * scale) as u32;
    let f = Frame::new(pw, ph, scale, pal);
    save("pill", pw, ph, &f, &pill, &mut r);
    // A new-mail banner.
    let banner = PanelView {
        banner: Some(perch_core::view::BannerView {
            leading: perch_core::view::RowLeading::Avatar {
                initials: "BB".into(),
                seed: perch_core::view::seed("Build Bot"),
            },
            title: "Build Bot".into(),
            body: "Nightly build passed".into(),
            tab: Some(Tab::Mail),
        }),
        ..PanelView::default()
    };
    let (bw, bh) = ((300.0 * scale) as u32, (56.0 * scale) as u32);
    let f = Frame::new(bw, bh, scale, pal);
    save("banner", bw, bh, &f, &banner, &mut r);
    // Panels with a shadow margin, as the app draws them.
    let m = 12.0 * scale;
    let (lw, lh) = (380.0 * scale, 300.0 * scale);
    for (name, tab, body) in panels() {
        let view = PanelView {
            collapsed: CollapsedView::Idle,
            tabs: tabs.clone(),
            tab: Some(tab),
            body,
            pinned: false,
            banner: None,
        };
        let (w, h) = ((lw + 2.0 * m) as u32, (lh + m) as u32);
        let mut f = Frame::new(w, h, scale, pal);
        f.progress = 1.0;
        f.inset = (m, 0.0, m, m);
        f.layout = Some((lw, lh));
        f.shadow = 12.0;
        save(name, w, h, &f, &view, &mut r);
    }
}
