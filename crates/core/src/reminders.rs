//! Water and stretch reminders driven by injected time.
//!
//! # Which clock each reminder uses
//!
//! * **Water** accumulates *wall-clock* time. You still need a drink after a
//!   long meeting away from the keyboard, so idle time counts toward it.
//! * **Stretch** accumulates *active* time only. It exists to break up long
//!   stretches of continuous typing, and an hour away from the desk already
//!   served that purpose, so idle time does not count and the stretch reminder
//!   cannot fire while idle.
//!
//! Idle/active transitions come from the OS (XScreenSaver or
//! `org.freedesktop.ScreenSaver` on Linux, `GetLastInputInfo`-style polling on
//! Windows) and are pushed in with [`Reminders::set_idle`]; this module never
//! reads a clock or an input device itself.

use std::time::Duration;

use crate::config::RemindersConfig;

/// Which reminder a value refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReminderKind {
    /// Drink some water; measured in wall-clock time.
    Water,
    /// Get up and stretch; measured in active (non-idle) time.
    Stretch,
}

/// Something the UI should surface as a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderEvent {
    /// The named reminder came due; its timer has already been re-armed.
    Due(ReminderKind),
}

/// One countdown with its configured interval.
#[derive(Debug, Clone)]
struct Timer {
    /// Configured interval; `ZERO` means the reminder is disabled.
    interval: Duration,
    /// Time left before it fires.
    remaining: Duration,
}

impl Timer {
    fn new(minutes: u32) -> Self {
        let interval = Duration::from_secs(u64::from(minutes) * 60);
        Self {
            interval,
            remaining: interval,
        }
    }

    fn enabled(&self) -> bool {
        !self.interval.is_zero()
    }

    /// Advance by `dt`, returning true when the interval elapsed.
    ///
    /// Fires at most once per call regardless of how large `dt` is, so a long
    /// suspend cannot queue up a backlog of notifications.
    fn advance(&mut self, dt: Duration) -> bool {
        if !self.enabled() {
            return false;
        }
        if dt >= self.remaining {
            self.remaining = self.interval;
            true
        } else {
            self.remaining -= dt;
            false
        }
    }
}

/// Water and stretch reminder state.
#[derive(Debug, Clone)]
pub struct Reminders {
    cfg: RemindersConfig,
    water: Timer,
    stretch: Timer,
    idle: bool,
}

impl Reminders {
    /// Build reminders from config, with both timers armed and the user active.
    pub fn new(cfg: RemindersConfig) -> Self {
        let water = Timer::new(cfg.water_minutes);
        let stretch = Timer::new(cfg.stretch_minutes);
        Self {
            cfg,
            water,
            stretch,
            idle: false,
        }
    }

    /// The configuration these reminders were built with.
    pub fn config(&self) -> &RemindersConfig {
        &self.cfg
    }

    /// Whether the user is currently considered idle.
    pub fn is_idle(&self) -> bool {
        self.idle
    }

    /// Record an idle/active transition reported by the platform layer.
    pub fn set_idle(&mut self, idle: bool) {
        self.idle = idle;
    }

    /// Time left on one reminder, or `None` when that reminder is disabled.
    pub fn remaining(&self, kind: ReminderKind) -> Option<Duration> {
        let timer = self.timer(kind);
        if timer.enabled() {
            Some(timer.remaining)
        } else {
            None
        }
    }

    /// Advance both timers by `dt` and report everything that came due.
    ///
    /// Each reminder fires **at most once per call**, so resuming from a long
    /// suspend produces one water notification and one stretch notification,
    /// not one per elapsed interval, and the normal cadence continues from
    /// there. The stretch timer only advances while the user is active.
    pub fn tick(&mut self, dt: Duration) -> Vec<ReminderEvent> {
        let mut events = Vec::new();
        if self.water.advance(dt) {
            events.push(ReminderEvent::Due(ReminderKind::Water));
        }
        if !self.idle && self.stretch.advance(dt) {
            events.push(ReminderEvent::Due(ReminderKind::Stretch));
        }
        events
    }

    /// How long until the soonest reminder that *can* fire; `None` when none can.
    ///
    /// While idle the stretch timer is excluded, because it cannot fire until
    /// the user comes back; the water timer still counts down.
    pub fn next_deadline(&self) -> Option<Duration> {
        let water = self.water.enabled().then_some(self.water.remaining);
        let stretch = (!self.idle && self.stretch.enabled()).then_some(self.stretch.remaining);
        match (water, stretch) {
            (Some(w), Some(s)) => Some(w.min(s)),
            (Some(w), None) => Some(w),
            (None, Some(s)) => Some(s),
            (None, None) => None,
        }
    }

    /// Re-arm one reminder to its full interval.
    pub fn reset(&mut self, kind: ReminderKind) {
        let timer = self.timer_mut(kind);
        timer.remaining = timer.interval;
    }

    /// Push one reminder out by `dt`, capped so it cannot overflow.
    ///
    /// Snoozing a disabled reminder does nothing.
    pub fn snooze(&mut self, kind: ReminderKind, dt: Duration) {
        let timer = self.timer_mut(kind);
        if timer.enabled() {
            timer.remaining = timer.remaining.saturating_add(dt);
        }
    }

    /// Set how long until one reminder fires, capped at its interval and
    /// floored at one second. Used when a stretch reminder turns out not to
    /// have been earned because the user was idle: the remaining active time
    /// is exactly the time they were away.
    pub fn postpone_to(&mut self, kind: ReminderKind, remaining: Duration) {
        let timer = self.timer_mut(kind);
        if timer.enabled() {
            timer.remaining = remaining.min(timer.interval).max(Duration::from_secs(1));
        }
    }

    fn timer(&self, kind: ReminderKind) -> &Timer {
        match kind {
            ReminderKind::Water => &self.water,
            ReminderKind::Stretch => &self.stretch,
        }
    }

    fn timer_mut(&mut self, kind: ReminderKind) -> &mut Timer {
        match kind {
            ReminderKind::Water => &mut self.water,
            ReminderKind::Stretch => &mut self.stretch,
        }
    }
}
