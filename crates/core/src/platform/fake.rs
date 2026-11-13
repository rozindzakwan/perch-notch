//! In-memory implementation of every platform trait, for tests.
//!
//! `FakePlatform` is scripted: push [`FakeScript`] steps to simulate what the
//! OS would have told us, then drain the resulting [`PlatformEvent`]s. It also
//! records everything the app asked the OS to do, so tests can assert on it.

use std::collections::VecDeque;
use std::time::Duration;

use super::{
    Autostart, EventSource, FullscreenWatch, HotZoneWatch, MediaControl, MediaEvent, MonitorSource,
    Notifier, PlatformEvent, SingleInstance, Track, TransportCommand,
};
use crate::error::{Error, Result};
use crate::geometry::{Monitor, Rect};

/// One scripted thing "the OS" does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeScript {
    /// Cursor entered the hover strip.
    HotZoneEnter,
    /// Cursor left the hover strip.
    HotZoneLeave,
    /// Foreground fullscreen state (deduplicated).
    Fullscreen(bool),
    /// A player started reporting this track.
    Track {
        /// Title.
        title: String,
        /// Artist.
        artist: String,
        /// Album.
        album: String,
        /// Length.
        duration: Duration,
        /// Playing or paused.
        playing: bool,
    },
    /// Play/pause changed on the current track.
    Playback(bool),
    /// A CLI command arrived over IPC.
    Ipc(String),
    /// The foreground application changed.
    ActiveWindow {
        /// Application id.
        app: String,
        /// Window title.
        title: String,
    },
    /// The user went idle / became active.
    Idle(bool),
    /// Monitors were plugged or unplugged.
    MonitorsChanged,
    /// Clipboard content changed.
    Clipboard,
}

fn default_monitors() -> Vec<Monitor> {
    vec![Monitor {
        id: 0,
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
        scale: 1.0,
        primary: true,
        work: None,
    }]
}

/// Scriptable fake platform.
#[derive(Debug)]
pub struct FakePlatform {
    monitors: Vec<Monitor>,
    events: VecDeque<PlatformEvent>,
    fullscreen: bool,
    track: Option<Track>,
    commands: Vec<TransportCommand>,
    notifications: Vec<(String, String)>,
    autostart: bool,
    lock_taken: bool,
    forwarded: Vec<String>,
    hot_zone: Option<Rect>,
    media_failure: Option<String>,
    notify_failure: Option<String>,
}

impl Default for FakePlatform {
    fn default() -> Self {
        Self {
            monitors: default_monitors(),
            events: VecDeque::new(),
            fullscreen: false,
            track: None,
            commands: Vec::new(),
            notifications: Vec::new(),
            autostart: false,
            lock_taken: false,
            forwarded: Vec::new(),
            hot_zone: None,
            media_failure: None,
            notify_failure: None,
        }
    }
}

impl FakePlatform {
    /// Build a fake with a specific monitor layout.
    pub fn with_monitors(monitors: Vec<Monitor>) -> Self {
        Self {
            monitors,
            ..Self::default()
        }
    }

    /// Make the media backend report itself as unavailable.
    pub fn fail_media(&mut self, reason: impl Into<String>) {
        self.media_failure = Some(reason.into());
    }

    /// Make notifications report themselves as unavailable.
    pub fn fail_notifications(&mut self, reason: impl Into<String>) {
        self.notify_failure = Some(reason.into());
    }

    /// Simulate one thing happening at the OS level.
    pub fn script(&mut self, step: FakeScript) {
        match step {
            FakeScript::HotZoneEnter => self.events.push_back(PlatformEvent::HotZoneEntered),
            FakeScript::HotZoneLeave => self.events.push_back(PlatformEvent::HotZoneLeft),
            FakeScript::Fullscreen(fs) => {
                if self.fullscreen != fs {
                    self.fullscreen = fs;
                    self.events.push_back(PlatformEvent::FullscreenChanged(fs));
                }
            }
            FakeScript::Track {
                title,
                artist,
                album,
                duration,
                playing,
            } => {
                let track = Track {
                    title,
                    artist,
                    album,
                    duration: Some(duration),
                    position: Some(Duration::ZERO),
                    playing,
                    cover: None,
                };
                self.track = Some(track.clone());
                self.events
                    .push_back(PlatformEvent::Media(MediaEvent::TrackChanged(track)));
            }
            FakeScript::Playback(playing) => {
                if let Some(t) = &mut self.track {
                    t.playing = playing;
                }
                self.events
                    .push_back(PlatformEvent::Media(MediaEvent::PlaybackChanged(playing)));
            }
            FakeScript::Ipc(cmd) => self.events.push_back(PlatformEvent::Command(cmd)),
            FakeScript::ActiveWindow { app, title } => self
                .events
                .push_back(PlatformEvent::ActiveWindowChanged { app, title }),
            FakeScript::Idle(idle) => self.events.push_back(PlatformEvent::IdleChanged(idle)),
            FakeScript::MonitorsChanged => self.events.push_back(PlatformEvent::MonitorsChanged),
            FakeScript::Clipboard => self.events.push_back(PlatformEvent::ClipboardChanged),
        }
    }

    /// Transport commands the app sent, in order.
    pub fn sent_commands(&self) -> Vec<TransportCommand> {
        self.commands.clone()
    }

    /// Notifications the app raised, in order.
    pub fn notifications(&self) -> Vec<(String, String)> {
        self.notifications.clone()
    }

    /// CLI commands forwarded to the owning instance.
    pub fn forwarded(&self) -> Vec<String> {
        self.forwarded.clone()
    }

    /// Where the hover strip was last placed.
    pub fn hot_zone(&self) -> Option<Rect> {
        self.hot_zone
    }
}

impl MonitorSource for FakePlatform {
    fn monitors(&self) -> Result<Vec<Monitor>> {
        Ok(self.monitors.clone())
    }
}

impl HotZoneWatch for FakePlatform {
    fn set_hot_zone(&mut self, rect: Rect) -> Result<()> {
        self.hot_zone = Some(rect);
        Ok(())
    }
}

impl FullscreenWatch for FakePlatform {
    fn is_fullscreen_active(&self) -> bool {
        self.fullscreen
    }
}

impl MediaControl for FakePlatform {
    fn now_playing(&self) -> Option<Track> {
        self.track.clone()
    }

    fn send(&mut self, cmd: TransportCommand) -> Result<()> {
        if let Some(reason) = &self.media_failure {
            return Err(Error::unavailable("media", reason.clone()));
        }
        self.commands.push(cmd);
        if let Some(t) = &mut self.track {
            match cmd {
                TransportCommand::Play => t.playing = true,
                TransportCommand::Pause => t.playing = false,
                TransportCommand::PlayPause => t.playing = !t.playing,
                TransportCommand::SeekTo(pos) => t.position = Some(pos),
                TransportCommand::Next | TransportCommand::Previous => {
                    t.position = Some(Duration::ZERO)
                }
            }
        }
        Ok(())
    }
}

impl Notifier for FakePlatform {
    fn notify(&mut self, summary: &str, body: &str) -> Result<()> {
        if let Some(reason) = &self.notify_failure {
            return Err(Error::unavailable("notifications", reason.clone()));
        }
        self.notifications
            .push((summary.to_string(), body.to_string()));
        Ok(())
    }
}

impl Autostart for FakePlatform {
    fn is_enabled(&self) -> Result<bool> {
        Ok(self.autostart)
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<()> {
        self.autostart = enabled;
        Ok(())
    }
}

impl SingleInstance for FakePlatform {
    fn acquire(&mut self) -> Result<bool> {
        if self.lock_taken {
            Ok(false)
        } else {
            self.lock_taken = true;
            Ok(true)
        }
    }

    fn forward(&mut self, command: &str) -> Result<()> {
        self.forwarded.push(command.to_string());
        Ok(())
    }
}

impl EventSource for FakePlatform {
    fn drain_events(&mut self) -> Vec<PlatformEvent> {
        self.events.drain(..).collect()
    }
}
