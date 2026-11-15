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
/// Logical size of a banner: the notch opened halfway.
const BANNER_SIZE: (f32, f32) = (360.0, 88.0);
/// Logical size of the stripe the notch rests as over fullscreen and maximized
/// apps.
const STRIPE_SIZE: (f32, f32) = (120.0, 5.0);
/// Debounce for config-file change notifications (editors write in bursts).
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(150);

/// Messages from the dictation workers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeechMsg {
    /// Show a status line in the notch.
    Status(String),
    /// Transcribed text, ready to paste.
    Text(String),
}

/// Events delivered into the loop from other threads.
#[derive(Debug)]
pub enum UserEvent {
    /// Dictation progress or result.
    Speech(SpeechMsg),
    /// The camera produced a new mirror frame.
    CameraFrame,
    /// The camera stopped (error message).
    CameraStopped(String),
    /// Something a platform backend reported.
    Platform(PlatformEvent),
    /// A lyrics lookup finished.
    Lyrics {
        /// Request key.
        key: String,
        /// LRC text, `None` if not found.
        lrc: Option<String>,
    },
    /// The mail client reported something.
    #[cfg(feature = "mail")]
    Mail(perch_core::mail::MailUpdate),
}

/// Which form the dialog window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogKind {
    Todo,
    Mail,
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
    /// Monitor (id) whose foreground window is maximized, if any.
    maximized_on: Option<u32>,
    /// When the local UTC offset was last read (it can change with daylight
    /// saving; read at most once a minute).
    offset_read: Option<Instant>,
    last_anim: Instant,
    settings: Option<FormWindow>,
    dialog: Option<FormWindow>,
    dialog_error: Option<String>,
    dialog_kind: DialogKind,
    #[cfg(feature = "mail")]
    mail: crate::mail::MailHub,
    #[cfg(feature = "mail")]
    secrets: Option<std::sync::Arc<dyn perch_core::platform::SecretStore>>,
    shot: Option<perch_ui::shot::ShotWindow>,
    mirror: Option<perch_ui::mirror::MirrorWindow>,
    #[cfg(feature = "camera")]
    capture: Option<crate::camera::Capture>,
    /// A screenshot is due once the notch has collapsed out of the way.
    shot_at: Option<Instant>,
    cursor: (f32, f32),
    press: Option<(f32, f32)>,
    /// A slider being dragged, and whether the pointer left the pill meanwhile
    /// (the leave is applied on release, so the panel stays open mid-drag).
    drag: Option<perch_core::view::SliderDrag>,
    left_during_drag: bool,
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
    /// Shadow margin around the surface in the pill window (physical px).
    inset: (f32, f32, f32, f32),
    /// Expanded panel size the content is laid out at (physical px).
    layout_px: (f32, f32),
    /// The cursor is over the pill window (for hover states).
    cursor_inside: bool,
    /// Tab slide, size springs, hover fade.
    motion: crate::motion::UiMotion,
    /// Index of the hit under the cursor (hover fade restarts on change).
    hovered_hit: Option<usize>,
    /// The OS asks for reduced motion.
    reduced: bool,
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
        let reduced = crate::sys::prefers_reduced_motion();
        let now = Now::real();
        let notch = Notch::new(
            Duration::from_millis(u64::from(config.notch.animation_ms)),
            Duration::from_millis(u64::from(config.notch.collapse_delay_ms)),
            reduced,
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
            maximized_on: None,
            offset_read: None,
            last_anim: Instant::now(),
            settings: None,
            dialog: None,
            dialog_error: None,
            dialog_kind: DialogKind::Todo,
            #[cfg(feature = "mail")]
            mail: Default::default(),
            #[cfg(feature = "mail")]
            secrets: None,
            shot: None,
            mirror: None,
            #[cfg(feature = "camera")]
            capture: None,
            shot_at: None,
            cursor: (0.0, 0.0),
            press: None,
            drag: None,
            left_during_drag: false,
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
            inset: (0.0, 0.0, 0.0, 0.0),
            layout_px: (0.0, 0.0),
            cursor_inside: false,
            motion: crate::motion::UiMotion::new(reduced),
            hovered_hit: None,
            reduced,
        }
    }

    fn config(&self) -> &Config {
        self.model.config()
    }

    fn geo(&self) -> NotchGeometry {
        self.config().notch.geometry()
    }

    /// Geometry for this frame: the pill as wide as its content (within the
    /// configured width and 320 px) and the panel as tall as the active tab.
    fn live_geo(&mut self, scale: f32) -> NotchGeometry {
        let mut geo = self.geo();
        let min = geo.collapsed.0;
        let max = min.max(320);
        if let Some(r) = &mut self.renderer {
            // Lyrics change every few seconds: a steady, wide pill reads
            // better than one that resizes on every line.
            let want = if self.model.pill_shows_lyric() {
                max
            } else {
                r.pill_width(&self.model.collapsed(), scale).clamp(min, max)
            };
            self.motion
                .target_size(want as f32, self.model.panel_height_hint() as f32);
        } else {
            self.motion
                .target_size(min as f32, self.model.panel_height_hint() as f32);
        }
        let stripe = self.notch.is_stripe();
        let banner = self.model.has_banner() && !stripe;
        self.notch.set_banner(banner);
        if banner {
            self.motion.target_size(
                BANNER_SIZE.0.max(min as f32),
                self.model.panel_height_hint() as f32,
            );
        } else if stripe {
            // Something is in front: just a thin bar until hovered.
            self.motion
                .target_size(STRIPE_SIZE.0, self.model.panel_height_hint() as f32);
        }
        self.motion.target_pill_height(if banner {
            BANNER_SIZE.1
        } else if stripe {
            STRIPE_SIZE.1
        } else {
            geo.collapsed.1 as f32
        });
        geo.collapsed.0 = self.motion.width().round().max(1.0) as u32;
        geo.collapsed.1 = self.motion.pill_height().round().max(1.0) as u32;
        geo.expanded.1 = self.motion.height().round().max(1.0) as u32;
        geo
    }

    fn palette(&self) -> Palette {
        Palette::resolve(self.config().theme, true, false)
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
        let geo = self.live_geo(monitor.scale as f32);
        let pos = self.config().notch.position;
        let v = self.notch.visual(geo.collapsed, geo.expanded);
        let mut pill = geometry::notch_rect_sized(&monitor, pos, &geo, (v.width, v.height));
        let visible = v.visible && self.notch.is_visible();
        let hot = geometry::hot_zone_rect(&monitor, pos, &geo).rect;
        let Some(w) = &self.windows else { return };
        // While open, the window grows by a shadow margin on every side but
        // the screen edge it hangs from. Never while collapsed, so the pill
        // cannot steal clicks around itself.
        let scale = monitor.scale as f32;
        let m = if v.progress > 0.0 && !w.needs_opaque() {
            (12.0 * scale).round() as i32
        } else {
            0
        };
        let (l, t, r, b) = match pos.edge() {
            geometry::Edge::Top => (m, 0, m, m),
            geometry::Edge::Bottom => (m, m, m, 0),
            geometry::Edge::Left => (0, m, m, m),
            geometry::Edge::Right => (m, m, 0, m),
        };
        pill.x -= l;
        pill.y -= t;
        pill.w += (l + r) as u32;
        pill.h += (t + b) as u32;
        self.inset = (l as f32, t as f32, r as f32, b as f32);
        self.layout_px = (geo.expanded.0 as f32 * scale, geo.expanded.1 as f32 * scale);
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
        let view = self.model.view(now);
        let content_alpha = self.notch.visual((1, 1), (1, 1)).content_alpha;
        let (inset, layout_px) = (self.inset, self.layout_px);
        let cursor = self.cursor_inside.then_some(self.cursor);
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
        frame.content_alpha = content_alpha;
        frame.inset = inset;
        frame.layout = Some(layout_px);
        frame.shadow = if inset == (0.0, 0.0, 0.0, 0.0) {
            0.0
        } else {
            12.0
        };
        frame.cursor = cursor;
        frame.hover_alpha = self.motion.hover_alpha();
        if let Some(i) = view
            .tab
            .and_then(|t| view.tabs.iter().position(|x| *x == t))
        {
            self.motion.target_tab(i as f32);
            frame.tab_x = self.motion.tab_x();
        }
        if frame.opaque {
            let s = scale;
            let r = corner_radius.map(|r| r * s).unwrap_or(if progress >= 0.5 {
                perch_ui::tokens::radius::PANEL * s
            } else {
                h as f32 / 2.0
            });
            windows.clip_to_pill(w, h, r.round() as u32);
        }
        self.hits = renderer.draw(pm, &frame, &view);
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
        let shot_open = self.shot.is_some();
        let view_banner = self
            .model
            .view(Now::real())
            .banner
            .map(|b| b.title)
            .unwrap_or_default();
        let status = Status {
            shot_open,
            mixer_sessions: self
                .model
                .sessions()
                .iter()
                .map(|s| s.name.clone())
                .collect(),
            lyrics_loaded: self.model.has_lyrics(),
            banner: view_banner,
            mail_unread: self.model.mail().total_unread(),
            mail_accounts: self
                .model
                .mail()
                .accounts()
                .iter()
                .map(|a| format!("{}: {}", a.name, a.status.label()))
                .collect(),
            mail_latest: self
                .model
                .mail()
                .items()
                .first()
                .map(|(_, m)| format!("{} - {}", m.from, m.subject))
                .unwrap_or_default(),
            state: format!("{:?}", self.notch.state()),
            pill,
            visible,
            stripe: self.notch.is_stripe(),
            obscured: self.notch.is_obscured(),
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
        self.sync_mail();
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
                self.reduced,
            );
        }
        self.notch
            .set_reveal_delay(Duration::from_millis(u64::from(cfg.notch.reveal_delay_ms)));
        self.notch.set_pill_lock(cfg.notch.keep_pill);
        self.sync_obscured();
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
        let spec = forms::settings_spec(self.model.config(), &monitors, self.model.mail());
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
        let reduced = crate::sys::prefers_reduced_motion();
        if reduced != self.reduced {
            self.reduced = reduced;
            self.notch.set_reduced_motion(reduced);
            self.motion.set_reduced(reduced);
        }
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
                Effect::OpenPanel => {
                    if self.notch.handle(NotchEvent::Open) {
                        self.after_notch_change(el);
                    }
                }
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
                Effect::Screenshot => {
                    if self.shot.is_none() {
                        self.notch.handle(NotchEvent::ForceCollapse);
                        self.after_notch_change(el);
                        self.shot_at = Some(Instant::now() + Duration::from_millis(250));
                    }
                }
                Effect::CameraToggle => self.toggle_mirror(el),
                Effect::DictateToggle => {
                    crate::extras::dictate_toggle(&mut self.model, self.proxy.clone())
                }
                Effect::DictateRelease => crate::extras::dictate_release(&mut self.model),
                Effect::ToggleNotch => {
                    self.notch.handle(NotchEvent::Toggle);
                    self.after_notch_change(el);
                }
                Effect::PinToggle => {
                    // The lock: keep the pill (no stripe) over fullscreen and
                    // maximized apps. A setting, so it survives a restart.
                    let mut cfg = self.config().clone();
                    cfg.notch.keep_pill = !cfg.notch.keep_pill;
                    self.save_config(&cfg);
                    self.apply_config(cfg);
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
        let spec = forms::settings_spec(self.model.config(), &monitors, self.model.mail());
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
            Ok(w) => {
                self.dialog = Some(w);
                self.dialog_kind = DialogKind::Todo;
            }
            Err(e) => log::warn!("cannot open the to-do dialog: {e}"),
        }
    }

    /// Start or stop mail watchers so they match the config.
    fn sync_mail(&mut self) {
        #[cfg(feature = "mail")]
        {
            let active = self.model.is_active(Feature::Mail);
            if !active && self.mail.is_empty() {
                return; // nothing running, and no keychain needed
            }
            let secrets = self.secrets();
            let report = self.mail_report();
            let cfg = self.model.config().mail.clone();
            self.mail.sync(&cfg, active, &secrets, &report);
        }
    }

    #[cfg(feature = "mail")]
    fn secrets(&mut self) -> std::sync::Arc<dyn perch_core::platform::SecretStore> {
        self.secrets
            .get_or_insert_with(crate::backends::secrets)
            .clone()
    }

    #[cfg(feature = "mail")]
    fn mail_report(&self) -> crate::mail::Report {
        let proxy = std::sync::Mutex::new(self.proxy.clone());
        std::sync::Arc::new(move |u| {
            if let Ok(p) = proxy.lock() {
                let _ = p.send_event(UserEvent::Mail(u));
            }
        })
    }

    /// Reconnect every mail account (`perch mail-retry`, or after storing a
    /// new password).
    fn retry_mail(&mut self) {
        #[cfg(feature = "mail")]
        {
            self.mail = Default::default();
            self.sync_mail();
        }
    }

    fn open_mail_dialog(
        &mut self,
        el: &ActiveEventLoop,
        form: &forms::MailForm,
        error: Option<&str>,
    ) {
        let spec = forms::mail_account_spec(form, error);
        if self.dialog_kind == DialogKind::Mail {
            if let Some(d) = &mut self.dialog {
                d.replace(spec);
                d.focus();
                return;
            }
        }
        self.dialog = None;
        match FormWindow::open(el, spec, self.palette()) {
            Ok(w) => {
                self.dialog = Some(w);
                self.dialog_kind = DialogKind::Mail;
            }
            Err(e) => log::warn!("cannot open the mail dialog: {e}"),
        }
    }

    fn remove_mail_account(&mut self, index: usize) {
        let mut cfg = self.model.config().clone();
        if index >= cfg.mail.accounts.len() {
            return;
        }
        let account = cfg.mail.accounts.remove(index);
        #[cfg(feature = "mail")]
        if let Err(e) = self.secrets().delete(&account.secret_key()) {
            log::warn!("removing the stored mail password: {e}");
        }
        self.save_config(&cfg);
        self.apply_config(cfg);
        self.model
            .flash(format!("Removed {}", account.name), Now::real());
        self.request_redraw();
    }

    fn handle_mail_dialog_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) {
        let Some(win) = &mut self.dialog else { return };
        let events = win.handle(event);
        let form = forms::MailForm::from_spec(win.state().spec());
        for ev in events {
            match ev {
                perch_ui::form::FormEvent::Chosen(id, i) if id == "provider" => {
                    let f = form.clone().with_preset(i);
                    if let Some(w) = &mut self.dialog {
                        w.replace(forms::mail_account_spec(&f, None));
                    }
                }
                perch_ui::form::FormEvent::Button(id) if id == "add" => {
                    let parsed = forms::parse_mail_form(&form, &self.model.config().mail.accounts);
                    match parsed {
                        Ok((account, password)) => match self.save_mail_account(account, &password)
                        {
                            Ok(()) => {
                                self.dialog = None;
                                crate::sys::trim_heap();
                                return;
                            }
                            Err(msg) => self.open_mail_dialog(el, &form, Some(&msg)),
                        },
                        Err(msg) => self.open_mail_dialog(el, &form, Some(&msg)),
                    }
                }
                perch_ui::form::FormEvent::PasteRequested => self.paste_into_dialog(),
                perch_ui::form::FormEvent::Button(_) | perch_ui::form::FormEvent::Closed => {
                    self.dialog = None;
                    crate::sys::trim_heap();
                    return;
                }
                _ => {}
            }
        }
    }

    /// Ctrl+V / Shift+Insert in a dialog: paste the clipboard's text into the
    /// focused field. Read only on this explicit request, never recorded.
    fn paste_into_dialog(&mut self) {
        let text = match self.services.ensure_clipboard().map(|c| c.paste_text()) {
            Some(Ok(Some(t))) => t,
            Some(Err(e)) => {
                log::warn!("paste: {e}");
                return;
            }
            _ => return,
        };
        if let Some(w) = &mut self.dialog {
            w.paste(&text);
        }
    }

    /// Store the password in the keychain, then add the account (and turn the
    /// feature on) in the config.
    fn save_mail_account(
        &mut self,
        account: perch_core::config::MailAccount,
        password: &str,
    ) -> std::result::Result<(), String> {
        #[cfg(feature = "mail")]
        {
            let label = format!("Perch mail: {}", account.username);
            self.secrets()
                .set(&account.secret_key(), &label, password)
                .map_err(|e| format!("could not save the password: {e}"))?;
            let mut cfg = self.model.config().clone();
            let name = account.name.clone();
            cfg.mail.accounts.push(account);
            cfg.features.mail = true;
            self.save_config(&cfg);
            self.apply_config(cfg);
            self.model.flash(format!("Added {name}"), Now::real());
            self.request_redraw();
            Ok(())
        }
        #[cfg(not(feature = "mail"))]
        {
            let _ = (account, password);
            Err("this build has no mail support".into())
        }
    }

    /// Tell the notch whether something sits in front of it: a fullscreen
    /// app, or a maximized window on the notch's own monitor (and the
    /// setting is on). Returns true when that changed.
    fn sync_obscured(&mut self) -> bool {
        let on_ours = match (self.maximized_on, self.resolve_monitor()) {
            (Some(id), Some(m)) => id == m.id,
            _ => false,
        };
        let obscured = self.config().notch.hide_on_fullscreen && (self.fullscreen || on_ours);
        self.notch.handle(NotchEvent::ObscuredChanged(obscured))
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
            "mail-retry" => self.retry_mail(),
            "mail-add" => self.open_mail_dialog(el, &forms::MailForm::default(), None),
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
                if self.sync_obscured() {
                    self.after_notch_change(el);
                }
            }
            PlatformEvent::MaximizedChanged(on) => {
                self.maximized_on = on;
                if self.sync_obscured() {
                    self.after_notch_change(el);
                }
            }
            PlatformEvent::MonitorsChanged => {
                self.monitor = None;
                self.last_pill = None;
                self.last_hot = None;
                self.sync_obscured();
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
                    self.apply_config(*cfg);
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
                SettingsOutcome::AddMailAccount => {
                    self.open_mail_dialog(el, &forms::MailForm::default(), None);
                }
                SettingsOutcome::RemoveMailAccount(i) => self.remove_mail_account(i),
                SettingsOutcome::None => {}
            }
        }
        let _ = el;
    }

    /// Show or hide the camera mirror under the notch.
    fn toggle_mirror(&mut self, el: &ActiveEventLoop) {
        if self.mirror.is_some() {
            self.close_mirror();
            return;
        }
        #[cfg(feature = "camera")]
        {
            let Some(m) = self.resolve_monitor() else {
                return;
            };
            let s = m.scale.max(1.0);
            let (w, h) = ((320.0 * s) as u32, (240.0 * s) as u32);
            let pill = self.last_pill.map(|(r, _)| r).unwrap_or(m.full());
            let x = pill.center_x() - w as i32 / 2;
            let y = pill.y + pill.h as i32 + (8.0 * s) as i32;
            let slot: perch_ui::mirror::FrameSlot = Default::default();
            match perch_ui::mirror::MirrorWindow::open(el, slot.clone(), x, y, w, h) {
                Ok(win) => match crate::camera::start(slot, self.proxy.clone()) {
                    Ok(c) => {
                        self.capture = Some(c);
                        self.mirror = Some(win);
                    }
                    Err(e) => self.model.flash(e, Now::real()),
                },
                Err(e) => log::warn!("mirror window: {e}"),
            }
        }
        #[cfg(not(feature = "camera"))]
        {
            let _ = el;
            self.model
                .flash("Camera mirror is not in this build", Now::real());
        }
        self.request_redraw();
    }

    fn close_mirror(&mut self) {
        #[cfg(feature = "camera")]
        {
            self.capture = None; // joins the thread, frees the camera
        }
        self.mirror = None;
        crate::sys::trim_heap();
    }

    /// Capture the notch's monitor and open the selection overlay.
    fn open_screenshot(&mut self, el: &ActiveEventLoop) {
        let Some(m) = self.resolve_monitor() else {
            return;
        };
        let rect = m.full();
        let shot = crate::sys::capture(rect).and_then(|(rgba, w, h)| {
            let state = perch_ui::shot::ShotState::new(&rgba, w, h)
                .ok_or_else(|| perch_core::Error::backend("capture too large"))?;
            perch_ui::shot::ShotWindow::open(el, state, rect.x, rect.y)
        });
        match shot {
            Ok(w) => self.shot = Some(w),
            Err(e) => {
                log::warn!("screenshot failed: {e}");
                self.model.flash("Screenshot failed", Now::real());
                self.request_redraw();
            }
        }
    }

    fn handle_shot_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) {
        use perch_ui::shot::ShotEvent;
        let Some(win) = &mut self.shot else { return };
        let events = win.handle(event);
        let now = Now::real();
        // Every overlay event ends the screenshot; only the first matters.
        if let Some(ev) = events.into_iter().next() {
            let message = match ev {
                ShotEvent::Cancel => None,
                ShotEvent::Copy(rgba, w, h) => Some(match encode_png(&rgba, w, h) {
                    Some(png) => {
                        let content = perch_core::clipboard::ClipboardContent::image(
                            png,
                            w,
                            h,
                            perch_core::clipboard::ImageFormat::Png,
                        );
                        match self.services.ensure_clipboard().map(|c| c.write(&content)) {
                            Some(Ok(())) => "Screenshot copied".to_string(),
                            _ => "Could not copy the screenshot".to_string(),
                        }
                    }
                    None => "Could not encode the screenshot".to_string(),
                }),
                ShotEvent::Save(rgba, w, h) => Some(match save_shot(&rgba, w, h, now.unix) {
                    Ok(path) => format!(
                        "Saved {}",
                        path.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ),
                    Err(e) => {
                        log::warn!("saving the screenshot: {e}");
                        "Could not save the screenshot".to_string()
                    }
                }),
                ShotEvent::Ocr(rgba, w, h) => Some(match crate::sys::ocr(&rgba, w, h) {
                    Ok(text) if text.trim().is_empty() => "No text found".to_string(),
                    Ok(text) => {
                        let n = text.chars().count();
                        let content = perch_core::clipboard::ClipboardContent::text(text);
                        match self.services.ensure_clipboard().map(|c| c.write(&content)) {
                            Some(Ok(())) => format!("Copied {n} characters of text"),
                            _ => "Could not copy the text".to_string(),
                        }
                    }
                    Err(e) => {
                        log::warn!("OCR: {e}");
                        if e.is_degradable() {
                            "Text recognition is not available".to_string()
                        } else {
                            "Text recognition failed".to_string()
                        }
                    }
                }),
            };
            self.shot = None;
            crate::sys::trim_heap();
            if let Some(m) = message {
                self.model.flash(m, now);
            }
            self.request_redraw();
            self.schedule(el);
        }
    }

    fn handle_dialog_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) {
        if self.dialog_kind == DialogKind::Mail {
            self.handle_mail_dialog_event(el, event);
            return;
        }
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
                perch_ui::form::FormEvent::PasteRequested => self.paste_into_dialog(),
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
        if let Some(at) = self.shot_at {
            take(at.saturating_duration_since(now));
        }
        if let Some(at) = self.reload_at {
            take(at.saturating_duration_since(now));
        }
        if let Some(d) = self.motion.next_deadline() {
            take(d);
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
        self.sync_mail();
        self.load_usage_once();
        self.sync_hotkeys();
        self.services.start_tray(self.config().autostart);
        self.services.watch_config(self.store.path());
        self.sync_autostart();

        self.fullscreen = self.services.fullscreen.is_fullscreen_active();
        self.notch.set_reveal_delay(Duration::from_millis(u64::from(
            self.config().notch.reveal_delay_ms,
        )));
        self.notch.set_pill_lock(self.config().notch.keep_pill);
        self.sync_obscured();
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
            UserEvent::CameraFrame => {
                if let Some(m) = &self.mirror {
                    m.frame_ready();
                }
            }
            UserEvent::CameraStopped(e) => {
                log::warn!("camera: {e}");
                self.close_mirror();
                self.model.flash(e, Now::real());
                self.request_redraw();
            }
            UserEvent::Speech(SpeechMsg::Status(s)) => {
                self.model.flash(s, Now::real());
                self.request_redraw();
            }
            UserEvent::Speech(SpeechMsg::Text(t)) => {
                let words = t.split_whitespace().count();
                let content = perch_core::clipboard::ClipboardContent::text(t);
                let copied = matches!(
                    self.services.ensure_clipboard().map(|c| c.write(&content)),
                    Some(Ok(()))
                );
                let msg = if !copied {
                    "Could not put the text on the clipboard".to_string()
                } else if crate::sys::paste().is_ok() {
                    format!("Typed {words} words")
                } else {
                    format!("{words} words copied; paste with Ctrl+V")
                };
                self.model.flash(msg, Now::real());
                self.request_redraw();
            }
            UserEvent::Lyrics { key, lrc } => {
                let fx = self.model.lyrics_loaded(&key, lrc.as_deref());
                self.run_effects(el, fx);
            }
            #[cfg(feature = "mail")]
            UserEvent::Mail(update) => {
                let status = matches!(update, perch_core::mail::MailUpdate::Status { .. });
                let fx = self.model.mail_update(update);
                self.run_effects(el, fx);
                if status && self.settings.is_some() {
                    self.refresh_settings_form();
                }
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        log::trace!("window_event {id:?}: {event:?}");
        if self.settings.as_ref().is_some_and(|s| s.id() == id) {
            self.handle_settings_event(el, &event);
            return;
        }
        if self.mirror.as_ref().is_some_and(|m| m.id() == id) {
            let close = self.mirror.as_mut().is_some_and(|m| m.handle(&event));
            if close {
                self.close_mirror();
            }
            return;
        }
        if self.shot.as_ref().is_some_and(|s| s.id() == id) {
            self.handle_shot_event(el, &event);
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
                if id == pill_id {
                    self.cursor_inside = true;
                    self.left_during_drag = false;
                    self.model.banner_hover(true, Now::real());
                }
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
                if id == pill_id {
                    self.cursor_inside = false;
                    self.model.banner_hover(false, Now::real());
                    self.request_redraw();
                }
                // Crossing from the strip into the pill fires HotZoneLeft then
                // PointerEntered; the enter cancels the pending collapse, so
                // the grace period makes the hand-over seamless.
                let ev = if id == hot_id {
                    NotchEvent::HotZoneLeft
                } else {
                    NotchEvent::PointerLeft
                };
                self.press = None;
                if id == pill_id && self.drag.is_some() {
                    // Mid-drag: keep the panel open; the leave counts on release.
                    self.left_during_drag = true;
                    return;
                }
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
                let (cx, cy) = self.cursor;
                if let Some(drag) = self.drag.clone() {
                    let fx =
                        self.model
                            .drag_slider(drag.target.clone(), drag.value_at(cx), Now::real());
                    self.run_effects(el, fx);
                    return;
                }
                let hovered = self
                    .hits
                    .iter()
                    .position(|h| cx >= h.x && cy >= h.y && cx < h.x + h.w && cy < h.y + h.h);
                if hovered != self.hovered_hit {
                    self.hovered_hit = hovered;
                    self.motion.hover_changed();
                }
                if self.expanded() {
                    // Hover highlights follow the pointer.
                    self.request_redraw();
                }
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
                ElementState::Pressed => {
                    let (cx, cy) = self.cursor;
                    match perch_core::view::slider_at(&self.hits, cx, cy) {
                        Some(drag) if self.expanded() => {
                            // Sliders follow the pointer while the button is held.
                            let fx = self.model.drag_slider(
                                drag.target.clone(),
                                drag.value_at(cx),
                                Now::real(),
                            );
                            self.drag = Some(drag);
                            self.left_during_drag = false;
                            self.run_effects(el, fx);
                        }
                        _ => self.press = Some(self.cursor),
                    }
                }
                ElementState::Released => {
                    if let Some(drag) = self.drag.take() {
                        let fx = self.model.end_drag(
                            drag.target.clone(),
                            drag.value_at(self.cursor.0),
                            Now::real(),
                        );
                        self.run_effects(el, fx);
                        if std::mem::take(&mut self.left_during_drag)
                            && self.notch.handle(NotchEvent::PointerLeft)
                        {
                            self.after_notch_change(el);
                        }
                        return;
                    }
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
        if self.shot_at.is_some_and(|at| at <= now) {
            self.shot_at = None;
            self.open_screenshot(el);
        }
        if self.reload_at.is_some_and(|at| at <= now) {
            self.reload_at = None;
            self.reload_config();
        }
        // Animation advances at most once per frame. Window events can wake the
        // loop far more often (on X11 every resize comes back as a
        // ConfigureNotify), and stepping on each of them would turn an
        // animation into a busy loop that feeds itself.
        let dt = now.saturating_duration_since(self.last_anim);
        let animating = self.motion.next_deadline().is_some() || self.notch.wants_animation();
        if !animating || dt >= FRAME_INTERVAL {
            self.last_anim = now;
            let was_moving = self.motion.next_deadline().is_some();
            self.motion.step(dt);
            let was = self.notch.state();
            let needs_redraw = self.notch.advance(dt);
            if needs_redraw || self.notch.state() != was || was_moving {
                self.place();
                self.request_redraw();
            }
        }
        self.model
            // Banners wait while the notch is a locked stripe.
            .set_panel_state(self.expanded(), !self.notch.is_locked(), Now::real());
        if self
            .offset_read
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(60))
        {
            self.offset_read = Some(Instant::now());
            self.model.set_utc_offset(crate::sys::utc_offset_seconds());
        }
        let fx = self.model.tick(Now::real());
        let queued = std::mem::take(&mut self.queued);
        self.run_effects(el, fx.into_iter().chain(queued).collect());
        // Banners come and go with model time; keep the window sized to them.
        self.place();
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

/// Encode straight RGBA as PNG.
fn encode_png(rgba: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let enc = image::codecs::png::PngEncoder::new(&mut out);
    image::ImageEncoder::write_image(enc, rgba, w, h, image::ExtendedColorType::Rgba8).ok()?;
    Some(out)
}

/// Save a screenshot under the user's Pictures folder (`Pictures/Perch`), or
/// the data directory when there is no Pictures folder.
fn save_shot(rgba: &[u8], w: u32, h: u32, unix: u64) -> std::io::Result<PathBuf> {
    let dir = directories::UserDirs::new()
        .and_then(|u| u.picture_dir().map(|p| p.join("Perch")))
        .or_else(|| persist::data_dir().map(|d| d.join("screenshots")))
        .ok_or_else(|| std::io::Error::other("no folder to save into"))?;
    let png = encode_png(rgba, w, h).ok_or_else(|| std::io::Error::other("PNG encoding failed"))?;
    let path = dir.join(format!("perch-shot-{unix}.png"));
    persist::write_atomic(&path, &png)?;
    Ok(path)
}
