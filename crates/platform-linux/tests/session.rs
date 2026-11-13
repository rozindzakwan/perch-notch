//! Session classification is pure environment logic, so it is tested on every
//! host (including the Windows dev box and the Windows CI runner).

use perch_platform_linux::session::{classify, snap_scale, support_note, SessionKind};

#[test]
fn xdg_session_type_wins_over_everything_else() {
    assert_eq!(
        classify(Some("x11"), Some("wayland-0"), Some(":0")),
        SessionKind::X11
    );
    assert_eq!(
        classify(Some("wayland"), None, Some(":0")),
        SessionKind::Wayland
    );
}

#[test]
fn session_type_is_matched_case_insensitively_and_trimmed() {
    assert_eq!(classify(Some(" X11 "), None, None), SessionKind::X11);
    assert_eq!(classify(Some("Wayland"), None, None), SessionKind::Wayland);
}

#[test]
fn wayland_display_implies_wayland_when_session_type_is_absent() {
    assert_eq!(
        classify(None, Some("wayland-0"), Some(":0")),
        SessionKind::Wayland,
        "a Wayland socket beats a stale DISPLAY from Xwayland"
    );
}

#[test]
fn display_alone_implies_x11() {
    assert_eq!(classify(None, None, Some(":0")), SessionKind::X11);
}

#[test]
fn empty_values_are_not_treated_as_present() {
    assert_eq!(classify(None, Some(""), Some("")), SessionKind::Headless);
    assert_eq!(classify(Some("tty"), None, None), SessionKind::Headless);
}

#[test]
fn nothing_set_is_headless() {
    assert_eq!(classify(None, None, None), SessionKind::Headless);
}

#[test]
fn every_session_kind_has_a_support_note() {
    for kind in [
        SessionKind::X11,
        SessionKind::Wayland,
        SessionKind::Headless,
    ] {
        assert!(!support_note(kind).is_empty(), "{kind:?} needs a note");
    }
    assert!(
        support_note(SessionKind::Wayland).contains("best effort"),
        "the Wayland note must state the limitation"
    );
}

#[test]
fn normal_density_panels_are_not_scaled() {
    // 96 dpi, and a 110 dpi laptop panel: both must stay at 1.0 rather than
    // being blown up to 1.25.
    assert_eq!(snap_scale(1.0), 1.0);
    assert_eq!(snap_scale(110.0 / 96.0), 1.0);
    assert_eq!(snap_scale(1.19), 1.0);
}

#[test]
fn dense_panels_snap_to_the_usual_steps() {
    assert_eq!(snap_scale(1.26), 1.25);
    assert_eq!(snap_scale(1.48), 1.5);
    assert_eq!(snap_scale(1.9), 2.0);
    assert_eq!(snap_scale(2.9), 3.0);
}

#[test]
fn nonsense_ratios_fall_back_to_one() {
    assert_eq!(snap_scale(0.0), 1.0);
    assert_eq!(snap_scale(-2.0), 1.0);
    assert_eq!(snap_scale(f64::NAN), 1.0);
    assert_eq!(snap_scale(f64::INFINITY), 1.0);
}
