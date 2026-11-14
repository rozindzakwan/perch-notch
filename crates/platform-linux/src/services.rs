//! Small Linux services: freedesktop notifications, `xdg-open`, idle time
//! (XScreenSaver) and the local UTC offset (TZif).

use std::path::Path;
use std::time::Duration;

use perch_core::error::{Error, Result};
use perch_core::platform::{IdleSource, Notifier, Opener};

/// `org.freedesktop.Notifications` over the session bus (Cinnamon, GNOME,
/// KDE, dunst, mako all implement it). Connects lazily on first use.
#[derive(Default)]
pub struct DbusNotifier {
    conn: Option<zbus::blocking::Connection>,
}

impl std::fmt::Debug for DbusNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbusNotifier")
            .field("connected", &self.conn.is_some())
            .finish()
    }
}

impl Notifier for DbusNotifier {
    fn notify(&mut self, summary: &str, body: &str) -> Result<()> {
        if self.conn.is_none() {
            self.conn =
                Some(zbus::blocking::Connection::session().map_err(|e| {
                    Error::unavailable("notifications", format!("session bus: {e}"))
                })?);
        }
        let Some(conn) = &self.conn else {
            return Err(Error::unavailable("notifications", "no session bus"));
        };
        let hints: std::collections::HashMap<&str, zbus::zvariant::Value<'_>> =
            std::collections::HashMap::new();
        let actions: Vec<&str> = Vec::new();
        conn.call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &("Perch", 0u32, "perch", summary, body, actions, hints, -1i32),
        )
        .map_err(|e| Error::unavailable("notifications", e.to_string()))?;
        Ok(())
    }
}

/// `xdg-open`, detached; the child is reaped on a short-lived thread so no
/// zombie is left behind.
#[derive(Debug, Default)]
pub struct XdgOpener;

impl Opener for XdgOpener {
    fn open(&self, path: &Path) -> Result<()> {
        let mut child = std::process::Command::new("xdg-open")
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| Error::io("running xdg-open", e))?;
        let _ = std::thread::Builder::new()
            .name("perch-reap".into())
            .spawn(move || {
                let _ = child.wait();
            });
        Ok(())
    }
}

/// Idle time from the MIT-SCREEN-SAVER extension (X11). On Wayland the
/// extension is unavailable and the user is reported as active.
#[derive(Debug, Default)]
pub struct X11Idle;

impl IdleSource for X11Idle {
    fn idle_for(&self) -> Result<Duration> {
        use x11rb::protocol::screensaver::ConnectionExt as _;
        let (conn, screen) = crate::x11::connect()?;
        let root = x11rb::connection::Connection::setup(&conn)
            .roots
            .get(screen)
            .map(|s| s.root)
            .ok_or_else(|| Error::backend("no such X screen"))?;
        let info = conn
            .screensaver_query_info(root)
            .map_err(|e| Error::unavailable("idle", e.to_string()))?
            .reply()
            .map_err(|e| Error::unavailable("idle", e.to_string()))?;
        Ok(Duration::from_millis(u64::from(info.ms_since_user_input)))
    }
}

/// Local UTC offset in seconds (local = UTC + offset), from `$TZ` or
/// `/etc/localtime`. Falls back to UTC.
pub fn utc_offset_seconds() -> i32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let path = match std::env::var("TZ") {
        Ok(tz) if !tz.is_empty() => {
            let tz = tz.trim_start_matches(':');
            if tz.starts_with('/') {
                tz.to_string()
            } else {
                format!("/usr/share/zoneinfo/{tz}")
            }
        }
        _ => "/etc/localtime".to_string(),
    };
    std::fs::read(&path)
        .ok()
        .and_then(|b| perch_core::tz::offset_at(&b, now))
        .unwrap_or(0)
}

/// Press Ctrl+V in the focused window via XTEST (used after dictation).
pub fn send_paste() -> Result<()> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
    use x11rb::protocol::xtest::ConnectionExt as _;
    let (conn, screen) = crate::x11::connect()?;
    let root = conn
        .setup()
        .roots
        .get(screen)
        .map(|s| s.root)
        .ok_or_else(|| Error::backend("no such X screen"))?;
    let ctrl = crate::x11_watch::keycode_for(&conn, 0xffe3)
        .ok_or_else(|| Error::backend("no Control key"))?;
    let v = crate::x11_watch::keycode_for(&conn, u32::from(b'v'))
        .ok_or_else(|| Error::backend("no V key"))?;
    for (kind, code) in [
        (KEY_PRESS_EVENT, ctrl),
        (KEY_PRESS_EVENT, v),
        (KEY_RELEASE_EVENT, v),
        (KEY_RELEASE_EVENT, ctrl),
    ] {
        conn.xtest_fake_input(kind, code, x11rb::CURRENT_TIME, root, 0, 0, 0)
            .map_err(|e| Error::unavailable("paste", format!("XTEST: {e}")))?;
    }
    conn.flush()
        .map_err(|e| Error::backend(format!("flush: {e}")))?;
    Ok(())
}
