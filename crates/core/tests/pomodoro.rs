//! TDD tests for the Pomodoro timer. Time is injected; nothing reads the clock.

use std::time::Duration;

use perch_core::config::PomodoroConfig;
use perch_core::pomodoro::{Phase, Pomodoro, PomodoroEvent, TimerState};

const MIN: u64 = 60;

fn cfg(work: u32, brk: u32, long: u32, before_long: u32) -> PomodoroConfig {
    PomodoroConfig {
        work_minutes: work,
        break_minutes: brk,
        long_break_minutes: long,
        sessions_before_long_break: before_long,
    }
}

#[test]
fn a_fresh_timer_is_idle_and_shows_the_full_work_duration() {
    let p = Pomodoro::new(cfg(25, 5, 15, 4));
    assert_eq!(p.state(), TimerState::Idle);
    assert_eq!(p.phase(), Phase::Work);
    assert_eq!(p.remaining(), Duration::from_secs(25 * MIN));
    assert_eq!(p.label(), "25:00");
}

#[test]
fn start_runs_the_work_phase_and_announces_it() {
    let mut p = Pomodoro::new(cfg(25, 5, 15, 4));
    let events = p.start();
    assert_eq!(events, vec![PomodoroEvent::PhaseStarted(Phase::Work)]);
    assert_eq!(p.state(), TimerState::Running);
    assert_eq!(p.next_deadline(), Some(Duration::from_secs(25 * MIN)));
}

#[test]
fn ticking_drains_the_current_phase_without_events_until_it_ends() {
    let mut p = Pomodoro::new(cfg(25, 5, 15, 4));
    p.start();
    let events = p.tick(Duration::from_secs(1));
    assert!(events.is_empty(), "mid-phase ticks are silent: {events:?}");
    assert_eq!(p.remaining(), Duration::from_secs(24 * MIN + 59));
    assert_eq!(p.label(), "24:59");
}

#[test]
fn pausing_freezes_remaining_and_resuming_continues_from_there() {
    let mut p = Pomodoro::new(cfg(25, 5, 15, 4));
    p.start();
    p.tick(Duration::from_secs(60));
    p.pause();
    assert_eq!(p.state(), TimerState::Paused);
    // A whole hour passes while paused; the timer must not drain at all.
    let events = p.tick(Duration::from_secs(3600));
    assert!(
        events.is_empty(),
        "a paused timer emits nothing: {events:?}"
    );
    assert_eq!(p.remaining(), Duration::from_secs(24 * MIN));
    assert_eq!(p.next_deadline(), None, "paused timers need no wakeup");

    p.resume();
    assert_eq!(p.state(), TimerState::Running);
    p.tick(Duration::from_secs(60));
    assert_eq!(p.remaining(), Duration::from_secs(23 * MIN));
}

#[test]
fn an_idle_timer_ignores_ticks_and_requests_no_wakeup() {
    let mut p = Pomodoro::new(cfg(25, 5, 15, 4));
    assert_eq!(p.next_deadline(), None);
    let events = p.tick(Duration::from_secs(10 * 3600));
    assert!(events.is_empty());
    assert_eq!(p.remaining(), Duration::from_secs(25 * MIN));
}

#[test]
fn finishing_work_completes_it_and_starts_a_short_break() {
    let mut p = Pomodoro::new(cfg(25, 5, 15, 4));
    p.start();
    let events = p.tick(Duration::from_secs(25 * MIN));
    assert_eq!(
        events,
        vec![
            PomodoroEvent::PhaseCompleted(Phase::Work),
            PomodoroEvent::PhaseStarted(Phase::ShortBreak),
        ]
    );
    assert_eq!(p.phase(), Phase::ShortBreak);
    assert_eq!(p.remaining(), Duration::from_secs(5 * MIN));
    assert_eq!(p.completed_work_sessions(), 1);
    assert_eq!(p.state(), TimerState::Running);
}

#[test]
fn a_tick_past_the_end_carries_the_remainder_into_the_next_phase() {
    let mut p = Pomodoro::new(cfg(25, 5, 15, 4));
    p.start();
    // 60 s past the end of work: break should already be 60 s in.
    let events = p.tick(Duration::from_secs(25 * MIN + 60));
    assert_eq!(
        events,
        vec![
            PomodoroEvent::PhaseCompleted(Phase::Work),
            PomodoroEvent::PhaseStarted(Phase::ShortBreak),
        ]
    );
    assert_eq!(p.phase(), Phase::ShortBreak);
    assert_eq!(p.remaining(), Duration::from_secs(4 * MIN));
    assert_eq!(p.label(), "04:00");
}

#[test]
fn one_sixty_minute_tick_from_a_fresh_start_walks_the_whole_sequence() {
    let mut p = Pomodoro::new(cfg(25, 5, 15, 4));
    p.start();
    // 60 min = work 25 + break 5 + work 25 = 55, leaving 5 min into break 2.
    let events = p.tick(Duration::from_secs(60 * MIN));
    assert_eq!(
        events,
        vec![
            PomodoroEvent::PhaseCompleted(Phase::Work),
            PomodoroEvent::PhaseStarted(Phase::ShortBreak),
            PomodoroEvent::PhaseCompleted(Phase::ShortBreak),
            PomodoroEvent::PhaseStarted(Phase::Work),
            PomodoroEvent::PhaseCompleted(Phase::Work),
            PomodoroEvent::PhaseStarted(Phase::ShortBreak),
            PomodoroEvent::PhaseCompleted(Phase::ShortBreak),
            PomodoroEvent::PhaseStarted(Phase::Work),
        ]
    );
    assert_eq!(p.phase(), Phase::Work);
    assert_eq!(p.completed_work_sessions(), 2);
    assert_eq!(p.remaining(), Duration::from_secs(25 * MIN));
    assert_eq!(
        p.next_deadline(),
        Some(Duration::from_secs(25 * MIN)),
        "the timer keeps running, it does not get stuck"
    );
}

#[test]
fn the_fourth_work_session_is_followed_by_a_long_break_and_resets_the_counter() {
    let mut p = Pomodoro::new(cfg(25, 5, 15, 4));
    p.start();
    // work, break, work, break, work, break, work -> long break
    for expected in 1..=3 {
        p.tick(Duration::from_secs(25 * MIN));
        assert_eq!(p.phase(), Phase::ShortBreak, "session {expected}");
        assert_eq!(p.completed_work_sessions(), expected);
        p.tick(Duration::from_secs(5 * MIN));
        assert_eq!(p.phase(), Phase::Work);
    }
    let events = p.tick(Duration::from_secs(25 * MIN));
    assert_eq!(
        events,
        vec![
            PomodoroEvent::PhaseCompleted(Phase::Work),
            PomodoroEvent::LongBreakReached,
            PomodoroEvent::PhaseStarted(Phase::LongBreak),
        ]
    );
    assert_eq!(p.phase(), Phase::LongBreak);
    assert_eq!(p.remaining(), Duration::from_secs(15 * MIN));
    // Finishing the long break resets the cycle counter.
    p.tick(Duration::from_secs(15 * MIN));
    assert_eq!(p.phase(), Phase::Work);
    assert_eq!(
        p.completed_work_sessions(),
        0,
        "counter resets after a long break"
    );
}

#[test]
fn sessions_before_long_break_of_one_makes_every_break_long() {
    let mut p = Pomodoro::new(cfg(25, 5, 15, 1));
    p.start();
    let events = p.tick(Duration::from_secs(25 * MIN));
    assert_eq!(
        events,
        vec![
            PomodoroEvent::PhaseCompleted(Phase::Work),
            PomodoroEvent::LongBreakReached,
            PomodoroEvent::PhaseStarted(Phase::LongBreak),
        ]
    );
    assert_eq!(p.remaining(), Duration::from_secs(15 * MIN));
    p.tick(Duration::from_secs(15 * MIN));
    assert_eq!(p.phase(), Phase::Work);
    assert_eq!(p.completed_work_sessions(), 0);
    // And again, to prove the counter did not drift.
    let events = p.tick(Duration::from_secs(25 * MIN));
    assert!(
        events.contains(&PomodoroEvent::LongBreakReached),
        "every cycle is a long break when the threshold is 1: {events:?}"
    );
}

#[test]
fn reset_returns_to_an_idle_fresh_work_phase() {
    let mut p = Pomodoro::new(cfg(25, 5, 15, 4));
    p.start();
    p.tick(Duration::from_secs(26 * MIN));
    assert_eq!(p.phase(), Phase::ShortBreak);
    p.reset();
    assert_eq!(p.state(), TimerState::Idle);
    assert_eq!(p.phase(), Phase::Work);
    assert_eq!(p.remaining(), Duration::from_secs(25 * MIN));
    assert_eq!(p.completed_work_sessions(), 0);
    assert_eq!(p.next_deadline(), None);
}

#[test]
fn skip_jumps_to_the_next_phase_without_waiting() {
    let mut p = Pomodoro::new(cfg(25, 5, 15, 4));
    p.start();
    p.tick(Duration::from_secs(60));
    let events = p.skip();
    assert_eq!(
        events,
        vec![
            PomodoroEvent::PhaseCompleted(Phase::Work),
            PomodoroEvent::PhaseStarted(Phase::ShortBreak),
        ]
    );
    assert_eq!(p.phase(), Phase::ShortBreak);
    assert_eq!(p.remaining(), Duration::from_secs(5 * MIN));
    assert_eq!(
        p.completed_work_sessions(),
        1,
        "a skipped work session counts"
    );
}

#[test]
fn skipping_an_idle_timer_does_nothing() {
    let mut p = Pomodoro::new(cfg(25, 5, 15, 4));
    let events = p.skip();
    assert!(events.is_empty(), "nothing to skip when idle: {events:?}");
    assert_eq!(p.phase(), Phase::Work);
    assert_eq!(p.state(), TimerState::Idle);
}

#[test]
fn the_label_never_goes_negative_and_reads_zero_at_the_boundary() {
    let mut p = Pomodoro::new(cfg(1, 5, 15, 4));
    p.start();
    p.tick(Duration::from_secs(59));
    assert_eq!(p.label(), "00:01");
    // Exactly at the boundary the phase flips, so the new phase shows its full
    // length rather than a negative or stuck value.
    p.tick(Duration::from_secs(1));
    assert_eq!(p.phase(), Phase::ShortBreak);
    assert_eq!(p.label(), "05:00");
    // A timer parked at zero by the per-tick transition bound renders "00:00",
    // never a negative value.
    let mut z = Pomodoro::new(cfg(0, 0, 0, 4));
    z.start();
    z.tick(Duration::from_secs(365 * 24 * 3600));
    assert_eq!(z.remaining(), Duration::ZERO);
    assert_eq!(z.label(), "00:00");
    z.pause();
    assert_eq!(z.label(), "00:00");
}

#[test]
fn a_timer_finished_by_stop_shows_zero_zero_and_no_deadline() {
    let mut p = Pomodoro::new(cfg(1, 5, 15, 4));
    p.start();
    p.tick(Duration::from_secs(30));
    p.stop();
    assert_eq!(p.state(), TimerState::Idle);
    assert_eq!(p.next_deadline(), None);
    assert_eq!(p.label(), "01:00", "stop rewinds the current phase");
}

#[test]
fn durations_over_ninety_nine_minutes_widen_the_minute_field() {
    let p = Pomodoro::new(cfg(120, 5, 15, 4));
    assert_eq!(p.remaining(), Duration::from_secs(120 * MIN));
    assert_eq!(
        p.label(),
        "120:00",
        "MMM:SS past 99 minutes, no hours field"
    );
    let mut q = Pomodoro::new(cfg(100, 5, 15, 4));
    q.start();
    q.tick(Duration::from_secs(1));
    assert_eq!(q.label(), "99:59");
}

#[test]
fn a_zero_duration_phase_is_clamped_so_a_tick_cannot_loop_forever() {
    // Built directly, bypassing PomodoroConfig::clamp, so every phase is 0.
    let mut p = Pomodoro::new(cfg(0, 0, 0, 4));
    // A zero-length phase is floored to one second rather than completing
    // instantly and forever.
    assert_eq!(p.remaining(), Duration::from_secs(1));
    assert_eq!(p.label(), "00:01");
    p.start();
    // One second of time = exactly one transition, not an unbounded run.
    let events = p.tick(Duration::from_secs(1));
    assert_eq!(
        events,
        vec![
            PomodoroEvent::PhaseCompleted(Phase::Work),
            PomodoroEvent::PhaseStarted(Phase::ShortBreak),
        ]
    );
    assert_eq!(p.remaining(), Duration::from_secs(1));
}

#[test]
fn an_enormous_tick_on_a_degenerate_config_stops_at_the_transition_bound() {
    let mut p = Pomodoro::new(cfg(0, 0, 0, 4));
    p.start();
    // A year of time against one-second phases: the per-tick bound stops it.
    let events = p.tick(Duration::from_secs(365 * 24 * 3600));
    // 64 transitions, each emitting PhaseCompleted + PhaseStarted, plus one
    // extra LongBreakReached per completed 4-work cycle (8 cycles in 64).
    assert_eq!(
        events.len(),
        64 * 2 + 8,
        "tick must stop at exactly 64 transitions"
    );
    assert_eq!(
        p.remaining(),
        Duration::ZERO,
        "the bound parks the timer at zero instead of spinning"
    );
}

#[test]
fn a_one_minute_config_is_not_clamped_away() {
    // The clamp floor must not disturb a legitimate short config.
    let p = Pomodoro::new(cfg(1, 1, 1, 2));
    assert_eq!(p.remaining(), Duration::from_secs(60));
    assert_eq!(p.label(), "01:00");
}
