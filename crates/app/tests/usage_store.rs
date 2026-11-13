//! Screen-time persistence: SQLite round trip and idempotent upserts.
#![cfg(feature = "screen-time")]

use perch_app::extras::usage_store::{open, read, write};
use perch_core::usage::{DayId, UsageAggregate, UsageTracker, UtcOffset};

#[test]
fn totals_round_trip_through_sqlite_and_upserts_are_absolute() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let db = open(&dir.path().join("usage.sqlite")).unwrap_or_else(|e| panic!("{e}"));
    let today = DayId(19_700);
    let mut agg = UsageAggregate::new();
    agg.add("editor", today, 600);
    agg.add("browser", today, 300);
    agg.add("editor", DayId(today.0 - 1), 50);

    write(&db, &agg, &[DayId(today.0 - 1), today]).unwrap_or_else(|e| panic!("{e}"));
    // Writing the same absolute totals again must not double them.
    write(&db, &agg, &[today]).unwrap_or_else(|e| panic!("{e}"));
    let back = read(&db, DayId(today.0 - 30)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(back, agg);

    // Older rows are excluded by the window.
    let recent = read(&db, today).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(recent.total_for("editor", DayId(today.0 - 1)), 0);
    assert_eq!(recent.total_for("editor", today), 600);
}

#[test]
fn a_restored_tracker_keeps_counting_on_top_of_persisted_totals() {
    let offset = UtcOffset(0);
    let day0 = 19_700i64 * 86_400;
    let mut persisted = UsageAggregate::new();
    persisted.add("editor", DayId(19_700), 1_000);
    let mut t = UsageTracker::new(offset);
    t.restore(&persisted);
    t.active_window_changed("editor", day0 as u64 + 10_000);
    t.active_window_changed("browser", day0 as u64 + 10_060);
    assert_eq!(t.aggregate().total_for("editor", DayId(19_700)), 1_060);
}
