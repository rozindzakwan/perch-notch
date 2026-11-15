//! X11 window properties, fullscreen/active-window watching and monitor
//! enumeration for Cinnamon (Muffin) and other EWMH window managers.
//!
//! Everything here is event-driven: we select `PropertyChange` on the root
//! window and read `_NET_ACTIVE_WINDOW` / `_NET_WM_STATE` when a
//! `PropertyNotify` arrives. Nothing polls.

use perch_core::error::{Error, Result};
use perch_core::geometry::Monitor;
use perch_core::platform::MonitorSource;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as RandrExt;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as XprotoExt, EventMask, PropMode,
    Window,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperExt;

use crate::session::snap_scale;

/// The EWMH atoms Perch needs.
#[derive(Debug, Clone, Copy)]
pub struct Atoms {
    /// `_NET_WM_WINDOW_TYPE`
    pub wm_window_type: Atom,
    /// `_NET_WM_WINDOW_TYPE_DOCK`
    pub window_type_dock: Atom,
    /// `_NET_WM_WINDOW_TYPE_NOTIFICATION`
    pub window_type_notification: Atom,
    /// `_NET_WM_STATE`
    pub wm_state: Atom,
    /// `_NET_WM_STATE_ABOVE`
    pub state_above: Atom,
    /// `_NET_WM_STATE_STICKY`
    pub state_sticky: Atom,
    /// `_NET_WM_STATE_SKIP_TASKBAR`
    pub state_skip_taskbar: Atom,
    /// `_NET_WM_STATE_SKIP_PAGER`
    pub state_skip_pager: Atom,
    /// `_NET_WM_STATE_FULLSCREEN`
    pub state_fullscreen: Atom,
    /// `_NET_WM_STATE_MAXIMIZED_VERT`
    pub state_max_vert: Atom,
    /// `_NET_WM_STATE_MAXIMIZED_HORZ`
    pub state_max_horz: Atom,
    /// `_NET_ACTIVE_WINDOW`
    pub active_window: Atom,
    /// `_NET_WM_DESKTOP`
    pub wm_desktop: Atom,
    /// `_NET_WM_PID`
    pub wm_pid: Atom,
    /// `WM_CLASS`
    pub wm_class: Atom,
}

pub(crate) fn intern(conn: &RustConnection, name: &str) -> Result<Atom> {
    let cookie = conn
        .intern_atom(false, name.as_bytes())
        .map_err(|e| Error::backend(format!("intern_atom({name}): {e}")))?;
    let reply = cookie
        .reply()
        .map_err(|e| Error::backend(format!("intern_atom({name}) reply: {e}")))?;
    Ok(reply.atom)
}

impl Atoms {
    /// Intern every atom in one round trip batch.
    pub fn new(conn: &RustConnection) -> Result<Self> {
        Ok(Self {
            wm_window_type: intern(conn, "_NET_WM_WINDOW_TYPE")?,
            window_type_dock: intern(conn, "_NET_WM_WINDOW_TYPE_DOCK")?,
            window_type_notification: intern(conn, "_NET_WM_WINDOW_TYPE_NOTIFICATION")?,
            wm_state: intern(conn, "_NET_WM_STATE")?,
            state_above: intern(conn, "_NET_WM_STATE_ABOVE")?,
            state_sticky: intern(conn, "_NET_WM_STATE_STICKY")?,
            state_skip_taskbar: intern(conn, "_NET_WM_STATE_SKIP_TASKBAR")?,
            state_skip_pager: intern(conn, "_NET_WM_STATE_SKIP_PAGER")?,
            state_fullscreen: intern(conn, "_NET_WM_STATE_FULLSCREEN")?,
            state_max_vert: intern(conn, "_NET_WM_STATE_MAXIMIZED_VERT")?,
            state_max_horz: intern(conn, "_NET_WM_STATE_MAXIMIZED_HORZ")?,
            active_window: intern(conn, "_NET_ACTIVE_WINDOW")?,
            wm_desktop: intern(conn, "_NET_WM_DESKTOP")?,
            wm_pid: intern(conn, "_NET_WM_PID")?,
            wm_class: AtomEnum::WM_CLASS.into(),
        })
    }
}

/// Open a connection to the X server.
pub fn connect() -> Result<(RustConnection, usize)> {
    x11rb::connect(None).map_err(|e| Error::unavailable("x11", format!("cannot connect: {e}")))
}

/// Apply the dock/above/sticky/skip-taskbar/skip-pager property set to the
/// pill and the hover strip.
///
/// Notably we do **not** set `_NET_WM_STRUT(_PARTIAL)`: Perch floats over the
/// desktop and must never reserve space or push tiled windows around.
pub fn apply_notch_properties(pill: u32, hot: u32) -> Result<()> {
    let (conn, _screen) = connect()?;
    let atoms = Atoms::new(&conn)?;
    for win in [pill, hot] {
        set_window_type(&conn, &atoms, win)?;
        set_states(&conn, &atoms, win)?;
        set_all_desktops(&conn, &atoms, win)?;
    }
    conn.flush()
        .map_err(|e| Error::backend(format!("flush: {e}")))?;
    Ok(())
}

/// Mark a window as a dock.
///
/// Muffin honours `_NET_WM_WINDOW_TYPE_DOCK` for stacking and for keeping the
/// window undecorated without reserving space, which is what we want. We list
/// `_NOTIFICATION` as a secondary type so window managers that stack
/// notifications higher still place us above normal windows.
pub fn set_window_type(conn: &RustConnection, atoms: &Atoms, win: Window) -> Result<()> {
    let types = [atoms.window_type_dock, atoms.window_type_notification];
    conn.change_property32(
        PropMode::REPLACE,
        win,
        atoms.wm_window_type,
        AtomEnum::ATOM,
        &types,
    )
    .map_err(|e| Error::backend(format!("set _NET_WM_WINDOW_TYPE: {e}")))?;
    Ok(())
}

/// Set above + sticky + skip taskbar + skip pager in one property write.
pub fn set_states(conn: &RustConnection, atoms: &Atoms, win: Window) -> Result<()> {
    let states = [
        atoms.state_above,
        atoms.state_sticky,
        atoms.state_skip_taskbar,
        atoms.state_skip_pager,
    ];
    conn.change_property32(
        PropMode::REPLACE,
        win,
        atoms.wm_state,
        AtomEnum::ATOM,
        &states,
    )
    .map_err(|e| Error::backend(format!("set _NET_WM_STATE: {e}")))?;
    Ok(())
}

/// Put the window on every workspace (0xFFFFFFFF), so switching workspaces in
/// Cinnamon does not make the notch disappear.
pub fn set_all_desktops(conn: &RustConnection, atoms: &Atoms, win: Window) -> Result<()> {
    conn.change_property32(
        PropMode::REPLACE,
        win,
        atoms.wm_desktop,
        AtomEnum::CARDINAL,
        &[0xFFFF_FFFFu32],
    )
    .map_err(|e| Error::backend(format!("set _NET_WM_DESKTOP: {e}")))?;
    Ok(())
}

/// Subscribe to root-window property changes, which is how we learn about
/// active-window and fullscreen transitions without polling.
pub fn watch_root(conn: &RustConnection, screen: usize) -> Result<Window> {
    let root = conn
        .setup()
        .roots
        .get(screen)
        .map(|s| s.root)
        .ok_or_else(|| Error::backend("no such X screen"))?;
    conn.change_window_attributes(
        root,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )
    .map_err(|e| Error::backend(format!("selecting PropertyChange on the root: {e}")))?;
    conn.flush()
        .map_err(|e| Error::backend(format!("flush: {e}")))?;
    Ok(root)
}

/// Read `_NET_ACTIVE_WINDOW`.
pub fn active_window(conn: &RustConnection, root: Window, atoms: &Atoms) -> Result<Option<Window>> {
    let reply = conn
        .get_property(false, root, atoms.active_window, AtomEnum::WINDOW, 0, 1)
        .map_err(|e| Error::backend(format!("get _NET_ACTIVE_WINDOW: {e}")))?
        .reply()
        .map_err(|e| Error::backend(format!("get _NET_ACTIVE_WINDOW reply: {e}")))?;
    let first = reply.value32().and_then(|mut v| v.next());
    Ok(first.filter(|w| *w != 0))
}

/// Whether the given window is fullscreen, via `_NET_WM_STATE`.
pub fn is_fullscreen(conn: &RustConnection, win: Window, atoms: &Atoms) -> Result<bool> {
    let reply = conn
        .get_property(false, win, atoms.wm_state, AtomEnum::ATOM, 0, 32)
        .map_err(|e| Error::backend(format!("get _NET_WM_STATE: {e}")))?
        .reply()
        .map_err(|e| Error::backend(format!("get _NET_WM_STATE reply: {e}")))?;
    let fullscreen = reply
        .value32()
        .map(|v| v.into_iter().any(|a| a == atoms.state_fullscreen))
        .unwrap_or(false);
    Ok(fullscreen)
}

/// Whether a window is maximized both ways (EWMH `_NET_WM_STATE`).
pub fn is_maximized(conn: &RustConnection, win: Window, atoms: &Atoms) -> Result<bool> {
    let reply = conn
        .get_property(false, win, atoms.wm_state, AtomEnum::ATOM, 0, 32)
        .map_err(|e| Error::backend(format!("get _NET_WM_STATE: {e}")))?
        .reply()
        .map_err(|e| Error::backend(format!("get _NET_WM_STATE reply: {e}")))?;
    let states: Vec<u32> = reply.value32().map(|v| v.collect()).unwrap_or_default();
    Ok(states.contains(&atoms.state_max_vert) && states.contains(&atoms.state_max_horz))
}

/// The window's centre in root coordinates.
pub fn window_centre(conn: &RustConnection, win: Window, root: Window) -> Option<(i32, i32)> {
    let g = conn.get_geometry(win).ok()?.reply().ok()?;
    let t = conn
        .translate_coordinates(win, root, 0, 0)
        .ok()?
        .reply()
        .ok()?;
    Some((
        i32::from(t.dst_x) + i32::from(g.width) / 2,
        i32::from(t.dst_y) + i32::from(g.height) / 2,
    ))
}

/// The application name of a window, from `WM_CLASS`, for screen-time tracking.
pub fn window_app(conn: &RustConnection, win: Window, atoms: &Atoms) -> Result<String> {
    let reply = conn
        .get_property(false, win, atoms.wm_class, AtomEnum::STRING, 0, 256)
        .map_err(|e| Error::backend(format!("get WM_CLASS: {e}")))?
        .reply()
        .map_err(|e| Error::backend(format!("get WM_CLASS reply: {e}")))?;
    // WM_CLASS is "instance\0class\0"; the class (second field) is the stable
    // application identity, so prefer it and fall back to the instance.
    let text = String::from_utf8_lossy(&reply.value);
    let mut parts = text.split('\0').filter(|s| !s.is_empty());
    let instance = parts.next().unwrap_or_default().to_string();
    let class = parts.next().map(|s| s.to_string());
    Ok(class.filter(|c| !c.is_empty()).unwrap_or(instance))
}

/// Fullscreen detection for EWMH window managers (Muffin, Mutter, KWin, ...).
///
/// Reads `_NET_ACTIVE_WINDOW` then that window's `_NET_WM_STATE`, exactly the
/// signal the spec defines. The caller refreshes this on a `PropertyNotify` for
/// the root window, so there is no polling; `is_fullscreen_active` is also safe
/// to call directly because each call is two cheap round trips.
#[derive(Debug, Default)]
pub struct X11Fullscreen {
    monitors: Vec<Monitor>,
}

impl X11Fullscreen {
    /// Create a detector. The monitor list is used for the geometry
    /// cross-check that catches borderless-fullscreen windows which do not set
    /// `_NET_WM_STATE_FULLSCREEN` (some games, and Chromium's kiosk mode).
    pub fn new(monitors: Vec<Monitor>) -> Self {
        Self { monitors }
    }

    /// Replace the monitor list after a display change.
    pub fn set_monitors(&mut self, monitors: Vec<Monitor>) {
        self.monitors = monitors;
    }

    fn query(&self) -> Result<bool> {
        let (conn, screen) = connect()?;
        let atoms = Atoms::new(&conn)?;
        let root = conn
            .setup()
            .roots
            .get(screen)
            .map(|s| s.root)
            .ok_or_else(|| Error::backend("no such X screen"))?;
        let Some(active) = active_window(&conn, root, &atoms)? else {
            return Ok(false);
        };
        if is_fullscreen(&conn, active, &atoms)? {
            return Ok(true);
        }
        if self.monitors.is_empty() {
            return Ok(false);
        }
        let geom = conn
            .get_geometry(active)
            .map_err(|e| Error::backend(format!("get_geometry: {e}")))?
            .reply()
            .map_err(|e| Error::backend(format!("get_geometry reply: {e}")))?;
        let covers = self
            .monitors
            .iter()
            .any(|m| u32::from(geom.width) >= m.width && u32::from(geom.height) >= m.height);
        Ok(covers)
    }
}

impl perch_core::platform::FullscreenWatch for X11Fullscreen {
    fn is_fullscreen_active(&self) -> bool {
        match self.query() {
            Ok(v) => v,
            Err(e) => {
                // A transient X error must never hide the notch forever.
                log::debug!("fullscreen check failed, assuming not fullscreen: {e}");
                false
            }
        }
    }
}

/// Monitor enumeration via RandR, including per-monitor scale derived from the
/// physical dimensions RandR reports.
#[derive(Debug, Default)]
pub struct X11Monitors;

impl MonitorSource for X11Monitors {
    fn monitors(&self) -> Result<Vec<Monitor>> {
        let (conn, screen) = connect()?;
        let root = conn
            .setup()
            .roots
            .get(screen)
            .map(|s| s.root)
            .ok_or_else(|| Error::backend("no such X screen"))?;

        let reply = conn
            .randr_get_monitors(root, true)
            .map_err(|e| Error::unavailable("randr", format!("get_monitors: {e}")))?
            .reply()
            .map_err(|e| Error::unavailable("randr", format!("get_monitors reply: {e}")))?;

        let wa = crate::x11_watch::work_area(&conn, root);
        let desktop = desktop_scale(&conn, root);
        let mut out = Vec::with_capacity(reply.monitors.len());
        for (i, m) in reply.monitors.iter().enumerate() {
            // Cinnamon's user-chosen scaling (Xft.dpi / GDK_SCALE) wins; the
            // RandR heuristic below is only a fallback.
            // RandR reports physical size in millimetres; the ratio of pixels
            // to millimetres gives the real DPI, which is how we derive a HiDPI
            // scale factor without reading desktop settings.
            let scale = if let Some(s) = desktop {
                s
            } else if m.width_in_millimeters > 0 {
                let dpi = f64::from(m.width) / (f64::from(m.width_in_millimeters) / 25.4);
                // Snap to the usual steps so a 110-dpi laptop panel does not
                // get a weird 1.14 scale.
                snap_scale(dpi / 96.0)
            } else {
                1.0
            };
            out.push(Monitor {
                id: i as u32,
                x: i32::from(m.x),
                y: i32::from(m.y),
                width: u32::from(m.width),
                height: u32::from(m.height),
                scale,
                primary: m.primary,
                work: wa.and_then(|wa| {
                    crate::x11_watch::monitor_work_area(
                        (
                            i32::from(m.x),
                            i32::from(m.y),
                            u32::from(m.width),
                            u32::from(m.height),
                        ),
                        wa,
                    )
                }),
            });
        }
        if out.is_empty() {
            return Err(Error::unavailable("randr", "no monitors reported"));
        }
        Ok(out)
    }
}

pub use crate::x11_watch::{
    compositor_running, make_background_parent_relative, monitor_work_area, rounded_spans,
    set_rounded_shape, window_title, work_area, WatchFlags, X11Watcher,
};

/// Parse `Xft.dpi` out of an X resource database string (the root window's
/// `RESOURCE_MANAGER`), which is where Cinnamon publishes its scaling factor.
/// Pure.
pub fn parse_xft_dpi(resources: &str) -> Option<f64> {
    resources.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.trim() == "Xft.dpi" {
            value.trim().parse::<f64>().ok().filter(|d| *d > 0.0)
        } else {
            None
        }
    })
}

/// The desktop's scale factor: `PERCH_SCALE` or `GDK_SCALE` if set, else
/// `Xft.dpi / 96` from `RESOURCE_MANAGER`, snapped to usual steps.
fn desktop_scale(conn: &RustConnection, root: Window) -> Option<f64> {
    for var in ["PERCH_SCALE", "GDK_SCALE"] {
        if let Some(s) = std::env::var(var).ok().and_then(|v| v.parse::<f64>().ok()) {
            if s > 0.0 && s.is_finite() {
                return Some(s);
            }
        }
    }
    let reply = conn
        .get_property(
            false,
            root,
            AtomEnum::RESOURCE_MANAGER,
            AtomEnum::STRING,
            0,
            64 * 1024,
        )
        .ok()?
        .reply()
        .ok()?;
    let text = String::from_utf8_lossy(&reply.value);
    parse_xft_dpi(&text).map(|dpi| {
        let raw = dpi / 96.0;
        // Respect deliberate fractional scaling, but snap to quarter steps.
        ((raw * 4.0).round() / 4.0).clamp(1.0, 4.0)
    })
}
