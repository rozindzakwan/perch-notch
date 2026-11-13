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
    /// The clipboard received new content.
    ClipboardChanged,
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
