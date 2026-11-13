//! X11 shell helpers (compositor detection, XShape clipping, work area) and
//! the event watcher thread.
//!
//! The watcher selects `PropertyChange` on the root window (active window,
//! work area) and on the active window (`_NET_WM_STATE`), RandR screen-change
//! notifications, and XFixes selection notifications for CLIPBOARD while the
//! clipboard feature is on. It blocks in `wait_for_event`, so it costs nothing
//! between X events.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use perch_core::error::{Error, Result};
use perch_core::geometry::Rect;
use perch_core::platform::{EventSink, PlatformEvent};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as XprotoExt, EventMask, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::x11::{active_window, connect, intern, is_fullscreen, window_app, Atoms};

static SHELL_CONN: std::sync::OnceLock<Option<(RustConnection, usize)>> =
    std::sync::OnceLock::new();

/// A lazily opened connection reused for shell calls made on every animation
/// frame (XShape), so they cost one request instead of one connection.
pub(crate) fn shell_conn() -> Option<&'static (RustConnection, usize)> {
    SHELL_CONN
        .get_or_init(|| match x11rb::connect(None) {
            Ok(c) => Some(c),
            Err(e) => {
                log::debug!("no X11 connection for the shell: {e}");
                None
            }
        })
        .as_ref()
}

/// Whether a compositing manager is running (`_NET_WM_CM_S<screen>` has an
/// owner). Without one, ARGB windows render with black corners, so the shell
/// paints the pill opaque and clips it with XShape instead.
pub fn compositor_running() -> bool {
    if let Ok(v) = std::env::var("PERCH_ASSUME_COMPOSITOR") {
        return v == "1";
    }
    let Some((conn, screen)) = shell_conn() else {
        return false;
    };
    let Ok(atom) = intern(conn, &format!("_NET_WM_CM_S{screen}")) else {
        return false;
    };
    match conn.get_selection_owner(atom).map(|c| c.reply()) {
        Ok(Ok(r)) => r.owner != x11rb::NONE,
        _ => false,
    }
}

/// Horizontal spans `(y, height, inset)` approximating a rounded rectangle:
/// each span is a run of rows sharing the same left/right inset. Pure.
pub fn rounded_spans(w: u32, h: u32, r: u32) -> Vec<(u32, u32, u32)> {
    let r = r.min(w / 2).min(h / 2);
    let mut spans: Vec<(u32, u32, u32)> = Vec::new();
    for y in 0..h {
        let dy = if r == 0 {
            0.0
        } else if y < r {
            r as f32 - y as f32 - 0.5
        } else if y >= h - r {
            y as f32 - (h - r) as f32 + 0.5
        } else {
            0.0
        };
        let inset = if dy <= 0.0 {
            0
        } else {
            let rf = r as f32;
            (rf - (rf * rf - dy * dy).max(0.0).sqrt()).round() as u32
        };
        match spans.last_mut() {
            Some(last) if last.2 == inset && last.0 + last.1 == y => last.1 += 1,
            _ => spans.push((y, 1, inset)),
        }
    }
    spans
}

/// Clip a window to a rounded rectangle with the XShape extension (used when
/// no compositor is running, so the pill still has round ends).
pub fn set_rounded_shape(win: u32, w: u32, h: u32, r: u32) -> Result<()> {
    use x11rb::protocol::shape::{ConnectionExt as ShapeExt, SK, SO};
    use x11rb::protocol::xproto::{ClipOrdering, Rectangle};
    let (conn, _) = shell_conn().ok_or_else(|| Error::unavailable("x11", "no connection"))?;
    let rects: Vec<Rectangle> = rounded_spans(w, h, r)
        .into_iter()
        .filter(|(_, _, inset)| inset * 2 < w)
        .map(|(y, hh, inset)| Rectangle {
            x: inset as i16,
            y: y as i16,
            width: (w - 2 * inset) as u16,
            height: hh as u16,
        })
        .collect();
    conn.shape_rectangles(
        SO::SET,
        SK::BOUNDING,
        ClipOrdering::UNSORTED,
        win,
        0,
        0,
        &rects,
    )
    .map_err(|e| Error::backend(format!("XShape: {e}")))?;
    conn.flush()
        .map_err(|e| Error::backend(format!("flush: {e}")))?;
    Ok(())
}

/// `_NET_WORKAREA` for the current desktop, as `(x, y, w, h)`.
pub fn work_area(conn: &RustConnection, root: Window) -> Option<(i32, i32, u32, u32)> {
    let wa = intern(conn, "_NET_WORKAREA").ok()?;
    let cur = intern(conn, "_NET_CURRENT_DESKTOP").ok()?;
    let desktop = conn
        .get_property(false, root, cur, AtomEnum::CARDINAL, 0, 1)
        .ok()?
        .reply()
        .ok()?
        .value32()
        .and_then(|mut v| v.next())
        .unwrap_or(0);
    let reply = conn
        .get_property(false, root, wa, AtomEnum::CARDINAL, 0, 4 * 64)
        .ok()?
        .reply()
        .ok()?;
    let vals: Vec<u32> = reply.value32()?.collect();
    let i = (desktop as usize) * 4;
    let v = vals.get(i..i + 4).or_else(|| vals.get(0..4))?;
    Some((v[0] as i32, v[1] as i32, v[2], v[3]))
}

/// Intersect a monitor `(x, y, w, h)` with the desktop work area. Returns
/// `None` when the work area does not plausibly describe this monitor (a
/// multi-monitor layout where `_NET_WORKAREA` reflects another screen). Pure.
pub fn monitor_work_area(m: (i32, i32, u32, u32), wa: (i32, i32, u32, u32)) -> Option<Rect> {
    let x0 = m.0.max(wa.0);
    let y0 = m.1.max(wa.1);
    let x1 = (m.0 + m.2 as i32).min(wa.0 + wa.2 as i32);
    let y1 = (m.1 + m.3 as i32).min(wa.1 + wa.3 as i32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
    // A panel takes a strip, not half the screen.
    if u64::from(w) * u64::from(h) * 2 < u64::from(m.2) * u64::from(m.3) {
        return None;
    }
    Some(Rect { x: x0, y: y0, w, h })
}

/// Optional event sources; flip and call [`X11Watcher::refresh`].
#[derive(Debug, Default)]
pub struct WatchFlags {
    /// Emit `ClipboardChanged` (XFixes selection notifications on CLIPBOARD).
    pub clipboard: AtomicBool,
    /// Emit `ActiveWindowChanged` (screen time).
    pub active_window: AtomicBool,
}

const WAKE_REFRESH: u32 = 1;
const WAKE_STOP: u32 = 2;

/// Handle to the watcher thread. Dropping it stops the thread.
pub struct X11Watcher {
    conn: Arc<RustConnection>,
    wake: Window,
    tracked: Arc<std::sync::Mutex<Vec<Window>>>,
    join: Option<std::thread::JoinHandle<()>>,
}

struct WatchState {
    root: Window,
    atoms: Atoms,
    /// ICCCM `WM_STATE`: the WM sets it when it (re)manages a window, which
    /// is the moment keep-above & co. can be requested and will be honoured.
    wm_state_icccm: Atom,
    /// Perch's own windows whose WM-owned state we keep asserted.
    notch: Vec<Window>,
    tracked: Arc<std::sync::Mutex<Vec<Window>>>,
    wm_name: Atom,
    utf8: Atom,
    workarea: Atom,
    wm_check: Atom,
    clipboard: Atom,
    type_desktop: Atom,
    active: Option<Window>,
    last_fullscreen: Option<bool>,
    last_app: String,
    clipboard_on: bool,
}

impl X11Watcher {
    /// Connect and start the thread.
    pub fn start(sink: EventSink, flags: Arc<WatchFlags>) -> Result<Self> {
        use x11rb::protocol::randr::{ConnectionExt as RandrExt, NotifyMask};
        use x11rb::protocol::xfixes::ConnectionExt as XfixesExt;
        use x11rb::protocol::xproto::{CreateWindowAux, WindowClass};

        let (conn, screen) = connect()?;
        let conn = Arc::new(conn);
        let root = conn
            .setup()
            .roots
            .get(screen)
            .map(|s| s.root)
            .ok_or_else(|| Error::backend("no such X screen"))?;
        let atoms = Atoms::new(&conn)?;
        conn.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .map_err(|e| Error::backend(format!("select root events: {e}")))?;
        if let Err(e) = conn.randr_select_input(root, NotifyMask::SCREEN_CHANGE) {
            log::debug!("RandR change notifications unavailable: {e}");
        }
        // XFixes must be version-negotiated before any other XFixes request.
        let _ = conn.xfixes_query_version(5, 0).map(|c| c.reply());

        // A private InputOnly window that only receives our own wake
        // ClientMessages (refresh/stop) inside the blocking event loop.
        let wake = conn
            .generate_id()
            .map_err(|e| Error::backend(format!("generate_id: {e}")))?;
        conn.create_window(
            0,
            wake,
            root,
            -10,
            -10,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().override_redirect(1),
        )
        .map_err(|e| Error::backend(format!("create wake window: {e}")))?;

        let tracked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let st = WatchState {
            root,
            atoms,
            wm_state_icccm: intern(&conn, "WM_STATE")?,
            notch: Vec::new(),
            tracked: tracked.clone(),
            wm_name: intern(&conn, "_NET_WM_NAME")?,
            utf8: intern(&conn, "UTF8_STRING")?,
            workarea: intern(&conn, "_NET_WORKAREA")?,
            wm_check: intern(&conn, "_NET_SUPPORTING_WM_CHECK")?,
            clipboard: intern(&conn, "CLIPBOARD")?,
            type_desktop: intern(&conn, "_NET_WM_WINDOW_TYPE_DESKTOP")?,
            active: None,
            last_fullscreen: None,
            last_app: String::new(),
            clipboard_on: false,
        };
        conn.flush()
            .map_err(|e| Error::backend(format!("flush: {e}")))?;

        let thread_conn = conn.clone();
        let join = std::thread::Builder::new()
            .name("perch-x11-events".into())
            .spawn(move || watch_loop(thread_conn, st, wake, sink, flags))
            .map_err(|e| Error::io("spawning the X11 event thread", e))?;
        let me = Self {
            conn,
            wake,
            tracked,
            join: Some(join),
        };
        me.refresh();
        Ok(me)
    }

    /// Keep keep-above / sticky / skip-taskbar / skip-pager asserted on these
    /// windows: requested whenever the WM (re)manages them, which covers
    /// startup races, re-mapping after fullscreen, and WM restarts.
    pub fn track_notch_windows(&self, ids: &[u32]) {
        if let Ok(mut t) = self.tracked.lock() {
            for id in ids {
                if !t.contains(id) {
                    t.push(*id);
                }
            }
        }
        self.refresh();
    }

    fn poke(&self, code: u32) {
        use x11rb::protocol::xproto::ClientMessageEvent;
        let ev = ClientMessageEvent::new(32, self.wake, AtomEnum::CARDINAL, [code, 0, 0, 0, 0]);
        let _ = self
            .conn
            .send_event(false, self.wake, EventMask::NO_EVENT, ev);
        let _ = self.conn.flush();
    }

    /// Re-read the flags.
    pub fn refresh(&self) {
        self.poke(WAKE_REFRESH);
    }
}

impl Drop for X11Watcher {
    fn drop(&mut self) {
        self.poke(WAKE_STOP);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn watch_loop(
    conn: Arc<RustConnection>,
    mut st: WatchState,
    wake: Window,
    sink: EventSink,
    flags: Arc<WatchFlags>,
) {
    use x11rb::protocol::Event;

    on_active_changed(&conn, &mut st, &sink, &flags);
    loop {
        let ev = match conn.wait_for_event() {
            Ok(ev) => ev,
            Err(e) => {
                log::warn!("X11 event watcher stopped: {e}");
                return;
            }
        };
        match ev {
            Event::ClientMessage(m) if m.window == wake => {
                if m.data.as_data32()[0] == WAKE_STOP {
                    return;
                }
                apply_clipboard_flag(&conn, &mut st, flags.clipboard.load(Ordering::Relaxed));
                sync_tracked(&conn, &mut st);
                if flags.active_window.load(Ordering::Relaxed) {
                    st.last_app.clear();
                    on_active_changed(&conn, &mut st, &sink, &flags);
                }
            }
            Event::PropertyNotify(p) if p.window == st.root => {
                if p.atom == st.atoms.active_window {
                    on_active_changed(&conn, &mut st, &sink, &flags);
                } else if p.atom == st.workarea {
                    sink(PlatformEvent::MonitorsChanged);
                } else if p.atom == st.wm_check {
                    for w in st.notch.clone() {
                        ensure_notch_states(&conn, &st, w);
                    }
                    sink(PlatformEvent::WindowManagerChanged);
                }
            }
            Event::PropertyNotify(p)
                if st.notch.contains(&p.window) && p.atom == st.wm_state_icccm =>
            {
                ensure_notch_states(&conn, &st, p.window);
            }
            Event::PropertyNotify(p)
                if Some(p.window) == st.active && p.atom == st.atoms.wm_state =>
            {
                check_fullscreen(&conn, &mut st, &sink);
            }
            Event::RandrScreenChangeNotify(_) => sink(PlatformEvent::MonitorsChanged),
            Event::XfixesSelectionNotify(n) if n.selection == st.clipboard => {
                if flags.clipboard.load(Ordering::Relaxed) {
                    sink(PlatformEvent::ClipboardChanged);
                }
            }
            Event::Error(e) => log::trace!("X11 error (ignored): {e:?}"),
            _ => {}
        }
    }
}

fn apply_clipboard_flag(conn: &RustConnection, st: &mut WatchState, want: bool) {
    use x11rb::protocol::xfixes::{ConnectionExt as XfixesExt, SelectionEventMask};
    if want == st.clipboard_on {
        return;
    }
    let mask = if want {
        SelectionEventMask::SET_SELECTION_OWNER
            | SelectionEventMask::SELECTION_WINDOW_DESTROY
            | SelectionEventMask::SELECTION_CLIENT_CLOSE
    } else {
        SelectionEventMask::from(0u32)
    };
    match conn.xfixes_select_selection_input(st.root, st.clipboard, mask) {
        Ok(_) => {
            st.clipboard_on = want;
            let _ = conn.flush();
        }
        Err(e) => log::warn!("clipboard change notifications unavailable: {e}"),
    }
}

fn on_active_changed(
    conn: &RustConnection,
    st: &mut WatchState,
    sink: &EventSink,
    flags: &WatchFlags,
) {
    let active = active_window(conn, st.root, &st.atoms).ok().flatten();
    if active != st.active {
        st.active = active;
        if let Some(w) = active {
            // Watch the new active window's own state (F11, a video going
            // fullscreen). Event masks are per client, so this does not
            // disturb the application.
            let _ = conn.change_window_attributes(
                w,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            );
            let _ = conn.flush();
        }
    }
    check_fullscreen(conn, st, sink);
    if flags.active_window.load(Ordering::Relaxed) {
        if let Some(w) = st.active {
            let app = window_app(conn, w, &st.atoms).unwrap_or_default();
            if !app.is_empty() && app != st.last_app {
                st.last_app = app.clone();
                let title = window_title(conn, w, st.wm_name, st.utf8).unwrap_or_default();
                sink(PlatformEvent::ActiveWindowChanged { app, title });
            }
        }
    }
}

fn is_desktop_window(conn: &RustConnection, st: &WatchState, w: Window) -> bool {
    conn.get_property(false, w, st.atoms.wm_window_type, AtomEnum::ATOM, 0, 8)
        .ok()
        .and_then(|c| c.reply().ok())
        .and_then(|r| r.value32().map(|mut v| v.any(|a| a == st.type_desktop)))
        .unwrap_or(false)
}

fn check_fullscreen(conn: &RustConnection, st: &mut WatchState, sink: &EventSink) {
    // EWMH defines fullscreen as `_NET_WM_STATE_FULLSCREEN` on the window;
    // Muffin sets it for F11, video players and games. A maximised window is
    // not fullscreen, and the desktop window (Nemo's) covers the screen but is
    // never "a fullscreen app".
    let fs = match st.active {
        Some(w) if !is_desktop_window(conn, st, w) => {
            is_fullscreen(conn, w, &st.atoms).unwrap_or(false)
        }
        _ => false,
    };
    if st.last_fullscreen != Some(fs) {
        st.last_fullscreen = Some(fs);
        sink(PlatformEvent::FullscreenChanged(fs));
    }
}

/// `_NET_WM_NAME` (UTF-8), falling back to `WM_NAME`.
pub fn window_title(
    conn: &RustConnection,
    win: Window,
    net_wm_name: Atom,
    utf8: Atom,
) -> Option<String> {
    let r = conn
        .get_property(false, win, net_wm_name, utf8, 0, 256)
        .ok()?
        .reply()
        .ok()?;
    if !r.value.is_empty() {
        return Some(String::from_utf8_lossy(&r.value).into_owned());
    }
    let r = conn
        .get_property(false, win, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 256)
        .ok()?
        .reply()
        .ok()?;
    Some(String::from_utf8_lossy(&r.value).into_owned())
}

fn sync_tracked(conn: &RustConnection, st: &mut WatchState) {
    let wanted: Vec<Window> = st.tracked.lock().map(|t| t.clone()).unwrap_or_default();
    for w in wanted {
        if st.notch.contains(&w) {
            continue;
        }
        st.notch.push(w);
        let _ = conn.change_window_attributes(
            w,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        );
        let _ = conn.flush();
        // It may already be managed (WM_STATE set before we started
        // listening); requests are idempotent, so just ensure now too.
        ensure_notch_states(conn, st, w);
    }
}

/// If a managed notch window lacks any of its states, ask the WM for them.
fn ensure_notch_states(conn: &RustConnection, st: &WatchState, win: Window) {
    use x11rb::protocol::xproto::ClientMessageEvent;
    let managed = conn
        .get_property(false, win, st.wm_state_icccm, AtomEnum::ANY, 0, 2)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some_and(|r| !r.value.is_empty());
    if !managed {
        return; // not managed yet; WM_STATE's PropertyNotify will call us again
    }
    let have: Vec<Atom> = conn
        .get_property(false, win, st.atoms.wm_state, AtomEnum::ATOM, 0, 32)
        .ok()
        .and_then(|c| c.reply().ok())
        .and_then(|r| r.value32().map(|v| v.collect()))
        .unwrap_or_default();
    let a = &st.atoms;
    let wanted = [
        a.state_above,
        a.state_sticky,
        a.state_skip_taskbar,
        a.state_skip_pager,
    ];
    if wanted.iter().all(|w| have.contains(w)) {
        return;
    }
    const ADD: u32 = 1;
    let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
    for (x, y) in [
        (a.state_above, a.state_sticky),
        (a.state_skip_taskbar, a.state_skip_pager),
    ] {
        // [action, first, second, source indication (1 = application), 0]
        let ev = ClientMessageEvent::new(32, win, a.wm_state, [ADD, x, y, 1, 0]);
        let _ = conn.send_event(false, st.root, mask, ev);
    }
    let ev = ClientMessageEvent::new(32, win, a.wm_desktop, [0xFFFF_FFFF, 1, 0, 0, 0]);
    let _ = conn.send_event(false, st.root, mask, ev);
    let _ = conn.flush();
    log::debug!("requested keep-above/sticky/skip states for 0x{win:x}");
}
