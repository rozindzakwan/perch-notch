//! Feature coordination: which optional subsystems are live, and the
//! collapsed-pill status they contribute.
//!
//! The rule from the brief is strict: a feature the user has not enabled must
//! not be loaded or run. This module is the single place that decides that, so
//! "disabled" cannot drift between the config, the tray menu and the loop.

use std::time::Duration;

use perch_core::config::Config;

/// A feature that can be on or off at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    /// Now-playing and transport control.
    Media,
    /// Synced lyrics (network, opt-in).
    Lyrics,
    /// To-do list.
    Todo,
    /// Pomodoro timer.
    Pomodoro,
    /// Clipboard board.
    Clipboard,
    /// File shelf.
    Shelf,
    /// Water/stretch reminders.
    Reminders,
    /// Per-app volume mixer.
    VolumeMixer,
    /// White noise.
    WhiteNoise,
    /// Screen time.
    ScreenTime,
    /// Offline speech to text.
    SpeechToText,
    /// Screenshot tool.
    Screenshot,
    /// Camera mirror.
    CameraMirror,
    /// Read-only mail summary (network, opt-in).
    Mail,
    /// Other apps' notifications shown in the notch.
    NotificationMirror,
    /// Place name on the Today tab (OS location + network lookup, opt-in).
    Location,
}

impl Feature {
    /// Every feature, in settings order.
    pub const ALL: &'static [Feature] = &[
        Feature::Media,
        Feature::Lyrics,
        Feature::Todo,
        Feature::Pomodoro,
        Feature::Clipboard,
        Feature::Shelf,
        Feature::Reminders,
        Feature::VolumeMixer,
        Feature::WhiteNoise,
        Feature::ScreenTime,
        Feature::SpeechToText,
        Feature::Screenshot,
        Feature::CameraMirror,
        Feature::Mail,
        Feature::NotificationMirror,
        Feature::Location,
    ];

    /// Label for the tray menu and the settings window.
    pub fn label(&self) -> &'static str {
        match self {
            Feature::Media => "Now playing",
            Feature::Lyrics => "Synced lyrics",
            Feature::Todo => "To-do",
            Feature::Pomodoro => "Pomodoro",
            Feature::Clipboard => "Clipboard board",
            Feature::Shelf => "File shelf",
            Feature::Reminders => "Reminders",
            Feature::VolumeMixer => "Volume mixer",
            Feature::WhiteNoise => "White noise",
            Feature::ScreenTime => "Screen time",
            Feature::SpeechToText => "Dictation",
            Feature::Screenshot => "Screenshot",
            Feature::CameraMirror => "Camera mirror",
            Feature::Mail => "Mail (read-only)",
            Feature::NotificationMirror => "Notifications from other apps",
            Feature::Location => "Location (Today tab)",
        }
    }

    /// Whether the feature uses the network at all, so the settings window can
    /// mark it clearly: lyrics, the speech model download, mail and the
    /// location lookup.
    pub fn uses_network(&self) -> bool {
        matches!(
            self,
            Feature::Lyrics | Feature::SpeechToText | Feature::Mail | Feature::Location
        )
    }

    /// Whether this feature was compiled into the binary. A feature can be
    /// enabled in the config but absent from a slim build; the UI must say so
    /// rather than silently doing nothing.
    pub fn compiled_in(&self) -> bool {
        match self {
            Feature::Media => cfg!(feature = "media"),
            Feature::Lyrics => cfg!(feature = "lyrics"),
            Feature::Todo | Feature::Pomodoro | Feature::Reminders | Feature::Shelf => {
                cfg!(feature = "productivity")
            }
            Feature::Clipboard => cfg!(feature = "clipboard"),
            Feature::ScreenTime => cfg!(feature = "screen-time"),
            Feature::WhiteNoise => cfg!(feature = "white-noise"),
            Feature::SpeechToText => cfg!(feature = "speech"),
            Feature::Screenshot => cfg!(feature = "screenshot"),
            Feature::CameraMirror => cfg!(feature = "camera"),
            Feature::Mail => cfg!(feature = "mail"),
            // Windows: UserNotificationListener. Linux: a D-Bus monitor.
            Feature::NotificationMirror => cfg!(any(windows, target_os = "linux")),
            Feature::Location => cfg!(feature = "location"),
            // The mixer needs no extra crates on either OS.
            Feature::VolumeMixer => true,
        }
    }

    /// Read this feature's flag out of the config.
    pub fn enabled_in(&self, cfg: &Config) -> bool {
        let f = &cfg.features;
        match self {
            Feature::Media => f.media,
            Feature::Lyrics => f.lyrics,
            Feature::Todo => f.todo,
            Feature::Pomodoro => f.pomodoro,
            Feature::Clipboard => f.clipboard,
            Feature::Shelf => f.shelf,
            Feature::Reminders => f.reminders,
            Feature::VolumeMixer => f.volume_mixer,
            Feature::WhiteNoise => f.white_noise,
            Feature::ScreenTime => f.screen_time,
            Feature::SpeechToText => f.speech_to_text,
            Feature::Screenshot => f.screenshot,
            Feature::CameraMirror => f.camera_mirror,
            Feature::Mail => f.mail,
            Feature::NotificationMirror => f.notification_mirror,
            Feature::Location => f.location,
        }
    }
}

/// Which features are actually active: enabled in the config *and* compiled in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActiveFeatures {
    active: Vec<Feature>,
    /// Features the user enabled that this build does not contain.
    missing: Vec<Feature>,
}

impl ActiveFeatures {
    /// Resolve the config against the compiled feature set.
    pub fn resolve(cfg: &Config) -> Self {
        let mut active = Vec::new();
        let mut missing = Vec::new();
        for f in Feature::ALL {
            if !f.enabled_in(cfg) {
                continue;
            }
            if f.compiled_in() {
                active.push(*f);
            } else {
                missing.push(*f);
            }
        }
        // Lyrics without media is meaningless: there is no track to sync to.
        if active.contains(&Feature::Lyrics) && !active.contains(&Feature::Media) {
            active.retain(|f| *f != Feature::Lyrics);
        }
        Self { active, missing }
    }

    /// Whether a feature is live.
    pub fn is_active(&self, f: Feature) -> bool {
        self.active.contains(&f)
    }

    /// Every live feature.
    pub fn active(&self) -> &[Feature] {
        &self.active
    }

    /// Features enabled in the config but missing from this build.
    pub fn missing(&self) -> &[Feature] {
        &self.missing
    }

    /// Whether anything at all needs the network. Used to prove, in the
    /// settings window, that a default install never talks to the internet.
    pub fn uses_network(&self) -> bool {
        self.active.iter().any(|f| f.uses_network())
    }
}

/// What the collapsed pill should show, in priority order. The pill is small,
/// so exactly one thing wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PillStatus {
    /// Nothing to report; show the mascot.
    Idle,
    /// A countdown such as the Pomodoro's remaining time.
    Countdown(String),
    /// The current track.
    NowPlaying(String),
}

/// Pick what the collapsed pill shows.
///
/// A running countdown beats now-playing: the user started a timer on purpose
/// and needs to see it, whereas the track is visible again as soon as the notch
/// is expanded.
pub fn pill_status(countdown: Option<String>, now_playing: Option<String>) -> PillStatus {
    match (countdown, now_playing) {
        (Some(c), _) if !c.is_empty() => PillStatus::Countdown(c),
        (_, Some(np)) if !np.is_empty() => PillStatus::NowPlaying(np),
        _ => PillStatus::Idle,
    }
}

/// Soonest of a set of optional deadlines; `None` when every one is `None`,
/// which is what lets the event loop block indefinitely.
pub fn soonest(deadlines: &[Option<Duration>]) -> Option<Duration> {
    deadlines.iter().filter_map(|d| *d).min()
}
