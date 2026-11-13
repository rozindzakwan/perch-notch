//! The winit application: owns the windows, the notch state machine, the
//! [`Model`] and the platform [`Services`], and moves events between them.
//!
//! Event-driven by construction: the loop sleeps in `ControlFlow::Wait` and
//! wakes only for OS events, user input, backend events (delivered through an
//! `EventLoopProxy`), or the soonest real deadline (an animation frame, a
//! running Pomodoro second, a reminder). With the notch collapsed and nothing
//! scheduled, Perch receives no wakeups at all.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use perch_core::config::{Config, ConfigStore};
use perch_core::geometry::{self, Monitor, NotchGeometry, Rect};
use perch_core::notch::{Notch, NotchEvent, NotchState};
use perch_core::platform::{PlatformEvent, TransportCommand};
use perch_core::view::{resolve_click, Action, Hit};
use perch_ui::form::FormWindow;
use perch_ui::render::{Frame, Renderer};
use perch_ui::shell::NotchWindows;
use perch_ui::theme::Palette;
use tiny_skia::Pixmap;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::window::WindowId;

use crate::features::Feature;
use crate::forms::{self, SettingsOutcome};
use crate::glue::Services;
use crate::model::{Effect, Model, Now};
use crate::persist::{self, Status};

/// Minimum interval between animation frames (~120 fps).
const FRAME_INTERVAL: Duration = Duration::from_millis(8);
/// Debounce for config-file change notifications (editors write in bursts).
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(150);

/// Events delivered into the loop from other threads.
#[derive(Debug)]
pub enum UserEvent {
    /// Something a platform backend reported.
    Platform(PlatformEvent),
    /// A lyrics lookup finished.
    Lyrics {
        /// Request key.
        key: String,
        /// LRC text, `None` if not found.
        lrc: Option<String>,
    },
}

/// The application.
pub struct Shell {
    model: Model,
    store: ConfigStore,
    services: Services,
    proxy: EventLoopProxy<UserEvent>,
    notch: Notch,
    windows: Option<NotchWindows>,
    renderer: Option<Renderer>,
    pixmap: Option<Pixmap>,
    hits: Vec<Hit>,
    monitor: Option<Monitor>,
    last_pill: Option<(Rect, bool)>,
    last_hot: Option<Rect>,
    fullscreen: bool,
    last_anim: Instant,
    settings: Option<FormWindow>,
    dialog: Option<FormWindow>,
    dialog_error: Option<String>,
    cursor: (f32, f32),
    press: Option<(f32, f32)>,
    reload_at: Option<Instant>,
    status_path: Option<PathBuf>,
    last_status: Option<Status>,
    frames: u64,
    first_frame_ms: Option<u64>,
    started: Instant,
    data_dir: Option<PathBuf>,
    quitting: bool,
    /// Effects produced where no `ActiveEventLoop` was at hand.
    queued: Vec<Effect>,
    /// Persisted screen time has been merged into the model.
    usage_loaded: bool,
}

impl Shell {
    /// Build the shell. Windows are created in `resumed`.
    pub fn new(
        config: Config,
        store: ConfigStore,
        services: Services,
        proxy: EventLoopProxy<UserEvent>,
        started: Instant,
    ) -> Self {
        let now = Now::real();
        let notch = Notch::new(
            Duration::from_millis(u64::from(config.notch.animation_ms)),
            Duration::from_millis(u64::from(config.notch.collapse_delay_ms)),
        );
        let mut model = Model::new(config, now);
        let data_dir = persist::data_dir();
        if let Some(dir) = &data_dir {
            model.set_todos(persist::load_todos(dir));
        }
        Self {
            model,
            store,
            services,
            proxy,
            notch,
            windows: None,
            renderer: None,
            pixmap: None,
            hits: Vec::new(),
            monitor: None,
            last_pill: None,
            last_hot: None,
            fullscreen: false,
            last_anim: Instant::now(),
            settings: None,
            dialog: None,
            dialog_error: None,
            cursor: (0.0, 0.0),
            press: None,
            reload_at: None,
            status_path: std::env::var_os("PERCH_STATUS_FILE").map(PathBuf::from),
            last_status: None,
            frames: 0,
            first_frame_ms: None,
            started,
            data_dir,
            quitting: false,
            queued: Vec::new(),
            usage_loaded: false,
        }
    }

    fn config(&self) -> &Config {
        self.model.config()
    }

    fn geo(&self) -> NotchGeometry {
        self.config().notch.geometry()
    }

    fn palette(&self) -> Palette {
        Palette::resolve(self.config().theme, true)
    }

    fn resolve_monitor(&mut self) -> Option<Monitor> {
        if let Some(m) = self.monitor {
            return Some(m);
        }
        match self
            .services
            .monitors
            .monitor_for(self.config().notch.monitor)
        {
            Ok(m) => {
                self.monitor = Some(m);
                Some(m)
            }
            Err(e) => {
                log::warn!("no usable monitor: {e}");
                None
            }
        }
    }

    /// Size and place both windows for the current animation frame.
    fn place(&mut self) {
        let Some(monitor) = self.resolve_monitor() else {
            return;
        };
        let geo = self.geo();
        let pos = self.config().notch.position;
        let v = self.notch.visual(geo.collapsed, geo.expanded);
        let pill = geometry::notch_rect_sized(&monitor, pos, &geo, (v.width, v.height));
        let visible = v.visible && self.notch.is_visible();
        let hot = geometry::hot_zone_rect(&monitor, pos, &geo).rect;
        let Some(w) = &self.windows else { return };
        if self.last_pill != Some((pill, visible)) {
            if let Err(e) = w.place_pill(pill, visible) {
                log::warn!("placing the pill: {e}");
            }
            self.last_pill = Some((pill, visible));
            w.request_redraw();
        }
        if self.last_hot != Some(hot) {
            // The hover strip stays mapped while a fullscreen app hides the
            // pill, otherwise hovering could never bring it back.
            if let Err(e) = w.place_hot_zone(hot, true) {
                log::warn!("placing the hot zone: {e}");
            }
            self.last_hot = Some(hot);
        }
    }

    fn expanded(&self) -> bool {
        matches!(
            self.notch.state(),
            NotchState::Expanded | NotchState::Expanding
        )
    }

    fn request_redraw(&self) {
        if let Some(w) = &self.windows {
            w.request_redraw();
        }
    }

    fn draw(&mut self) {
        log::trace!("draw");
        let now = Now::real();
        let palette = self.palette();
        let scale = self.monitor.map(|m| m.scale as f32).unwrap_or(1.0);
        let progress = self.notch.progress();
        let corner_radius = self.config().notch.corner_radius.map(|r| r as f32);
        let mut view = self.model.view(now);
        let Some(windows) = &mut self.windows else {
            return;
        };
        let (w, h) = windows.pill_size();
        if w == 0 || h == 0 {
            return;
        }
        if self.pixmap.as_ref().map(|p| (p.width(), p.height())) != Some((w, h)) {
            self.pixmap = Pixmap::new(w, h);
        }
        let (Some(pm), Some(renderer)) = (&mut self.pixmap, &mut self.renderer) else {
            return;
        };
        let mut frame = Frame::new(w, h, scale, palette);
        frame.progress = progress;
        frame.corner_radius = corner_radius;
        frame.opaque = windows.needs_opaque();
        if frame.opaque {
            let s = scale;
            let r = corner_radius.map(|r| r * s).unwrap_or(if progress >= 0.5 {
                18.0 * s
            } else {
                h as f32 / 2.0
            });
            windows.clip_to_pill(w, h, r.round() as u32);
        }
        // Mid-animation the pill is between sizes; show just the shape so
        // neither layout is drawn squashed.
        if (0.35..0.85).contains(&progress) {
            view.collapsed = perch_core::view::CollapsedView::Idle;
            view.tabs.clear();
            view.body = perch_core::view::Body::Empty(String::new());
        }
        self.hits = renderer.draw(pm, &frame, &view);
        if (0.35..0.85).contains(&progress) {
            self.hits.clear();
        }
        match windows.present(pm) {
            Ok(()) => {
                self.frames += 1;
                if self.first_frame_ms.is_none() {
                    let ms = self.started.elapsed().as_millis() as u64;
                    self.first_frame_ms = Some(ms);
                    log::info!("first frame presented {ms} ms after process start");
                }
            }
            Err(e) => log::warn!("presenting a frame: {e}"),
        }
        self.write_status();
    }

    fn write_status(&mut self) {
        let Some(path) = &self.status_path else {
            return;
        };
        let (pill, visible) = self
            .last_pill
            .map(|(r, v)| {
                (
                    [
                        i64::from(r.x),
                        i64::from(r.y),
                        i64::from(r.w),
                        i64::from(r.h),
                    ],
                    v,
                )
            })
            .unwrap_or(([0; 4], false));
        let lyric = match self.model.view(Now::real()).body {
            perch_core::view::Body::Media(m) => m.lyric.unwrap_or_default(),
            _ => String::new(),
        };
        let status = Status {
            state: format!("{:?}", self.notch.state()),
            pill,
            visible,
            tab: self
                .model
                .tab()
                .map(|t| t.label().to_string())
                .unwrap_or_default(),
            track: self.model.media().label().unwrap_or_default(),
            playing: self.model.media().is_playing(),
            clipboard: self.model.clipboard().len(),
            shelf: self.model.shelf().len(),
            todos: self.model.todos().len(),
            features: self
                .model
                .features()
                .active()
                .iter()
                .map(|f| format!("{f:?}"))
                .collect(),
            frames: self.frames,
            first_frame_ms: self.first_frame_ms,
            noise: crate::extras::noise_stream_open(),
            lyric,
            usage_apps: {
                let now = Now::real();
                self.model.flush_usage(now);
                let day =
                    perch_core::usage::DayId::from_unix(now.unix, crate::model::local_offset());
                self.model.usage().aggregate().apps_for(day).len()
            },
        };
        // Frames change constantly during animation; only write when
        // something other than the frame counter changed, or every 30 frames.
        let mut cmp = status.clone();
        cmp.frames = 0;
        let changed = self.last_status.as_ref().map(|s| {
            let mut s = s.clone();
            s.frames = 0;
            s != cmp
        });
        if changed != Some(false) || self.frames % 30 == 0 {
            persist::write_status(path, &status);
            self.last_status = Some(status);
        }
    }

    /// Apply a new config everywhere (settings window, file edit, CLI).
    fn apply_config(&mut self, cfg: Config) {
        let old = self.model.config().clone();
        let fx = self.model.apply_config(cfg.clone());
        self.services.apply_features(self.model.features());
        self.load_usage_once();
        if old.hotkeys != cfg.hotkeys || old.features != cfg.features {
            self.sync_hotkeys();
        }
        if old.notch.animation_ms != cfg.notch.animation_ms
            || old.notch.collapse_delay_ms != cfg.notch.collapse_delay_ms
        {
            self.notch = Notch::new(
                Duration::from_millis(u64::from(cfg.notch.animation_ms)),
                Duration::from_millis(u64::from(cfg.notch.collapse_delay_ms)),
            );
        }
        if old.notch.hide_on_fullscreen != cfg.notch.hide_on_fullscreen && self.fullscreen {
            self.notch
                .handle(NotchEvent::FullscreenChanged(cfg.notch.hide_on_fullscreen));
        }
        self.sync_autostart();
        self.monitor = None;
        self.last_pill = None;
        self.last_hot = None;
        self.place();
        self.refresh_settings_form();
        self.pending(fx);
    }

    /// Merge persisted screen time the first time the feature is live.
    fn load_usage_once(&mut self) {
        if self.usage_loaded || !self.model.is_active(Feature::ScreenTime) {
            return;
        }
        self.usage_loaded = true;
        if let Some(agg) = crate::extras::load_usage() {
            self.model.restore_usage(&agg);
        }
    }

    /// Register the configured hotkeys for live features only, so Perch never
    /// grabs a key it would not use.
    fn sync_hotkeys(&mut self) {
        let (bindings, problems) = self.config().hotkeys.bindings();
        for p in problems {
            log::warn!("{p}");
        }
        let live: Vec<_> = bindings
            .into_iter()
            .filter(|b| match b.id.as_str() {
                "shot" => self.model.is_active(Feature::Screenshot),
                "dictate" => self.model.is_active(Feature::SpeechToText),
                _ => true,
            })
            .collect();
        self.services.set_hotkeys(live);
    }

    fn sync_autostart(&mut self) {
        let want = self.config().autostart;
        match self.services.autostart.is_enabled() {
            Ok(have) if have == want => {}
            _ => {
                if let Err(e) = self.services.autostart.set_enabled(want) {
                    log::warn!("could not change start-at-login: {e}");
                }
            }
        }
        self.services.set_tray_autostart(want);
    }

    fn refresh_settings_form(&mut self) {
        let monitors = self.services.monitors.monitors().unwrap_or_default();
        let spec = forms::settings_spec(self.model.config(), &monitors);
        if let Some(s) = &mut self.settings {
            s.replace(spec);
        }
    }

    fn save_config(&mut self, cfg: &Config) {
        if let Err(e) = self.store.save(cfg) {
            log::warn!("saving the config: {e}");
            self.model.flash("Could not save settings", Now::real());
        }
    }

    fn reload_config(&mut self) {
        let (cfg, problem) = self.store.load_or_default();
        if let Some(p) = problem {
            log::warn!("config problem, using defaults where needed: {p}");
            self.model
                .flash("Config has errors; using defaults", Now::real());
        }
        if &cfg != self.model.config() {
            self.apply_config(cfg);
        }
    }

    // Effects produced outside of a `&ActiveEventLoop` context are queued and
    // run on the next loop iteration.
    fn pending(&mut self, fx: Vec<Effect>) {
        self.queued.extend(fx);
        // A request_redraw also wakes the loop if it was idle.
        self.request_redraw();
    }

    fn run_effects(&mut self, el: &ActiveEventLoop, fx: Vec<Effect>) {
        let mut queue: std::collections::VecDeque<Effect> = fx.into();
        while let Some(effect) = queue.pop_front() {
            match effect {
                Effect::Redraw => self.request_redraw(),
                Effect::Notify { summary, body } => {
                    if let Err(e) = self.services.notifier.notify(&summary, &body) {
                        log::info!("notification shown in the notch only: {e}");
                    }
                }
                Effect::Transport(cmd) => self.transport(cmd),
                Effect::FetchLyrics(req) => {
                    let proxy = self.proxy.clone();
                    let cache = persist::cache_dir().map(|d| d.join("lyrics"));
                    crate::lyrics::fetch_async(req, cache, move |key, lrc| {
                        let _ = proxy.send_event(UserEvent::Lyrics { key, lrc });
                    });
                }
                Effect::ClipboardWrite(content) => match &mut self.services.clipboard {
                    Some(c) => {
                        if let Err(e) = c.write(&content) {
                            log::warn!("clipboard write: {e}");
                        }
                    }
                    None => log::warn!("clipboard unavailable"),
                },
                Effect::ClipboardWriteFiles(paths) => match &mut self.services.clipboard {
                    Some(c) => {
                        if let Err(e) = c.write_files(&paths) {
                            log::warn!("clipboard file write: {e}");
                        }
                    }
                    None => log::warn!("clipboard unavailable"),
                },
                Effect::Open(path) => {
                    if let Err(e) = self.services.opener.open(&path) {
                        log::warn!("opening {}: {e}", path.display());
                        self.model.flash("Could not open that file", Now::real());
                    }
                }
                Effect::SetVolume { id, volume } => {
                    if let Some(m) = &mut self.services.mixer {
                        if let Err(e) = m.set_volume(&id, volume) {
                            log::warn!("set volume: {e}");
                        }
                    }
                }
                Effect::SetMute { id, muted } => {
                    if let Some(m) = &mut self.services.mixer {
                        if let Err(e) = m.set_mute(&id, muted) {
                            log::warn!("set mute: {e}");
                        }
                    }
                }
                Effect::Noise(mix) => crate::extras::set_noise(mix),
                Effect::SaveTodos => {
                    if let Some(dir) = &self.data_dir {
                        persist::save_todos(dir, self.model.todos());
                    }
                }
                Effect::SaveUsage => crate::extras::save_usage(self.model.usage()),
                Effect::OpenSettings => self.open_settings(el),
                Effect::OpenTodoDialog => self.open_todo_dialog(el),
                Effect::Screenshot => crate::extras::screenshot(el, &mut self.model),
                Effect::CameraToggle => crate::extras::camera_toggle(el, &mut self.model),
                Effect::DictateToggle => crate::extras::dictate_toggle(&mut self.model),
                Effect::ToggleNotch => {
                    self.notch.handle(NotchEvent::Toggle);
                    self.after_notch_change(el);
                }
                Effect::PinToggle => {
                    self.notch.handle(NotchEvent::Toggle);
                    self.after_notch_change(el);
                }
                Effect::ReloadConfig => self.reload_config(),
                Effect::Quit => {
                    self.quitting = true;
                    self.model.flush_usage(Now::real());
                    self.model.flush_usage(Now::real());
                    crate::extras::shutdown(self.model.usage());
                    el.exit();
                }
                Effect::QueryIdle => {
                    let idle = self.services.idle.idle_for().unwrap_or(Duration::ZERO);
                    queue.extend(self.model.idle_measured(idle, Now::real()));
                }
            }
        }
        let queued = std::mem::take(&mut self.queued);
        if !queued.is_empty() {
            self.run_effects(el, queued);
        }
    }

    fn transport(&mut self, cmd: TransportCommand) {
        match &mut self.services.media {
            Some(m) => {
                if let Err(e) = m.send(cmd) {
                    log::warn!("media command failed: {e}");
                }
            }
            None => log::debug!("no media backend for {cmd:?}"),
        }
    }

    fn open_settings(&mut self, el: &ActiveEventLoop) {
        if let Some(s) = &self.settings {
            s.focus();
            return;
        }
        let monitors = self.services.monitors.monitors().unwrap_or_default();
        let spec = forms::settings_spec(self.model.config(), &monitors);
        match FormWindow::open(el, spec, self.palette()) {
            Ok(w) => self.settings = Some(w),
            Err(e) => log::warn!("cannot open settings: {e}"),
        }
    }

    fn open_todo_dialog(&mut self, el: &ActiveEventLoop) {
        if let Some(d) = &self.dialog {
            d.focus();
            return;
        }
        self.dialog_error = None;
        match FormWindow::open(el, forms::todo_spec(), self.palette()) {
            Ok(w) => self.dialog = Some(w),
            Err(e) => log::warn!("cannot open the to-do dialog: {e}"),
        }
    }

    fn after_notch_change(&mut self, el: &ActiveEventLoop) {
        self.last_anim = Instant::now();
        self.place();
        self.request_redraw();
        self.schedule(el);
    }

    fn handle_command(&mut self, el: &ActiveEventLoop, verb: &str) {
        log::debug!("command: {verb:?}");
        let now = Now::real();
        match verb.trim() {
            "autostart-toggle" => {
                let mut cfg = self.model.config().clone();
                cfg.autostart = !cfg.autostart;
                self.save_config(&cfg);
                self.apply_config(cfg);
            }
            other => {
                let fx = self.model.command(other, now);
                self.run_effects(el, fx);
            }
        }
    }

    fn handle_platform(&mut self, el: &ActiveEventLoop, ev: PlatformEvent) {
        let now = Now::real();
        match ev {
            PlatformEvent::FullscreenChanged(fs) => {
                self.fullscreen = fs;
                if self.config().notch.hide_on_fullscreen
                    && self.notch.handle(NotchEvent::FullscreenChanged(fs))
                {
                    self.after_notch_change(el);
                }
            }
            PlatformEvent::MonitorsChanged => {
                self.monitor = None;
                self.last_pill = None;
                self.last_hot = None;
                self.place();
            }
            PlatformEvent::WindowManagerChanged => {
                log::info!("window manager changed; re-applying keep-above and friends");
                self.monitor = None;
                self.last_pill = None;
                self.last_hot = None;
                self.place();
            }
            PlatformEvent::ConfigFileChanged => {
                self.reload_at = Some(Instant::now() + RELOAD_DEBOUNCE);
            }
            PlatformEvent::ClipboardChanged => {
                let read = match &mut self.services.clipboard {
                    Some(c) => c.read(),
                    None => return,
                };
                match read {
                    Ok(Some((content, flags))) => {
                        let fx = self.model.clipboard_captured(content, &flags, now);
                        self.run_effects(el, fx);
                    }
                    Ok(None) => {}
                    Err(e) => log::debug!("clipboard read: {e}"),
                }
            }
            PlatformEvent::AudioSessionsChanged => {
                let sessions = match &mut self.services.mixer {
                    Some(m) => m.sessions(),
                    None => return,
                };
                match sessions {
                    Ok(s) => {
                        let fx = self.model.sessions_captured(s);
                        self.run_effects(el, fx);
                    }
                    Err(e) => log::debug!("audio sessions: {e}"),
                }
            }
            PlatformEvent::FileDropped(path) => {
                let is_dir = path.is_dir();
                let fx = self.model.file_dropped(path, is_dir, now);
                self.run_effects(el, fx);
            }
            PlatformEvent::Command(verb) => self.handle_command(el, &verb),
            PlatformEvent::Hotkey { id, pressed } => {
                let fx = crate::extras::hotkey(&mut self.model, &id, pressed, now);
                self.run_effects(el, fx);
            }
            PlatformEvent::HotZoneEntered => {
                if self.notch.handle(NotchEvent::HotZoneEntered) {
                    self.after_notch_change(el);
                }
            }
            PlatformEvent::HotZoneLeft => {
                if self.notch.handle(NotchEvent::HotZoneLeft) {
                    self.after_notch_change(el);
                }
            }
            other => {
                let fx = self.model.handle_event(&other, now);
                self.run_effects(el, fx);
            }
        }
    }

    fn pill_click(&mut self, el: &ActiveEventLoop, x: f32, y: f32) {
        if let Some(action) = resolve_click(&self.hits, x, y) {
            let fx = self.model.action(action, Now::real());
            self.run_effects(el, fx);
        }
    }

    fn handle_settings_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) {
        let Some(win) = &mut self.settings else {
            return;
        };
        let events = win.handle(event);
        for ev in events {
            let monitors = self.services.monitors.monitors().unwrap_or_default();
            match forms::apply_settings_event(self.model.config(), &monitors, &ev) {
                SettingsOutcome::Changed(cfg) => {
                    self.save_config(&cfg);
                    self.apply_config(cfg);
                }
                SettingsOutcome::OpenConfig => {
                    let path = self.store.path().to_path_buf();
                    if !path.exists() {
                        let cfg = self.model.config().clone();
                        self.save_config(&cfg);
                    }
                    if let Err(e) = self.services.opener.open(&path) {
                        log::warn!("opening the config file: {e}");
                    }
                }
                SettingsOutcome::Close => {
                    self.settings = None;
                    crate::sys::trim_heap();
                    return;
                }
                SettingsOutcome::None => {}
            }
        }
        let _ = el;
    }

    fn handle_dialog_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) {
        let Some(win) = &mut self.dialog else { return };
        let events = win.handle(event);
        let text = win.state().text("text").unwrap_or_default().to_string();
        let due_text = win.state().text("due").unwrap_or_default().to_string();
        for ev in events {
            match ev {
                perch_ui::form::FormEvent::Button(id) if id == "add" => {
                    let now = Now::real();
                    match forms::parse_due(&due_text, now.unix, crate::sys::utc_offset_seconds()) {
                        Ok(due) => {
                            if text.trim().is_empty() {
                                continue;
                            }
                            let fx = self.model.add_todo(&text, due, now);
                            self.dialog = None;
                            self.run_effects(el, fx);
                            return;
                        }
                        Err(msg) => {
                            self.dialog_error = Some(msg.clone());
                            self.model.flash(msg, now);
                            self.request_redraw();
                        }
                    }
                }
                perch_ui::form::FormEvent::Button(_) | perch_ui::form::FormEvent::Closed => {
                    self.dialog = None;
                    crate::sys::trim_heap();
                    return;
                }
                _ => {}
            }
        }
    }

    /// Pick the next wakeup from every deadline source.
    fn schedule(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
        let mut next: Option<Duration> = None;
        let mut take = |d: Duration| next = Some(next.map_or(d, |n: Duration| n.min(d)));
        if let Some(d) = self.notch.next_deadline() {
            take(if self.notch.wants_animation() {
                d.max(FRAME_INTERVAL)
            } else {
                d
            });
        }
        if let Some(d) = self.model.next_deadline(Now::real(), self.expanded()) {
            take(d.max(Duration::from_millis(10)));
        }
        if let Some(at) = self.reload_at {
            take(at.saturating_duration_since(now));
        }
        el.set_control_flow(match next {
            Some(d) => ControlFlow::WaitUntil(now + d),
            None => ControlFlow::Wait,
        });
    }
}

impl ApplicationHandler<UserEvent> for Shell {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.windows.is_some() {
            return;
        }
        let Some(monitor) = self.resolve_monitor() else {
            log::error!("no monitors available; exiting instead of running invisibly");
            el.exit();
            return;
        };
        let geo = self.geo();
        let pos = self.config().notch.position;
        let pill = geometry::notch_rect(&monitor, pos, &geo, false);
        let hot = geometry::hot_zone_rect(&monitor, pos, &geo);
        match NotchWindows::create(el, pill, hot.rect) {
            Ok(w) => self.windows = Some(w),
            Err(e) => {
                log::error!("cannot create the notch windows: {e}");
                el.exit();
                return;
            }
        }
        match Renderer::new() {
            Ok(r) => self.renderer = Some(r),
            Err(e) => {
                log::error!("cannot initialise the renderer: {e}");
                el.exit();
                return;
            }
        }
        if let Some((pill, hot)) = self.windows.as_ref().and_then(|w| w.x11_ids()) {
            self.services.track_notch_windows(&[pill, hot]);
        }
        self.services.apply_features(self.model.features());
        self.load_usage_once();
        self.sync_hotkeys();
        self.services.start_tray(self.config().autostart);
        self.services.watch_config(self.store.path());
        self.sync_autostart();

        if self.config().notch.hide_on_fullscreen && self.services.fullscreen.is_fullscreen_active()
        {
            self.fullscreen = true;
            self.notch.handle(NotchEvent::FullscreenChanged(true));
        }
        self.place();
        self.request_redraw();
        el.set_control_flow(ControlFlow::Wait);
        log::info!(
            "Perch is up on monitor {} ({}x{} @{:.2}x); live features: {:?}",
            monitor.id,
            monitor.width,
            monitor.height,
            monitor.scale,
            self.model.features().active()
        );
        for missing in self.model.features().missing() {
            log::warn!(
                "{} is enabled in the config but not compiled into this build",
                missing.label()
            );
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, event: UserEvent) {
        if self.quitting {
            return;
        }
        match event {
            UserEvent::Platform(ev) => self.handle_platform(el, ev),
            UserEvent::Lyrics { key, lrc } => {
                let fx = self.model.lyrics_loaded(&key, lrc.as_deref());
                self.run_effects(el, fx);
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        log::trace!("window_event {id:?}: {event:?}");
        if self.settings.as_ref().is_some_and(|s| s.id() == id) {
            self.handle_settings_event(el, &event);
            return;
        }
        if self.dialog.as_ref().is_some_and(|d| d.id() == id) {
            self.handle_dialog_event(el, &event);
            return;
        }
        let (pill_id, hot_id) = match &self.windows {
            Some(w) => (w.pill_id(), w.hot_id()),
            None => return,
        };
        if id != pill_id && id != hot_id {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.quitting = true;
                self.model.flush_usage(Now::real());
                crate::extras::shutdown(self.model.usage());
                el.exit();
            }
            WindowEvent::CursorEntered { .. } => {
                let ev = if id == hot_id {
                    NotchEvent::HotZoneEntered
                } else {
                    NotchEvent::PointerEntered
                };
                if self.notch.handle(ev) {
                    self.after_notch_change(el);
                }
            }
            WindowEvent::CursorLeft { .. } => {
                // Crossing from the strip into the pill fires HotZoneLeft then
                // PointerEntered; the enter cancels the pending collapse, so
                // the grace period makes the hand-over seamless.
                let ev = if id == hot_id {
                    NotchEvent::HotZoneLeft
                } else {
                    NotchEvent::PointerLeft
                };
                self.press = None;
                if self.notch.handle(ev) {
                    self.after_notch_change(el);
                }
            }
            WindowEvent::HoveredFile(_) => {
                // Dragging a file over the notch opens it so there is a target.
                if self.notch.handle(NotchEvent::HotZoneEntered) {
                    self.after_notch_change(el);
                }
            }
            WindowEvent::DroppedFile(path) => {
                self.handle_platform(el, PlatformEvent::FileDropped(path));
            }
            WindowEvent::CursorMoved { position, .. } if id == pill_id => {
                self.cursor = (position.x as f32, position.y as f32);
                if let Some((px, py)) = self.press {
                    let moved = (self.cursor.0 - px).abs() + (self.cursor.1 - py).abs();
                    if moved > 8.0 {
                        if let Some(item) = perch_core::view::shelf_item_at(&self.hits, px, py) {
                            self.press = None;
                            let path = self.model.shelf().get(item).map(|i| i.path.clone());
                            if let Some(path) = path {
                                crate::extras::drag_out(&path);
                            }
                        }
                    }
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } if id == pill_id => match state {
                ElementState::Pressed => self.press = Some(self.cursor),
                ElementState::Released => {
                    if let Some((px, py)) = self.press.take() {
                        let moved = (self.cursor.0 - px).abs() + (self.cursor.1 - py).abs();
                        if moved <= 8.0 {
                            self.pill_click(el, px, py);
                        }
                    }
                }
            },
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } if id == pill_id => {
                let fx = self.model.action(Action::OpenSettings, Now::real());
                self.run_effects(el, fx);
            }
            WindowEvent::MouseWheel { delta, .. } if id == pill_id => {
                let rows = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y.signum() as i32,
                    MouseScrollDelta::PixelDelta(p) => -(p.y.signum() as i32),
                };
                if rows != 0 {
                    let fx = self.model.action(Action::Scroll(rows), Now::real());
                    self.run_effects(el, fx);
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                self.monitor = None;
                self.last_pill = None;
                self.last_hot = None;
                self.place();
            }
            WindowEvent::RedrawRequested if id == pill_id => self.draw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.quitting {
            return;
        }
        let now = Instant::now();
        if self.reload_at.is_some_and(|at| at <= now) {
            self.reload_at = None;
            self.reload_config();
        }
        let dt = now.saturating_duration_since(self.last_anim);
        self.last_anim = now;
        let was = self.notch.state();
        let needs_redraw = self.notch.advance(dt);
        if needs_redraw || self.notch.state() != was {
            self.place();
            self.request_redraw();
        }
        let fx = self.model.tick(Now::real());
        let queued = std::mem::take(&mut self.queued);
        self.run_effects(el, fx.into_iter().chain(queued).collect());
        if self.model.is_active(Feature::Shelf) && !self.model.shelf().is_empty() && self.expanded()
        {
            let fx = self.model.prune_shelf(|p| p.exists());
            self.run_effects(el, fx);
        }
        // A hidden pill never draws, so the test status must not depend on it.
        if self.status_path.is_some() {
            self.write_status();
        }
        self.schedule(el);
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        if let Some(dir) = &self.data_dir {
            persist::save_todos(dir, self.model.todos());
        }
    }
}
