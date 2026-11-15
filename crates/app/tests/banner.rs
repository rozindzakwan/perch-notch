//! The in-notch banner queue.

use std::time::{Duration, Instant};

use perch_app::banner::BannerQueue;
use perch_core::view::{BannerView, RowLeading, Tab};

fn banner(title: &str, tab: Option<Tab>) -> BannerView {
    BannerView {
        leading: RowLeading::None,
        title: title.into(),
        body: String::new(),
        tab,
        app: None,
    }
}

#[test]
fn queue_shows_each_for_four_seconds_and_pauses_on_hover() {
    let t0 = Instant::now();
    let mut q = BannerQueue::default();
    q.push(banner("one", None), t0);
    q.push(banner("two", None), t0);
    assert_eq!(q.current().map(|b| b.title.as_str()), Some("one"));
    assert_eq!(q.next_deadline(t0), Some(Duration::from_secs(4)));
    // Hover at 3 s for 5 s: the first banner stays.
    q.hover(true, t0 + Duration::from_secs(3));
    assert!(!q.tick(t0 + Duration::from_secs(8)), "paused while hovered");
    assert_eq!(q.next_deadline(t0 + Duration::from_secs(8)), None);
    q.hover(false, t0 + Duration::from_secs(8));
    // One second of its four is left.
    assert!(!q.tick(t0 + Duration::from_millis(8_900)));
    assert!(
        q.tick(t0 + Duration::from_millis(9_100)),
        "expires after the remaining second"
    );
    assert_eq!(q.current().map(|b| b.title.as_str()), Some("two"));
    assert!(q.tick(t0 + Duration::from_millis(13_200)));
    assert!(q.current().is_none());
    assert_eq!(q.next_deadline(t0 + Duration::from_secs(14)), None);
}

#[test]
fn ten_mails_merge_into_one_count_banner() {
    let t0 = Instant::now();
    let mut q = BannerQueue::default();
    for i in 0..10 {
        q.push(banner(&format!("New mail from {i}"), Some(Tab::Mail)), t0);
        assert!(q.len() <= 3, "never more than three queued");
    }
    let last = q.last().unwrap_or_else(|| panic!("a banner"));
    assert_eq!(
        last.title, "8 new messages",
        "the overflow is merged into one count"
    );
}

#[test]
fn non_mail_banners_beyond_three_are_dropped() {
    let t0 = Instant::now();
    let mut q = BannerQueue::default();
    for i in 0..5 {
        q.push(banner(&format!("reminder {i}"), None), t0);
    }
    assert_eq!(q.len(), 3);
}
