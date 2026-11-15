//! Every OS capability Perch needs, as a trait.
//!
//! Real implementations live in `perch-platform-windows` and
//! `perch-platform-linux`; [`fake`] holds the in-memory implementation that the
//! integration tests drive, so all logic above this layer is testable without a
//! window system, a media player, or a D-Bus session.

use std::time::Duration;

use crate::error::{Error, Result};
use crate::geometry::{Monitor, Rect};

pub mod fake;

/// A now-playing track as reported by SMTC / MPRIS.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Track {
    /// Track title.
    pub title: String,
    /// Primary artist.
    pub artist: String,
    /// Album name, empty when unknown.
    pub album: String,
    /// Total length, when the player reports one.
    pub duration: Option<Duration>,
    /// Playback position at the time of the report.
    pub position: Option<Duration>,
    /// Whether the player is currently playing.
    pub playing: bool,
    /// Cover art as encoded image bytes (PNG/JPEG), when available.
    pub cover: Option<Vec<u8>>,
}

impl Track {
    /// `"Artist - Title"`, or just the title when the artist is unknown.
    pub fn label(&self) -> String {
        if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} - {}", self.artist, self.title)
        }
    }
}

/// Transport actions Perch can ask a player to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportCommand {
    /// Resume playback.
    Play,
    /// Pause playback.
    Pause,
    /// Toggle play/pause.
    PlayPause,
    /// Skip to the next track.
    Next,
    /// Go to the previous track.
    Previous,
    /// Seek to an absolute position.
    SeekTo(Duration),
}

/// Media-backend notifications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaEvent {
    /// A different track (or different metadata) is now current.
    TrackChanged(Track),
    /// Play/pause state changed for the current track.
    PlaybackChanged(bool),
    /// Playback position jumped (seek, or a periodic report while expanded).
    PositionChanged(Duration),
    /// No player is available any more.
    Gone,
}

/// A notification shown by another app. Held in memory only for the banner;
/// never stored or logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignNotification {
    /// The app that sent it ("WhatsApp").
    pub app: String,
    /// First line (usually the sender or subject).
    pub title: String,
    /// The rest of the text.
    pub body: String,
}

/// Anything the platform layer can wake the app up for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformEvent {
    /// The cursor entered the invisible hover strip.
    HotZoneEntered,
    /// The cursor left the hover strip.
    HotZoneLeft,
    /// The foreground window's fullscreen state changed.
    FullscreenChanged(bool),
    /// Media backend news.
    Media(MediaEvent),
    /// A CLI command arrived over single-instance IPC.
    Command(String),
    /// The foreground application changed (screen-time tracking).
    ActiveWindowChanged {
        /// Executable / application id.
        app: String,
        /// Window title.
        title: String,
    },
    /// The user went idle or became active again (reminders).
    IdleChanged(bool),
    /// The monitor layout changed; placement must be recomputed.
    MonitorsChanged,
    /// The clipboard received new content. The app re-reads it through
    /// [`ClipboardAccess`]; the event itself carries nothing so it stays cheap.
    ClipboardChanged,
    /// The config file was written (by the user, an editor, or the settings
    /// window); the app reloads it.
    ConfigFileChanged,
    /// The set of audio sessions or one of their volumes changed; the app
    /// re-reads them through [`AudioMixer`].
    AudioSessionsChanged,
    /// Another app showed a notification (mirrored into the notch).
    ForeignNotification(ForeignNotification),
    /// A registered global hotkey went down (`pressed = true`) or up.
    Hotkey {
        /// The id the hotkey was registered under (e.g. `"dictate"`).
        id: String,
        /// Press or release.
        pressed: bool,
    },
    /// The session was locked or unlocked (counts as idle for reminders).
    SessionLocked(bool),
    /// A file was dropped onto the notch.
    FileDropped(std::path::PathBuf),
    /// The window manager started or was replaced (e.g. "Restart Cinnamon");
    /// window-manager-owned state such as keep-above must be requested again.
    WindowManagerChanged,
}

/// Where backends deliver [`PlatformEvent`]s. The app wraps winit's
/// `EventLoopProxy` in one of these, so an event from any backend thread wakes
/// the (otherwise fully idle) event loop immediately.
pub type EventSink = std::sync::Arc<dyn Fn(PlatformEvent) + Send + Sync>;

/// One application's audio session, as the mixer shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioSession {
    /// Backend-stable id (WASAPI session instance id / Pulse sink-input index).
    pub id: String,
    /// Display name (process name or `application.name`).
    pub name: String,
    /// Volume 0.0 ..= 1.0.
    pub volume: f32,
    /// Muted.
    pub muted: bool,
}

/// Id of the mixer row for the output device's main volume (what the
/// taskbar / panel speaker shows). Backends list it first in
/// [`AudioMixer::sessions`]; `set_volume` / `set_mute` with this id change the
/// device itself.
pub const SYSTEM_VOLUME_ID: &str = "@system";

/// Per-application volume control.
pub trait AudioMixer {
    /// Current sessions, loudest/most relevant first is not required.
    fn sessions(&mut self) -> Result<Vec<AudioSession>>;
    /// Set one session's volume (clamped to 0..=1 by the caller).
    fn set_volume(&mut self, id: &str, volume: f32) -> Result<()>;
    /// Mute or unmute one session.
    fn set_mute(&mut self, id: &str, muted: bool) -> Result<()>;
}

/// The OS keychain (Windows Credential Manager, Linux Secret Service). Used
/// for mail passwords so they never sit in the config file.
pub trait SecretStore: Send + Sync {
    /// The secret stored under `key`, if any.
    fn get(&self, key: &str) -> Result<Option<String>>;
    /// Store (or replace) a secret. `label` is what keychain UIs show.
    fn set(&self, key: &str, label: &str, secret: &str) -> Result<()>;
    /// Remove a secret (missing is fine).
    fn delete(&self, key: &str) -> Result<()>;
}

/// A [`SecretStore`] that keeps secrets in memory only (tests, or when no
/// keychain is available: the password then lasts until Perch exits).
#[derive(Debug, Default)]
pub struct MemorySecrets(std::sync::Mutex<std::collections::HashMap<String, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.0.lock().ok().and_then(|m| m.get(key).cloned()))
    }
    fn set(&self, key: &str, _label: &str, secret: &str) -> Result<()> {
        if let Ok(mut m) = self.0.lock() {
            m.insert(key.to_string(), secret.to_string());
        }
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        if let Ok(mut m) = self.0.lock() {
            m.remove(key);
        }
        Ok(())
    }
}

/// Reading and writing the system clipboard.
pub trait ClipboardAccess {
    /// Read the current clipboard content plus the flags (password-manager
    /// hints, source app) used to decide whether it may be recorded. `None`
    /// when the clipboard holds nothing Perch understands.
    fn read(
        &mut self,
    ) -> Result<
        Option<(
            crate::clipboard::ClipboardContent,
            crate::clipboard::ContentFlags,
        )>,
    >;
    /// Put content on the clipboard.
    fn write(&mut self, content: &crate::clipboard::ClipboardContent) -> Result<()>;
    /// Put a list of files on the clipboard (so they can be pasted in a file
    /// manager).
    fn write_files(&mut self, paths: &[std::path::PathBuf]) -> Result<()>;
}

/// How long the user has been idle. Queried once per reminder deadline (never
/// on a timer loop), so it may be a cheap synchronous call.
pub trait IdleSource {
    /// Time since the last keyboard/mouse input.
    fn idle_for(&self) -> Result<Duration>;
}

/// Opening files and folders with the desktop's default handler.
pub trait Opener {
    /// Open a file or folder.
    fn open(&self, path: &std::path::Path) -> Result<()>;
}

/// Enumerates monitors and picks the one the notch should live on.
pub trait MonitorSource {
    /// All currently connected monitors.
    fn monitors(&self) -> Result<Vec<Monitor>>;

    /// The monitor for a configured index, falling back to the primary monitor
    /// (then the first) when the index is stale. Never fails because a monitor
    /// was unplugged.
    fn monitor_for(&self, index: Option<u32>) -> Result<Monitor> {
        let monitors = self.monitors()?;
        if monitors.is_empty() {
            return Err(Error::unavailable("monitors", "no monitors reported"));
        }
        let chosen = index
            .and_then(|i| monitors.iter().find(|m| m.id == i))
            .or_else(|| monitors.iter().find(|m| m.primary))
            .or_else(|| monitors.first());
        match chosen {
            Some(m) => Ok(*m),
            None => Err(Error::unavailable("monitors", "no monitors reported")),
        }
    }
}

/// An event-driven hover strip. Implementations create a 1-2 px window and
/// report enter/leave; polling the cursor on a timer is forbidden.
pub trait HotZoneWatch {
    /// Place (or move) the hover strip.
    fn set_hot_zone(&mut self, rect: Rect) -> Result<()>;

    /// Number of times the implementation polled the cursor position. Real
    /// backends always return 0; this exists so tests can assert it.
    fn cursor_polls(&self) -> u64 {
        0
    }
}

/// Fullscreen-app detection, so the notch can get out of the way.
pub trait FullscreenWatch {
    /// Whether a fullscreen app currently owns the foreground.
    fn is_fullscreen_active(&self) -> bool;
}

/// Now-playing metadata and transport control.
pub trait MediaControl {
    /// The current track, if a player is present.
    fn now_playing(&self) -> Option<Track>;

    /// Ask the active player to do something.
    fn send(&mut self, cmd: TransportCommand) -> Result<()>;
}

/// Desktop notifications.
pub trait Notifier {
    /// Show a notification. Failure is degradable: Perch keeps running.
    fn notify(&mut self, summary: &str, body: &str) -> Result<()>;
}

/// Run-at-login toggle.
pub trait Autostart {
    /// Whether Perch is registered to start at login.
    fn is_enabled(&self) -> Result<bool>;

    /// Register or unregister Perch for login start.
    fn set_enabled(&mut self, enabled: bool) -> Result<()>;
}

/// Single-instance enforcement plus the CLI forwarding channel.
pub trait SingleInstance {
    /// Try to become the one running instance. `false` means another one owns it.
    fn acquire(&mut self) -> Result<bool>;

    /// Send a CLI command to the instance that owns the lock.
    fn forward(&mut self, command: &str) -> Result<()>;
}

/// Pulls queued platform events. The real loop is woken by the OS; this is the
/// drain step both the real and fake backends share.
pub trait EventSource {
    /// Take every queued event.
    fn drain_events(&mut self) -> Vec<PlatformEvent>;
}
