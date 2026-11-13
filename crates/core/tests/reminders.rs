//! TDD tests for the water/stretch reminders. Time is injected via `tick`.

use std::time::Duration;

use perch_core::config::RemindersConfig;
use perch_core::reminders::{ReminderEvent, ReminderKind, Reminders};

const MIN: u64 = 60;

fn cfg(water: u32, stretch: u32) -> RemindersConfig {
    RemindersConfig {
        water_minutes: water,
        stretch_minutes: stretch,
        idle_threshold_seconds: 120,
    }
}

fn mins(n: u64) -> Duration {
    Duration::from_secs(n * MIN)
}

#[test]
fn a_fresh_set_of_reminders_schedules_the_soonest_enabled_timer() {
    let r = Reminders::new(cfg(60, 45));
    assert_eq!(r.next_deadline(), Some(mins(45)), "stretch is sooner");
    assert!(!r.is_idle(), "sessions start active");
    assert_eq!(r.remaining(ReminderKind::Water), Some(mins(60)));
    assert_eq!(r.remaining(ReminderKind::Stretch), Some(mins(45)));
}

#[test]
fn the_water_reminder_fires_on_its_interval_and_then_rearms() {
    let mut r = Reminders::new(cfg(60, 0));
    assert!(r.tick(mins(59)).is_empty(), "nothing before the interval");
    assert_eq!(r.next_deadline(), Some(mins(1)));
    let events = r.tick(mins(1));
    assert_eq!(events, vec![ReminderEvent::Due(ReminderKind::Water)]);
    assert_eq!(
        r.remaining(ReminderKind::Water),
        Some(mins(60)),
        "the timer resets after firing"
    );
    assert!(r.tick(mins(59)).is_empty());
    assert_eq!(
        r.tick(mins(1)),
        vec![ReminderEvent::Due(ReminderKind::Water)]
    );
}

#[test]
fn the_stretch_timer_does_not_accumulate_while_idle() {
    let mut r = Reminders::new(cfg(0, 45));
    r.tick(mins(10));
    assert_eq!(r.remaining(ReminderKind::Stretch), Some(mins(35)));

    r.set_idle(true);
    assert!(r.is_idle());
    let events = r.tick(mins(120));
    assert!(
        events.is_empty(),
        "two idle hours must not trigger a stretch: {events:?}"
    );
    assert_eq!(
        r.remaining(ReminderKind::Stretch),
        Some(mins(35)),
        "idle time does not drain the stretch timer"
    );

    r.set_idle(false);
    r.tick(mins(35));
    assert_eq!(
        r.tick(Duration::ZERO),
        vec![],
        "it fired exactly on the active boundary, not twice"
    );
    // Prove it did fire at that boundary by checking the re-arm.
    assert_eq!(r.remaining(ReminderKind::Stretch), Some(mins(45)));
}

#[test]
fn the_water_timer_keeps_running_while_idle() {
    let mut r = Reminders::new(cfg(60, 45));
    r.set_idle(true);
    let events = r.tick(mins(60));
    assert_eq!(
        events,
        vec![ReminderEvent::Due(ReminderKind::Water)],
        "water is wall-clock: it still fires after a break"
    );
    assert_eq!(
        r.remaining(ReminderKind::Stretch),
        Some(mins(45)),
        "stretch untouched by the same idle hour"
    );
}

#[test]
fn going_idle_removes_stretch_from_the_deadline_but_not_water() {
    let mut r = Reminders::new(cfg(60, 45));
    assert_eq!(r.next_deadline(), Some(mins(45)), "active: stretch is next");
    r.set_idle(true);
    assert_eq!(
        r.next_deadline(),
        Some(mins(60)),
        "idle: only water can fire, so water sets the wakeup"
    );
    r.set_idle(false);
    assert_eq!(r.next_deadline(), Some(mins(45)), "active again");
}

#[test]
fn idle_with_only_a_stretch_reminder_needs_no_wakeup_at_all() {
    let mut r = Reminders::new(cfg(0, 45));
    assert_eq!(r.next_deadline(), Some(mins(45)));
    r.set_idle(true);
    assert_eq!(
        r.next_deadline(),
        None,
        "nothing can fire while idle, so the loop can sleep indefinitely"
    );
}

#[test]
fn a_zero_minute_reminder_is_disabled_and_never_fires() {
    let mut r = Reminders::new(cfg(0, 0));
    assert_eq!(r.remaining(ReminderKind::Water), None);
    assert_eq!(r.remaining(ReminderKind::Stretch), None);
    assert_eq!(
        r.next_deadline(),
        None,
        "disabled reminders contribute no deadline"
    );
    let events = r.tick(Duration::from_secs(24 * 3600));
    assert!(
        events.is_empty(),
        "disabled reminders never fire: {events:?}"
    );

    // One disabled, one enabled: only the enabled one counts.
    let mut w = Reminders::new(cfg(60, 0));
    assert_eq!(w.next_deadline(), Some(mins(60)));
    assert_eq!(
        w.tick(mins(60)),
        vec![ReminderEvent::Due(ReminderKind::Water)]
    );
    let mut s = Reminders::new(cfg(0, 45));
    assert_eq!(s.next_deadline(), Some(mins(45)));
    assert_eq!(
        s.tick(mins(45)),
        vec![ReminderEvent::Due(ReminderKind::Stretch)]
    );
}

#[test]
fn a_twenty_four_hour_tick_fires_each_reminder_only_once() {
    let mut r = Reminders::new(cfg(60, 45));
    let events = r.tick(Duration::from_secs(24 * 3600));
    assert_eq!(
        events,
        vec![
            ReminderEvent::Due(ReminderKind::Water),
            ReminderEvent::Due(ReminderKind::Stretch),
        ],
        "resume from suspend notifies once per reminder, not 24 and 32 times"
    );
    // Normal cadence resumes from a full interval.
    assert_eq!(r.remaining(ReminderKind::Water), Some(mins(60)));
    assert_eq!(r.remaining(ReminderKind::Stretch), Some(mins(45)));
    assert_eq!(r.next_deadline(), Some(mins(45)));
    assert!(r.tick(mins(44)).is_empty());
    assert_eq!(
        r.tick(mins(1)),
        vec![ReminderEvent::Due(ReminderKind::Stretch)]
    );
}

#[test]
fn after_a_long_tick_the_timer_restarts_from_a_full_interval() {
    // A dt that is NOT a whole multiple of the interval: the timer must re-arm
    // to the full interval rather than aligning to the elapsed remainder, so
    // the next reminder is one full interval after the resume, not sooner.
    let mut r = Reminders::new(cfg(60, 0));
    assert_eq!(
        r.tick(mins(100)),
        vec![ReminderEvent::Due(ReminderKind::Water)]
    );
    assert_eq!(
        r.remaining(ReminderKind::Water),
        Some(mins(60)),
        "the 40 minutes of overshoot are discarded, not carried over"
    );
    assert_eq!(r.next_deadline(), Some(mins(60)));
    assert!(r.tick(mins(59)).is_empty(), "no early second reminder");
    assert_eq!(
        r.tick(mins(1)),
        vec![ReminderEvent::Due(ReminderKind::Water)]
    );

    // Same for the stretch timer, with an interval that does not divide the dt.
    let mut s = Reminders::new(cfg(0, 45));
    assert_eq!(
        s.tick(mins(70)),
        vec![ReminderEvent::Due(ReminderKind::Stretch)]
    );
    assert_eq!(s.remaining(ReminderKind::Stretch), Some(mins(45)));
}

#[test]
fn snooze_postpones_only_the_named_reminder() {
    let mut r = Reminders::new(cfg(60, 45));
    r.tick(mins(44));
    assert_eq!(r.remaining(ReminderKind::Stretch), Some(mins(1)));
    r.snooze(ReminderKind::Stretch, mins(10));
    assert_eq!(r.remaining(ReminderKind::Stretch), Some(mins(11)));
    assert_eq!(
        r.remaining(ReminderKind::Water),
        Some(mins(16)),
        "water is unaffected by a stretch snooze"
    );
    assert!(r.tick(mins(10)).is_empty(), "still snoozed");
    assert_eq!(
        r.tick(mins(1)),
        vec![ReminderEvent::Due(ReminderKind::Stretch)]
    );
}

#[test]
fn snoozing_a_disabled_reminder_does_nothing() {
    let mut r = Reminders::new(cfg(0, 45));
    r.snooze(ReminderKind::Water, mins(10));
    assert_eq!(r.remaining(ReminderKind::Water), None);
    assert_eq!(r.next_deadline(), Some(mins(45)));
    assert!(r
        .tick(Duration::from_secs(10 * 3600))
        .iter()
        .all(|e| *e == ReminderEvent::Due(ReminderKind::Stretch)));
}

#[test]
fn reset_rearms_a_reminder_to_its_full_interval() {
    let mut r = Reminders::new(cfg(60, 45));
    r.tick(mins(40));
    assert_eq!(r.remaining(ReminderKind::Water), Some(mins(20)));
    r.reset(ReminderKind::Water);
    assert_eq!(r.remaining(ReminderKind::Water), Some(mins(60)));
    // The stretch timer kept its position.
    assert_eq!(r.remaining(ReminderKind::Stretch), Some(mins(5)));
    r.reset(ReminderKind::Stretch);
    assert_eq!(r.remaining(ReminderKind::Stretch), Some(mins(45)));
}

#[test]
fn a_zero_length_tick_changes_nothing() {
    let mut r = Reminders::new(cfg(60, 45));
    assert!(r.tick(Duration::ZERO).is_empty());
    assert_eq!(r.remaining(ReminderKind::Water), Some(mins(60)));
    assert_eq!(r.remaining(ReminderKind::Stretch), Some(mins(45)));
}

#[test]
fn many_small_ticks_match_one_big_tick_for_the_water_timer() {
    let mut small = Reminders::new(cfg(60, 0));
    let mut fired = 0;
    for _ in 0..60 {
        fired += small.tick(mins(1)).len();
    }
    assert_eq!(fired, 1, "60 one-minute ticks produce exactly one reminder");
    assert_eq!(small.remaining(ReminderKind::Water), Some(mins(60)));
}

#[test]
fn postpone_to_caps_at_the_interval_and_floors_at_one_second() {
    use perch_core::config::RemindersConfig;
    use perch_core::reminders::{ReminderKind, Reminders};
    use std::time::Duration;
    let mut r = Reminders::new(RemindersConfig {
        water_minutes: 0,
        stretch_minutes: 45,
        idle_threshold_seconds: 120,
    });
    r.postpone_to(ReminderKind::Stretch, Duration::from_secs(600));
    assert_eq!(
        r.remaining(ReminderKind::Stretch),
        Some(Duration::from_secs(600))
    );
    r.postpone_to(ReminderKind::Stretch, Duration::from_secs(99_999));
    assert_eq!(
        r.remaining(ReminderKind::Stretch),
        Some(Duration::from_secs(45 * 60))
    );
    r.postpone_to(ReminderKind::Stretch, Duration::ZERO);
    assert_eq!(
        r.remaining(ReminderKind::Stretch),
        Some(Duration::from_secs(1))
    );
    r.postpone_to(ReminderKind::Water, Duration::from_secs(5));
    assert_eq!(
        r.remaining(ReminderKind::Water),
        None,
        "disabled stays disabled"
    );
}
