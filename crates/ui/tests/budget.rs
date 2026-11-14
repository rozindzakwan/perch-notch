//! Render budget: an expanded panel must draw fast enough that the spring
//! animation (120 Hz deadlines) never drops frames on a modest CPU.
//! Release builds only; debug timings mean nothing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use perch_core::view::{Body, CollapsedView, MediaView, PanelView, Tab};
use perch_ui::render::{Frame, Renderer};
use perch_ui::theme::Palette;
use tiny_skia::Pixmap;

fn cover(side: u32) -> Arc<Vec<u8>> {
    let mut v = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            v.extend_from_slice(&[(x * 2) as u8, (y * 2) as u8, 140, 255]);
        }
    }
    Arc::new(v)
}

fn music() -> PanelView {
    PanelView {
        collapsed: CollapsedView::Idle,
        tabs: vec![
            Tab::Media,
            Tab::Tasks,
            Tab::Mail,
            Tab::Clipboard,
            Tab::Mixer,
            Tab::Time,
        ],
        tab: Some(Tab::Media),
        body: Body::Media(MediaView {
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
        pinned: false,
        banner: None,
    }
}

/// Median time of 20 renders of the Music panel at `scale`, after warm-up
/// (font and shadow caches filled, as in the running app).
fn median_render(scale: f32) -> Duration {
    let Ok(mut r) = Renderer::new() else {
        panic!("renderer");
    };
    let m = 12.0 * scale;
    let (lw, lh) = (380.0 * scale, 300.0 * scale);
    let (w, h) = ((lw + 2.0 * m) as u32, (lh + m) as u32);
    let mut f = Frame::new(w, h, scale, Palette::dark(false));
    f.progress = 1.0;
    f.inset = (m, 0.0, m, m);
    f.layout = Some((lw, lh));
    f.shadow = 12.0;
    let view = music();
    let mut pm = Pixmap::new(w, h).unwrap_or_else(|| panic!("pixmap"));
    for _ in 0..3 {
        pm.fill(tiny_skia::Color::TRANSPARENT);
        r.draw(&mut pm, &f, &view);
    }
    let mut times: Vec<Duration> = (0..20)
        .map(|_| {
            let t = Instant::now();
            pm.fill(tiny_skia::Color::TRANSPARENT);
            r.draw(&mut pm, &f, &view);
            t.elapsed()
        })
        .collect();
    times.sort();
    times[times.len() / 2]
}

#[test]
#[cfg_attr(debug_assertions, ignore = "timings are only meaningful in release")]
fn expanded_panel_renders_within_budget() {
    let one = median_render(1.0);
    let two = median_render(2.0);
    println!("music panel median: {one:?} at 1x, {two:?} at 2x");
    assert!(one <= Duration::from_millis(4), "1x: {one:?}");
    assert!(two <= Duration::from_millis(8), "2x: {two:?}");
}
