//! Screen lock / screensaver activity as [`PlatformEvent::SessionLocked`].
//!
//! Listens for the `ActiveChanged(bool)` signal that Cinnamon
//! (`org.cinnamon.ScreenSaver`), GNOME, MATE and freedesktop screensavers emit
//! on the session bus. One thread blocks on the signal iterator; nothing polls.
//! Used so reminders and screen time treat a locked screen as idle.

use perch_core::error::{Error, Result};
use perch_core::platform::{EventSink, PlatformEvent};

const INTERFACES: &[&str] = &[
    "org.cinnamon.ScreenSaver",
    "org.gnome.ScreenSaver",
    "org.mate.ScreenSaver",
    "org.freedesktop.ScreenSaver",
];

/// Running lock watcher. The thread lives as long as the bus connection; drop
/// the handle to stop reporting.
pub struct LockWatch {
    _conn: zbus::blocking::Connection,
}

impl LockWatch {
    /// Subscribe and start the thread.
    pub fn start(sink: EventSink) -> Result<Self> {
        let conn = zbus::blocking::Connection::session()
            .map_err(|e| Error::unavailable("screen lock", format!("session bus: {e}")))?;
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .member("ActiveChanged")
            .map(|b| b.build())
            .map_err(|e| Error::backend(format!("match rule: {e}")))?;
        let iter = zbus::blocking::MessageIterator::for_match_rule(rule, &conn, Some(16))
            .map_err(|e| Error::backend(format!("subscribe ActiveChanged: {e}")))?;
        std::thread::Builder::new()
            .name("perch-lock-watch".into())
            .spawn(move || {
                for msg in iter {
                    let Ok(msg) = msg else { continue };
                    let header = msg.header();
                    let known = header
                        .interface()
                        .is_some_and(|i| INTERFACES.contains(&i.as_str()));
                    if !known {
                        continue;
                    }
                    if let Ok(active) = msg.body().deserialize::<bool>() {
                        sink(PlatformEvent::SessionLocked(active));
                    }
                }
            })
            .map_err(|e| Error::io("spawning the lock watcher", e))?;
        Ok(Self { _conn: conn })
    }
}
