//! Tests for per-app screen-time aggregation.

use perch_core::usage::{
    DayId, UsageAggregate, UsageTracker, UtcOffset, SECONDS_PER_DAY, UNKNOWN_APP, UTC,
};

/// Local midnight on 1970-01-02 under UTC, i.e. the start of day 1.
const DAY1: u64 = 86_400;

#[test]
fn elapsed_time_is_credited_to_the_app_that_was_in_the_foreground() {
    // A at t=0, switch to B at t=60, back to A at t=100, flush at t=130.
    // A should get 60 + 30 = 90, B should get 40.
    let mut t = UsageTracker::new(UTC);
    t.active_window_changed("A", 0);
    t.active_window_changed("B", 60);
    t.active_window_changed("A", 100);
    t.flush(130);

    let agg = t.aggregate();
    let day = DayId::from_unix(0, UTC);
    assert_eq!(
        agg.total_for("A", day),
        90,
        "A keeps the time it was focused"
    );
    assert_eq!(agg.total_for("B", day), 40, "B is not credited A's time");
    assert_eq!(agg.grand_total(day), 130);
}

#[test]
fn a_session_spanning_midnight_is_split_across_the_two_days() {
    // Focus Editor at 23:50 local and leave it until 00:20 the next day:
    // 10 minutes belong to day 0, 20 minutes to day 1.
    let start = DAY1 - 10 * 60; // 23:50 on day 0
    let end = DAY1 + 20 * 60; // 00:20 on day 1
    let mut t = UsageTracker::new(UTC);
    t.active_window_changed("Editor", start);
    t.flush(end);

    let agg = t.aggregate();
    let day0 = DayId::from_unix(start, UTC);
    let day1 = DayId::from_unix(end, UTC);
    assert_eq!(
        day1.0,
        day0.0 + 1,
        "the two instants really are on two days"
    );
    assert_eq!(agg.total_for("Editor", day0), 10 * 60, "before midnight");
    assert_eq!(agg.total_for("Editor", day1), 20 * 60, "after midnight");
    assert_eq!(agg.grand_total(day0), 10 * 60);
    assert_eq!(agg.grand_total(day1), 20 * 60);
}

#[test]
fn a_session_spanning_more_than_a_whole_day_fills_every_bucket() {
    // Start at 12:00 on day 0 and run for two days and six hours.
    let start = 12 * 3600;
    let span = 2 * SECONDS_PER_DAY as u64 + 6 * 3600;
    let mut t = UsageTracker::new(UTC);
    t.active_window_changed("Server", start);
    t.flush(start + span);

    let agg = t.aggregate();
    let d0 = DayId::from_unix(start, UTC);
    let d1 = DayId(d0.0 + 1);
    let d2 = DayId(d0.0 + 2);
    assert_eq!(agg.total_for("Server", d0), 12 * 3600, "12:00 -> midnight");
    assert_eq!(
        agg.total_for("Server", d1),
        SECONDS_PER_DAY as u64,
        "a whole day in the middle"
    );
    assert_eq!(agg.total_for("Server", d2), 18 * 3600, "midnight -> 18:00");
    let summed =
        agg.total_for("Server", d0) + agg.total_for("Server", d1) + agg.total_for("Server", d2);
    assert_eq!(summed, span, "nothing is lost or double-counted");
}

#[test]
fn a_non_utc_offset_moves_the_day_boundary() {
    // UTC+08:00: local midnight is 16:00 UTC the previous day.
    let plus8 = UtcOffset(8 * 3600);
    let local_midnight = DAY1 - 8 * 3600; // 16:00 UTC on day 0 = 00:00 local day 1
    let mut t = UsageTracker::new(plus8);
    t.active_window_changed("Chat", local_midnight - 600);
    t.flush(local_midnight + 1200);

    let agg = t.aggregate();
    let before = DayId::from_unix(local_midnight - 600, plus8);
    let after = DayId::from_unix(local_midnight + 1200, plus8);
    assert_eq!(after.0, before.0 + 1, "the local boundary was crossed");
    assert_eq!(agg.total_for("Chat", before), 600);
    assert_eq!(agg.total_for("Chat", after), 1200);
}

#[test]
fn idle_time_is_not_credited_and_resuming_credits_again() {
    // Focused at t=0, idle at t=100, back at t=400, flush at t=450.
    // 100 seconds before idle + 50 after resuming = 150. The 300 idle seconds
    // are discarded.
    let mut t = UsageTracker::new(UTC);
    t.active_window_changed("Reader", 0);
    t.set_idle(true, 100);
    t.set_idle(false, 400);
    t.flush(450);

    let day = DayId::from_unix(0, UTC);
    assert_eq!(
        t.aggregate().total_for("Reader", day),
        150,
        "idle seconds are not counted"
    );
    assert_eq!(t.aggregate().grand_total(day), 150);
}

#[test]
fn a_flush_while_idle_credits_nothing() {
    let mut t = UsageTracker::new(UTC);
    t.active_window_changed("Reader", 0);
    t.set_idle(true, 10);
    t.flush(5_000);
    t.flush(9_000);
    let day = DayId::from_unix(0, UTC);
    assert_eq!(t.aggregate().total_for("Reader", day), 10);
    assert!(t.is_idle(), "still idle");
}

#[test]
fn a_window_change_while_idle_does_not_resurrect_counting() {
    let mut t = UsageTracker::new(UTC);
    t.active_window_changed("A", 0);
    t.set_idle(true, 50);
    t.active_window_changed("B", 100);
    t.flush(200);
    let day = DayId::from_unix(0, UTC);
    assert_eq!(
        t.aggregate().total_for("A", day),
        50,
        "A only got its active time"
    );
    assert_eq!(t.aggregate().total_for("B", day), 0, "B was never active");
}

#[test]
fn a_clock_going_backwards_credits_nothing_and_does_not_underflow() {
    let mut t = UsageTracker::new(UTC);
    t.active_window_changed("A", 10_000);
    // NTP correction / resume: `now` is earlier than the previous event.
    t.flush(9_000);
    t.active_window_changed("B", 8_000);
    t.flush(7_000);

    let day = DayId::from_unix(10_000, UTC);
    let earlier = DayId::from_unix(7_000, UTC);
    assert_eq!(t.aggregate().total_for("A", day), 0, "no bogus duration");
    assert_eq!(t.aggregate().total_for("B", day), 0);
    assert_eq!(t.aggregate().grand_total(day), 0);
    assert_eq!(t.aggregate().grand_total(earlier), 0);

    // After the step the tracker keeps working from the new time base.
    t.flush(7_500);
    assert_eq!(t.aggregate().total_for("B", earlier), 500, "recovers");
}

#[test]
fn backwards_clock_steps_are_counted_so_the_app_layer_can_log_them() {
    let mut t = UsageTracker::new(UTC);
    t.active_window_changed("A", 10_000);
    assert_eq!(t.clock_regressions(), 0, "nothing odd yet");

    t.flush(9_000); // NTP step backwards
    assert_eq!(t.clock_regressions(), 1, "the regression was noticed");

    t.active_window_changed("B", 8_000); // another step backwards
    assert_eq!(t.clock_regressions(), 2);

    t.flush(8_500); // forward again
    assert_eq!(t.clock_regressions(), 2, "forward time is not a regression");
    assert_eq!(
        t.aggregate().total_for("B", DayId::from_unix(8_000, UTC)),
        500
    );
}

#[test]
fn unknown_and_unicode_app_names_are_handled() {
    let mut t = UsageTracker::new(UTC);
    t.active_window_changed("", 0);
    t.active_window_changed("   ", 10);
    t.active_window_changed("日本語アプリ", 25);
    t.flush(55);

    let day = DayId::from_unix(0, UTC);
    let agg = t.aggregate();
    assert_eq!(
        agg.total_for(UNKNOWN_APP, day),
        25,
        "empty and whitespace names land in the unknown bucket"
    );
    assert_eq!(
        agg.total_for("日本語アプリ", day),
        30,
        "unicode is preserved"
    );
    assert_eq!(agg.grand_total(day), 55, "nothing was dropped");
    assert_eq!(
        agg.total_for("", day),
        25,
        "querying with an empty name hits the same bucket"
    );
}

#[test]
fn top_apps_orders_by_duration_then_name_and_respects_n() {
    let day = DayId(19_000);
    let mut agg = UsageAggregate::new();
    agg.add("zeta", day, 300);
    agg.add("alpha", day, 100); // tie with "beta"
    agg.add("beta", day, 100); // tie with "alpha"
    agg.add("gamma", day, 500);
    agg.add("other-day", DayId(19_001), 9_999);

    let top = agg.top_apps(day, 3);
    assert_eq!(
        top,
        vec![
            ("gamma".to_owned(), 500),
            ("zeta".to_owned(), 300),
            ("alpha".to_owned(), 100),
        ],
        "duration descending, then app name ascending for ties"
    );

    let all = agg.top_apps(day, 10);
    assert_eq!(all.len(), 4, "other days are excluded");
    assert_eq!(
        all[2..],
        [("alpha".to_owned(), 100), ("beta".to_owned(), 100)],
        "the tie is broken by name, ascending"
    );
    assert!(agg.top_apps(day, 0).is_empty(), "n = 0 yields nothing");
}

#[test]
fn merging_two_aggregates_is_commutative() {
    let day = DayId(19_000);
    let other = DayId(19_001);

    let mut a = UsageAggregate::new();
    a.add("editor", day, 120);
    a.add("browser", day, 60);
    a.add("editor", other, 5);

    let mut b = UsageAggregate::new();
    b.add("editor", day, 30);
    b.add("terminal", day, 90);

    let mut ab = a.clone();
    ab.merge(&b);
    let mut ba = b.clone();
    ba.merge(&a);
    assert_eq!(ab, ba, "merge is commutative");

    assert_eq!(ab.total_for("editor", day), 150, "overlapping keys sum");
    assert_eq!(ab.total_for("browser", day), 60);
    assert_eq!(ab.total_for("terminal", day), 90);
    assert_eq!(ab.total_for("editor", other), 5, "other days survive");
    assert_eq!(ab.grand_total(day), 300);

    // Merging an empty aggregate changes nothing.
    let mut c = a.clone();
    c.merge(&UsageAggregate::new());
    assert_eq!(c, a, "empty merge is the identity");
}

#[test]
fn an_aggregate_round_trips_through_serde() {
    let day = DayId(19_321);
    let mut agg = UsageAggregate::new();
    agg.add("Code.exe", day, 3_600);
    agg.add("日本語アプリ", day, 42);
    agg.add("firefox", DayId(19_322), 120);

    let encoded = match toml::to_string(&agg) {
        Ok(s) => s,
        Err(e) => panic!("serialize failed: {e}"),
    };
    let back: UsageAggregate = match toml::from_str(&encoded) {
        Ok(v) => v,
        Err(e) => panic!("deserialize failed: {e}\n{encoded}"),
    };
    assert_eq!(back, agg, "round-trip is lossless");
    assert_eq!(back.total_for("Code.exe", day), 3_600);
    assert_eq!(back.total_for("日本語アプリ", day), 42);
    assert_eq!(back.grand_total(DayId(19_322)), 120);

    // An empty payload must load as an empty aggregate.
    let empty: UsageAggregate = match toml::from_str("") {
        Ok(v) => v,
        Err(e) => panic!("an empty payload must load: {e}"),
    };
    assert_eq!(empty, UsageAggregate::new());
    assert_eq!(empty.grand_total(day), 0);
}

#[test]
fn a_full_day_of_switching_adds_up() {
    // A realistic-ish sequence: work, lunch (idle), work, overnight idle.
    let mut t = UsageTracker::new(UTC);
    t.active_window_changed("Code.exe", 9 * 3600);
    t.active_window_changed("firefox", 11 * 3600);
    t.set_idle(true, 12 * 3600);
    t.set_idle(false, 13 * 3600);
    t.active_window_changed("Code.exe", 13 * 3600);
    t.flush(17 * 3600);

    let day = DayId::from_unix(9 * 3600, UTC);
    let agg = t.aggregate();
    assert_eq!(agg.total_for("Code.exe", day), 6 * 3600, "2h + 4h");
    assert_eq!(agg.total_for("firefox", day), 3600, "11:00 -> 12:00");
    assert_eq!(agg.grand_total(day), 7 * 3600, "the idle hour is excluded");
    assert_eq!(
        agg.top_apps(day, 1),
        vec![("Code.exe".to_owned(), 6 * 3600)]
    );
}
