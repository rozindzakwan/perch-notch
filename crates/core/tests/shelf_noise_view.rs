use std::time::Duration;

use perch_core::geometry::{
    hot_zone_rect, notch_rect, Monitor, NotchGeometry, NotchPosition, Rect,
};
use perch_core::noise::{NoiseGenerator, NoiseKind, NoiseMix};
use perch_core::shelf::{Shelf, ShelfAdd};
use perch_core::view::{clock, human_duration};
use proptest::prelude::*;

#[test]
fn shelf_dedups_promotes_and_limits() {
    let mut s = Shelf::new(2);
    let a = s.add("/a/one.txt", false);
    assert!(matches!(a, ShelfAdd::Added(_)));
    s.add("/a/two", true);
    assert!(matches!(
        s.add("/a/one.txt", false),
        ShelfAdd::AlreadyThere(_)
    ));
    assert_eq!(
        s.items()[0].name,
        "one.txt",
        "re-adding promotes to the front"
    );
    s.add("/a/three", false);
    assert_eq!(s.len(), 2, "limit respected");
    assert!(matches!(s.add("", false), ShelfAdd::Rejected(_)));
    assert_eq!(s.prune(|p| p.ends_with("three")), 1);
    s.clear();
    assert!(s.is_empty());
}

#[test]
fn noise_mix_audibility_and_gain() {
    let mut m = NoiseMix::default();
    assert!(
        !m.is_audible(),
        "silent by default: the audio device is never opened"
    );
    m.toggle(NoiseKind::Rain);
    assert!(m.is_audible());
    m.set_level(NoiseKind::Rain, 5.0);
    assert_eq!(m.level(NoiseKind::Rain), 1.0, "clamped");
    m.set_level(NoiseKind::Rain, f32::NAN);
    assert_eq!(m.level(NoiseKind::Rain), 0.0, "NaN is rejected");
    assert!(!m.is_audible());
    assert_eq!(m.gain(NoiseKind::Waves), 0.0);
}

#[test]
fn generator_is_deterministic_bounded_and_silent_when_off() {
    let mut a = NoiseGenerator::new(48_000, 7);
    let mut b = NoiseGenerator::new(48_000, 7);
    for _ in 0..1000 {
        assert_eq!(a.next_sample(), 0.0, "all layers off -> silence");
        b.next_sample();
    }
    let mut mix = NoiseMix::default();
    for k in NoiseKind::ALL {
        mix.set_on(*k, true);
        mix.set_level(*k, 1.0);
    }
    mix.master = 1.0;
    a.set_mix(mix);
    b.set_mix(mix);
    let mut energy = 0.0f64;
    for _ in 0..96_000 {
        let x = a.next_sample();
        assert_eq!(x, b.next_sample(), "same seed, same samples");
        assert!((-1.0..=1.0).contains(&x));
        energy += f64::from(x * x);
    }
    assert!(energy / 96_000.0 > 1e-5, "it actually makes sound");
}

#[test]
fn formatting_helpers() {
    assert_eq!(clock(Duration::from_secs(65)), "1:05");
    assert_eq!(clock(Duration::from_secs(3725)), "1:02:05");
    assert_eq!(human_duration(40), "40 s");
    assert_eq!(human_duration(600), "10 min");
    assert_eq!(human_duration(3900), "1 h 05 min");
}

fn monitor(work: Option<Rect>) -> Monitor {
    Monitor {
        id: 0,
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
        scale: 1.0,
        primary: true,
        work,
    }
}

#[test]
fn a_top_taskbar_or_panel_pushes_the_notch_and_hot_zone_down() {
    let geo = NotchGeometry::default();
    let top_panel = Rect {
        x: 0,
        y: 40,
        w: 1920,
        h: 1040,
    };
    let m = monitor(Some(top_panel));
    let pill = notch_rect(&m, NotchPosition::TopCenter, &geo, false);
    assert_eq!(pill.y, 40);
    let hot = hot_zone_rect(&m, NotchPosition::TopCenter, &geo);
    assert_eq!(
        hot.rect.y, 40,
        "the hover strip hugs the panel's edge, not the screen's"
    );
    // A bottom taskbar does not move a top notch.
    let bottom = monitor(Some(Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1032,
    }));
    assert_eq!(
        notch_rect(&bottom, NotchPosition::TopCenter, &geo, false).y,
        0
    );
    assert_eq!(
        notch_rect(&bottom, NotchPosition::BottomCenter, &geo, false).y,
        1032 - 32
    );
}

#[test]
fn bogus_work_areas_are_ignored() {
    let geo = NotchGeometry::default();
    for bad in [
        Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        },
        Rect {
            x: -50,
            y: 0,
            w: 1920,
            h: 1080,
        },
        Rect {
            x: 0,
            y: 0,
            w: 4000,
            h: 1080,
        },
    ] {
        let pill = notch_rect(&monitor(Some(bad)), NotchPosition::TopCenter, &geo, false);
        assert_eq!((pill.x, pill.y), (860, 0));
    }
}

proptest! {
    #[test]
    fn the_pill_always_stays_inside_the_work_area(
        wx in 0i32..200, wy in 0i32..200, ww in 400u32..1920, wh in 300u32..1080,
        scale in 0.5f64..3.0, pos in 0usize..8, expanded in any::<bool>()
    ) {
        let ww = ww.min(1920 - wx as u32);
        let wh = wh.min(1080 - wy as u32);
        let work = Rect { x: wx, y: wy, w: ww, h: wh };
        let mut m = monitor(Some(work));
        m.scale = scale;
        let p = NotchPosition::ALL[pos];
        let r = notch_rect(&m, p, &NotchGeometry::default(), expanded);
        prop_assert!(r.x >= work.x && r.y >= work.y);
        prop_assert!(r.x + r.w as i32 <= work.x + work.w as i32);
        prop_assert!(r.y + r.h as i32 <= work.y + work.h as i32);
    }
}
