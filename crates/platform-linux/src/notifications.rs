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
                // Icon files read once per name (apps send the same one).
                let mut icons: HashMap<String, Option<Vec<u8>>> = HashMap::new();
                for msg in zbus::blocking::MessageIterator::from(reader) {
                    let Ok(msg) = msg else { break };
                    let header = msg.header();
                    if header.member().map(|m| m.as_str()) != Some("Notify") {
                        continue;
                    }
                    let Ok((app, _, app_icon, summary, body, _, hints, _)) =
                        msg.body().deserialize::<NotifyArgs>()
                    else {
                        continue;
                    };
                    let Some(mut n) = describe(&app, &summary, &body) else {
                        continue;
                    };
                    let entry = hint_str(&hints, &["desktop-entry"]);
                    // The app's own icon first (by name or file), then the
                    // picture the notification carries (often the sender).
                    let named = [Some(app_icon.as_str()), entry.as_deref()]
                        .into_iter()
                        .flatten()
                        .map(str::trim)
                        .find(|s| !s.is_empty())
                        .map(str::to_string);
                    n.icon = named
                        .and_then(|name| {
                            icons
                                .entry(name.clone())
                                .or_insert_with(|| load_icon(&name))
                                .clone()
                        })
                        .or_else(|| image_data(&hints))
                        .or_else(|| {
                            hint_str(&hints, &["image-path", "image_path"])
                                .and_then(|p| load_icon(&p))
                        });
                    n.launch = entry.filter(|e| desktop_file(e).is_some());
                    sink(PlatformEvent::ForeignNotification(n));
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
        icon: None,
        launch: None,
    })
}

/// Largest icon file read (bytes).
const MAX_ICON: u64 = 1024 * 1024;

/// A string hint under any of these keys.
fn hint_str(hints: &HashMap<String, OwnedValue>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        let v = hints.get(*k)?;
        let s: &str = v.downcast_ref().ok()?;
        Some(s.to_string())
    })
}

/// The raw picture a notification carries (`image-data`, or the older
/// `image_data` / `icon_data`), as a PNG.
fn image_data(hints: &HashMap<String, OwnedValue>) -> Option<Vec<u8>> {
    type Raw = (i32, i32, i32, bool, i32, i32, Vec<u8>);
    let v = ["image-data", "image_data", "icon_data"]
        .iter()
        .find_map(|k| hints.get(*k))?;
    let (w, h, stride, alpha, bits, channels, data) = Raw::try_from(v.try_clone().ok()?).ok()?;
    let rgba = rgba_from_raw(w, h, stride, alpha, bits, channels, &data)?;
    encode_png(&rgba, w as u32, h as u32)
}

/// Raw `image-data` pixels (RGB or RGBA, 8 bits, rows `stride` bytes apart)
/// as tightly packed RGBA; `None` for anything malformed or huge. Pure.
pub fn rgba_from_raw(
    w: i32,
    h: i32,
    stride: i32,
    alpha: bool,
    bits: i32,
    channels: i32,
    data: &[u8],
) -> Option<Vec<u8>> {
    let (w, h, stride) = (
        usize::try_from(w).ok()?,
        usize::try_from(h).ok()?,
        usize::try_from(stride).ok()?,
    );
    let ch = if alpha { 4 } else { 3 };
    if bits != 8 || channels != ch as i32 || w == 0 || h == 0 || w > 1024 || h > 1024 {
        return None;
    }
    if stride < w * ch || data.len() < stride * (h - 1) + w * ch {
        return None;
    }
    let mut out = Vec::with_capacity(w * h * 4);
    for row in data.chunks(stride).take(h) {
        for px in row[..w * ch].chunks_exact(ch) {
            out.extend_from_slice(&px[..3]);
            out.push(if alpha { px[3] } else { 255 });
        }
    }
    Some(out)
}

fn encode_png(rgba: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let enc = image::codecs::png::PngEncoder::new(&mut out);
    image::ImageEncoder::write_image(enc, rgba, w, h, image::ExtendedColorType::Rgba8).ok()?;
    Some(out)
}

/// An icon by file path / `file://` URL or by theme name, as file bytes
/// (PNG, JPEG, ...). SVG-only icons are skipped (no SVG renderer).
fn load_icon(name: &str) -> Option<Vec<u8>> {
    let path = match name.strip_prefix("file://") {
        Some(p) => std::path::PathBuf::from(p),
        None if name.starts_with('/') => std::path::PathBuf::from(name),
        None => icon_file(name, &data_dirs())?,
    };
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_ICON {
        return None;
    }
    std::fs::read(path).ok()
}

/// The XDG data directories, user first.
fn data_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(d) => dirs.push(d.into()),
        None => {
            if let Some(home) = std::env::var_os("HOME") {
                dirs.push(std::path::Path::new(&home).join(".local/share"));
            }
        }
    }
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|d| !d.is_empty()).map(Into::into));
    dirs
}

/// Where a theme icon name lives: the `hicolor` theme (where apps install
/// their icons), nearest the banner's size first, then `pixmaps`. Only plain
/// names are looked up (no `/`, no `..`).
pub fn icon_file(name: &str, dirs: &[std::path::PathBuf]) -> Option<std::path::PathBuf> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return None;
    }
    const SIZES: [&str; 6] = ["64x64", "48x48", "96x96", "128x128", "256x256", "32x32"];
    for dir in dirs {
        for size in SIZES {
            let p = dir
                .join("icons/hicolor")
                .join(size)
                .join("apps")
                .join(format!("{name}.png"));
            if p.is_file() {
                return Some(p);
            }
        }
        let p = dir.join("pixmaps").join(format!("{name}.png"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// The installed `.desktop` file for an entry name (the `desktop-entry`
/// hint), if any.
pub fn desktop_file(entry: &str) -> Option<std::path::PathBuf> {
    let entry = entry.trim_end_matches(".desktop");
    if !perch_core::platform::launchable_id(entry) || entry.contains(['\\', ' ']) {
        return None;
    }
    data_dirs()
        .into_iter()
        .map(|d| d.join("applications").join(format!("{entry}.desktop")))
        .find(|p| p.is_file())
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
    fn raw_image_data_becomes_rgba() {
        // 2x1 RGB with a padded stride of 8.
        let rgb = [1, 2, 3, 4, 5, 6, 0, 0];
        assert_eq!(
            rgba_from_raw(2, 1, 8, false, 8, 3, &rgb),
            Some(vec![1, 2, 3, 255, 4, 5, 6, 255])
        );
        let rgba = [9, 8, 7, 6];
        assert_eq!(
            rgba_from_raw(1, 1, 4, true, 8, 4, &rgba),
            Some(rgba.to_vec())
        );
        assert_eq!(rgba_from_raw(2, 2, 8, false, 8, 3, &rgb), None, "short");
        assert_eq!(rgba_from_raw(1, 1, 4, true, 16, 4, &rgba), None, "16-bit");
        assert_eq!(rgba_from_raw(-1, 1, 4, true, 8, 4, &rgba), None);
    }

    #[test]
    fn theme_icons_are_found_by_name_and_paths_are_refused() {
        let root = std::env::temp_dir().join(format!("perch-icons-{}", std::process::id()));
        let apps = root.join("icons/hicolor/48x48/apps");
        std::fs::create_dir_all(&apps).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(apps.join("thunderbird.png"), b"png").unwrap_or_else(|e| panic!("{e}"));
        let dirs = [root.join("missing"), root.clone()];
        assert_eq!(
            icon_file("thunderbird", &dirs),
            Some(apps.join("thunderbird.png"))
        );
        assert_eq!(icon_file("nope", &dirs), None);
        assert_eq!(icon_file("../thunderbird", &dirs), None);
        assert_eq!(icon_file("a/b", &dirs), None);
        let _ = std::fs::remove_dir_all(root);
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
