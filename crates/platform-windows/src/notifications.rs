//! Mirror other apps' notifications into the notch.
//!
//! `UserNotificationListener` lets this unpackaged app read the notification
//! list once the user allows it (Settings > Privacy > Notifications), but its
//! `NotificationChanged` event needs package identity ("Element not found"
//! without it), and the notification store sends no file-change events (it
//! is written through a memory map). So while the feature is on, a worker
//! thread checks the store's size and timestamp every [`CHECK_EVERY`]
//! (microseconds), reads the list only when they changed (tens of
//! milliseconds), and reports only notifications it has not seen
//! ([`perch_core::mirror::SeenNotifications`]). The text is handed to the app
//! for the banner and never stored or logged.

use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use perch_core::error::{Error, Result};
use perch_core::mirror::SeenNotifications;
use perch_core::platform::{EventSink, ForeignNotification, PlatformEvent};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};

/// How often the notification store is checked while mirroring is on.
pub const CHECK_EVERY: Duration = Duration::from_secs(1);

/// Our own toasts are already banners.
const OWN_APP: &str = "Perch";

/// The running mirror; dropping it stops the worker.
pub struct NotificationMirror {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl NotificationMirror {
    /// Ask for access (Windows may prompt once) and start mirroring.
    pub fn start(sink: EventSink) -> Result<Self> {
        let (stop_tx, stop_rx) = channel::<()>();
        let (ready_tx, ready_rx) = channel::<Result<()>>();
        let thread = std::thread::Builder::new()
            .name("perch-notifications".into())
            .spawn(move || {
                // SAFETY: a fresh thread joins the multithreaded apartment
                // for the WinRT calls below; it lives until the thread exits.
                let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                let listener = match listener() {
                    Ok(l) => {
                        let _ = ready_tx.send(Ok(()));
                        l
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let mut seen = SeenNotifications::default();
                let store = store_file();
                let mut last_stamp = None;
                loop {
                    // Reading the list costs tens of milliseconds; the store's
                    // size and timestamp cost microseconds and change with
                    // every new toast, so only read the list when they moved.
                    let stamp = store.as_deref().and_then(stamp_of);
                    let changed = stamp.is_none() || stamp != last_stamp;
                    last_stamp = stamp;
                    if !changed {
                        match stop_rx.recv_timeout(CHECK_EVERY) {
                            Err(RecvTimeoutError::Timeout) => continue,
                            _ => break,
                        }
                    }
                    match snapshot(&listener) {
                        Ok(list) => {
                            let ids: Vec<u32> = list.iter().map(|(id, _)| *id).collect();
                            // Only new notifications are read in full (app, text):
                            // that is the expensive part.
                            for id in seen.update(&ids) {
                                let Some((_, n)) = list.iter().find(|(i, _)| *i == id) else {
                                    continue;
                                };
                                if let Some(f) = describe(n) {
                                    if f.app != OWN_APP {
                                        sink(PlatformEvent::ForeignNotification(f));
                                    }
                                }
                            }
                        }
                        Err(e) => log::debug!("reading notifications: {e}"),
                    }
                    match stop_rx.recv_timeout(CHECK_EVERY) {
                        Err(RecvTimeoutError::Timeout) => continue,
                        _ => break,
                    }
                }
            })
            .map_err(|e| Error::backend(format!("notification thread: {e}")))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                stop: Some(stop_tx),
                thread: Some(thread),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(Error::unavailable(
                "notifications",
                "no answer to the notification access request",
            )),
        }
    }
}

impl Drop for NotificationMirror {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The notification store's write-ahead log, which Windows appends to for
/// every new toast (directory-change events do not fire for it: it is
/// written through a memory map).
fn store_file() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    let p = std::path::Path::new(&base)
        .join("Microsoft")
        .join("Windows")
        .join("Notifications")
        .join("wpndatabase.db-wal");
    p.exists().then_some(p)
}

/// Size and modification time of a file.
fn stamp_of(p: &std::path::Path) -> Option<(u64, std::time::SystemTime)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.len(), m.modified().ok()?))
}

/// The listener, once the user has allowed access.
fn listener() -> Result<UserNotificationListener> {
    let unavailable = |m: String| Error::unavailable("notifications", m);
    let l = UserNotificationListener::Current().map_err(|e| unavailable(format!("{e}")))?;
    let mut status = l
        .GetAccessStatus()
        .map_err(|e| unavailable(format!("{e}")))?;
    if status != UserNotificationListenerAccessStatus::Allowed {
        status = l
            .RequestAccessAsync()
            .and_then(|op| op.get())
            .map_err(|e| unavailable(format!("{e}")))?;
    }
    if status == UserNotificationListenerAccessStatus::Allowed {
        Ok(l)
    } else {
        Err(unavailable(
            "access denied; allow it in Settings > Privacy & security > Notifications".into(),
        ))
    }
}

/// The current toasts, by id (not yet read in full).
fn snapshot(l: &UserNotificationListener) -> windows::core::Result<Vec<(u32, UserNotification)>> {
    let list = l.GetNotificationsAsync(NotificationKinds::Toast)?.get()?;
    Ok(list
        .into_iter()
        .filter_map(|n| n.Id().ok().map(|id| (id, n)))
        .collect())
}

fn describe(n: &UserNotification) -> Option<ForeignNotification> {
    let app = n
        .AppInfo()
        .and_then(|a| a.DisplayInfo())
        .and_then(|d| d.DisplayName())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let texts: Vec<String> = n
        .Notification()
        .and_then(|t| t.Visual())
        .and_then(|v| v.GetBinding(&KnownNotificationBindings::ToastGeneric()?))
        .and_then(|b| b.GetTextElements())
        .map(|els| {
            els.into_iter()
                .filter_map(|e| e.Text().ok())
                .map(|s| s.to_string())
                .filter(|s| !s.trim().is_empty())
                .collect()
        })
        .unwrap_or_default();
    let (title, body) = split_text(&app, &texts)?;
    Some(ForeignNotification { app, title, body })
}

/// First line as the title, the rest as the body; an app that sends only one
/// line gets its own name as the title. `None` when there is no text at all.
pub fn split_text(app: &str, texts: &[String]) -> Option<(String, String)> {
    match texts {
        [] => None,
        [only] => Some((app.to_string(), only.clone())),
        [first, rest @ ..] => Some((first.clone(), rest.join(" "))),
    }
}

#[cfg(test)]
mod tests {
    use super::split_text;

    #[test]
    fn text_lines_become_title_and_body() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(split_text("WhatsApp", &s(&[])), None);
        assert_eq!(
            split_text("WhatsApp", &s(&["Ann", "See you at 8", "and bring it"])),
            Some(("Ann".into(), "See you at 8 and bring it".into()))
        );
        assert_eq!(
            split_text("Teams", &s(&["Meeting in 5 minutes"])),
            Some(("Teams".into(), "Meeting in 5 minutes".into()))
        );
    }
}
