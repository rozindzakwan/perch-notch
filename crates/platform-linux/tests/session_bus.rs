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
