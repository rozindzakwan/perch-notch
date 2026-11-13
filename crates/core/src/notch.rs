//! The notch state machine: hover in, animate open, animate shut, hide for
//! fullscreen apps.
//!
//! This type is driven entirely by events plus an explicit elapsed-time
//! `advance`, so it is fully testable without a window system, and so the event
//! loop can sleep forever whenever [`Notch::next_deadline`] returns `None`.

use std::time::Duration;

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
    /// The foreground window became (or stopped being) fullscreen.
    FullscreenChanged(bool),
    /// Keep the notch open until toggled again (hotkey / CLI / tray).
    Toggle,
    /// Force the notch shut, e.g. on Escape.
    ForceCollapse,
}

/// What the renderer needs for the current frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Visual {
    /// Interpolated width in logical pixels.
    pub width: u32,
    /// Interpolated height in logical pixels.
    pub height: u32,
    /// Eased open-ness, 0.0 = collapsed, 1.0 = expanded.
    pub progress: f32,
    /// Whether to show the window at all.
    pub visible: bool,
}

/// Hover-driven notch with time-based animation.
#[derive(Debug)]
pub struct Notch {
    state: NotchState,
    /// Linear animation position, 0.0..=1.0.
    t: f32,
    expand_duration: Duration,
    collapse_duration: Duration,
    collapse_delay: Duration,
    /// Time left before a pending collapse starts; `None` = none pending.
    pending_collapse: Option<Duration>,
    pointer_inside: bool,
    fullscreen: bool,
    pinned: bool,
}

impl Notch {
    /// Create a notch with the given open animation duration and collapse delay.
    pub fn new(animation: Duration, collapse_delay: Duration) -> Self {
        Self {
            state: NotchState::Collapsed,
            t: 0.0,
            expand_duration: animation,
            collapse_duration: animation,
            collapse_delay,
            pending_collapse: None,
            pointer_inside: false,
            fullscreen: false,
            pinned: false,
        }
    }

    /// Current state.
    pub fn state(&self) -> NotchState {
        self.state
    }

    /// Eased open-ness for the current frame.
    pub fn progress(&self) -> f32 {
        ease_in_out(self.t)
    }

    /// Whether the window should be shown at all.
    pub fn is_visible(&self) -> bool {
        !self.fullscreen
    }

    /// Whether the notch is held open by a toggle.
    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    /// True while a frame is actually needed; false when idle so the event loop
    /// can block instead of spinning.
    pub fn wants_animation(&self) -> bool {
        self.is_visible() && matches!(self.state, NotchState::Expanding | NotchState::Collapsing)
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
        self.pending_collapse
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
        }
    }

    /// Feed in an event. Returns true when something observable changed.
    pub fn handle(&mut self, event: NotchEvent) -> bool {
        match event {
            NotchEvent::HotZoneEntered | NotchEvent::PointerEntered => {
                let was_inside = self.pointer_inside;
                self.pointer_inside = true;
                self.pending_collapse = None;
                if !self.is_visible() {
                    // Remember the hover; it is honored when fullscreen ends.
                    return !was_inside;
                }
                self.start_expanding()
            }
            NotchEvent::HotZoneLeft | NotchEvent::PointerLeft => {
                let was_inside = self.pointer_inside;
                self.pointer_inside = false;
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
            NotchEvent::FullscreenChanged(fs) => {
                if self.fullscreen == fs {
                    return false;
                }
                self.fullscreen = fs;
                if fs {
                    // Hidden: stop all animation work immediately.
                    self.state = NotchState::Collapsed;
                    self.t = 0.0;
                    self.pending_collapse = None;
                } else if self.pointer_inside || self.pinned {
                    self.start_expanding();
                }
                true
            }
            NotchEvent::Toggle => {
                self.pinned = !self.pinned;
                if self.pinned {
                    self.pending_collapse = None;
                    if !self.is_visible() {
                        return true;
                    }
                    self.start_expanding();
                } else if !self.pointer_inside {
                    self.start_collapsing();
                }
                true
            }
            NotchEvent::ForceCollapse => {
                self.pinned = false;
                self.pointer_inside = false;
                self.pending_collapse = None;
                self.start_collapsing()
            }
        }
    }

    fn start_expanding(&mut self) -> bool {
        match self.state {
            NotchState::Expanded => false,
            NotchState::Expanding => false,
            _ => {
                self.state = NotchState::Expanding;
                if self.expand_duration.is_zero() {
                    self.t = 1.0;
                    self.state = NotchState::Expanded;
                }
                true
            }
        }
    }

    fn start_collapsing(&mut self) -> bool {
        match self.state {
            NotchState::Collapsed => false,
            NotchState::Collapsing => false,
            _ => {
                self.state = NotchState::Collapsing;
                if self.collapse_duration.is_zero() {
                    self.t = 0.0;
                    self.state = NotchState::Collapsed;
                }
                true
            }
        }
    }

    /// Advance time. Returns true when a redraw is needed.
    pub fn advance(&mut self, dt: Duration) -> bool {
        if !self.is_visible() {
            return false;
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
                let step = ratio(dt, self.expand_duration);
                self.t = (self.t + step).min(1.0);
                if self.t >= 1.0 {
                    self.t = 1.0;
                    self.state = NotchState::Expanded;
                }
                true
            }
            NotchState::Collapsing => {
                let step = ratio(dt, self.collapse_duration);
                self.t = (self.t - step).max(0.0);
                if self.t <= 0.0 {
                    self.t = 0.0;
                    self.state = NotchState::Collapsed;
                }
                true
            }
            _ => false,
        }
    }
}

fn ratio(dt: Duration, total: Duration) -> f32 {
    if total.is_zero() {
        return 1.0;
    }
    (dt.as_secs_f32() / total.as_secs_f32()).clamp(0.0, 1.0)
}

/// Cubic ease-in-out, the standard "feels right" curve for this kind of UI.
fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let f = -2.0 * t + 2.0;
        1.0 - f * f * f / 2.0
    }
}
