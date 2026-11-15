//! The notch state machine: hover in, animate open, animate shut, and rest as
//! a thin stripe over fullscreen and maximized apps (hover it for a few
//! seconds to open it).
//!
//! This type is driven entirely by events plus an explicit elapsed-time
//! `advance`, so it is fully testable without a window system, and so the event
//! loop can sleep forever whenever [`Notch::next_deadline`] returns `None`.

use std::time::Duration;

use crate::anim::{ease_out, Spring};

/// How long the pointer must rest on the stripe before the notch opens.
pub const DEFAULT_REVEAL_DELAY: Duration = Duration::from_secs(2);
/// How long the content takes to fade in once the panel is 40 % open.
const CONTENT_FADE: Duration = Duration::from_millis(140);
/// Content starts to appear at this open-ness.
const CONTENT_FROM: f32 = 0.4;

/// Where the notch currently is in its open/close cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotchState {
    /// Fully closed, showing only the small pill.
    Collapsed,
    /// Animating open.
    Expanding,
    /// Fully open.
    Expanded,
    /// Animating shut.
    Collapsing,
}

/// Inputs the state machine understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotchEvent {
    /// The cursor entered the invisible hover strip.
    HotZoneEntered,
    /// The cursor left the invisible hover strip. Treated as a pointer-leave,
    /// because when the pill is open the cursor may never have entered it.
    HotZoneLeft,
    /// The cursor entered the pill window itself.
    PointerEntered,
    /// The cursor left the pill.
    PointerLeft,
    /// A fullscreen app or a maximized window is (or stopped being) in front
    /// of the notch: rest as a stripe that opens only after a long hover.
    ObscuredChanged(bool),
    /// Keep the notch open until toggled again (hotkey / CLI / tray).
    Toggle,
    /// Force the notch shut, e.g. on Escape.
    ForceCollapse,
    /// Open the panel (a click on a banner); it closes again when the
    /// pointer leaves, like a hover.
    Open,
}

/// What the renderer needs for the current frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Visual {
    /// Interpolated width in logical pixels.
    pub width: u32,
    /// Interpolated height in logical pixels.
    pub height: u32,
    /// Open-ness, 0.0 = collapsed, 1.0 = expanded (briefly up to 1.04 while
    /// the open spring overshoots).
    pub progress: f32,
    /// Whether to show the window at all.
    pub visible: bool,
    /// Opacity of the panel content (fades in after 40 % open, hidden at
    /// once when closing).
    pub content_alpha: f32,
}

/// Hover-driven notch with time-based animation.
#[derive(Debug)]
pub struct Notch {
    state: NotchState,
    /// Current open-ness.
    p: f32,
    /// Drives opening.
    spring: Spring,
    /// Open-ness when the current collapse started, and time since.
    collapse_from: f32,
    collapse_elapsed: Duration,
    /// Content opacity.
    content: f32,
    /// OS asks for reduced motion: no springs or slides.
    reduced: bool,
    /// A banner is showing in the pill: pointing at it must not open the
    /// panel (it only pauses the banner).
    banner: bool,
    expand_duration: Duration,
    collapse_duration: Duration,
    collapse_delay: Duration,
    /// Time left before a pending collapse starts; `None` = none pending.
    pending_collapse: Option<Duration>,
    /// Pointer is over the hover strip.
    in_hot: bool,
    /// Pointer is over the pill.
    in_pill: bool,
    /// Something sits under the notch: rest as a stripe.
    obscured: bool,
    /// Time left before a hover on the stripe opens the notch.
    pending_reveal: Option<Duration>,
    reveal_delay: Duration,
    pinned: bool,
}

impl Notch {
    /// Create a notch. `animation` is the open spring's settle time; closing
    /// takes 56 % of it. `reduced_motion` makes every change instant.
    pub fn new(animation: Duration, collapse_delay: Duration, reduced_motion: bool) -> Self {
        Self {
            state: NotchState::Collapsed,
            p: 0.0,
            spring: Spring::new(0.0),
            collapse_from: 0.0,
            collapse_elapsed: Duration::ZERO,
            content: 0.0,
            reduced: reduced_motion,
            banner: false,
            expand_duration: animation,
            collapse_duration: animation.mul_f32(0.56),
            collapse_delay,
            pending_collapse: None,
            in_hot: false,
            in_pill: false,
            obscured: false,
            pending_reveal: None,
            reveal_delay: DEFAULT_REVEAL_DELAY,
            pinned: false,
        }
    }

    fn inside(&self) -> bool {
        self.in_hot || self.in_pill
    }

    /// Current state.
    pub fn state(&self) -> NotchState {
        self.state
    }

    /// Open-ness for the current frame.
    pub fn progress(&self) -> f32 {
        self.p
    }

    /// A banner is (or stopped being) shown in the pill.
    pub fn set_banner(&mut self, on: bool) {
        self.banner = on;
    }

    /// Follow the OS reduced-motion setting from now on.
    pub fn set_reduced_motion(&mut self, on: bool) {
        self.reduced = on;
    }

    fn instant(&self, d: Duration) -> bool {
        self.reduced || d.is_zero()
    }

    /// Whether the window should be shown at all (always: when something is
    /// in front it shrinks to a stripe instead of disappearing).
    pub fn is_visible(&self) -> bool {
        true
    }

    /// Resting as a stripe: something is in front and the notch is closed.
    pub fn is_stripe(&self) -> bool {
        self.obscured && self.state == NotchState::Collapsed
    }

    /// Something sits in front of the notch (banners wait meanwhile).
    pub fn is_obscured(&self) -> bool {
        self.obscured
    }

    /// How long a hover on the stripe takes to open the notch.
    pub fn set_reveal_delay(&mut self, d: Duration) {
        self.reveal_delay = d;
    }

    /// Whether the notch is held open by a toggle.
    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    /// True while a frame is actually needed; false when idle so the event loop
    /// can block instead of spinning.
    pub fn wants_animation(&self) -> bool {
        self.is_visible()
            && (matches!(self.state, NotchState::Expanding | NotchState::Collapsing)
                || (self.state == NotchState::Expanded && self.content < 1.0))
    }

    /// How long until the next required wakeup, or `None` when fully idle.
    pub fn next_deadline(&self) -> Option<Duration> {
        if !self.is_visible() {
            return None;
        }
        if self.wants_animation() {
            // Next animation frame; the UI layer picks the real frame interval.
            return Some(Duration::from_millis(8));
        }
        match (self.pending_collapse, self.pending_reveal) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Interpolated size for the renderer.
    pub fn visual(&self, collapsed: (u32, u32), expanded: (u32, u32)) -> Visual {
        let p = self.progress();
        let lerp = |a: u32, b: u32| -> u32 {
            let a = a as f32;
            let b = b as f32;
            (a + (b - a) * p).round().max(0.0) as u32
        };
        Visual {
            width: lerp(collapsed.0, expanded.0),
            height: lerp(collapsed.1, expanded.1),
            progress: p,
            visible: self.is_visible(),
            content_alpha: self.content,
        }
    }

    /// Feed in an event. Returns true when something observable changed.
    pub fn handle(&mut self, event: NotchEvent) -> bool {
        match event {
            NotchEvent::HotZoneEntered | NotchEvent::PointerEntered => {
                let was_inside = self.inside();
                if event == NotchEvent::HotZoneEntered {
                    self.in_hot = true;
                } else {
                    self.in_pill = true;
                }
                self.pending_collapse = None;
                if self.obscured && self.state == NotchState::Collapsed && !self.pinned {
                    // A stripe: open only after the pointer rests on it.
                    if !was_inside {
                        self.pending_reveal = Some(self.reveal_delay);
                    }
                    return !was_inside;
                }
                if self.banner && event == NotchEvent::PointerEntered && !self.in_hot {
                    // Over a banner: pause it, don't open the panel.
                    return !was_inside;
                }
                self.start_expanding()
            }
            NotchEvent::HotZoneLeft | NotchEvent::PointerLeft => {
                let was_inside = self.inside();
                if event == NotchEvent::HotZoneLeft {
                    self.in_hot = false;
                } else {
                    self.in_pill = false;
                }
                // Enter/leave pairs between the strip and the pill can arrive
                // in either order (Windows delivers the pill's enter before
                // the strip's leave). Only leaving *both* means leaving.
                if self.inside() {
                    return false;
                }
                self.pending_reveal = None;
                if self.pinned {
                    return was_inside;
                }
                match self.state {
                    NotchState::Collapsed => was_inside,
                    _ => {
                        let already = self.pending_collapse.is_some();
                        self.pending_collapse = Some(self.collapse_delay);
                        if self.collapse_delay.is_zero() {
                            return self.start_collapsing() || was_inside;
                        }
                        !already || was_inside
                    }
                }
            }
            NotchEvent::ObscuredChanged(on) => {
                if self.obscured == on {
                    return false;
                }
                self.obscured = on;
                self.pending_reveal = None;
                if on && !self.pinned && !self.inside() {
                    // Something came to the front: close down to the stripe.
                    self.pending_collapse = None;
                    self.start_collapsing();
                }
                true
            }
            NotchEvent::Toggle => {
                self.pinned = !self.pinned;
                if self.pinned {
                    self.pending_collapse = None;
                    self.pending_reveal = None;
                    self.start_expanding();
                } else if !self.inside() {
                    self.start_collapsing();
                }
                true
            }
            NotchEvent::Open => {
                self.pending_collapse = None;
                self.pending_reveal = None;
                self.start_expanding()
            }
            NotchEvent::ForceCollapse => {
                self.pinned = false;
                self.in_hot = false;
                self.in_pill = false;
                self.pending_collapse = None;
                self.pending_reveal = None;
                self.start_collapsing()
            }
        }
    }

    fn start_expanding(&mut self) -> bool {
        match self.state {
            NotchState::Expanded | NotchState::Expanding => false,
            _ => {
                if self.instant(self.expand_duration) {
                    self.p = 1.0;
                    self.spring.snap(1.0);
                    self.content = 1.0;
                    self.state = NotchState::Expanded;
                } else {
                    // Start from wherever we are, so reversing a collapse
                    // never jumps.
                    self.spring.snap(self.p);
                    self.spring.set_target(1.0);
                    self.state = NotchState::Expanding;
                }
                true
            }
        }
    }

    fn start_collapsing(&mut self) -> bool {
        match self.state {
            NotchState::Collapsed | NotchState::Collapsing => false,
            _ => {
                self.content = 0.0;
                if self.instant(self.collapse_duration) {
                    self.p = 0.0;
                    self.spring.snap(0.0);
                    self.state = NotchState::Collapsed;
                } else {
                    self.collapse_from = self.p;
                    self.collapse_elapsed = Duration::ZERO;
                    self.state = NotchState::Collapsing;
                }
                true
            }
        }
    }

    fn fade_content(&mut self, dt: Duration) {
        if self.p >= CONTENT_FROM {
            let step = dt.as_secs_f32() / CONTENT_FADE.as_secs_f32();
            self.content = (self.content + step).min(1.0);
        }
    }

    /// Advance time. Returns true when a redraw is needed.
    pub fn advance(&mut self, dt: Duration) -> bool {
        // A hover resting on the stripe opens the notch when its time is up.
        if let Some(left) = self.pending_reveal {
            if dt >= left {
                self.pending_reveal = None;
                if self.inside() {
                    self.start_expanding();
                }
            } else {
                self.pending_reveal = Some(left - dt);
            }
        }

        // Time is consumed by the pending-collapse delay first; only the
        // remainder drives the animation, otherwise a single long tick would
        // skip straight through the whole collapse.
        let mut dt = dt;
        if let Some(left) = self.pending_collapse {
            if dt >= left {
                dt -= left;
                self.pending_collapse = None;
                self.start_collapsing();
            } else {
                self.pending_collapse = Some(left - dt);
                dt = Duration::ZERO;
            }
        }

        if dt.is_zero() {
            return self.wants_animation();
        }

        match self.state {
            NotchState::Expanding => {
                let moving = self.spring.step(dt, self.expand_duration);
                self.p = self.spring.value();
                if !moving {
                    self.p = 1.0;
                    self.state = NotchState::Expanded;
                }
                self.fade_content(dt);
                true
            }
            NotchState::Expanded if self.content < 1.0 => {
                self.fade_content(dt);
                true
            }
            NotchState::Collapsing => {
                self.collapse_elapsed += dt;
                let t = self.collapse_elapsed.as_secs_f32() / self.collapse_duration.as_secs_f32();
                if t >= 1.0 {
                    self.p = 0.0;
                    self.spring.snap(0.0);
                    self.state = NotchState::Collapsed;
                } else {
                    self.p = self.collapse_from * (1.0 - ease_out(t));
                }
                true
            }
            _ => false,
        }
    }
}
