//! Mirror other apps' notifications into the notch (Linux).
//!
//! Every app sends notifications to the desktop's daemon as an
//! `org.freedesktop.Notifications.Notify` call on the session bus. A separate
//! connection becomes a D-Bus *monitor* for exactly those calls
//! (`BecomeMonitor`), so each one arrives as an event: no timer, no polling.
//! The daemon still shows its own popup; Perch only reads a copy. The text is
//! handed to the app for the banner and never stored or logged.

use std::collections::HashMap;
use std::thread::JoinHandle;

use perch_core::error::{Error, Result};
use perch_core::platform::{EventSink, ForeignNotification, PlatformEvent};
use zbus::message::Type;
use zbus::zvariant::OwnedValue;
use zbus::MatchRule;

/// Our own notifications are already banners.
const OWN_APP: &str = "Perch";

type NotifyArgs = (
    String,
    u32,
    String,
    String,
    String,
    Vec<String>,
    HashMap<String, OwnedValue>,
    i32,
);

/// The running monitor; dropping it closes the connection and stops it.
pub struct NotificationMirror {
    conn: zbus::blocking::Connection,
    _thread: JoinHandle<()>,
}

impl NotificationMirror {
    /// Start watching `Notify` calls on the session bus.
    pub fn start(sink: EventSink) -> Result<Self> {
        let unavailable = |m: String| Error::unavailable("notifications", m);
        let conn = zbus::blocking::Connection::session()
            .map_err(|e| unavailable(format!("no session bus: {e}")))?;
        let rule = MatchRule::builder()
            .msg_type(Type::MethodCall)
            .interface("org.freedesktop.Notifications")
            .and_then(|b| b.member("Notify"))
            .map_err(|e| Error::backend(format!("match rule: {e}")))?
            .build();
        zbus::blocking::fdo::MonitoringProxy::new(&conn)
            .and_then(|p| p.become_monitor(&[rule], 0).map_err(Into::into))
            .map_err(|e| unavailable(format!("cannot monitor notifications: {e}")))?;
        let reader = conn.clone();
        let thread = std::thread::Builder::new()
            .name("perch-notifications".into())
            .spawn(move || {
                for msg in zbus::blocking::MessageIterator::from(reader) {
                    let Ok(msg) = msg else { break };
                    let header = msg.header();
                    if header.member().map(|m| m.as_str()) != Some("Notify") {
                        continue;
                    }
                    let Ok((app, _, _, summary, body, ..)) = msg.body().deserialize::<NotifyArgs>()
                    else {
                        continue;
                    };
                    if let Some(n) = describe(&app, &summary, &body) {
                        sink(PlatformEvent::ForeignNotification(n));
                    }
                }
            })
            .map_err(|e| Error::backend(format!("notification thread: {e}")))?;
        Ok(Self {
            conn,
            _thread: thread,
        })
    }
}

impl Drop for NotificationMirror {
    fn drop(&mut self) {
        // Closing ends the reader's iterator (all clones share the socket).
        let _ = self.conn.clone().close();
    }
}

/// A `Notify` call as a banner, or `None` for our own and for empty ones.
pub fn describe(app: &str, summary: &str, body: &str) -> Option<ForeignNotification> {
    let app = app.trim();
    if app == OWN_APP {
        return None;
    }
    let summary = strip_markup(summary);
    let body = strip_markup(body);
    let app = if app.is_empty() { "Notification" } else { app };
    let (title, body) = match (summary.is_empty(), body.is_empty()) {
        (true, true) => return None,
        (true, false) => (app.to_string(), body),
        (false, _) => (summary, body),
    };
    Some(ForeignNotification {
        app: app.to_string(),
        title,
        body,
    })
}

/// Notification bodies may carry a little markup (`<b>`, `<i>`, `<a href>`)
/// and entities; the banner wants plain text on one line. Pure.
pub fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_and_entities_become_plain_text() {
        assert_eq!(
            strip_markup("<b>See</b> you at <a href=\"x\">8</a> &amp; bring it\nplease"),
            "See you at 8 & bring it please"
        );
        assert_eq!(strip_markup("1 &lt; 2"), "1 < 2");
    }

    #[test]
    fn own_and_empty_notifications_are_skipped() {
        assert_eq!(describe("Perch", "Time to stretch", ""), None);
        assert_eq!(describe("Slack", "", ""), None);
        let n = describe("Thunderbird", "", "2 new messages").unwrap_or_else(|| panic!());
        assert_eq!(
            (n.title.as_str(), n.body.as_str()),
            ("Thunderbird", "2 new messages")
        );
        let n = describe("", "Ann", "hi").unwrap_or_else(|| panic!());
        assert_eq!(n.app, "Notification");
    }
}
