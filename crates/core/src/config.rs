//! Config file handling: `%APPDATA%\perch\config.toml` on Windows,
//! `~/.config/perch/config.toml` on Linux.
//!
//! Loading never panics. Out-of-range values are clamped, unknown enum spellings
//! fall back to the default, and [`ConfigStore::load_or_default`] always returns
//! a usable config plus an optional problem description for the UI to surface.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::geometry::{NotchGeometry, NotchPosition};

/// Visual theme for the pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    /// Follow the OS light/dark preference, defaulting to dark.
    #[default]
    System,
    /// Always dark.
    Dark,
    /// Always light.
    Light,
}

/// Notch appearance and behavior. Sizes are logical pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotchConfig {
    /// Which screen anchor the pill uses.
    #[serde(deserialize_with = "de_position")]
    pub position: NotchPosition,
    /// Monitor index to live on; `None` = the primary monitor.
    pub monitor: Option<u32>,
    /// Collapsed width.
    pub collapsed_width: u32,
    /// Collapsed height.
    pub collapsed_height: u32,
    /// Expanded width.
    pub expanded_width: u32,
    /// Expanded height.
    pub expanded_height: u32,
    /// Gap from the screen edge for non-centered anchors.
    pub margin: u32,
    /// Expand/collapse animation duration in milliseconds.
    pub animation_ms: u32,
    /// Thickness of the invisible hover strip (1-8 px).
    pub hot_zone_thickness: u32,
    /// Extra hover length added to each side of the collapsed pill.
    pub hot_zone_padding: u32,
    /// Delay before collapsing after the cursor leaves, in milliseconds.
    pub collapse_delay_ms: u32,
    /// Hide the pill while a fullscreen app is in the foreground.
    pub hide_on_fullscreen: bool,
    /// Corner radius override; `None` = fully rounded (height / 2).
    pub corner_radius: Option<u32>,
}

impl Default for NotchConfig {
    fn default() -> Self {
        Self {
            position: NotchPosition::TopCenter,
            monitor: None,
            collapsed_width: 200,
            collapsed_height: 32,
            expanded_width: 380,
            expanded_height: 300,
            margin: 12,
            animation_ms: 320,
            hot_zone_thickness: 2,
            hot_zone_padding: 40,
            collapse_delay_ms: 220,
            hide_on_fullscreen: true,
            corner_radius: None,
        }
    }
}

impl NotchConfig {
    /// Clamp every field into a range that cannot produce a broken window.
    pub fn clamp(&mut self) {
        self.collapsed_width = self.collapsed_width.clamp(80, 4000);
        self.collapsed_height = self.collapsed_height.clamp(16, 400);
        self.expanded_width = self.expanded_width.clamp(self.collapsed_width, 4000);
        self.expanded_height = self.expanded_height.clamp(self.collapsed_height, 2000);
        self.margin = self.margin.min(2000);
        self.animation_ms = self.animation_ms.min(2000);
        self.hot_zone_thickness = self.hot_zone_thickness.clamp(1, 8);
        self.hot_zone_padding = self.hot_zone_padding.min(1000);
        self.collapse_delay_ms = self.collapse_delay_ms.min(5000);
        if let Some(r) = self.corner_radius {
            self.corner_radius = Some(r.min(self.expanded_height / 2));
        }
    }

    /// Project the config onto the geometry struct used by the placement math.
    pub fn geometry(&self) -> NotchGeometry {
        NotchGeometry {
            collapsed: (self.collapsed_width, self.collapsed_height),
            expanded: (self.expanded_width, self.expanded_height),
            margin: self.margin,
            hot_zone_thickness: self.hot_zone_thickness,
            hot_zone_padding: self.hot_zone_padding,
        }
    }
}

/// Which optional features are loaded at startup. Anything off here is never
/// initialised, so its background work and memory cost do not exist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FeatureFlags {
    /// Now-playing via SMTC / MPRIS.
    pub media: bool,
    /// Synced lyrics from LRCLIB (network, opt-in).
    pub lyrics: bool,
    /// To-do list with notifications.
    pub todo: bool,
    /// Pomodoro timer.
    pub pomodoro: bool,
    /// Clipboard history board.
    pub clipboard: bool,
    /// Drag-and-drop file shelf.
    pub shelf: bool,
    /// Water/stretch reminders.
    pub reminders: bool,
    /// Per-app volume mixer.
    pub volume_mixer: bool,
    /// White-noise player.
    pub white_noise: bool,
    /// Per-app screen time tracking.
    pub screen_time: bool,
    /// Offline speech-to-text (heavy, downloads a model).
    pub speech_to_text: bool,
    /// Screenshot + annotate + OCR.
    pub screenshot: bool,
    /// Camera mirror.
    pub camera_mirror: bool,
    /// Read-only mail summary (network, opt-in).
    pub mail: bool,
    /// Show other apps' notifications in the notch (opt-in: reads their text).
    pub notification_mirror: bool,
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self {
            media: true,
            lyrics: false,
            todo: true,
            pomodoro: false,
            clipboard: false,
            shelf: false,
            reminders: false,
            volume_mixer: false,
            white_noise: false,
            screen_time: false,
            speech_to_text: false,
            screenshot: false,
            camera_mirror: false,
            mail: false,
            notification_mirror: false,
        }
    }
}

/// Clipboard board settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClipboardConfig {
    /// How many entries to retain.
    pub history_limit: usize,
    /// Keep image entries as well as text.
    pub keep_images: bool,
    /// Largest image to retain, in kilobytes.
    pub max_image_kb: usize,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            history_limit: 50,
            keep_images: true,
            max_image_kb: 4096,
        }
    }
}

impl ClipboardConfig {
    fn clamp(&mut self) {
        self.history_limit = self.history_limit.clamp(1, 1000);
        self.max_image_kb = self.max_image_kb.clamp(16, 65536);
    }
}

/// Pomodoro durations in minutes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PomodoroConfig {
    /// Focus length.
    pub work_minutes: u32,
    /// Short break length.
    pub break_minutes: u32,
    /// Long break length.
    pub long_break_minutes: u32,
    /// Focus sessions before a long break.
    pub sessions_before_long_break: u32,
}

impl Default for PomodoroConfig {
    fn default() -> Self {
        Self {
            work_minutes: 25,
            break_minutes: 5,
            long_break_minutes: 15,
            sessions_before_long_break: 4,
        }
    }
}

impl PomodoroConfig {
    fn clamp(&mut self) {
        self.work_minutes = self.work_minutes.clamp(1, 240);
        self.break_minutes = self.break_minutes.clamp(1, 120);
        self.long_break_minutes = self.long_break_minutes.clamp(1, 240);
        self.sessions_before_long_break = self.sessions_before_long_break.clamp(1, 20);
    }
}

/// Reminder intervals in minutes; `0` disables one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemindersConfig {
    /// Drink-water interval.
    pub water_minutes: u32,
    /// Stretch interval, measured in *active* time.
    pub stretch_minutes: u32,
    /// Idle time after which active-time accumulation pauses.
    pub idle_threshold_seconds: u32,
}

impl Default for RemindersConfig {
    fn default() -> Self {
        Self {
            water_minutes: 60,
            stretch_minutes: 45,
            idle_threshold_seconds: 120,
        }
    }
}

impl RemindersConfig {
    fn clamp(&mut self) {
        self.water_minutes = self.water_minutes.min(1440);
        self.stretch_minutes = self.stretch_minutes.min(1440);
        self.idle_threshold_seconds = self.idle_threshold_seconds.clamp(10, 3600);
    }
}

/// Global shortcuts. Empty string = not bound. Parsed with
/// [`crate::hotkey::Accel::parse`]; an invalid one is reported and skipped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HotkeysConfig {
    /// Open/close the notch.
    pub toggle_notch: String,
    /// Start a screenshot (when the feature is on).
    pub screenshot: String,
    /// Start/stop dictation (when the feature is on).
    pub dictate: String,
}

impl Default for HotkeysConfig {
    fn default() -> Self {
        Self {
            toggle_notch: "Ctrl+Alt+N".into(),
            screenshot: "Ctrl+Alt+S".into(),
            dictate: "Ctrl+Alt+D".into(),
        }
    }
}

impl HotkeysConfig {
    /// The valid bindings, plus a message for each invalid one.
    pub fn bindings(&self) -> (Vec<crate::hotkey::Binding>, Vec<String>) {
        let mut ok = Vec::new();
        let mut bad = Vec::new();
        for (id, s) in [
            ("toggle-notch", &self.toggle_notch),
            ("shot", &self.screenshot),
            ("dictate", &self.dictate),
        ] {
            if s.trim().is_empty() {
                continue;
            }
            match crate::hotkey::Accel::parse(s) {
                Ok(accel) => ok.push(crate::hotkey::Binding {
                    id: id.to_string(),
                    accel,
                }),
                Err(e) => bad.push(format!("hotkeys.{id}: \"{s}\": {e}")),
            }
        }
        (ok, bad)
    }
}

/// How notifications are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationsConfig {
    /// Show alerts as banners in the notch itself.
    pub in_notch: bool,
    /// Also show system notifications (Windows toasts, freedesktop).
    pub system: bool,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            in_notch: true,
            system: false,
        }
    }
}

/// One mail account (read-only IMAP). The password is never stored here; it
/// lives in the OS keychain under [`MailAccount::secret_key`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MailAccount {
    /// Shown in the Mail tab ("Work", "Personal").
    pub name: String,
    /// Sign-in name, usually the address.
    pub username: String,
    /// IMAP server.
    pub host: String,
    /// IMAP port (993 for TLS, 143 for STARTTLS).
    pub port: u16,
    /// Connection security.
    pub security: crate::mail::Security,
    /// Folder to watch.
    pub folder: String,
    /// Extra PEM CA certificate to trust (self-hosted servers); empty = system roots only.
    pub ca_file: String,
}

impl Default for MailAccount {
    fn default() -> Self {
        Self {
            name: String::new(),
            username: String::new(),
            host: String::new(),
            port: 993,
            security: crate::mail::Security::Tls,
            folder: "INBOX".into(),
            ca_file: String::new(),
        }
    }
}

impl MailAccount {
    /// Keychain key for this account's password.
    pub fn secret_key(&self) -> String {
        format!(
            "mail:{}@{}",
            self.username.trim(),
            self.host.trim().to_ascii_lowercase()
        )
    }

    /// Why this account cannot be used, if it cannot.
    pub fn problem(&self) -> Option<String> {
        if self.username.trim().is_empty() {
            return Some("no username".into());
        }
        if self.host.trim().is_empty() {
            return Some("no server".into());
        }
        if self.security == crate::mail::Security::Plain && !crate::mail::is_loopback(&self.host) {
            return Some("unencrypted connections are only allowed to localhost".into());
        }
        None
    }
}

/// Read-only mail settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MailConfig {
    /// Show a notification when new mail arrives.
    pub notify: bool,
    /// Include the subject in notifications (off = sender only).
    pub notify_subject: bool,
    /// How many unread messages per account the Mail tab lists.
    pub max_items: usize,
    /// Minutes between checks for servers without IMAP IDLE (push).
    pub poll_minutes: u32,
    /// Accounts.
    pub accounts: Vec<MailAccount>,
}

impl Default for MailConfig {
    fn default() -> Self {
        Self {
            notify: true,
            notify_subject: true,
            max_items: 10,
            poll_minutes: 5,
            accounts: Vec::new(),
        }
    }
}

impl MailConfig {
    fn clamp(&mut self) {
        self.max_items = self.max_items.clamp(1, 50);
        self.poll_minutes = self.poll_minutes.clamp(1, 120);
        for a in &mut self.accounts {
            if a.folder.trim().is_empty() {
                a.folder = "INBOX".into();
            }
            if a.port == 0 {
                a.port = 993;
            }
            if a.name.trim().is_empty() {
                a.name = a.username.clone();
            }
        }
        // Names identify accounts in the UI and in updates; keep them unique.
        let mut seen: Vec<String> = Vec::new();
        for a in &mut self.accounts {
            let mut name = a.name.clone();
            let mut n = 2;
            while seen.contains(&name) {
                name = format!("{} ({n})", a.name);
                n += 1;
            }
            a.name = name.clone();
            seen.push(name);
        }
    }
}

/// The whole persisted configuration.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Pill placement and behavior.
    pub notch: NotchConfig,
    /// Which optional features to load.
    pub features: FeatureFlags,
    /// Clipboard board settings.
    pub clipboard: ClipboardConfig,
    /// Pomodoro durations.
    pub pomodoro: PomodoroConfig,
    /// Reminder intervals.
    pub reminders: RemindersConfig,
    /// Visual theme.
    pub theme: Theme,
    /// Start Perch at login.
    pub autostart: bool,
    /// Global shortcuts.
    pub hotkeys: HotkeysConfig,
    /// Read-only mail.
    pub mail: MailConfig,
    /// Where alerts appear.
    pub notifications: NotificationsConfig,
}

impl Config {
    /// Settings files store the whole config, so old files carry the old
    /// defaults verbatim. Exactly those values mean "default" and move to
    /// the Glass defaults; anything else is a deliberate choice and stays.
    pub fn migrate_legacy(&mut self) {
        if (self.notch.expanded_width, self.notch.expanded_height) == (480, 200) {
            self.notch.expanded_width = 380;
            self.notch.expanded_height = 300;
        }
        if self.notch.animation_ms == 180 {
            self.notch.animation_ms = 320;
        }
    }

    /// Clamp every sub-section into safe ranges.
    pub fn clamp(&mut self) {
        self.notch.clamp();
        self.clipboard.clamp();
        self.pomodoro.clamp();
        self.reminders.clamp();
        self.mail.clamp();
    }
}

/// Deserialize a notch position, falling back to the default for unknown
/// spellings instead of failing the whole file.
fn de_position<'de, D>(d: D) -> std::result::Result<NotchPosition, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    Ok(NotchPosition::from_str_opt(&s).unwrap_or_default())
}

/// Reads and writes the config file at a known path.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    /// Use an explicit path (used by tests and `--config`).
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Use the per-user platform config directory.
    pub fn discover() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("", "", crate::APP_ID)
            .ok_or(Error::NoDirectory { kind: "config" })?;
        Ok(Self {
            path: dirs.config_dir().join("config.toml"),
        })
    }

    /// Path of the config file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the config, clamping values. A missing file yields defaults.
    pub fn load(&self) -> Result<Config> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Config::default());
            }
            Err(source) => {
                return Err(Error::ConfigIo {
                    path: self.path.clone(),
                    source,
                })
            }
        };
        let mut cfg: Config = toml::from_str(&text).map_err(|e| Error::ConfigInvalid {
            path: self.path.clone(),
            message: e.to_string(),
        })?;
        cfg.clamp();
        Ok(cfg)
    }

    /// Load, but never fail: on any problem return defaults plus a description.
    pub fn load_or_default(&self) -> (Config, Option<String>) {
        match self.load() {
            Ok(mut cfg) => {
                cfg.migrate_legacy();
                // A valid file can still contain an unknown position spelling,
                // which `de_position` silently defaulted. Report that so the UI
                // can warn, by re-reading the raw value.
                let note = self.unknown_position_note();
                (cfg, note)
            }
            Err(e) => (Config::default(), Some(e.to_string())),
        }
    }

    fn unknown_position_note(&self) -> Option<String> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        let value: toml::Value = text.parse().ok()?;
        let raw = value.get("notch")?.get("position")?.as_str()?;
        if NotchPosition::from_str_opt(raw).is_none() {
            Some(format!(
                "unknown notch.position \"{raw}\", using \"{}\"",
                NotchPosition::default().as_str()
            ))
        } else {
            None
        }
    }

    /// Write the config, creating parent directories as needed.
    pub fn save(&self, cfg: &Config) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::ConfigIo {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let text = toml::to_string_pretty(cfg).map_err(|e| Error::ConfigInvalid {
            path: self.path.clone(),
            message: e.to_string(),
        })?;
        let body = format!(
            "# {} configuration\n# Edits are picked up live; invalid values fall back to defaults.\n\n{text}",
            crate::APP_NAME
        );
        // Write to a sibling temp file then rename, so a crash mid-write cannot
        // leave a truncated config behind.
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, body.as_bytes()).map_err(|source| Error::ConfigIo {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, &self.path).map_err(|source| Error::ConfigIo {
            path: self.path.clone(),
            source,
        })
    }
}
