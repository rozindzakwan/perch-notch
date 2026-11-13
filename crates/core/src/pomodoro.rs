//! Pomodoro timer logic: a pure state machine driven by injected time.
//!
//! Nothing here reads the clock. The UI event loop calls [`Pomodoro::tick`]
//! with the elapsed wall time and uses [`Pomodoro::next_deadline`] to sleep
//! exactly until the current phase ends instead of polling.

use std::time::Duration;

use crate::config::PomodoroConfig;

/// Which kind of interval the timer is currently counting down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// A focus session.
    Work,
    /// The short break taken after most focus sessions.
    ShortBreak,
    /// The longer break taken after `sessions_before_long_break` focus sessions.
    LongBreak,
}

/// Whether the countdown is advancing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerState {
    /// Stopped and reset; `tick` does nothing.
    Idle,
    /// Counting down.
    Running,
    /// Frozen at the current remaining time.
    Paused,
}

/// Something the UI should react to (notify, animate, relabel the pill).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PomodoroEvent {
    /// The named phase just started counting down.
    PhaseStarted(Phase),
    /// The named phase ran out of time.
    PhaseCompleted(Phase),
    /// Enough work sessions finished that the upcoming break is a long one.
    /// Emitted between the `PhaseCompleted(Work)` and `PhaseStarted(LongBreak)`
    /// of the same transition.
    LongBreakReached,
}

/// Upper bound on phase transitions handled in a single [`Pomodoro::tick`].
///
/// Defense in depth for degenerate configs: combined with [`MIN_PHASE`] a tick
/// can never loop forever and hang the UI thread, no matter how large `dt` is
/// relative to the configured phase lengths.
const MAX_TRANSITIONS_PER_TICK: usize = 64;

/// Shortest phase the timer will run.
///
/// [`PomodoroConfig::clamp`] already enforces at least one minute, but a config
/// built directly (in tests, or by a future caller) can hold `0`. A zero-length
/// phase would complete instantly and forever, so every phase is floored here.
const MIN_PHASE: Duration = Duration::from_secs(1);

/// A 25/5 style Pomodoro timer.
#[derive(Debug, Clone)]
pub struct Pomodoro {
    cfg: PomodoroConfig,
    phase: Phase,
    state: TimerState,
    remaining: Duration,
    completed_work: u32,
}

impl Pomodoro {
    /// Build an idle timer primed with the configured work duration.
    pub fn new(cfg: PomodoroConfig) -> Self {
        let remaining = phase_len(&cfg, Phase::Work);
        Self {
            cfg,
            phase: Phase::Work,
            state: TimerState::Idle,
            remaining,
            completed_work: 0,
        }
    }

    /// The configuration this timer was built with.
    pub fn config(&self) -> &PomodoroConfig {
        &self.cfg
    }

    /// Current run state.
    pub fn state(&self) -> TimerState {
        self.state
    }

    /// Phase the timer is on (or will start on, when idle).
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Work sessions finished in the current cycle, reset after a long break.
    pub fn completed_work_sessions(&self) -> u32 {
        self.completed_work
    }

    /// Time left in the current phase; never negative.
    pub fn remaining(&self) -> Duration {
        self.remaining
    }

    /// Countdown rendered for the collapsed pill.
    ///
    /// Formatted `MM:SS` with at least two minute digits (`"24:59"`, `"00:00"`),
    /// widening to `MMM:SS` past 99 minutes (`"100:00"`) rather than switching to
    /// an hours field, so the pill label never changes shape mid-countdown.
    pub fn label(&self) -> String {
        let secs = self.remaining.as_secs();
        format!("{:02}:{:02}", secs / 60, secs % 60)
    }

    /// Start counting down from the current phase.
    pub fn start(&mut self) -> Vec<PomodoroEvent> {
        self.state = TimerState::Running;
        vec![PomodoroEvent::PhaseStarted(self.phase)]
    }

    /// Freeze the countdown, keeping the remaining time.
    pub fn pause(&mut self) {
        if self.state == TimerState::Running {
            self.state = TimerState::Paused;
        }
    }

    /// Continue a paused countdown from where it stopped.
    pub fn resume(&mut self) {
        if self.state == TimerState::Paused {
            self.state = TimerState::Running;
        }
    }

    /// Go idle and rewind the *current* phase, keeping the session counter.
    pub fn stop(&mut self) {
        self.state = TimerState::Idle;
        self.remaining = phase_len(&self.cfg, self.phase);
    }

    /// Go idle and return to a fresh first work session.
    pub fn reset(&mut self) {
        self.state = TimerState::Idle;
        self.phase = Phase::Work;
        self.completed_work = 0;
        self.remaining = phase_len(&self.cfg, Phase::Work);
    }

    /// End the current phase now and move to the next one.
    ///
    /// Does nothing while idle. A skipped work session still counts toward the
    /// long break, matching how every mainstream Pomodoro app behaves.
    pub fn skip(&mut self) -> Vec<PomodoroEvent> {
        if self.state == TimerState::Idle {
            return Vec::new();
        }
        self.advance_phase()
    }

    /// Advance the timer by `dt`, returning the transitions it crossed in order.
    ///
    /// A `dt` longer than the current phase carries its remainder into the next
    /// phase rather than discarding it, so one long tick after a suspend lands
    /// on the correct phase with the correct remaining time. At most
    /// [`MAX_TRANSITIONS_PER_TICK`] transitions are processed per call.
    pub fn tick(&mut self, dt: Duration) -> Vec<PomodoroEvent> {
        let mut events = Vec::new();
        if self.state != TimerState::Running {
            return events;
        }
        let mut left = dt;
        let mut transitions = 0usize;
        while left >= self.remaining {
            if transitions >= MAX_TRANSITIONS_PER_TICK {
                // Degenerate zero-length config: stop rather than spin forever.
                self.remaining = Duration::ZERO;
                return events;
            }
            left -= self.remaining;
            self.remaining = Duration::ZERO;
            events.extend(self.advance_phase());
            transitions += 1;
        }
        self.remaining -= left;
        events
    }

    /// Finish the current phase and start the next one, reporting both.
    fn advance_phase(&mut self) -> Vec<PomodoroEvent> {
        let finished = self.phase;
        let mut events = vec![PomodoroEvent::PhaseCompleted(finished)];
        let next = match finished {
            Phase::Work => {
                self.completed_work = self.completed_work.saturating_add(1);
                if self.completed_work >= self.cfg.sessions_before_long_break.max(1) {
                    events.push(PomodoroEvent::LongBreakReached);
                    Phase::LongBreak
                } else {
                    Phase::ShortBreak
                }
            }
            Phase::ShortBreak => Phase::Work,
            Phase::LongBreak => {
                self.completed_work = 0;
                Phase::Work
            }
        };
        self.phase = next;
        self.remaining = phase_len(&self.cfg, next);
        events.push(PomodoroEvent::PhaseStarted(next));
        events
    }

    /// How long until the timer needs waking; `None` when it cannot progress.
    pub fn next_deadline(&self) -> Option<Duration> {
        match self.state {
            TimerState::Running => Some(self.remaining),
            _ => None,
        }
    }
}

/// Configured length of one phase, floored at [`MIN_PHASE`].
fn phase_len(cfg: &PomodoroConfig, phase: Phase) -> Duration {
    let minutes = match phase {
        Phase::Work => cfg.work_minutes,
        Phase::ShortBreak => cfg.break_minutes,
        Phase::LongBreak => cfg.long_break_minutes,
    };
    Duration::from_secs(u64::from(minutes) * 60).max(MIN_PHASE)
}
