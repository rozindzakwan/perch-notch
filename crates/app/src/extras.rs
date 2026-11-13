//! Hooks for the heavier, optional features (white noise, screen-time
//! persistence, screenshot, camera, dictation, hotkeys, shelf drag-out).
//!
//! Each hook is a no-op that logs when its feature is not compiled in, so the
//! shell can call them unconditionally and a slim build simply lacks the code.

use std::path::Path;

use perch_core::noise::NoiseMix;
use perch_core::usage::UsageTracker;
use winit::event_loop::ActiveEventLoop;

use crate::model::{Effect, Model, Now};

/// Apply a white-noise mix.
pub fn set_noise(mix: NoiseMix) {
    log::debug!(
        "white noise not compiled in; ignoring mix (audible: {})",
        mix.is_audible()
    );
}

/// Persist screen-time totals.
pub fn save_usage(_usage: &UsageTracker) {}

/// Start a screenshot.
pub fn screenshot(_el: &ActiveEventLoop, model: &mut Model) {
    model.flash("Screenshot is not in this build", Now::real());
}

/// Toggle the camera mirror.
pub fn camera_toggle(_el: &ActiveEventLoop, model: &mut Model) {
    model.flash("Camera mirror is not in this build", Now::real());
}

/// Toggle dictation.
pub fn dictate_toggle(model: &mut Model) {
    model.flash("Dictation is not in this build", Now::real());
}

/// A global hotkey fired.
pub fn hotkey(_model: &mut Model, id: &str, _pressed: bool, _now: Now) -> Vec<Effect> {
    log::debug!("hotkey {id} has no handler in this build");
    Vec::new()
}

/// Start dragging a shelf file out of the notch.
pub fn drag_out(path: &Path) {
    log::debug!(
        "drag-out not available; {} stays on the shelf",
        path.display()
    );
}

/// Flush anything that must survive exit.
pub fn shutdown(usage: &UsageTracker) {
    save_usage(usage);
}
