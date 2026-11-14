//! The in-notch notification queue: each banner shows for four seconds
//! (paused while the pointer is over it), at most three wait their turn, and
//! further mail merges into one "N new messages" banner.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use perch_core::view::{BannerView, RowKind, RowLeading, Tab};

/// How long a banner stays.
pub const SHOW_FOR: Duration = Duration::from_secs(4);
/// At most this many banners wait (including the one showing).
pub const MAX_QUEUED: usize = 3;

#[derive(Debug, Clone)]
struct Entry {
    view: BannerView,
    /// Mail messages this banner stands for (merged banners count several).
    mail: u32,
}

/// Banners waiting to be shown, front = showing now.
#[derive(Debug, Clone, Default)]
pub struct BannerQueue {
    queue: VecDeque<Entry>,
    /// When the front banner started showing (moved forward by pauses).
    shown_at: Option<Instant>,
    /// When the current hover pause began.
    paused_at: Option<Instant>,
}

impl BannerQueue {
    /// Queue a banner.
    pub fn push(&mut self, view: BannerView, now: Instant) {
        let is_mail = view.tab == Some(Tab::Mail);
        if self.queue.len() >= MAX_QUEUED {
            if is_mail {
                if let Some(e) = self.queue.iter_mut().rev().find(|e| e.mail > 0) {
                    e.mail += 1;
                    e.view.title = format!("{} new messages", e.mail);
                    e.view.body = String::new();
                    e.view.leading = RowLeading::Kind(RowKind::Mail);
                }
            }
            return;
        }
        self.queue.push_back(Entry {
            view,
            mail: u32::from(is_mail),
        });
        if self.queue.len() == 1 {
            self.shown_at = Some(now);
            self.paused_at = None;
        }
    }

    /// The banner showing now.
    pub fn current(&self) -> Option<&BannerView> {
        self.queue.front().map(|e| &e.view)
    }

    /// The last queued banner.
    pub fn last(&self) -> Option<&BannerView> {
        self.queue.back().map(|e| &e.view)
    }

    /// How many are queued (including the one showing).
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Nothing queued.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Dismiss the banner showing now (clicked); the next one starts.
    pub fn dismiss(&mut self, now: Instant) -> Option<BannerView> {
        let e = self.queue.pop_front()?;
        self.shown_at = (!self.queue.is_empty()).then_some(now);
        self.paused_at = None;
        Some(e.view)
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        self.queue.clear();
        self.shown_at = None;
        self.paused_at = None;
    }

    /// The pointer moved onto (or off) the banner: pause (or resume) it.
    pub fn hover(&mut self, inside: bool, now: Instant) {
        match (inside, self.paused_at) {
            (true, None) if self.current().is_some() => self.paused_at = Some(now),
            (false, Some(p)) => {
                if let Some(s) = self.shown_at.as_mut() {
                    *s += now.saturating_duration_since(p);
                }
                self.paused_at = None;
            }
            _ => {}
        }
    }

    /// Expire the showing banner when its time is up. True = what is shown
    /// changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        if self.paused_at.is_some() {
            return false;
        }
        match self.shown_at {
            Some(s) if now.saturating_duration_since(s) >= SHOW_FOR => {
                self.dismiss(now);
                true
            }
            _ => false,
        }
    }

    /// Time until the showing banner expires (`None` when nothing shows or
    /// it is paused).
    pub fn next_deadline(&self, now: Instant) -> Option<Duration> {
        if self.paused_at.is_some() || self.queue.is_empty() {
            return None;
        }
        self.shown_at
            .map(|s| SHOW_FOR.saturating_sub(now.saturating_duration_since(s)))
    }
}
