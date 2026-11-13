//! TDD tests for the to-do list. All instants are injected as unix seconds.

use std::time::Duration;

use perch_core::todo::{TodoId, TodoList};

/// A fixed "now": 2024-01-01T00:00:00Z.
const T0: u64 = 1_704_067_200;

#[test]
fn a_new_list_is_empty_and_needs_no_wakeup() {
    let list = TodoList::new();
    assert!(list.is_empty());
    assert_eq!(list.len(), 0);
    assert!(list.all().is_empty());
    assert_eq!(list.next_deadline(T0), None);
}

#[test]
fn adding_an_item_stores_text_created_at_and_a_fresh_id() {
    let mut list = TodoList::new();
    let a = list.add("buy milk", None, T0);
    let b = list.add("walk dog", Some(T0 + 600), T0 + 1);
    assert_ne!(a, b, "ids are distinct");
    assert_eq!(list.len(), 2);

    let item = match list.get(a) {
        Some(i) => i,
        None => panic!("item {a:?} should exist"),
    };
    assert_eq!(item.text, "buy milk");
    assert!(!item.done);
    assert_eq!(item.due, None);
    assert_eq!(item.created_at, T0);

    let item = match list.get(b) {
        Some(i) => i,
        None => panic!("item {b:?} should exist"),
    };
    assert_eq!(item.due, Some(T0 + 600));
}

#[test]
fn toggle_flips_done_and_reports_unknown_ids() {
    let mut list = TodoList::new();
    let a = list.add("task", None, T0);
    assert!(list.toggle(a), "toggling a real id succeeds");
    assert_eq!(list.get(a).map(|i| i.done), Some(true));
    assert!(list.toggle(a));
    assert_eq!(list.get(a).map(|i| i.done), Some(false));
    assert!(!list.toggle(TodoId(9999)), "unknown ids report failure");
}

#[test]
fn set_done_is_idempotent() {
    let mut list = TodoList::new();
    let a = list.add("task", None, T0);
    assert!(list.set_done(a, true));
    assert!(list.set_done(a, true));
    assert_eq!(list.get(a).map(|i| i.done), Some(true));
    assert!(list.set_done(a, false));
    assert_eq!(list.get(a).map(|i| i.done), Some(false));
}

#[test]
fn remove_drops_the_item_and_ids_are_never_recycled() {
    let mut list = TodoList::new();
    let a = list.add("first", None, T0);
    let b = list.add("second", None, T0);
    assert!(list.remove(b), "removing a real id succeeds");
    assert_eq!(list.len(), 1);
    assert!(list.get(b).is_none());
    assert!(!list.remove(b), "removing twice fails");

    // Removing the newest id must not let the next add reuse it.
    let c = list.add("third", None, T0);
    assert_ne!(c, b, "ids are never recycled after removal");
    assert_ne!(c, a);
    // Remove everything and add again: still no reuse.
    assert!(list.remove(a));
    assert!(list.remove(c));
    assert!(list.is_empty());
    let d = list.add("fourth", None, T0);
    assert!(
        d != a && d != b && d != c,
        "id {d:?} collided with a retired id"
    );
}

#[test]
fn edit_text_and_due_time_mutations_stick() {
    let mut list = TodoList::new();
    let a = list.add("typo", None, T0);
    assert!(list.edit_text(a, "fixed"));
    assert_eq!(list.get(a).map(|i| i.text.as_str()), Some("fixed"));
    assert!(!list.edit_text(TodoId(42), "nope"));

    assert!(list.set_due(a, Some(T0 + 300)));
    assert_eq!(list.get(a).and_then(|i| i.due), Some(T0 + 300));
    assert!(list.set_due(a, None));
    assert_eq!(list.get(a).and_then(|i| i.due), None);
    assert!(list.clear_due(a));
    assert_eq!(list.get(a).and_then(|i| i.due), None);
}

#[test]
fn changing_a_due_time_rearms_the_notification() {
    let mut list = TodoList::new();
    let a = list.add("ping", Some(T0), T0);
    assert_eq!(list.due_now(T0).len(), 1);
    list.mark_notified(a);
    assert!(list.due_now(T0).is_empty());
    // Pushing the due time out must make it fire again at the new time.
    assert!(list.set_due(a, Some(T0 + 600)));
    assert!(list.due_now(T0).is_empty(), "not due yet");
    assert_eq!(list.due_now(T0 + 600).len(), 1, "re-armed at the new time");
}

#[test]
fn pending_done_overdue_and_due_soon_views_filter_correctly() {
    let mut list = TodoList::new();
    let soon = list.add("soon", Some(T0 + 60), T0);
    let late = list.add("late", Some(T0 - 60), T0);
    let far = list.add("far", Some(T0 + 10_000), T0);
    let plain = list.add("plain", None, T0);
    let finished = list.add("finished", Some(T0 - 5), T0);
    assert!(list.set_done(finished, true));

    let ids = |v: Vec<&perch_core::todo::TodoItem>| v.into_iter().map(|i| i.id).collect::<Vec<_>>();

    assert_eq!(ids(list.pending()), vec![late, soon, far, plain]);
    assert_eq!(ids(list.done()), vec![finished]);
    assert_eq!(
        ids(list.overdue(T0)),
        vec![late],
        "done items are never overdue"
    );
    assert_eq!(
        ids(list.due_soon(T0, Duration::from_secs(120))),
        vec![late, soon],
        "due-soon includes already-overdue work inside the window"
    );
    assert!(list
        .due_soon(T0, Duration::from_secs(0))
        .iter()
        .all(|i| i.id == late));
}

#[test]
fn ordering_is_pending_then_due_then_created_then_id() {
    let mut list = TodoList::new();
    // Two items identical except for id, to pin the final tiebreaker.
    let tie_a = list.add("tie a", Some(T0 + 100), T0);
    let tie_b = list.add("tie b", Some(T0 + 100), T0);
    let earlier = list.add("earlier due", Some(T0 + 50), T0 + 5);
    let undated_old = list.add("undated old", None, T0 + 1);
    let undated_new = list.add("undated new", None, T0 + 2);
    let done_early = list.add("done but early", Some(T0), T0);
    assert!(list.set_done(done_early, true));

    let order: Vec<_> = list.all().into_iter().map(|i| i.id).collect();
    assert_eq!(
        order,
        vec![
            earlier, // soonest due
            tie_a,   // same due + same created_at -> lower id first
            tie_b,
            undated_old, // no due date: after dated, by created_at
            undated_new,
            done_early, // done always last, even though it was due first
        ]
    );
}

#[test]
fn fully_tied_items_fall_back_to_ascending_id() {
    // A stored payload listing fully-tied items in descending id order: the
    // only field that can order them is the id, so loading it and listing must
    // reorder them ascending rather than echoing the stored sequence.
    let text = r#"
        next_id = 4
        [[items]]
        id = 3
        text = "same"
        due = 1704067210
        created_at = 1704067200
        [[items]]
        id = 1
        text = "same"
        due = 1704067210
        created_at = 1704067200
        [[items]]
        id = 2
        text = "same"
        due = 1704067210
        created_at = 1704067200
    "#;
    let list: TodoList = match toml::from_str(text) {
        Ok(v) => v,
        Err(e) => panic!("load: {e}"),
    };
    let ids: Vec<_> = list.all().into_iter().map(|i| i.id).collect();
    assert_eq!(
        ids,
        vec![TodoId(1), TodoId(2), TodoId(3)],
        "fully tied items are ordered by ascending id, not storage order"
    );
}

#[test]
fn an_item_due_exactly_now_counts_as_overdue_and_due() {
    let mut list = TodoList::new();
    let a = list.add("on the dot", Some(T0), T0 - 10);
    // The boundary itself is inclusive: due == now means it has arrived.
    assert_eq!(
        list.overdue(T0).len(),
        1,
        "an item due exactly at now is overdue"
    );
    assert_eq!(list.due_now(T0).len(), 1);
    assert_eq!(list.next_deadline(T0), Some(Duration::ZERO));
    // One second earlier it is not yet due.
    assert!(list.overdue(T0 - 1).is_empty());
    assert!(list.due_now(T0 - 1).is_empty());
    assert_eq!(list.next_deadline(T0 - 1), Some(Duration::from_secs(1)));
    // And one second later it still is.
    assert_eq!(list.overdue(T0 + 1).len(), 1);
    assert_eq!(list.due_now(T0 + 1).first().map(|i| i.id), Some(a));
}

#[test]
fn a_due_item_is_reported_exactly_once() {
    let mut list = TodoList::new();
    let a = list.add("ring", Some(T0 + 10), T0);

    assert!(list.due_now(T0).is_empty(), "not due yet");
    let first = list.due_now(T0 + 10);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].id, a);

    // Until it is marked, the same now keeps reporting it...
    assert_eq!(list.due_now(T0 + 10).len(), 1);
    assert!(list.mark_notified(a));
    // ...and after marking it never comes back, at this or any later now.
    assert!(
        list.due_now(T0 + 10).is_empty(),
        "no repeat notification for the same now"
    );
    assert!(list.due_now(T0 + 9999).is_empty(), "nor on later ticks");
}

#[test]
fn take_due_is_safe_to_call_every_tick() {
    let mut list = TodoList::new();
    let a = list.add("one", Some(T0), T0);
    let b = list.add("two", Some(T0 + 5), T0);
    assert_eq!(list.take_due(T0), vec![a]);
    assert!(list.take_due(T0).is_empty(), "already delivered");
    assert_eq!(list.take_due(T0 + 5), vec![b]);
    // Twenty more ticks must not produce twenty more notifications.
    for t in 0..20 {
        assert!(
            list.take_due(T0 + 100 + t).is_empty(),
            "repeat notification at +{t}"
        );
    }
}

#[test]
fn next_deadline_is_zero_for_an_overdue_item_and_never_underflows() {
    let mut list = TodoList::new();
    let a = list.add("way overdue", Some(T0 - 86_400), T0);
    assert_eq!(
        list.next_deadline(T0),
        Some(Duration::ZERO),
        "an overdue item needs waking immediately, not never"
    );
    // The extreme case: due at epoch 0 with a large now.
    let mut l2 = TodoList::new();
    l2.add("epoch", Some(0), T0);
    assert_eq!(l2.next_deadline(u64::MAX), Some(Duration::ZERO));

    // Once notified it stops contributing.
    assert!(list.mark_notified(a));
    assert_eq!(list.next_deadline(T0), None);
}

#[test]
fn next_deadline_picks_the_soonest_pending_item_and_ignores_done_ones() {
    let mut list = TodoList::new();
    list.add("later", Some(T0 + 900), T0);
    let soon = list.add("sooner", Some(T0 + 60), T0);
    list.add("undated", None, T0);
    assert_eq!(list.next_deadline(T0), Some(Duration::from_secs(60)));

    assert!(list.set_done(soon, true));
    assert_eq!(
        list.next_deadline(T0),
        Some(Duration::from_secs(900)),
        "completed items do not schedule wakeups"
    );
}

#[test]
fn only_undated_items_means_no_wakeup() {
    let mut list = TodoList::new();
    list.add("someday", None, T0);
    list.add("maybe", None, T0 + 1);
    assert_eq!(list.next_deadline(T0), None);
}

#[test]
fn the_list_round_trips_through_serde() {
    let mut list = TodoList::new();
    let a = list.add("with due", Some(T0 + 60), T0);
    let b = list.add("plain", None, T0 + 1);
    assert!(list.set_done(b, true));
    assert!(list.mark_notified(a));
    // Retire an id so next_id is proven to survive the round trip.
    let c = list.add("doomed", None, T0 + 2);
    assert!(list.remove(c));

    let text = match toml::to_string(&list) {
        Ok(s) => s,
        Err(e) => panic!("serialize: {e}"),
    };
    let mut back: TodoList = match toml::from_str(&text) {
        Ok(v) => v,
        Err(e) => panic!("deserialize: {e}\n{text}"),
    };
    assert_eq!(back, list);
    assert_eq!(back.get(a).map(|i| i.notified), Some(true));
    assert_eq!(back.get(b).map(|i| i.done), Some(true));
    // Ids keep not being recycled across a save/load cycle.
    let d = back.add("after reload", None, T0 + 3);
    assert_ne!(d, c, "a retired id must not reappear after reload");
}

#[test]
fn stored_data_missing_newer_fields_still_loads() {
    // An older on-disk payload: no `notified`, no `created_at`, no `next_id`.
    let text = r#"
        [[items]]
        id = 0
        text = "old item"
        done = false
        due = 1704067260

        [[items]]
        id = 1
        text = "another"
    "#;
    let list: TodoList = match toml::from_str(text) {
        Ok(v) => v,
        Err(e) => panic!("legacy payload must load: {e}"),
    };
    assert_eq!(list.len(), 2);
    let first = match list.get(TodoId(0)) {
        Some(i) => i,
        None => panic!("item 0 missing"),
    };
    assert_eq!(first.text, "old item");
    assert!(!first.notified, "missing notified defaults to false");
    assert_eq!(first.created_at, 0, "missing created_at defaults to 0");
    assert_eq!(first.due, Some(T0 + 60));
    let second = match list.get(TodoId(1)) {
        Some(i) => i,
        None => panic!("item 1 missing"),
    };
    assert_eq!(second.due, None, "missing due defaults to None");
    assert!(!second.done);
}

#[test]
fn unknown_fields_in_stored_data_are_ignored() {
    let text = r#"
        next_id = 8
        schema_version = 99

        [[items]]
        id = 7
        text = "from the future"
        priority = "high"
    "#;
    let mut list: TodoList = match toml::from_str(text) {
        Ok(v) => v,
        Err(e) => panic!("forward-compatible load failed: {e}"),
    };
    assert_eq!(list.len(), 1);
    let id = list.add("new", None, T0);
    assert_eq!(id, TodoId(8), "next_id was honoured from the payload");
}

#[test]
fn ids_keep_climbing_across_many_adds_and_removes() {
    let mut list = TodoList::new();
    let mut seen = std::collections::HashSet::new();
    for i in 0..500u64 {
        let id = list.add(format!("item {i}"), None, T0 + i);
        assert!(seen.insert(id), "id {id:?} was handed out twice");
        if i % 2 == 0 {
            assert!(list.remove(id));
        }
    }
    assert_eq!(seen.len(), 500);
    assert_eq!(list.len(), 250);
}
