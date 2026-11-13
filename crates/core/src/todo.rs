//! To-do items with optional due times, ordered deterministically and
//! persisted by the app layer.
//!
//! # Time representation
//!
//! Instants are plain `u64` seconds since the unix epoch. That keeps the type
//! `serde`-friendly with no date-time dependency, survives a round trip through
//! JSON/TOML unchanged, and is directly comparable. Nothing in this module
//! reads the system clock: every operation that needs the current time takes
//! `now: u64` from the caller, which makes the whole module deterministic under
//! test and lets the event loop schedule exact wakeups.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Stable identifier for a to-do item.
///
/// Ids are allocated from a monotonically increasing counter and are never
/// reused, so a notification or UI reference cannot be silently rebound to a
/// different item after a removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TodoId(pub u64);

/// A single to-do entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    /// Stable, never-reused identifier.
    pub id: TodoId,
    /// What the user typed.
    pub text: String,
    /// Whether it has been completed.
    #[serde(default)]
    pub done: bool,
    /// Optional due instant, unix seconds.
    #[serde(default)]
    pub due: Option<u64>,
    /// When the item was created, unix seconds.
    #[serde(default)]
    pub created_at: u64,
    /// Whether the due notification has already been delivered.
    #[serde(default)]
    pub notified: bool,
}

/// An ordered collection of [`TodoItem`]s.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TodoList {
    /// Next id to hand out; never decreases, so ids are never recycled.
    next_id: u64,
    items: Vec<TodoItem>,
}

impl TodoList {
    /// An empty list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of items, done and pending.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when there are no items at all.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Append a new item and return its id.
    ///
    /// Runs in amortised O(1): the id comes from a counter and the item is
    /// pushed, with ordering applied only by the listing methods.
    pub fn add(&mut self, text: impl Into<String>, due: Option<u64>, now: u64) -> TodoId {
        let id = TodoId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        self.items.push(TodoItem {
            id,
            text: text.into(),
            done: false,
            due,
            created_at: now,
            notified: false,
        });
        id
    }

    /// Borrow one item by id.
    pub fn get(&self, id: TodoId) -> Option<&TodoItem> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Mutable access by id, used by the editing helpers.
    fn get_mut(&mut self, id: TodoId) -> Option<&mut TodoItem> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// Flip the done flag. Returns `false` when the id is unknown.
    pub fn toggle(&mut self, id: TodoId) -> bool {
        match self.get_mut(id) {
            Some(item) => {
                item.done = !item.done;
                true
            }
            None => false,
        }
    }

    /// Force the done flag to `done`. Returns `false` when the id is unknown.
    pub fn set_done(&mut self, id: TodoId, done: bool) -> bool {
        match self.get_mut(id) {
            Some(item) => {
                item.done = done;
                true
            }
            None => false,
        }
    }

    /// Delete an item. Returns `false` when the id is unknown.
    ///
    /// The id counter is untouched, so the removed id is never handed out again.
    pub fn remove(&mut self, id: TodoId) -> bool {
        let before = self.items.len();
        self.items.retain(|i| i.id != id);
        self.items.len() != before
    }

    /// Replace an item's text. Returns `false` when the id is unknown.
    pub fn edit_text(&mut self, id: TodoId, text: impl Into<String>) -> bool {
        match self.get_mut(id) {
            Some(item) => {
                item.text = text.into();
                true
            }
            None => false,
        }
    }

    /// Set or clear the due time, re-arming the due notification.
    ///
    /// Changing the due time clears the notified flag, so moving a deadline
    /// forward or back makes the item fire again at its new time.
    pub fn set_due(&mut self, id: TodoId, due: Option<u64>) -> bool {
        match self.get_mut(id) {
            Some(item) => {
                if item.due != due {
                    item.notified = false;
                }
                item.due = due;
                true
            }
            None => false,
        }
    }

    /// Drop the due time entirely.
    pub fn clear_due(&mut self, id: TodoId) -> bool {
        self.set_due(id, None)
    }

    /// Record that the due notification for `id` has been delivered.
    pub fn mark_notified(&mut self, id: TodoId) -> bool {
        match self.get_mut(id) {
            Some(item) => {
                item.notified = true;
                true
            }
            None => false,
        }
    }

    /// Every item, in display order.
    ///
    /// Order is pending before done, then by due time with undated items last,
    /// then by `created_at`, then by id as the final tiebreaker.
    pub fn all(&self) -> Vec<&TodoItem> {
        let mut out: Vec<&TodoItem> = self.items.iter().collect();
        out.sort_by_key(|i| order_key(i));
        out
    }

    /// Items not yet done, in display order.
    pub fn pending(&self) -> Vec<&TodoItem> {
        self.all().into_iter().filter(|i| !i.done).collect()
    }

    /// Completed items, in display order.
    pub fn done(&self) -> Vec<&TodoItem> {
        self.all().into_iter().filter(|i| i.done).collect()
    }

    /// Pending items whose due time is at or before `now`.
    pub fn overdue(&self, now: u64) -> Vec<&TodoItem> {
        self.all()
            .into_iter()
            .filter(|i| !i.done && i.due.is_some_and(|d| d <= now))
            .collect()
    }

    /// Pending items due within `window` of `now`, including overdue ones.
    pub fn due_soon(&self, now: u64, window: Duration) -> Vec<&TodoItem> {
        let limit = now.saturating_add(window.as_secs());
        self.all()
            .into_iter()
            .filter(|i| !i.done && i.due.is_some_and(|d| d <= limit))
            .collect()
    }

    /// Pending items that have come due and not yet been notified.
    ///
    /// Reporting is *not* idempotent on its own: call [`TodoList::mark_notified`]
    /// (or [`TodoList::take_due`]) for each returned id so a given item is
    /// surfaced exactly once instead of on every tick.
    pub fn due_now(&self, now: u64) -> Vec<&TodoItem> {
        self.all()
            .into_iter()
            .filter(|i| !i.done && !i.notified && i.due.is_some_and(|d| d <= now))
            .collect()
    }

    /// Ids that have just come due, marking them notified in the same step.
    ///
    /// This is the call the event loop should use: it is safe to invoke on every
    /// tick and will yield each item exactly once.
    pub fn take_due(&mut self, now: u64) -> Vec<TodoId> {
        let ids: Vec<TodoId> = self.due_now(now).into_iter().map(|i| i.id).collect();
        for id in &ids {
            self.mark_notified(*id);
        }
        ids
    }

    /// How long until the earliest un-notified due item; `None` when idle.
    ///
    /// An item already past its due time yields `Duration::ZERO` rather than
    /// underflowing the `u64` subtraction.
    pub fn next_deadline(&self, now: u64) -> Option<Duration> {
        let soonest = self
            .items
            .iter()
            .filter(|i| !i.done && !i.notified)
            .filter_map(|i| i.due)
            .min()?;
        Some(Duration::from_secs(soonest.saturating_sub(now)))
    }
}

/// Sort key implementing the documented display order: pending before done,
/// then by due time with undated items last, then created-at, then id.
fn order_key(item: &TodoItem) -> (bool, u64, u64, u64) {
    (
        item.done,
        item.due.unwrap_or(u64::MAX),
        item.created_at,
        item.id.0,
    )
}
