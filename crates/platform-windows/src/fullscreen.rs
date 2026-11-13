//! Fullscreen-app detection as a pull-style [`FullscreenWatch`].
//!
//! The push-style watcher lives in [`crate::events`]; this wrapper exists for
//! the initial state at startup and for callers that just want to ask. Both
//! use the same rules ([`crate::events::is_fullscreen_now`]): the shell's own
//! "quiet" state (exclusive D3D, presentation mode, busy) or a foreground
//! window that covers a whole monitor and is not the desktop or the taskbar.

use perch_core::geometry::Monitor;
use perch_core::platform::FullscreenWatch;

/// Reports whether a fullscreen app owns the foreground.
#[derive(Debug, Default)]
pub struct FullscreenDetector {
    monitors: Vec<Monitor>,
}

impl FullscreenDetector {
    /// Create a detector that compares against the given monitor layout.
    pub fn new(monitors: Vec<Monitor>) -> Self {
        Self { monitors }
    }

    /// Refresh the monitor layout after a display change.
    pub fn set_monitors(&mut self, monitors: Vec<Monitor>) {
        self.monitors = monitors;
    }
}

impl FullscreenWatch for FullscreenDetector {
    fn is_fullscreen_active(&self) -> bool {
        crate::events::is_fullscreen_now(&self.monitors)
    }
}
