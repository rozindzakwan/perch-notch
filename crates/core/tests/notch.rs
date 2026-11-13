//! RED-phase tests for the notch state machine (expand/collapse/fullscreen).

use std::time::Duration;

use perch_core::notch::{Notch, NotchEvent, NotchState, Visual};

fn notch() -> Notch {
    Notch::new(Duration::from_millis(200), Duration::from_millis(200))
}

#[test]
fn starts_collapsed_and_visible() {
    let n = notch();
    assert_eq!(n.state(), NotchState::Collapsed);
    assert!(n.is_visible());
}

#[test]
fn hot_zone_entry_starts_expanding() {
    let mut n = notch();
    let changed = n.handle(NotchEvent::HotZoneEntered);
    assert!(changed, "entering the hot zone is a state change");
    assert_eq!(n.state(), NotchState::Expanding);
    assert!(n.wants_animation(), "animation runs while expanding");
}

#[test]
fn expansion_completes_after_the_animation_duration() {
    let mut n = notch();
    n.handle(NotchEvent::HotZoneEntered);
    n.advance(Duration::from_millis(100));
    assert_eq!(n.state(), NotchState::Expanding, "still mid-animation");
    assert!(n.progress() > 0.0 && n.progress() < 1.0);
    n.advance(Duration::from_millis(100));
    assert_eq!(n.state(), NotchState::Expanded);
    assert_eq!(n.progress(), 1.0);
    assert!(
        !n.wants_animation(),
        "no animation work once fully expanded (idle CPU must be 0)"
    );
}

#[test]
fn leaving_the_pill_collapses_after_the_delay() {
    let mut n = notch();
    n.handle(NotchEvent::HotZoneEntered);
    n.advance(Duration::from_millis(200));
    assert_eq!(n.state(), NotchState::Expanded);

    n.handle(NotchEvent::HotZoneLeft);
    n.handle(NotchEvent::PointerLeft);
    assert_eq!(
        n.state(),
        NotchState::Expanded,
        "grace period before collapsing"
    );
    n.advance(Duration::from_millis(100));
    assert_eq!(n.state(), NotchState::Expanded, "still within the delay");
    n.advance(Duration::from_millis(150));
    assert_eq!(n.state(), NotchState::Collapsing);
    n.advance(Duration::from_millis(200));
    assert_eq!(n.state(), NotchState::Collapsed);
    assert!(!n.wants_animation());
}

#[test]
fn leaving_the_hot_zone_without_reaching_the_pill_collapses_it() {
    // Regression (found by scripts/smoke-windows.ps1): hovering the 2px strip
    // expands the pill, but if the cursor then leaves the strip sideways it
    // never enters the pill, so the hot-zone leave is the ONLY signal we get.
    // Ignoring it left the notch stuck open forever.
    let mut n = notch();
    n.handle(NotchEvent::HotZoneEntered);
    n.advance(Duration::from_millis(200));
    assert_eq!(n.state(), NotchState::Expanded);

    n.handle(NotchEvent::HotZoneLeft);
    n.advance(Duration::from_millis(500));
    assert_eq!(
        n.state(),
        NotchState::Collapsed,
        "leaving the strip must close the notch"
    );
}

#[test]
fn moving_from_the_hot_zone_into_the_pill_keeps_it_open() {
    // The cursor crossing from the strip into the pill fires a strip-leave
    // immediately followed by a pill-enter. That must NOT close the notch.
    let mut n = notch();
    n.handle(NotchEvent::HotZoneEntered);
    n.advance(Duration::from_millis(200));

    n.handle(NotchEvent::HotZoneLeft);
    n.handle(NotchEvent::PointerEntered);
    n.advance(Duration::from_millis(800));
    assert_eq!(
        n.state(),
        NotchState::Expanded,
        "the pointer is inside the pill, so it stays open"
    );
}

#[test]
fn re_entering_during_collapse_cancels_it_without_a_jump() {
    let mut n = notch();
    n.handle(NotchEvent::HotZoneEntered);
    n.advance(Duration::from_millis(200));
    n.handle(NotchEvent::HotZoneLeft);
    n.handle(NotchEvent::PointerLeft);
    // 200ms delay + 50ms of the 200ms collapse animation: the delay and the
    // animation share one tick, so we land a quarter of the way shut.
    n.advance(Duration::from_millis(250));
    assert_eq!(n.state(), NotchState::Collapsing);
    let mid = n.progress();
    assert!(mid > 0.0 && mid < 1.0, "mid-collapse progress {mid}");

    n.handle(NotchEvent::PointerEntered);
    assert_eq!(n.state(), NotchState::Expanding);
    assert!(
        (n.progress() - mid).abs() < 0.05,
        "progress is preserved on reversal: {mid} -> {}",
        n.progress()
    );
}

#[test]
fn the_collapse_delay_does_not_eat_the_collapse_animation() {
    // Regression: a single tick longer than the delay must spend only its
    // remainder on the animation, never skip the animation entirely.
    let mut n = notch();
    n.handle(NotchEvent::HotZoneEntered);
    n.advance(Duration::from_millis(200));
    n.handle(NotchEvent::HotZoneLeft);
    n.handle(NotchEvent::PointerLeft);
    n.advance(Duration::from_millis(210)); // delay + 10ms
    assert_eq!(n.state(), NotchState::Collapsing);
    assert!(
        n.progress() > 0.8,
        "only 10ms of a 200ms collapse has run, so it is still nearly open: {}",
        n.progress()
    );
}

#[test]
fn pointer_entering_the_pill_cancels_a_pending_collapse() {
    let mut n = notch();
    n.handle(NotchEvent::HotZoneEntered);
    n.advance(Duration::from_millis(200));
    n.handle(NotchEvent::HotZoneLeft);
    n.handle(NotchEvent::PointerLeft);
    n.advance(Duration::from_millis(100));
    n.handle(NotchEvent::PointerEntered);
    n.advance(Duration::from_millis(500));
    assert_eq!(
        n.state(),
        NotchState::Expanded,
        "stays open while the pointer is inside"
    );
}

#[test]
fn fullscreen_hides_the_notch_and_restores_it_afterwards() {
    let mut n = notch();
    assert!(n.is_visible());
    let changed = n.handle(NotchEvent::FullscreenChanged(true));
    assert!(changed);
    assert!(!n.is_visible(), "hidden while a fullscreen app is focused");
    assert!(!n.wants_animation(), "no rendering while hidden");

    n.handle(NotchEvent::FullscreenChanged(false));
    assert!(n.is_visible());
}

#[test]
fn hover_while_fullscreen_does_not_expand() {
    let mut n = notch();
    n.handle(NotchEvent::FullscreenChanged(true));
    n.handle(NotchEvent::HotZoneEntered);
    n.advance(Duration::from_millis(500));
    assert_eq!(n.state(), NotchState::Collapsed);
    assert!(!n.is_visible());
}

#[test]
fn leaving_fullscreen_while_hovering_expands_once_visible_again() {
    let mut n = notch();
    n.handle(NotchEvent::FullscreenChanged(true));
    n.handle(NotchEvent::HotZoneEntered);
    n.handle(NotchEvent::FullscreenChanged(false));
    n.advance(Duration::from_millis(200));
    assert_eq!(
        n.state(),
        NotchState::Expanded,
        "the remembered hover is honored"
    );
}

#[test]
fn toggle_opens_and_closes_regardless_of_the_pointer() {
    let mut n = notch();
    n.handle(NotchEvent::Toggle);
    n.advance(Duration::from_millis(200));
    assert_eq!(n.state(), NotchState::Expanded);
    assert!(n.is_pinned(), "a toggled-open notch stays open");

    n.handle(NotchEvent::HotZoneLeft);
    n.handle(NotchEvent::PointerLeft);
    n.advance(Duration::from_millis(1000));
    assert_eq!(
        n.state(),
        NotchState::Expanded,
        "pinned, so the pointer is irrelevant"
    );

    n.handle(NotchEvent::Toggle);
    n.advance(Duration::from_millis(200));
    assert_eq!(n.state(), NotchState::Collapsed);
    assert!(!n.is_pinned());
}

#[test]
fn visual_interpolates_between_collapsed_and_expanded_sizes() {
    let mut n = notch();
    let v = n.visual((200, 32), (440, 132));
    assert_eq!(
        v,
        Visual {
            width: 200,
            height: 32,
            progress: 0.0,
            visible: true
        }
    );

    n.handle(NotchEvent::HotZoneEntered);
    n.advance(Duration::from_millis(200));
    let v = n.visual((200, 32), (440, 132));
    assert_eq!(v.width, 440);
    assert_eq!(v.height, 132);

    let mut n = notch();
    n.handle(NotchEvent::HotZoneEntered);
    n.advance(Duration::from_millis(100));
    let v = n.visual((200, 32), (440, 132));
    assert!(
        v.width > 200 && v.width < 440,
        "mid animation width {}",
        v.width
    );
    assert!(v.height > 32 && v.height < 132);
}

#[test]
fn zero_duration_animation_snaps_immediately() {
    let mut n = Notch::new(Duration::ZERO, Duration::ZERO);
    n.handle(NotchEvent::HotZoneEntered);
    n.advance(Duration::from_millis(1));
    assert_eq!(n.state(), NotchState::Expanded);
    assert!(!n.wants_animation());
}

#[test]
fn next_deadline_is_none_when_idle_so_the_loop_can_sleep_forever() {
    let mut n = notch();
    assert_eq!(n.next_deadline(), None, "collapsed and idle: no wakeups");

    n.handle(NotchEvent::HotZoneEntered);
    assert!(n.next_deadline().is_some(), "animating: needs a wakeup");
    n.advance(Duration::from_millis(200));
    assert_eq!(n.next_deadline(), None, "expanded and idle: no wakeups");

    n.handle(NotchEvent::HotZoneLeft);
    n.handle(NotchEvent::PointerLeft);
    assert!(
        n.next_deadline().is_some(),
        "pending collapse: needs one wakeup"
    );
}

#[test]
fn repeated_identical_events_do_not_report_changes() {
    let mut n = notch();
    assert!(n.handle(NotchEvent::HotZoneEntered));
    assert!(
        !n.handle(NotchEvent::HotZoneEntered),
        "already expanding, nothing changed"
    );
    assert!(
        !n.handle(NotchEvent::FullscreenChanged(false)),
        "already not fullscreen"
    );
}

#[test]
fn strip_to_pill_handover_works_in_either_event_order() {
    use perch_core::notch::{Notch, NotchEvent, NotchState};
    use std::time::Duration;
    for pill_enter_first in [false, true] {
        let mut n = Notch::new(Duration::from_millis(100), Duration::from_millis(200));
        n.handle(NotchEvent::HotZoneEntered);
        n.advance(Duration::from_millis(150));
        assert_eq!(n.state(), NotchState::Expanded);
        if pill_enter_first {
            // Windows: the growing pill reports its enter before the strip's leave.
            n.handle(NotchEvent::PointerEntered);
            n.handle(NotchEvent::HotZoneLeft);
        } else {
            n.handle(NotchEvent::HotZoneLeft);
            n.handle(NotchEvent::PointerEntered);
        }
        n.advance(Duration::from_secs(2));
        assert_eq!(
            n.state(),
            NotchState::Expanded,
            "cursor still on the pill (pill_enter_first={pill_enter_first})"
        );
        n.handle(NotchEvent::HotZoneLeft);
        n.handle(NotchEvent::PointerLeft);
        n.advance(Duration::from_secs(2));
        assert_eq!(n.state(), NotchState::Collapsed);
    }
}
