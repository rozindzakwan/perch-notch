//! Shows one real Windows toast, so it is opt-in:
//! `cargo test -p perch-platform-windows --test toast -- --ignored`.
#![cfg(windows)]

use perch_core::platform::Notifier;
use perch_platform_windows::services::register_app_id;
use perch_platform_windows::ToastNotifier;

#[test]
#[ignore = "shows a real toast on the desktop"]
fn a_toast_is_accepted_by_the_notification_platform() {
    register_app_id(None).unwrap_or_else(|e| panic!("AppUserModelID: {e}"));
    let r = ToastNotifier.notify("Perch test", "If you can read this, notifications work.");
    assert!(r.is_ok(), "{r:?}");
}
