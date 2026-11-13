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
            expanded_width: 440,
            expanded_height: 132,
            margin: 12,
            animation_ms: 180,
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
}

impl Config {
    /// Clamp every sub-section into safe ranges.
    pub fn clamp(&mut self) {
        self.notch.clamp();
        self.clipboard.clamp();
        self.pomodoro.clamp();
        self.reminders.clamp();
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
            Ok(cfg) => {
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
