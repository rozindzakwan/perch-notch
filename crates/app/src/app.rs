//! The winit application: owns the notch state machine, the windows and the
//! renderer, and translates OS events into notch events.
//!
//! Event-driven by construction. `ControlFlow::Wait` is the default; a timed
//! wakeup is only requested while something is actually animating or a timer is
//! pending, and the deadline comes from the state machine rather than a fixed
//! tick. When the notch is collapsed and nothing is scheduled, Perch receives
//! no wakeups at all.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use perch_core::config::{Config, ConfigStore};
use perch_core::geometry::{self, Monitor, NotchPosition};
use perch_core::notch::{Notch, NotchEvent, NotchState};
use perch_core::platform::{FullscreenWatch, MonitorSource};
use perch_ui::render::{Collapsed, Frame, Renderer};
use perch_ui::shell::NotchWindows;
use perch_ui::theme::Palette;
use tiny_skia::Pixmap;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowId;

use crate::features::{pill_status, ActiveFeatures, PillStatus};

/// Minimum interval between animation frames: ~120 fps, which is plenty for a
/// 180 ms pill animation and keeps a redraw storm impossible.
const FRAME_INTERVAL: Duration = Duration::from_millis(8);

/// Commands arriving from the tray, the CLI over IPC, or a hotkey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCommand {
    /// Toggle the notch.
    ToggleNotch,
    /// Reload the config from disk.
    ReloadConfig,
    /// Quit.
    Quit,
    /// Something not understood; logged, never fatal.
    Unknown(String),
}

impl AppCommand {
    /// Parse an IPC verb.
    pub fn parse(verb: &str) -> Self {
        match verb.trim() {
            "toggle-notch" | "toggle" => AppCommand::ToggleNotch,
            "reload" => AppCommand::ReloadConfig,
            "quit" | "exit" => AppCommand::Quit,
            other => AppCommand::Unknown(other.to_string()),
        }
    }
}

/// Runtime state the UI renders from, kept separate from the window plumbing
/// so it can be driven by tests.
#[derive(Debug)]
pub struct UiState {
    /// Collapsed-pill content.
    pub status: PillStatus,
    /// Whether a fullscreen app is hiding us.
    pub fullscreen: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            status: PillStatus::Idle,
            fullscreen: false,
        }
    }
}

/// The winit application handler.
pub struct PerchApp<M, F>
where
    M: MonitorSource,
    F: FullscreenWatch,
{
    config: Config,
    store: ConfigStore,
    features: ActiveFeatures,
    notch: Notch,
    monitors: M,
    fullscreen: F,
    windows: Option<NotchWindows>,
    renderer: Option<Renderer>,
    pixmap: Option<Pixmap>,
    monitor: Option<Monitor>,
    last_tick: Instant,
    ui: UiState,
    commands: Option<Receiver<String>>,
    /// Set when the loop should exit after the current iteration.
    quitting: bool,
    /// Counts presented frames; the smoke test asserts on it.
    frames: u64,
}

impl<M, F> PerchApp<M, F>
where
    M: MonitorSource,
    F: FullscreenWatch,
{
    /// Build the application state. Windows are created in `resumed`, which is
    /// where winit guarantees a usable display connection.
    pub fn new(config: Config, store: ConfigStore, monitors: M, fullscreen: F) -> Self {
        let animation = Duration::from_millis(u64::from(config.notch.animation_ms));
        let delay = Duration::from_millis(u64::from(config.notch.collapse_delay_ms));
        Self {
            features: ActiveFeatures::resolve(&config),
            notch: Notch::new(animation, delay),
            config,
            store,
            monitors,
            fullscreen,
            windows: None,
            renderer: None,
            pixmap: None,
            monitor: None,
            last_tick: Instant::now(),
            ui: UiState::default(),
            commands: None,
            quitting: false,
            frames: 0,
        }
    }

    /// Attach the IPC command channel.
    pub fn with_commands(mut self, rx: Receiver<String>) -> Self {
        self.commands = Some(rx);
        self
    }

    /// Frames presented so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Which features ended up live.
    pub fn features(&self) -> &ActiveFeatures {
        &self.features
    }

    fn palette(&self) -> Palette {
        // Perch does not read the OS light/dark preference yet; `System`
        // resolves to dark, which is documented in the README.
        Palette::resolve(self.config.theme, true)
    }

    fn position(&self) -> NotchPosition {
        self.config.notch.position
    }

    fn resolve_monitor(&mut self) -> Option<Monitor> {
        match self.monitors.monitor_for(self.config.notch.monitor) {
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

    /// Recompute and apply both window rectangles.
    fn replace_windows(&mut self) {
        let Some(monitor) = self.monitor.or_else(|| self.resolve_monitor()) else {
            return;
        };
        let geo = self.config.notch.geometry();
        let expanded = !matches!(self.notch.state(), NotchState::Collapsed);
        let pill = geometry::notch_rect(&monitor, self.position(), &geo, expanded);
        let hot = geometry::hot_zone_rect(&monitor, self.position(), &geo);
        let visible = self.notch.is_visible();

        if let Some(w) = &self.windows {
            if let Err(e) = w.place_pill(pill, visible) {
                log::warn!("placing the pill: {e}");
            }
            // The hover strip stays mapped even while the pill is hidden for a
            // fullscreen app, otherwise hovering would never wake us again.
            if let Err(e) = w.place_hot_zone(hot.rect, true) {
                log::warn!("placing the hot zone: {e}");
            }
        }
    }

    /// Animate and schedule the next wakeup. Returns the control flow to use.
    fn pump(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;

        let was = self.notch.state();
        let needs_redraw = self.notch.advance(dt);
        if self.notch.state() != was {
            log::debug!("notch {:?} -> {:?}", was, self.notch.state());
            self.replace_windows();
        }
        if needs_redraw {
            if let Some(w) = &self.windows {
                w.request_redraw();
            }
        }

        match self.notch.next_deadline() {
            Some(d) if self.notch.wants_animation() => {
                el.set_control_flow(ControlFlow::WaitUntil(now + d.max(FRAME_INTERVAL)));
            }
            Some(d) => el.set_control_flow(ControlFlow::WaitUntil(now + d)),
            None => el.set_control_flow(ControlFlow::Wait),
        }
    }

    fn drain_commands(&mut self, el: &ActiveEventLoop) {
        let verbs: Vec<String> = match &self.commands {
            Some(rx) => rx.try_iter().collect(),
            None => Vec::new(),
        };
        for verb in verbs {
            match AppCommand::parse(&verb) {
                AppCommand::ToggleNotch => {
                    self.notch.handle(NotchEvent::Toggle);
                    self.replace_windows();
                }
                AppCommand::ReloadConfig => self.reload_config(),
                AppCommand::Quit => {
                    self.quitting = true;
                    el.exit();
                }
                AppCommand::Unknown(v) => log::warn!("ignoring unknown command {v:?}"),
            }
        }
    }

    /// Re-read the config file and apply everything that can change live.
    pub fn reload_config(&mut self) {
        let (cfg, problem) = self.store.load_or_default();
        if let Some(p) = problem {
            log::warn!("config problem, using defaults where needed: {p}");
        }
        self.config = cfg;
        self.features = ActiveFeatures::resolve(&self.config);
        self.notch = Notch::new(
            Duration::from_millis(u64::from(self.config.notch.animation_ms)),
            Duration::from_millis(u64::from(self.config.notch.collapse_delay_ms)),
        );
        self.monitor = None;
        self.replace_windows();
        if let Some(w) = &self.windows {
            w.request_redraw();
        }
    }

    fn check_fullscreen(&mut self) {
        let fs = self.config.notch.hide_on_fullscreen && self.fullscreen.is_fullscreen_active();
        if fs != self.ui.fullscreen {
            self.ui.fullscreen = fs;
            if self.notch.handle(NotchEvent::FullscreenChanged(fs)) {
                self.replace_windows();
            }
        }
    }

    fn draw(&mut self) {
        // Everything that reads `self` immutably is computed up front, because
        // the window and the pixmap are borrowed mutably below.
        let palette = self.palette();
        let scale = self.monitor.map(|m| m.scale as f32).unwrap_or(1.0);
        let progress = self.notch.progress();
        let corner_radius = self.config.notch.corner_radius.map(|r| r as f32);
        let collapsed = match &self.ui.status {
            PillStatus::Idle => Collapsed::Idle,
            PillStatus::Countdown(s) => Collapsed::Status(s.clone()),
            PillStatus::NowPlaying(s) => Collapsed::NowPlaying(s.clone()),
        };

        let Some(windows) = &mut self.windows else {
            return;
        };
        let (w, h) = windows.pill_size();
        if w == 0 || h == 0 {
            return;
        }

        let need_new = match &self.pixmap {
            Some(pm) => pm.width() != w || pm.height() != h,
            None => true,
        };
        if need_new {
            self.pixmap = Pixmap::new(w, h);
        }
        let Some(pm) = &mut self.pixmap else {
            log::error!("could not allocate a {w}x{h} pixmap");
            return;
        };
        let Some(renderer) = &mut self.renderer else {
            return;
        };

        let mut frame = Frame::new(w, h, scale, palette);
        frame.progress = progress;
        frame.corner_radius = corner_radius;
        frame.collapsed = collapsed;

        if renderer.draw(pm, &frame) {
            if let Err(e) = windows.present(pm) {
                log::warn!("presenting a frame: {e}");
            } else {
                self.frames += 1;
            }
        }
    }
}

impl<M, F> ApplicationHandler for PerchApp<M, F>
where
    M: MonitorSource,
    F: FullscreenWatch,
{
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.windows.is_some() {
            return;
        }
        let Some(monitor) = self.resolve_monitor() else {
            log::error!("no monitors available; exiting instead of running invisibly");
            el.exit();
            return;
        };
        let geo = self.config.notch.geometry();
        let pill = geometry::notch_rect(&monitor, self.position(), &geo, false);
        let hot = geometry::hot_zone_rect(&monitor, self.position(), &geo);

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

        self.check_fullscreen();
        self.replace_windows();
        if let Some(w) = &self.windows {
            w.request_redraw();
        }
        el.set_control_flow(ControlFlow::Wait);
        log::info!(
            "Perch is up on monitor {} ({}x{} @{:.2}x); live features: {:?}",
            monitor.id,
            monitor.width,
            monitor.height,
            monitor.scale,
            self.features.active()
        );
        for missing in self.features.missing() {
            log::warn!(
                "{} is enabled in the config but not compiled into this build",
                missing.label()
            );
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let (pill_id, hot_id) = match &self.windows {
            Some(w) => (w.pill_id(), w.hot_id()),
            None => return,
        };

        match event {
            WindowEvent::CloseRequested => {
                // The pill has no close button; this only arrives from the WM.
                self.quitting = true;
                el.exit();
            }
            WindowEvent::CursorEntered { .. } => {
                let ev = if id == hot_id {
                    NotchEvent::HotZoneEntered
                } else if id == pill_id {
                    NotchEvent::PointerEntered
                } else {
                    return;
                };
                self.check_fullscreen();
                if self.notch.handle(ev) {
                    self.replace_windows();
                    self.pump(el);
                }
            }
            WindowEvent::CursorLeft { .. } => {
                // Both leaves must be honoured. Crossing from the strip into
                // the pill fires HotZoneLeft immediately followed by
                // PointerEntered, and the enter cancels the pending collapse,
                // so the grace period is what makes this seamless. Ignoring
                // the strip's leave left the notch stuck open whenever the
                // cursor exited sideways without ever touching the pill.
                let ev = if id == hot_id {
                    NotchEvent::HotZoneLeft
                } else if id == pill_id {
                    NotchEvent::PointerLeft
                } else {
                    return;
                };
                if self.notch.handle(ev) {
                    self.pump(el);
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if id == pill_id => {
                // Transport hit-testing lands here once the media feature is
                // wired; for now a click pins the notch open, which is the
                // behaviour users expect from the pill.
                self.notch.handle(NotchEvent::Toggle);
                self.replace_windows();
                self.pump(el);
            }
            WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Moved(_) => {
                self.monitor = None;
                self.resolve_monitor();
                self.replace_windows();
            }
            WindowEvent::RedrawRequested if id == pill_id => self.draw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.quitting {
            return;
        }
        self.drain_commands(el);
        self.check_fullscreen();
        self.pump(el);
    }
}

/// Decide what the collapsed pill shows from the live feature set. Exposed so
/// the integration tests can exercise the priority rules without a window.
pub fn collapsed_status(countdown: Option<String>, now_playing: Option<String>) -> PillStatus {
    pill_status(countdown, now_playing)
}
