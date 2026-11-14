//! UI motion that is not the notch's open/close: the tab highlight sliding,
//! the pill width and panel height following their content, and the hover
//! fade. Pure and stepped by explicit time, like `perch_core::notch`, so the
//! loop only wakes while something actually moves.

use std::time::Duration;

use perch_core::anim::Spring;

/// Tab highlight slide.
const TAB_SETTLE: Duration = Duration::from_millis(220);
/// Pill width / panel height changes.
const SIZE_SETTLE: Duration = Duration::from_millis(320);
/// Hover highlight fade-in.
const HOVER_FADE: Duration = Duration::from_millis(100);

/// Springs and fades for the panel's secondary motion.
#[derive(Debug, Clone)]
pub struct UiMotion {
    tab: Spring,
    width: Spring,
    height: Spring,
    pill_h: Spring,
    hover: f32,
    reduced: bool,
    started: (bool, bool, bool),
}

impl UiMotion {
    /// New, at rest. `reduced` = the OS asks for reduced motion.
    pub fn new(reduced: bool) -> Self {
        Self {
            tab: Spring::new(0.0),
            width: Spring::new(0.0),
            height: Spring::new(0.0),
            pill_h: Spring::new(0.0),
            hover: 1.0,
            reduced,
            started: (false, false, false),
        }
    }

    /// Follow the OS setting from now on.
    pub fn set_reduced(&mut self, on: bool) {
        self.reduced = on;
        if on {
            self.tab.snap(self.tab.target());
            self.width.snap(self.width.target());
            self.height.snap(self.height.target());
            self.pill_h.snap(self.pill_h.target());
            self.hover = 1.0;
        }
    }

    /// The active tab moved to `index` (the first target snaps).
    pub fn target_tab(&mut self, index: f32) {
        if !self.started.0 || self.reduced {
            self.tab.snap(index);
            self.started.0 = true;
        } else if (self.tab.target() - index).abs() > f32::EPSILON {
            self.tab.set_target(index);
        }
    }

    /// The content wants this pill width / panel height (logical px; the
    /// first target snaps).
    pub fn target_size(&mut self, width: f32, height: f32) {
        if !self.started.1 || self.reduced {
            self.width.snap(width);
            self.height.snap(height);
            self.started.1 = true;
            return;
        }
        if (self.width.target() - width).abs() > 0.5 {
            self.width.set_target(width);
        }
        if (self.height.target() - height).abs() > 0.5 {
            self.height.set_target(height);
        }
    }

    /// The collapsed pill wants this height (taller while a banner shows;
    /// the first target snaps).
    pub fn target_pill_height(&mut self, h: f32) {
        if !self.started.2 || self.reduced {
            self.pill_h.snap(h);
            self.started.2 = true;
        } else if (self.pill_h.target() - h).abs() > 0.5 {
            self.pill_h.set_target(h);
        }
    }

    /// Animated collapsed pill height.
    pub fn pill_height(&self) -> f32 {
        self.pill_h.value()
    }

    /// The hovered element changed: its highlight fades in from nothing.
    pub fn hover_changed(&mut self) {
        self.hover = if self.reduced { 1.0 } else { 0.0 };
    }

    /// Animated tab highlight position.
    pub fn tab_x(&self) -> f32 {
        self.tab.value()
    }

    /// Animated pill width.
    pub fn width(&self) -> f32 {
        self.width.value()
    }

    /// Animated panel height.
    pub fn height(&self) -> f32 {
        self.height.value()
    }

    /// Hover highlight opacity.
    pub fn hover_alpha(&self) -> f32 {
        self.hover
    }

    fn moving(&self) -> bool {
        !self.tab.settled()
            || !self.width.settled()
            || !self.height.settled()
            || !self.pill_h.settled()
            || self.hover < 1.0
    }

    /// Advance by `dt`. Returns true while anything still moves.
    pub fn step(&mut self, dt: Duration) -> bool {
        self.tab.step(dt, TAB_SETTLE);
        self.width.step(dt, SIZE_SETTLE);
        self.height.step(dt, SIZE_SETTLE);
        self.pill_h.step(dt, SIZE_SETTLE);
        if self.hover < 1.0 {
            self.hover = (self.hover + dt.as_secs_f32() / HOVER_FADE.as_secs_f32()).min(1.0);
        }
        self.moving()
    }

    /// Next frame while moving, `None` when settled (the loop can sleep).
    pub fn next_deadline(&self) -> Option<Duration> {
        self.moving().then_some(Duration::from_millis(8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn run(m: &mut UiMotion, ms: u64) {
        for _ in 0..ms / 8 {
            m.step(Duration::from_millis(8));
        }
    }

    #[test]
    fn ui_motion_has_no_deadline_when_settled() {
        let mut m = UiMotion::new(false);
        m.target_tab(0.0);
        m.target_size(200.0, 300.0);
        assert_eq!(
            m.next_deadline(),
            None,
            "first targets snap: nothing to animate"
        );
        m.target_tab(2.0);
        assert!(m.next_deadline().is_some());
        run(&mut m, 600);
        assert_eq!(m.next_deadline(), None, "settled again");
    }

    #[test]
    fn tab_switch_moves_highlight_and_settles_within_300ms() {
        let mut m = UiMotion::new(false);
        m.target_tab(0.0);
        m.target_tab(3.0);
        run(&mut m, 40);
        let mid = m.tab_x();
        assert!(mid > 0.0 && mid < 3.0, "sliding: {mid}");
        run(&mut m, 260);
        assert!((m.tab_x() - 3.0).abs() < 0.01, "arrived: {}", m.tab_x());
    }

    #[test]
    fn reduced_motion_snaps_tab_and_width() {
        let mut m = UiMotion::new(true);
        m.target_tab(0.0);
        m.target_size(200.0, 250.0);
        m.target_tab(2.0);
        m.target_size(280.0, 300.0);
        assert_eq!(m.tab_x(), 2.0);
        assert_eq!((m.width(), m.height()), (280.0, 300.0));
        assert_eq!(m.next_deadline(), None);
    }

    #[test]
    fn hover_fades_in_over_100ms_and_restarts_on_a_new_target() {
        let mut m = UiMotion::new(false);
        m.hover_changed();
        assert_eq!(m.hover_alpha(), 0.0);
        run(&mut m, 48);
        let half = m.hover_alpha();
        assert!(half > 0.2 && half < 0.8, "{half}");
        run(&mut m, 80);
        assert_eq!(m.hover_alpha(), 1.0);
        m.hover_changed();
        assert_eq!(m.hover_alpha(), 0.0, "a new row fades in from nothing");
    }
}
