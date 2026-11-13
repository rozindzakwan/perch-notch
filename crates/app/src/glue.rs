//! Wiring: picks the platform backend for each trait and starts/stops the
//! optional ones as features are switched on and off.
//!
//! This is the only module in the app that names concrete platform types; the
//! rest of the app talks to `Box<dyn Trait>`s.

use std::path::Path;
use std::sync::atomic::Ordering;

use perch_core::platform::{
    AudioMixer, Autostart, ClipboardAccess, EventSink, FullscreenWatch, IdleSource, MediaControl,
    MonitorSource, Notifier, Opener, PlatformEvent,
};

use crate::features::{ActiveFeatures, Feature};

/// Every platform service the app uses.
pub struct Services {
    /// Event delivery into the winit loop.
    pub sink: EventSink,
    /// Monitor enumeration.
    pub monitors: Box<dyn MonitorSource>,
    /// Fullscreen state, for the initial check.
    pub fullscreen: Box<dyn FullscreenWatch>,
    /// Desktop notifications.
    pub notifier: Box<dyn Notifier>,
    /// Run at login.
    pub autostart: Box<dyn Autostart>,
    /// Open files.
    pub opener: Box<dyn Opener>,
    /// Idle time.
    pub idle: Box<dyn IdleSource>,
    /// Clipboard (only while the clipboard or shelf feature is live).
    pub clipboard: Option<Box<dyn ClipboardAccess>>,
    /// Media (only while the media feature is live).
    pub media: Option<Box<dyn MediaControl>>,
    /// Per-app volume (only while the mixer is live).
    pub mixer: Option<Box<dyn AudioMixer>>,
    config_watcher: Option<notify::RecommendedWatcher>,
    os: os::OsServices,
}

impl Services {
    /// Create the always-on services and start the OS event thread.
    pub fn start(sink: EventSink) -> Self {
        let parts = os::base(sink.clone());
        Self {
            sink,
            monitors: parts.monitors,
            fullscreen: parts.fullscreen,
            notifier: parts.notifier,
            autostart: parts.autostart,
            opener: parts.opener,
            idle: parts.idle,
            clipboard: None,
            media: None,
            mixer: None,
            config_watcher: None,
            os: parts.os,
        }
    }

    /// Watch the config file's directory; edits arrive as
    /// [`PlatformEvent::ConfigFileChanged`]. Event-driven (inotify /
    /// ReadDirectoryChangesW), no polling.
    pub fn watch_config(&mut self, config_path: &Path) {
        use notify::{RecursiveMode, Watcher};
        let Some(dir) = config_path.parent() else {
            return;
        };
        if let Err(e) = std::fs::create_dir_all(dir) {
            log::warn!("cannot create the config directory {}: {e}", dir.display());
            return;
        }
        let name = config_path.file_name().map(|n| n.to_os_string());
        let sink = self.sink.clone();
        let handler = move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            if ev
                .paths
                .iter()
                .any(|p| p.file_name().map(|n| n.to_os_string()) == name)
            {
                sink(PlatformEvent::ConfigFileChanged);
            }
        };
        match notify::recommended_watcher(handler) {
            Ok(mut w) => match w.watch(dir, RecursiveMode::NonRecursive) {
                Ok(()) => self.config_watcher = Some(w),
                Err(e) => log::warn!("live config reload unavailable: {e}"),
            },
            Err(e) => log::warn!("live config reload unavailable: {e}"),
        }
    }

    /// Start the tray icon (must run on the event-loop thread, after it started).
    pub fn start_tray(&mut self, autostart: bool) {
        self.os.start_tray(self.sink.clone(), autostart);
    }

    /// Reflect the autostart state in the tray menu.
    pub fn set_tray_autostart(&mut self, on: bool) {
        self.os.set_tray_autostart(on);
    }

    /// X11: keep the notch windows' WM-owned states asserted.
    pub fn track_notch_windows(&self, ids: &[u32]) {
        self.os.track_notch_windows(ids);
    }

    /// Start or stop optional backends so exactly the live features run.
    /// Anything not live is dropped (its thread/handles released).
    pub fn apply_features(&mut self, features: &ActiveFeatures) {
        let flags = self.os.flags();
        flags
            .clipboard
            .store(features.is_active(Feature::Clipboard), Ordering::Relaxed);
        flags
            .active_window
            .store(features.is_active(Feature::ScreenTime), Ordering::Relaxed);
        self.os.refresh();
        self.os.set_lock_watch(
            features.is_active(Feature::Reminders) || features.is_active(Feature::ScreenTime),
            &self.sink,
        );

        let want_clipboard =
            features.is_active(Feature::Clipboard) || features.is_active(Feature::Shelf);
        if want_clipboard && self.clipboard.is_none() {
            match self.os.clipboard() {
                Ok(c) => self.clipboard = Some(c),
                Err(e) => log::warn!("clipboard unavailable: {e}"),
            }
        } else if !want_clipboard {
            self.clipboard = None;
        }

        let want_media = features.is_active(Feature::Media);
        if want_media && self.media.is_none() {
            match os::media(self.sink.clone()) {
                Ok(m) => self.media = Some(m),
                Err(e) => log::warn!("now-playing unavailable: {e}"),
            }
        } else if !want_media && self.media.is_some() {
            self.media = None;
            (self.sink)(PlatformEvent::Media(perch_core::platform::MediaEvent::Gone));
        }

        let want_mixer = features.is_active(Feature::VolumeMixer);
        if want_mixer && self.mixer.is_none() {
            match os::mixer(self.sink.clone()) {
                Ok(m) => {
                    self.mixer = Some(m);
                    (self.sink)(PlatformEvent::AudioSessionsChanged);
                }
                Err(e) => log::warn!("volume mixer unavailable: {e}"),
            }
        } else if !want_mixer {
            self.mixer = None;
        }
    }
}

struct BaseParts {
    monitors: Box<dyn MonitorSource>,
    fullscreen: Box<dyn FullscreenWatch>,
    notifier: Box<dyn Notifier>,
    autostart: Box<dyn Autostart>,
    opener: Box<dyn Opener>,
    idle: Box<dyn IdleSource>,
    os: os::OsServices,
}

#[cfg(windows)]
mod os {
    use std::sync::Arc;

    use perch_core::error::Result;
    use perch_core::platform::{
        AudioMixer, ClipboardAccess, EventSink, MediaControl, MonitorSource,
    };
    use perch_platform_windows as win;

    use super::BaseParts;

    pub struct OsServices {
        flags: Arc<win::EventFlags>,
        events: Option<win::WindowsEvents>,
        tray: Option<crate::tray::Tray>,
    }

    pub fn base(sink: EventSink) -> BaseParts {
        win::WindowsMonitors::enable_per_monitor_dpi_v2();
        if let Err(e) = win::services::register_app_id(None) {
            log::warn!("toast registration failed; notifications fall back to the notch: {e}");
        }
        let flags = Arc::new(win::EventFlags::default());
        let events = match win::WindowsEvents::start(sink, flags.clone()) {
            Ok(e) => Some(e),
            Err(e) => {
                log::warn!("OS event thread unavailable (fullscreen auto-hide degraded): {e}");
                None
            }
        };
        let monitors = win::WindowsMonitors;
        let list = monitors.monitors().unwrap_or_default();
        BaseParts {
            fullscreen: Box::new(win::FullscreenDetector::new(list)),
            monitors: Box::new(monitors),
            notifier: Box::new(win::ToastNotifier),
            autostart: Box::new(win::RegistryAutostart),
            opener: Box::new(win::ShellOpener),
            idle: Box::new(win::LastInputIdle),
            os: OsServices {
                flags,
                events,
                tray: None,
            },
        }
    }

    impl OsServices {
        pub fn flags(&self) -> Arc<win::EventFlags> {
            self.flags.clone()
        }

        pub fn refresh(&self) {
            if let Some(e) = &self.events {
                e.refresh();
            }
        }

        pub fn start_tray(&mut self, sink: EventSink, autostart: bool) {
            if self.tray.is_some() {
                return;
            }
            match crate::tray::Tray::start(sink, autostart) {
                Ok(t) => self.tray = Some(t),
                Err(e) => log::warn!("tray icon unavailable: {e}"),
            }
        }

        pub fn set_tray_autostart(&mut self, on: bool) {
            if let Some(t) = &self.tray {
                t.set_autostart(on);
            }
        }

        /// Windows has no WM-owned state to re-assert.
        pub fn track_notch_windows(&self, _ids: &[u32]) {}

        /// Lock/unlock already arrives through the event thread (WTS).
        pub fn set_lock_watch(&mut self, _on: bool, _sink: &EventSink) {}
    }

    pub fn media(sink: EventSink) -> Result<Box<dyn MediaControl>> {
        crate::backends::media(sink)
    }

    pub fn mixer(sink: EventSink) -> Result<Box<dyn AudioMixer>> {
        crate::backends::mixer(sink)
    }

    impl OsServices {
        pub fn clipboard(&self) -> Result<Box<dyn ClipboardAccess>> {
            Ok(Box::new(win::WindowsClipboard::new()?))
        }
    }
}

#[cfg(target_os = "linux")]
mod os {
    use std::sync::Arc;

    use perch_core::error::Result;
    use perch_core::platform::{
        AudioMixer, ClipboardAccess, EventSink, MediaControl, MonitorSource,
    };
    use perch_platform_linux as lx;

    use super::BaseParts;

    pub struct OsServices {
        flags: Arc<lx::x11::WatchFlags>,
        watcher: Option<lx::x11::X11Watcher>,
        tray: Option<lx::tray::TrayHandle>,
        lock: Option<lx::screensaver::LockWatch>,
    }

    pub fn base(sink: EventSink) -> BaseParts {
        let flags = Arc::new(lx::x11::WatchFlags::default());
        let watcher = match lx::x11::X11Watcher::start(sink, flags.clone()) {
            Ok(w) => Some(w),
            Err(e) => {
                log::warn!("X11 event watcher unavailable (fullscreen auto-hide off): {e}");
                None
            }
        };
        let monitors = lx::x11::X11Monitors;
        let list = monitors.monitors().unwrap_or_default();
        BaseParts {
            fullscreen: Box::new(lx::x11::X11Fullscreen::new(list)),
            monitors: Box::new(monitors),
            notifier: Box::new(lx::services::DbusNotifier::default()),
            autostart: Box::new(lx::XdgAutostart),
            opener: Box::new(lx::services::XdgOpener),
            idle: Box::new(lx::services::X11Idle),
            os: OsServices {
                flags,
                watcher,
                tray: None,
                lock: None,
            },
        }
    }

    impl OsServices {
        pub fn flags(&self) -> Arc<lx::x11::WatchFlags> {
            self.flags.clone()
        }

        pub fn refresh(&self) {
            if let Some(w) = &self.watcher {
                w.refresh();
            }
        }

        pub fn start_tray(&mut self, sink: EventSink, autostart: bool) {
            if self.tray.is_some() {
                return;
            }
            match lx::tray::TrayHandle::start(sink, autostart, crate::icon::icon_argb32(32)) {
                Ok(t) => self.tray = Some(t),
                Err(e) => log::warn!("tray icon unavailable: {e}"),
            }
        }

        pub fn set_tray_autostart(&mut self, on: bool) {
            if let Some(t) = &self.tray {
                t.set_autostart(on);
            }
        }

        pub fn track_notch_windows(&self, ids: &[u32]) {
            if let Some(w) = &self.watcher {
                w.track_notch_windows(ids);
            }
        }

        /// Start or stop the screensaver lock watcher.
        pub fn set_lock_watch(&mut self, on: bool, sink: &EventSink) {
            if on && self.lock.is_none() {
                match lx::screensaver::LockWatch::start(sink.clone()) {
                    Ok(l) => self.lock = Some(l),
                    Err(e) => log::info!("screen-lock events unavailable: {e}"),
                }
            } else if !on {
                self.lock = None;
            }
        }
    }

    pub fn media(sink: EventSink) -> Result<Box<dyn MediaControl>> {
        crate::backends::media(sink)
    }

    pub fn mixer(sink: EventSink) -> Result<Box<dyn AudioMixer>> {
        crate::backends::mixer(sink)
    }

    impl OsServices {
        pub fn clipboard(&self) -> Result<Box<dyn ClipboardAccess>> {
            Ok(Box::new(lx::clipboard::X11Clipboard::new(
                self.flags.clone(),
            )?))
        }
    }
}
