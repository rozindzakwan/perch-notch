//! Tests for the clipboard history board.

use perch_core::clipboard::{
    ClipboardContent, ClipboardHistory, ClipboardSnapshot, ContentFlags, ImageFormat, PushOutcome,
    SkipReason,
};
use perch_core::config::ClipboardConfig;

fn cfg(limit: usize) -> ClipboardConfig {
    ClipboardConfig {
        history_limit: limit,
        keep_images: true,
        max_image_kb: 64,
    }
}

fn text(s: &str) -> ClipboardContent {
    ClipboardContent::text(s)
}

#[test]
fn newest_entry_is_first_and_push_reports_the_stored_id() {
    let mut h = ClipboardHistory::new(cfg(10));
    let first = match h.push(text("one"), 100) {
        PushOutcome::Stored { id, .. } => id,
        other => panic!("expected Stored, got {other:?}"),
    };
    let second = match h.push(text("two"), 200) {
        PushOutcome::Stored { id, .. } => id,
        other => panic!("expected Stored, got {other:?}"),
    };
    assert_ne!(first, second, "ids are distinct");
    assert_eq!(h.len(), 2);

    let order: Vec<&str> = h.iter().filter_map(|e| e.as_text()).collect();
    assert_eq!(order, vec!["two", "one"], "iteration is most-recent-first");

    let newest = match h.iter().next() {
        Some(e) => e,
        None => panic!("history should not be empty"),
    };
    assert_eq!(newest.id(), second);
    assert_eq!(newest.captured_at(), 200);
}

#[test]
fn an_image_entry_keeps_its_dimensions_and_format() {
    let mut h = ClipboardHistory::new(cfg(10));
    let img = ClipboardContent::image(vec![0u8; 32], 4, 2, ImageFormat::Png);
    match h.push(img, 7) {
        PushOutcome::Stored { .. } => {}
        other => panic!("expected Stored, got {other:?}"),
    }
    let e = match h.iter().next() {
        Some(e) => e,
        None => panic!("history should not be empty"),
    };
    assert!(e.as_text().is_none(), "an image is not text");
    match e.content() {
        ClipboardContent::Image {
            width,
            height,
            format,
            bytes,
        } => {
            assert_eq!((*width, *height), (4, 2));
            assert_eq!(*format, ImageFormat::Png);
            assert_eq!(bytes.len(), 32);
        }
        other => panic!("expected an image, got {other:?}"),
    }
}

#[test]
fn history_limit_evicts_the_oldest_and_is_never_exceeded() {
    // limit 1: every push replaces the single slot.
    let mut one = ClipboardHistory::new(cfg(1));
    let a = match one.push(text("a"), 1) {
        PushOutcome::Stored { id, .. } => id,
        other => panic!("expected Stored, got {other:?}"),
    };
    let evicted = match one.push(text("b"), 2) {
        PushOutcome::Stored { evicted, .. } => evicted,
        other => panic!("expected Stored, got {other:?}"),
    };
    assert_eq!(one.len(), 1, "limit 1 keeps exactly one");
    assert_eq!(evicted, vec![a], "the older entry is reported as evicted");
    assert_eq!(one.iter().next().and_then(|e| e.as_text()), Some("b"));

    // limit 3, pushing 8 items: only the newest three survive, newest first.
    let mut h = ClipboardHistory::new(cfg(3));
    for i in 0..8u64 {
        match h.push(text(&format!("item{i}")), 100 + i) {
            PushOutcome::Stored { .. } => {}
            other => panic!("push {i} should store, got {other:?}"),
        }
        assert!(h.len() <= 3, "limit exceeded after push {i}: {}", h.len());
    }
    let order: Vec<&str> = h.iter().filter_map(|e| e.as_text()).collect();
    assert_eq!(order, vec!["item7", "item6", "item5"]);
}

#[test]
fn re_copying_the_newest_entry_refreshes_it_instead_of_duplicating() {
    let mut h = ClipboardHistory::new(cfg(10));
    let id = match h.push(text("same"), 100) {
        PushOutcome::Stored { id, .. } => id,
        other => panic!("expected Stored, got {other:?}"),
    };
    match h.push(text("same"), 500) {
        PushOutcome::Refreshed { id: again } => assert_eq!(again, id, "identity is preserved"),
        other => panic!("expected Refreshed, got {other:?}"),
    }
    assert_eq!(h.len(), 1, "no duplicate was created");
    let e = match h.iter().next() {
        Some(e) => e,
        None => panic!("history should not be empty"),
    };
    assert_eq!(
        e.captured_at(),
        500,
        "timestamp is refreshed to the new copy"
    );
}

#[test]
fn re_copying_an_older_entry_promotes_it_to_most_recent() {
    let mut h = ClipboardHistory::new(cfg(10));
    let old = match h.push(text("alpha"), 100) {
        PushOutcome::Stored { id, .. } => id,
        other => panic!("expected Stored, got {other:?}"),
    };
    h.push(text("beta"), 200);
    h.push(text("gamma"), 300);

    match h.push(text("alpha"), 400) {
        PushOutcome::Refreshed { id } => assert_eq!(id, old, "the original entry is reused"),
        other => panic!("expected Refreshed, got {other:?}"),
    }
    assert_eq!(h.len(), 3, "promotion must not duplicate");
    let order: Vec<&str> = h.iter().filter_map(|e| e.as_text()).collect();
    assert_eq!(order, vec!["alpha", "gamma", "beta"], "promoted to front");
    assert_eq!(h.iter().next().map(|e| e.captured_at()), Some(400));
}

#[test]
fn images_are_dropped_when_keep_images_is_false_without_disturbing_text() {
    let mut h = ClipboardHistory::new(ClipboardConfig {
        history_limit: 2,
        keep_images: false,
        max_image_kb: 64,
    });
    h.push(text("keep me"), 10);
    h.push(text("and me"), 20);
    let before: Vec<String> = h
        .iter()
        .filter_map(|e| e.as_text().map(str::to_owned))
        .collect();
    assert_eq!(before, vec!["and me".to_owned(), "keep me".to_owned()]);

    let img = ClipboardContent::image(vec![1u8; 10], 1, 1, ImageFormat::Png);
    assert_eq!(
        h.push(img, 30),
        PushOutcome::Skipped(SkipReason::ImagesDisabled),
        "the caller learns why"
    );

    assert_eq!(h.len(), 2, "a dropped image must not evict text");
    let after: Vec<String> = h
        .iter()
        .filter_map(|e| e.as_text().map(str::to_owned))
        .collect();
    assert_eq!(after, before, "history is untouched");
}

#[test]
fn an_oversized_image_is_rejected_not_truncated() {
    let mut h = ClipboardHistory::new(ClipboardConfig {
        history_limit: 5,
        keep_images: true,
        max_image_kb: 2,
    });
    // 2 KiB exactly is allowed; one byte more is not.
    let ok = ClipboardContent::image(vec![7u8; 2 * 1024], 8, 8, ImageFormat::Bmp);
    match h.push(ok, 1) {
        PushOutcome::Stored { .. } => {}
        other => panic!("an image at the cap should store, got {other:?}"),
    }
    let big = ClipboardContent::image(vec![7u8; 2 * 1024 + 1], 9, 9, ImageFormat::Bmp);
    assert_eq!(
        h.push(big, 2),
        PushOutcome::Skipped(SkipReason::ImageTooLarge)
    );
    assert_eq!(h.len(), 1, "the oversized image was not stored");
    let stored_len = h.iter().next().map(|e| e.content().byte_len());
    assert_eq!(
        stored_len,
        Some(2 * 1024),
        "the stored image was not truncated"
    );
}

#[test]
fn empty_content_is_skipped() {
    let mut h = ClipboardHistory::new(cfg(5));
    assert_eq!(h.push(text(""), 1), PushOutcome::Skipped(SkipReason::Empty));
    let empty_img = ClipboardContent::image(Vec::new(), 0, 0, ImageFormat::Png);
    assert_eq!(
        h.push(empty_img, 2),
        PushOutcome::Skipped(SkipReason::Empty)
    );
    assert!(h.is_empty());
}

#[test]
fn plain_flags_do_not_skip_anything() {
    let h = ClipboardHistory::new(cfg(5));
    let flags = ContentFlags::default();
    assert_eq!(
        h.should_skip(&flags),
        None,
        "nothing flagged, nothing to do"
    );
    let mut h = h;
    match h.push_with_flags(text("ordinary"), 1, &flags) {
        PushOutcome::Stored { .. } => {}
        other => panic!("expected Stored, got {other:?}"),
    }
    assert_eq!(h.len(), 1);
}

#[test]
fn the_kde_password_manager_hint_secret_skips_the_entry() {
    let mut h = ClipboardHistory::new(cfg(5));
    // X11/Wayland: target `x-kde-passwordManagerHint` with value `secret`.
    let flags = ContentFlags::default().with_x11_target("x-kde-passwordManagerHint", "secret");
    assert_eq!(h.should_skip(&flags), Some(SkipReason::Excluded));
    assert_eq!(
        h.push_with_flags(text("hunter2"), 1, &flags),
        PushOutcome::Skipped(SkipReason::Excluded)
    );
    assert!(h.is_empty(), "a secret must never be retained");

    // A different value on the same target is not a secret.
    let benign = ContentFlags::default().with_x11_target("x-kde-passwordManagerHint", "no");
    assert_eq!(h.should_skip(&benign), None);
}

#[test]
fn each_windows_exclusion_format_skips_the_entry() {
    let h = ClipboardHistory::new(cfg(5));

    let excl = ContentFlags::default()
        .with_windows_format("ExcludeClipboardContentFromMonitorProcessing", 1);
    assert_eq!(
        h.should_skip(&excl),
        Some(SkipReason::Excluded),
        "ExcludeClipboardContentFromMonitorProcessing"
    );

    let no_history = ContentFlags::default().with_windows_format("CanIncludeInClipboardHistory", 0);
    assert_eq!(
        h.should_skip(&no_history),
        Some(SkipReason::Excluded),
        "CanIncludeInClipboardHistory=0"
    );

    let no_cloud = ContentFlags::default().with_windows_format("CanUploadToCloudClipboard", 0);
    assert_eq!(
        h.should_skip(&no_cloud),
        Some(SkipReason::Excluded),
        "CanUploadToCloudClipboard=0"
    );

    // The permissive spellings must NOT skip.
    let allowed = ContentFlags::default()
        .with_windows_format("CanIncludeInClipboardHistory", 1)
        .with_windows_format("CanUploadToCloudClipboard", 1);
    assert_eq!(h.should_skip(&allowed), None, "value 1 means allowed");
}

#[test]
fn the_concealed_and_password_manager_hint_booleans_skip_the_entry() {
    let h = ClipboardHistory::new(cfg(5));
    let concealed = ContentFlags {
        concealed: true,
        ..ContentFlags::default()
    };
    assert_eq!(h.should_skip(&concealed), Some(SkipReason::Excluded));
    let hinted = ContentFlags {
        password_manager_hint: true,
        ..ContentFlags::default()
    };
    assert_eq!(h.should_skip(&hinted), Some(SkipReason::Excluded));
}

#[test]
fn the_deny_list_matches_source_apps_case_insensitively() {
    let mut h = ClipboardHistory::new(cfg(5));
    h.set_deny_list(["KeePassXC.exe", "  Bitwarden  "]);

    let exact = ContentFlags::default().with_source_app("KeePassXC.exe");
    assert_eq!(h.should_skip(&exact), Some(SkipReason::DenyListed));

    let shouty = ContentFlags::default().with_source_app("KEEPASSXC.EXE");
    assert_eq!(
        h.should_skip(&shouty),
        Some(SkipReason::DenyListed),
        "matching is case-insensitive"
    );

    let padded = ContentFlags::default().with_source_app("bitwarden");
    assert_eq!(
        h.should_skip(&padded),
        Some(SkipReason::DenyListed),
        "deny-list entries are trimmed"
    );

    let other = ContentFlags::default().with_source_app("notepad.exe");
    assert_eq!(h.should_skip(&other), None);

    assert_eq!(
        h.push_with_flags(text("master password"), 1, &exact),
        PushOutcome::Skipped(SkipReason::DenyListed)
    );
    assert!(h.is_empty());
}

#[test]
fn debug_never_prints_the_clipboard_payload() {
    let mut h = ClipboardHistory::new(cfg(5));
    let secret = "correct horse battery staple";
    h.push(text(secret), 42);
    let entry = match h.iter().next() {
        Some(e) => e,
        None => panic!("history should not be empty"),
    };

    let rendered = format!("{entry:?}");
    assert!(
        !rendered.contains(secret),
        "entry Debug leaked the payload: {rendered}"
    );
    assert!(
        !rendered.contains("horse"),
        "entry Debug leaked part of the payload: {rendered}"
    );
    assert!(rendered.contains("redacted"), "redaction is visible");
    assert!(rendered.contains("42"), "metadata is still useful");

    // The content type and the whole history must be just as safe.
    let content = format!("{:?}", entry.content());
    assert!(!content.contains(secret), "content Debug leaked: {content}");
    let whole = format!("{h:?}");
    assert!(!whole.contains(secret), "history Debug leaked: {whole}");

    // Images must not dump their bytes either.
    let mut imgs = ClipboardHistory::new(cfg(5));
    imgs.push(
        ClipboardContent::image(vec![0xAB; 3], 1, 1, ImageFormat::Jpeg),
        1,
    );
    let img_dbg = format!("{imgs:?}");
    assert!(!img_dbg.contains("171"), "image bytes leaked: {img_dbg}");
    assert!(!img_dbg.contains("0xAB"), "image bytes leaked: {img_dbg}");
}

#[test]
fn remove_deletes_one_entry_and_clear_empties_the_board() {
    let mut h = ClipboardHistory::new(cfg(5));
    let a = match h.push(text("a"), 1) {
        PushOutcome::Stored { id, .. } => id,
        other => panic!("expected Stored, got {other:?}"),
    };
    let b = match h.push(text("b"), 2) {
        PushOutcome::Stored { id, .. } => id,
        other => panic!("expected Stored, got {other:?}"),
    };
    h.push(text("c"), 3);

    assert!(h.remove(b), "removing a present id reports true");
    assert_eq!(h.len(), 2);
    let order: Vec<String> = h
        .iter()
        .filter_map(|e| e.as_text().map(str::to_owned))
        .collect();
    assert_eq!(order, vec!["c".to_owned(), "a".to_owned()]);
    assert!(!h.remove(b), "removing it again reports false");
    assert!(h.remove(a));

    h.clear();
    assert!(h.is_empty(), "clear empties everything");
    assert_eq!(h.len(), 0);
    assert_eq!(h.iter().count(), 0);
}

#[test]
fn entries_round_trip_through_serde_and_tolerate_a_missing_field() {
    let mut h = ClipboardHistory::new(cfg(5));
    h.push(text("round trip"), 1700000000);
    h.push(
        ClipboardContent::image(vec![1, 2, 3], 2, 3, ImageFormat::Png),
        1700000100,
    );
    let snapshot = h.snapshot();

    let encoded = match toml::to_string(&snapshot) {
        Ok(j) => j,
        Err(e) => panic!("serialize failed: {e}"),
    };
    let back: ClipboardSnapshot = match toml::from_str(&encoded) {
        Ok(v) => v,
        Err(e) => panic!("deserialize failed: {e}\n{encoded}"),
    };
    assert_eq!(back, snapshot, "serde round-trip is lossless");

    // An older payload without `captured_at` and a newer one carrying an
    // unknown field must both load, thanks to #[serde(default)] and no
    // deny_unknown_fields.
    let legacy = r#"
        [[entries]]
        id = 7
        future_field = "ignored"
        [entries.content]
        kind = "text"
        value = "old"
    "#;
    let loaded: ClipboardSnapshot = match toml::from_str(legacy) {
        Ok(v) => v,
        Err(e) => panic!("a payload missing captured_at must load: {e}"),
    };
    assert_eq!(loaded.entries.len(), 1);
    assert_eq!(loaded.entries[0].captured_at(), 0, "missing field defaults");

    let restored = ClipboardHistory::from_snapshot(cfg(5), loaded);
    assert_eq!(restored.len(), 1);
    assert_eq!(
        restored.iter().next().and_then(|e| e.as_text()),
        Some("old")
    );
}
