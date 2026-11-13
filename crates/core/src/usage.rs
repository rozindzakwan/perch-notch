//! Per-app screen-time aggregation, shown as "per-app usage per day".
//!
//! This module is the pure aggregation layer. It never reads a clock and never
//! touches the OS: the app layer feeds it active-window events
//! (`SetWinEventHook` on Windows, `_NET_ACTIVE_WINDOW` `PropertyNotify` on X11)
//! and persists [`UsageAggregate`] rows in SQLite.
//!
//! # Time representation
//!
//! Instants are **`u64` seconds since the Unix epoch**. Durations are `u64`
//! seconds. Day bucketing needs to know where the user's local midnight falls,
//! so the tracker takes a [`UtcOffset`] — the local offset east of UTC in
//! seconds. The timezone is deliberately *not* guessed here and no date library
//! is used; the app layer passes the offset it got from the OS.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Seconds in one day.
pub const SECONDS_PER_DAY: i64 = 86_400;

/// Bucket used when an app name is unknown or empty.
pub const UNKNOWN_APP: &str = "unknown";

/// Local offset east of UTC, in seconds (e.g. `28_800` for UTC+08:00).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UtcOffset(pub i32);

/// Convenience offset for UTC itself.
pub const UTC: UtcOffset = UtcOffset(0);

/// A local calendar day, counted in days since the Unix epoch *in local time*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DayId(pub i64);

impl DayId {
    /// The local day containing `unix_seconds` under `offset`.
    pub fn from_unix(unix_seconds: u64, offset: UtcOffset) -> Self {
        let local = unix_seconds as i64 + offset.0 as i64;
        Self(local.div_euclid(SECONDS_PER_DAY))
    }

    /// Unix second of this day's local midnight.
    pub fn start_unix(self, offset: UtcOffset) -> i64 {
        self.0 * SECONDS_PER_DAY - offset.0 as i64
    }
}

/// Per-day, per-app accumulated seconds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageAggregate {
    /// `"<day>\u{1f}<app>" -> seconds`. A flat `BTreeMap` keeps iteration
    /// deterministic and stays serde-friendly for map-keyed formats.
    totals: BTreeMap<String, u64>,
}

/// Build the flat map key for a `(day, app)` pair.
fn key(day: DayId, app: &str) -> String {
    format!("{}\u{1f}{}", day.0, app)
}

impl UsageAggregate {
    /// An empty aggregate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `seconds` to `app` on `day`.
    pub fn add(&mut self, app: &str, day: DayId, seconds: u64) {
        if seconds == 0 {
            return;
        }
        let k = key(day, &normalize_app(app));
        *self.totals.entry(k).or_insert(0) += seconds;
    }

    /// Seconds credited to `app` on `day`.
    pub fn total_for(&self, app: &str, day: DayId) -> u64 {
        self.totals
            .get(&key(day, &normalize_app(app)))
            .copied()
            .unwrap_or(0)
    }

    /// Total seconds across every app on `day`.
    pub fn grand_total(&self, day: DayId) -> u64 {
        self.entries_for(day).map(|(_, secs)| secs).sum()
    }

    /// Every `(app, seconds)` pair recorded on `day`.
    fn entries_for(&self, day: DayId) -> impl Iterator<Item = (&str, u64)> {
        let prefix = format!("{}\u{1f}", day.0);
        self.totals
            .iter()
            .filter_map(move |(k, secs)| k.strip_prefix(&prefix).map(|app| (app, *secs)))
    }

    /// Every app recorded on `day`, ordered by duration descending and then by
    /// app name ascending so ties are deterministic.
    pub fn apps_for(&self, day: DayId) -> Vec<(String, u64)> {
        let mut rows: Vec<(String, u64)> = self
            .entries_for(day)
            .map(|(app, secs)| (app.to_owned(), secs))
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        rows
    }

    /// The `n` most-used apps on `day`, using [`UsageAggregate::apps_for`]'s
    /// deterministic ordering.
    pub fn top_apps(&self, day: DayId, n: usize) -> Vec<(String, u64)> {
        let mut rows = self.apps_for(day);
        rows.truncate(n);
        rows
    }

    /// Add every bucket of `other` into `self`.
    ///
    /// Used by the app layer when it loads previously persisted rows from
    /// SQLite. Merging is commutative and associative: overlapping buckets are
    /// summed, and an empty aggregate is the identity.
    pub fn merge(&mut self, other: &UsageAggregate) {
        for (k, secs) in &other.totals {
            *self.totals.entry(k.clone()).or_insert(0) += *secs;
        }
    }
}

/// Map an app name onto its bucket, folding unknown/empty names into
/// [`UNKNOWN_APP`] so nothing is silently dropped.
fn normalize_app(app: &str) -> String {
    let trimmed = app.trim();
    if trimmed.is_empty() {
        UNKNOWN_APP.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Accumulates active-window events into a [`UsageAggregate`].
#[derive(Debug, Clone)]
pub struct UsageTracker {
    offset: UtcOffset,
    aggregate: UsageAggregate,
    current: Option<String>,
    since: u64,
    idle: bool,
    clock_regressions: u64,
}

impl UsageTracker {
    /// A tracker that buckets days using `offset`.
    pub fn new(offset: UtcOffset) -> Self {
        Self {
            offset,
            aggregate: UsageAggregate::new(),
            current: None,
            since: 0,
            idle: false,
            clock_regressions: 0,
        }
    }

    /// How many times an event arrived with a `now` earlier than the previous
    /// one (NTP correction, resume from suspend). The app layer can log this;
    /// no time is ever credited for such a step.
    pub fn clock_regressions(&self) -> u64 {
        self.clock_regressions
    }

    /// True while the user is considered away.
    pub fn is_idle(&self) -> bool {
        self.idle
    }

    /// The user went idle (`true`) or came back (`false`) at `at`.
    ///
    /// Time while idle is credited to nobody: the user walked away. Going idle
    /// first settles everything earned up to `at`; coming back restarts the
    /// clock at `at` for the app that is still in the foreground.
    pub fn set_idle(&mut self, idle: bool, at: u64) {
        if idle == self.idle {
            return;
        }
        if idle {
            self.settle(at);
        }
        self.idle = idle;
        self.since = at;
    }

    /// The aggregate accumulated so far. The in-flight slice is settled by
    /// [`UsageTracker::flush`] or the next window change.
    pub fn aggregate(&self) -> &UsageAggregate {
        &self.aggregate
    }

    /// The foreground app changed to `app` at `at`.
    ///
    /// The elapsed time since the previous event is credited to the *previous*
    /// app, never to `app`.
    pub fn active_window_changed(&mut self, app: &str, at: u64) {
        self.settle(at);
        self.current = Some(normalize_app(app));
        self.since = at;
    }

    /// Credit everything accumulated up to `now` without changing the
    /// foreground app. Safe to call from a periodic tick.
    ///
    /// A `now` earlier than the previous event (NTP step, resume from suspend)
    /// credits nothing and simply re-bases the clock.
    pub fn flush(&mut self, now: u64) {
        self.settle(now);
        self.since = now;
    }

    /// Credit the open slice `[since, now]` to the current app.
    ///
    /// A slice crossing one or more local midnights is split so each day gets
    /// exactly the seconds that fell inside it.
    fn settle(&mut self, now: u64) {
        if self.idle {
            return;
        }
        let app = match &self.current {
            Some(a) => a.clone(),
            None => return,
        };
        // Clock went backwards (NTP step, resume from suspend): never underflow
        // and never credit a bogus duration. Count it so the app layer can log.
        if now < self.since {
            self.clock_regressions += 1;
            return;
        }
        if now == self.since {
            return;
        }
        let mut cursor = self.since;
        while cursor < now {
            let day = DayId::from_unix(cursor, self.offset);
            let next_midnight = day.start_unix(self.offset) + SECONDS_PER_DAY;
            // next_midnight is strictly after cursor, so this is positive.
            let slice_end = if next_midnight <= 0 {
                now
            } else {
                now.min(next_midnight as u64)
            };
            self.aggregate
                .add(&app, day, slice_end.saturating_sub(cursor));
            cursor = slice_end;
        }
    }
}
