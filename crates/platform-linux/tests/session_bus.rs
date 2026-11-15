//! Session-bus integration tests: freedesktop notifications and screen-lock
//! signals. Run under `dbus-run-session` with `--include-ignored`
//! (scripts/linux-ci.sh does this).
#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use perch_core::platform::{EventSink, Notifier, PlatformEvent};
use perch_platform_linux::screensaver::LockWatch;
use perch_platform_linux::services::DbusNotifier;
use zbus::zvariant::OwnedValue;

struct FakeDaemon {
    seen: Arc<Mutex<Vec<(String, String, String)>>>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl FakeDaemon {
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        _replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        if let Ok(mut s) = self.seen.lock() {
            s.push((app_name, summary, body));
        }
        1
    }
}

#[test]
#[ignore = "needs a D-Bus session bus (run under dbus-run-session)"]
fn notifications_reach_the_freedesktop_daemon() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let _daemon = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.name("org.freedesktop.Notifications"))
        .and_then(|b| {
            b.serve_at(
                "/org/freedesktop/Notifications",
                FakeDaemon { seen: seen.clone() },
            )
        })
        .and_then(|b| b.build())
        .unwrap_or_else(|e| panic!("fake daemon: {e}"));
    let mut n = DbusNotifier::default();
    n.notify("Time to stretch", "Stand up for a minute.")
        .unwrap_or_else(|e| panic!("notify: {e}"));
    let got = seen.lock().map(|s| s.clone()).unwrap_or_default();
    assert_eq!(
        got,
        vec![(
            "Perch".to_string(),
            "Time to stretch".to_string(),
            "Stand up for a minute.".to_string()
        )]
    );
}

#[test]
#[ignore = "needs a D-Bus session bus (run under dbus-run-session)"]
fn notifier_reports_a_missing_daemon_as_degradable() {
    // No daemon owns the name in this test: the error must be "unavailable",
    // which the app turns into an in-notch message instead of a failure.
    let mut n = DbusNotifier::default();
    if let Err(e) = n.notify("x", "y") {
        assert!(e.is_degradable(), "{e}");
    }
}

struct FakeScreensaver;

#[zbus::interface(name = "org.cinnamon.ScreenSaver")]
impl FakeScreensaver {
    #[zbus(signal)]
    async fn active_changed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        active: bool,
    ) -> zbus::Result<()>;
}

#[test]
#[ignore = "needs a D-Bus session bus (run under dbus-run-session)"]
fn cinnamon_screensaver_lock_becomes_session_locked() {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let sink: EventSink = Arc::new(move |ev| {
        if let PlatformEvent::SessionLocked(v) = ev {
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(v);
            }
        }
    });
    let _watch = LockWatch::start(sink).unwrap_or_else(|e| panic!("{e}"));
    let conn = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.name("org.cinnamon.ScreenSaver"))
        .and_then(|b| b.serve_at("/org/cinnamon/ScreenSaver", FakeScreensaver))
        .and_then(|b| b.build())
        .unwrap_or_else(|e| panic!("{e}"));
    // Give the watcher's match rule a moment to be registered with the bus.
    std::thread::sleep(Duration::from_millis(200));
    let iface = conn
        .object_server()
        .interface::<_, FakeScreensaver>("/org/cinnamon/ScreenSaver")
        .unwrap_or_else(|e| panic!("{e}"));
    let em = iface.signal_emitter().clone();
    zbus::block_on(FakeScreensaver::active_changed(&em, true)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(true));
    zbus::block_on(FakeScreensaver::active_changed(&em, false)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(false));
}

#[test]
#[ignore = "needs a D-Bus session bus (run under dbus-run-session)"]
fn other_apps_notifications_are_mirrored_as_they_are_sent() {
    use perch_core::platform::ForeignNotification;
    use perch_platform_linux::notifications::NotificationMirror;

    // The desktop's daemon (still answers every call: Perch only watches).
    let seen = Arc::new(Mutex::new(Vec::new()));
    let _daemon = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.name("org.freedesktop.Notifications"))
        .and_then(|b| {
            b.serve_at(
                "/org/freedesktop/Notifications",
                FakeDaemon { seen: seen.clone() },
            )
        })
        .and_then(|b| b.build())
        .unwrap_or_else(|e| panic!("fake daemon: {e}"));

    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let sink: EventSink = Arc::new(move |ev| {
        if let PlatformEvent::ForeignNotification(n) = ev {
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(n);
            }
        }
    });
    let mirror = NotificationMirror::start(sink).unwrap_or_else(|e| panic!("monitor: {e}"));

    // Perch's own notification is not mirrored back.
    DbusNotifier::default()
        .notify("Time to stretch", "Stand up.")
        .unwrap_or_else(|e| panic!("notify: {e}"));
    // Another app's is, as plain text.
    let app = zbus::blocking::Connection::session().unwrap_or_else(|e| panic!("{e}"));
    let reply = app.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &(
            "Thunderbird",
            0u32,
            "",
            "Ann Lee",
            "<b>See you</b> at 8 &amp; bring it",
            Vec::<String>::new(),
            HashMap::<String, zbus::zvariant::Value>::new(),
            -1i32,
        ),
    );
    assert!(reply.is_ok(), "the daemon still answers: {reply:?}");

    let got = rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|e| panic!("no mirrored notification: {e}"));
    assert_eq!(
        got,
        ForeignNotification {
            app: "Thunderbird".into(),
            title: "Ann Lee".into(),
            body: "See you at 8 & bring it".into(),
        }
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "only that one: Perch's own was skipped"
    );
    assert_eq!(
        seen.lock().map(|s| s.len()).unwrap_or(0),
        2,
        "the daemon got both"
    );

    // Dropping the mirror stops it.
    drop(mirror);
    let _ = app.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &(
            "Thunderbird",
            0u32,
            "",
            "Later",
            "",
            Vec::<String>::new(),
            HashMap::<String, zbus::zvariant::Value>::new(),
            -1i32,
        ),
    );
    assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
}
