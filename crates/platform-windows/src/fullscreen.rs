//! Fullscreen-app detection.
//!
//! Primary signal is `SHQueryUserNotificationState`, which is exactly what the
//! shell uses to decide whether a toast would be rude. It covers exclusive
//! fullscreen games, presentations and Do-Not-Disturb. We cross-check with a
//! foreground-window geometry test so borderless-fullscreen windows that do not
//! flip the shell state are still caught.

use perch_core::geometry::Monitor;
use perch_core::platform::FullscreenWatch;
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};

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

    /// The shell's own "should I stay quiet" state.
    fn shell_says_fullscreen() -> bool {
        // SAFETY: SHQueryUserNotificationState takes a single out-param and has
        // no other side effects. A failure is treated as "not fullscreen".
        match unsafe { SHQueryUserNotificationState() } {
            Ok(state) => {
                state == QUNS_RUNNING_D3D_FULL_SCREEN
                    || state == QUNS_PRESENTATION_MODE
                    || state == QUNS_BUSY
            }
            Err(_) => false,
        }
    }

    /// Catch borderless-fullscreen windows: foreground window covers a whole
    /// monitor exactly.
    fn foreground_covers_a_monitor(&self) -> bool {
        // SAFETY: GetForegroundWindow returns a handle (possibly null) that we
        // only pass straight back into GetWindowRect.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_invalid() {
            return false;
        }
        let mut rect = RECT::default();
        // SAFETY: `hwnd` came from the OS moments ago and `rect` is a valid
        // local that GetWindowRect only writes into.
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
            return false;
        }
        self.monitors.iter().any(|m| {
            rect.left <= m.x
                && rect.top <= m.y
                && rect.right >= m.x + m.width as i32
                && rect.bottom >= m.y + m.height as i32
        })
    }
}

impl FullscreenWatch for FullscreenDetector {
    fn is_fullscreen_active(&self) -> bool {
        Self::shell_says_fullscreen() || self.foreground_covers_a_monitor()
    }
}
