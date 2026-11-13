//! Runtime session detection: X11, Wayland, or neither.
//!
//! Perch never assumes a session type at build time. `XDG_SESSION_TYPE` is the
//! primary signal, with `WAYLAND_DISPLAY` / `DISPLAY` as the fallback, matching
//! what desktop sessions actually set.

/// Which display server we are talking to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// X11, the fully supported path (Cinnamon's default session).
    X11,
    /// Wayland: best effort, documented limitations.
    Wayland,
    /// No graphical session was detected (e.g. a bare TTY or CI without xvfb).
    Headless,
}

/// Classify the current session from the environment.
///
/// Split out from [`detect_session`] so it can be tested without touching the
/// process environment.
pub fn classify(
    session_type: Option<&str>,
    wayland: Option<&str>,
    display: Option<&str>,
) -> SessionKind {
    let st = session_type.map(|s| s.trim().to_ascii_lowercase());
    match st.as_deref() {
        Some("x11") => return SessionKind::X11,
        Some("wayland") => return SessionKind::Wayland,
        _ => {}
    }
    if wayland.is_some_and(|w| !w.is_empty()) {
        return SessionKind::Wayland;
    }
    if display.is_some_and(|d| !d.is_empty()) {
        return SessionKind::X11;
    }
    SessionKind::Headless
}

/// Detect the session from this process's environment.
pub fn detect_session() -> SessionKind {
    let st = std::env::var("XDG_SESSION_TYPE").ok();
    let wl = std::env::var("WAYLAND_DISPLAY").ok();
    let dp = std::env::var("DISPLAY").ok();
    classify(st.as_deref(), wl.as_deref(), dp.as_deref())
}

/// Snap a computed DPI ratio to a sensible scale step.
///
/// RandR gives us pixels and millimetres, from which a raw ratio falls out.
/// Raw ratios are noisy (a 110 dpi panel yields 1.14), so we only honour a
/// non-unity scale on genuinely dense panels and then snap to a usual step.
pub fn snap_scale(raw: f64) -> f64 {
    const STEPS: [f64; 6] = [1.0, 1.25, 1.5, 1.75, 2.0, 3.0];
    if !raw.is_finite() || raw <= 0.0 {
        return 1.0;
    }
    if raw < 1.2 {
        return 1.0;
    }
    let mut best = 1.0;
    let mut best_d = f64::MAX;
    for s in STEPS {
        let d = (raw - s).abs();
        if d < best_d {
            best_d = d;
            best = s;
        }
    }
    best
}

/// Human-readable description of what is supported in this session, used in
/// logs and in the settings window.
pub fn support_note(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::X11 => "X11 session: full support (dock window, ARGB transparency, XInput2 hover, global hotkeys).",
        SessionKind::Wayland => "Wayland session: best effort. wlr-layer-shell is used where available (Sway, Hyprland, KDE); GNOME and Cinnamon-Wayland have no layer-shell, so the pill falls back to a normal top-level and global hotkeys need the XDG GlobalShortcuts portal.",
        SessionKind::Headless => "No graphical session detected; Perch will exit instead of starting invisibly.",
    }
}
