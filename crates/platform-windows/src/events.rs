//! The Windows event thread.
//!
//! One background thread owns a hidden top-level window and a set of
//! out-of-context WinEvent hooks, and turns OS notifications into
//! [`PlatformEvent`]s:
//!
//! | OS signal | Event |
//! |---|---|
//! | `EVENT_SYSTEM_FOREGROUND` | `FullscreenChanged`, `ActiveWindowChanged` |
//! | `EVENT_OBJECT_LOCATIONCHANGE` (foreground process only) | `FullscreenChanged` (F11, video players) |
//! | `WM_DISPLAYCHANGE`, `WM_SETTINGCHANGE(SPI_SETWORKAREA)` | `MonitorsChanged` |
//! | `WM_WTSSESSION_CHANGE` lock/unlock | `SessionLocked` |
//! | `WM_CLIPBOARDUPDATE` (only while the clipboard feature is on) | `ClipboardChanged` |
//!
//! Everything is delivered by the OS; the thread sleeps in `GetMessageW`
//! between notifications. Nothing polls.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use perch_core::error::{Error, Result};
use perch_core::geometry::Monitor;
use perch_core::platform::{EventSink, MonitorSource, PlatformEvent};
use windows::core::{w, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, RemoveClipboardFormatListener,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClassNameW,
    GetForegroundWindow, GetMessageW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
    IsZoomed, KillTimer, PeekMessageW, PostThreadMessageW, RegisterClassW, SetTimer,
    TranslateMessage, EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_FOREGROUND, HMENU, MSG,
    OBJID_WINDOW, PM_NOREMOVE, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_APP,
    WM_CLIPBOARDUPDATE, WM_DISPLAYCHANGE, WM_QUIT, WM_SETTINGCHANGE, WM_TIMER,
    WM_WTSSESSION_CHANGE, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::monitors::WindowsMonitors;

/// Re-evaluate flags (clipboard listener on/off) and the monitor list.
const WM_APP_REFRESH: u32 = WM_APP + 1;
const SPI_SETWORKAREA: usize = 0x002F;
const WTS_SESSION_LOCK: usize = 0x7;
const WTS_SESSION_UNLOCK: usize = 0x8;
/// One-shot timer id: re-check fullscreen shortly after a foreground change,
/// because the shell's "quiet" state lags behind the window that caused it.
const RECHECK_TIMER: usize = 1;
const RECHECK_MS: u32 = 1500;

/// Which optional event sources are wanted. Flip a flag and call
/// [`WindowsEvents::refresh`]; the thread picks it up without restarting.
#[derive(Debug, Default)]
pub struct EventFlags {
    /// Emit `ClipboardChanged` (registers a clipboard format listener).
    pub clipboard: AtomicBool,
    /// Emit `ActiveWindowChanged` (screen time).
    pub active_window: AtomicBool,
}

struct State {
    sink: EventSink,
    flags: Arc<EventFlags>,
    monitors: Vec<Monitor>,
    hwnd: HWND,
    last_fullscreen: Option<bool>,
    last_app: String,
    location_hook: Option<HWINEVENTHOOK>,
    location_pid: u32,
    clipboard_listening: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Handle to the running event thread. Dropping it stops the thread.
pub struct WindowsEvents {
    thread_id: u32,
    flags: Arc<EventFlags>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl WindowsEvents {
    /// Start the thread. Returns once its message queue exists, so
    /// [`WindowsEvents::refresh`] can never be lost.
    pub fn start(sink: EventSink, flags: Arc<EventFlags>) -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<u32, String>>();
        let thread_flags = flags.clone();
        let join = std::thread::Builder::new()
            .name("perch-win-events".into())
            .spawn(move || run(sink, thread_flags, tx))
            .map_err(|e| Error::io("spawning the Windows event thread", e))?;
        match rx.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                thread_id,
                flags,
                join: Some(join),
            }),
            Ok(Err(e)) => Err(Error::backend(e)),
            Err(_) => Err(Error::backend("Windows event thread died during startup")),
        }
    }

    /// The flags this thread reads.
    pub fn flags(&self) -> &Arc<EventFlags> {
        &self.flags
    }

    /// Ask the thread to re-read the flags and the monitor list.
    pub fn refresh(&self) {
        // SAFETY: posting a message to a thread id we created; failure only
        // means the thread already exited, which is harmless.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_APP_REFRESH, WPARAM(0), LPARAM(0)) };
    }
}

impl Drop for WindowsEvents {
    fn drop(&mut self) {
        // SAFETY: as in `refresh`; WM_QUIT makes GetMessageW return 0.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn run(
    sink: EventSink,
    flags: Arc<EventFlags>,
    ready: std::sync::mpsc::Sender<std::result::Result<u32, String>>,
) {
    // SAFETY: plain thread-local queries/initialisation with no pointers.
    let thread_id = unsafe { GetCurrentThreadId() };
    let mut msg = MSG::default();
    // Force the thread's message queue into existence before announcing the
    // thread id, so early PostThreadMessageW calls are not dropped.
    // SAFETY: `msg` is a valid out-param; PM_NOREMOVE leaves the queue intact.
    let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };

    let hwnd = match create_hidden_window() {
        Ok(h) => h,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };

    // SAFETY: `hwnd` is the window we just created on this thread.
    if let Err(e) = unsafe { WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) } {
        log::debug!("lock/unlock notifications unavailable: {e}");
    }

    // SAFETY: registering an out-of-context hook with a valid callback; the
    // hook is removed before the thread exits.
    let fg_hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    };
    if fg_hook.is_invalid() {
        log::warn!("SetWinEventHook(foreground) failed; fullscreen auto-hide will be late");
    }

    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            sink,
            flags,
            monitors: WindowsMonitors.monitors().unwrap_or_default(),
            hwnd,
            last_fullscreen: None,
            last_app: String::new(),
            location_hook: None,
            location_pid: 0,
            clipboard_listening: false,
        });
    });
    refresh_flags();
    // Report the initial foreground state.
    on_foreground_changed();

    let _ = ready.send(Ok(thread_id));

    loop {
        // SAFETY: standard message loop on this thread's queue.
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break; // WM_QUIT or error
        }
        if msg.hwnd.is_invalid() && msg.message == WM_APP_REFRESH {
            refresh_flags();
            with_state(|st| st.monitors = WindowsMonitors.monitors().unwrap_or_default());
            continue;
        }
        // SAFETY: dispatching a message we just received.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // Teardown.
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().take() {
            // SAFETY: undoing registrations made on this thread for `st.hwnd`.
            unsafe {
                if st.clipboard_listening {
                    let _ = RemoveClipboardFormatListener(st.hwnd);
                }
                if let Some(h) = st.location_hook {
                    let _ = UnhookWinEvent(h);
                }
                let _ = WTSUnRegisterSessionNotification(st.hwnd);
                let _ = DestroyWindow(st.hwnd);
            }
        }
    });
    if !fg_hook.is_invalid() {
        // SAFETY: the hook handle we registered above.
        let _ = unsafe { UnhookWinEvent(fg_hook) };
    }
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

fn create_hidden_window() -> Result<HWND> {
    let class = w!("PerchEvents");
    // SAFETY: GetModuleHandleW(None) returns this executable's module.
    let hinstance = unsafe { GetModuleHandleW(None) }
        .map_err(|e| Error::backend(format!("GetModuleHandleW: {e}")))?;
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance.into(),
        lpszClassName: class,
        ..Default::default()
    };
    // SAFETY: `wc` is fully initialised; registering twice (second event
    // thread) fails harmlessly and CreateWindowExW still finds the class.
    let _ = unsafe { RegisterClassW(&wc) };
    // A real (never shown) top-level window, not HWND_MESSAGE: only top-level
    // windows receive the WM_DISPLAYCHANGE / WM_SETTINGCHANGE broadcasts.
    // SAFETY: class registered above; all pointers are static or null.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class,
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            HMENU::default(),
            hinstance,
            None,
        )
    }
    .map_err(|e| Error::backend(format!("CreateWindowExW(PerchEvents): {e}")))?;
    Ok(hwnd)
}

fn emit(ev: PlatformEvent) {
    // Clone the sink out so the callback runs without the RefCell borrowed.
    let sink = with_state(|st| st.sink.clone());
    if let Some(sink) = sink {
        sink(ev);
    }
}

fn refresh_flags() {
    with_state(|st| {
        let want = st.flags.clipboard.load(Ordering::Relaxed);
        if want != st.clipboard_listening {
            // SAFETY: (un)registering our own window as a clipboard listener.
            let ok = unsafe {
                if want {
                    AddClipboardFormatListener(st.hwnd).is_ok()
                } else {
                    RemoveClipboardFormatListener(st.hwnd).is_ok()
                }
            };
            if ok {
                st.clipboard_listening = want;
            } else {
                log::warn!(
                    "clipboard listener could not be {}",
                    if want { "added" } else { "removed" }
                );
            }
        }
    });
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_DISPLAYCHANGE => {
            with_state(|st| st.monitors = WindowsMonitors.monitors().unwrap_or_default());
            emit(PlatformEvent::MonitorsChanged);
            LRESULT(0)
        }
        WM_SETTINGCHANGE if wparam.0 == SPI_SETWORKAREA => {
            with_state(|st| st.monitors = WindowsMonitors.monitors().unwrap_or_default());
            emit(PlatformEvent::MonitorsChanged);
            LRESULT(0)
        }
        WM_WTSSESSION_CHANGE => {
            match wparam.0 {
                WTS_SESSION_LOCK => emit(PlatformEvent::SessionLocked(true)),
                WTS_SESSION_UNLOCK => emit(PlatformEvent::SessionLocked(false)),
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLIPBOARDUPDATE => {
            emit(PlatformEvent::ClipboardChanged);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == RECHECK_TIMER => {
            // SAFETY: killing the one-shot timer we set on this window.
            let _ = unsafe { KillTimer(hwnd, RECHECK_TIMER) };
            check_fullscreen();
            LRESULT(0)
        }
        // SAFETY: default handling for everything else, same arguments.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    match event {
        EVENT_SYSTEM_FOREGROUND => on_foreground_changed(),
        EVENT_OBJECT_LOCATIONCHANGE => {
            // Only the foreground window's own frame matters (F11, a player
            // going fullscreen). Cursor and caret location changes are
            // filtered out here cheaply.
            if id_object != OBJID_WINDOW.0 {
                return;
            }
            // SAFETY: no arguments; returns a possibly-null handle.
            if hwnd != unsafe { GetForegroundWindow() } {
                return;
            }
            check_fullscreen();
        }
        _ => {}
    }
}

fn on_foreground_changed() {
    // SAFETY: no arguments.
    let fg = unsafe { GetForegroundWindow() };
    if log::log_enabled!(log::Level::Debug) && !fg.is_invalid() {
        let mut r = RECT::default();
        // SAFETY: `fg` from the OS; `r` a valid local.
        let _ = unsafe { GetWindowRect(fg, &mut r) };
        log::debug!(
            "foreground: class {:?} rect ({},{} to {},{})",
            class_name(fg),
            r.left,
            r.top,
            r.right,
            r.bottom
        );
    }
    rescope_location_hook(fg);
    check_fullscreen();
    if let Some(hwnd) = with_state(|st| st.hwnd) {
        // SAFETY: (re)arming a one-shot timer on our own window; it is killed
        // when it fires, so a long-lived foreground window costs one wakeup.
        unsafe { SetTimer(hwnd, RECHECK_TIMER, RECHECK_MS, None) };
    }
    let want_app = with_state(|st| st.flags.active_window.load(Ordering::Relaxed)).unwrap_or(false);
    if want_app && !fg.is_invalid() {
        let (app, title) = window_app_and_title(fg);
        let changed = with_state(|st| {
            if st.last_app != app {
                st.last_app = app.clone();
                true
            } else {
                false
            }
        })
        .unwrap_or(false);
        if changed {
            emit(PlatformEvent::ActiveWindowChanged { app, title });
        }
    }
}

/// Re-register the location-change hook for the new foreground process only,
/// so we are not woken by every window move on the desktop.
fn rescope_location_hook(fg: HWND) {
    let mut pid = 0u32;
    if !fg.is_invalid() {
        // SAFETY: `fg` is a window handle; `pid` a valid out-param.
        unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) };
    }
    // SAFETY: no arguments.
    let own = unsafe { GetCurrentProcessId() };
    with_state(|st| {
        if pid == st.location_pid {
            return;
        }
        if let Some(h) = st.location_hook.take() {
            // SAFETY: a hook handle we registered earlier.
            let _ = unsafe { UnhookWinEvent(h) };
        }
        st.location_pid = pid;
        if pid == 0 || pid == own {
            return;
        }
        // SAFETY: valid callback; scoped to one process id.
        let h = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_LOCATIONCHANGE,
                EVENT_OBJECT_LOCATIONCHANGE,
                None,
                Some(win_event_proc),
                pid,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if !h.is_invalid() {
            st.location_hook = Some(h);
        }
    });
}

fn check_fullscreen() {
    let monitors = with_state(|st| st.monitors.clone()).unwrap_or_default();
    let reason = fullscreen_reason(&monitors);
    let fs = reason.is_some();
    let changed = with_state(|st| {
        if st.last_fullscreen != Some(fs) {
            st.last_fullscreen = Some(fs);
            true
        } else {
            false
        }
    })
    .unwrap_or(false);
    if changed {
        match &reason {
            Some(r) => log::info!("fullscreen app detected: {r}"),
            None => log::info!("no fullscreen app in the foreground"),
        }
        emit(PlatformEvent::FullscreenChanged(fs));
    }
}

/// Windows that cover a whole monitor without being "a fullscreen app".
const NOT_FULLSCREEN_CLASSES: &[&str] = &[
    "Progman",       // the desktop
    "WorkerW",       // the desktop (wallpaper host)
    "Shell_TrayWnd", // taskbar
    "Shell_SecondaryTrayWnd",
    "PerchNotch",
    "PerchHotZone",
    "Windows.UI.Core.CoreWindow",      // Start menu / search overlays
    "XamlExplorerHostIslandWindow",    // Win11 Task View / Alt-Tab / snap overlays
    "MultitaskingViewFrame",           // Win10 Task View
    "TaskSwitcherWnd",                 // classic Alt-Tab
    "ForegroundStaging",               // transient shell staging window
    "Shell_InputSwitchTopLevelWindow", // input language switcher
    "LockScreenControllerProxyWindow", // lock screen
];

/// Fullscreen = the shell says so (D3D exclusive, presentation mode, busy),
/// or the foreground window covers a whole monitor and is not the desktop.
pub fn is_fullscreen_now(monitors: &[Monitor]) -> bool {
    fullscreen_reason(monitors).is_some()
}

/// Why the foreground counts as fullscreen, or `None` if it does not.
/// Logged on every change so a wrongly hidden notch is diagnosable.
pub fn fullscreen_reason(monitors: &[Monitor]) -> Option<String> {
    // SAFETY: single out-param, no other side effects.
    if let Ok(state) = unsafe { SHQueryUserNotificationState() } {
        // QUNS_BUSY is deliberately ignored: it is reported with no
        // fullscreen app in front at all (seen with overlay software
        // running), and it lingers after a fullscreen window closes.
        // Borderless fullscreen is caught by the geometry check below.
        if state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE {
            return Some(format!("shell notification state {}", state.0));
        }
    }
    // SAFETY: no arguments.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return None;
    }
    let class = class_name(hwnd);
    if NOT_FULLSCREEN_CLASSES.contains(&class.as_str()) {
        return None;
    }
    // A maximised window covers the monitor too when the taskbar auto-hides
    // (its frame even overhangs by a few pixels), but it is not fullscreen.
    // Real fullscreen (F11, games, players) is never "zoomed".
    // SAFETY: `hwnd` is a window handle from the OS; no other arguments.
    if unsafe { IsZoomed(hwnd) }.as_bool() {
        return None;
    }
    let mut rect = RECT::default();
    // SAFETY: `hwnd` from the OS; `rect` a valid local.
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return None;
    }
    let covers = monitors.iter().any(|m| {
        rect.left <= m.x
            && rect.top <= m.y
            && rect.right >= m.x + m.width as i32
            && rect.bottom >= m.y + m.height as i32
    });
    covers.then(|| {
        format!(
            "foreground window class {class:?} covers a monitor ({},{} to {},{})",
            rect.left, rect.top, rect.right, rect.bottom
        )
    })
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    // SAFETY: `buf` is a valid writable buffer of the stated length.
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Executable stem (e.g. `chrome`) and title of a window.
pub fn window_app_and_title(hwnd: HWND) -> (String, String) {
    let mut title = [0u16; 256];
    // SAFETY: valid buffer.
    let n = unsafe { GetWindowTextW(hwnd, &mut title) };
    let title = String::from_utf16_lossy(&title[..n.max(0) as usize]);

    let mut pid = 0u32;
    // SAFETY: valid out-param.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let app = process_name(pid).unwrap_or_else(|| "unknown".to_string());
    (app, title)
}

fn process_name(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    // SAFETY: opening a process for limited query access; closed below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 520];
    let mut len = buf.len() as u32;
    // SAFETY: `buf`/`len` describe a valid buffer; `handle` is open.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    // SAFETY: closing the handle we opened.
    let _ = unsafe { CloseHandle(handle) };
    ok.ok()?;
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    let stem = std::path::Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())?;
    Some(stem)
}
