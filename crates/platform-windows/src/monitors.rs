//! Monitor enumeration with per-monitor DPI v2 awareness.

use perch_core::error::{Error, Result};
use perch_core::geometry::Monitor;
use perch_core::platform::MonitorSource;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    MONITORINFOF_PRIMARY,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, MDT_EFFECTIVE_DPI,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

/// Enumerates monitors via `EnumDisplayMonitors`.
#[derive(Debug, Default)]
pub struct WindowsMonitors;

impl WindowsMonitors {
    /// Opt the process into per-monitor DPI v2 before any window is created.
    ///
    /// Safe to call more than once; a failure means the manifest (or an earlier
    /// call) already set an awareness level, which is not an error for us.
    pub fn enable_per_monitor_dpi_v2() {
        // SAFETY: SetProcessDpiAwarenessContext takes a well-known constant and
        // only mutates this process's DPI awareness. It returns an error when
        // awareness was already set, which we deliberately ignore.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }
}

struct Collector {
    monitors: Vec<Monitor>,
    next_id: u32,
}

unsafe extern "system" fn enum_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: `data` is the `&mut Collector` we passed to EnumDisplayMonitors,
    // which lives for the whole duration of that call, and Windows invokes this
    // callback synchronously on the calling thread, so the reference is unique.
    let collector = unsafe { &mut *(data.0 as *mut Collector) };

    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: `hmon` is the handle Windows just handed us, and `info` is a
    // correctly sized MONITORINFOEXW whose cbSize we initialised above.
    let ok = unsafe { GetMonitorInfoW(hmon, &mut info.monitorInfo as *mut MONITORINFO) };
    if !ok.as_bool() {
        return TRUE; // skip this monitor, keep enumerating
    }

    let r = info.monitorInfo.rcMonitor;
    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    // SAFETY: both out-params are valid locals; GetDpiForMonitor only writes
    // them. A failure leaves our 96 dpi default in place.
    let _ = unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };

    collector.monitors.push(Monitor {
        id: collector.next_id,
        x: r.left,
        y: r.top,
        width: (r.right - r.left).max(0) as u32,
        height: (r.bottom - r.top).max(0) as u32,
        scale: f64::from(dpi_x.max(1)) / 96.0,
        primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
    });
    collector.next_id += 1;
    TRUE
}

impl MonitorSource for WindowsMonitors {
    fn monitors(&self) -> Result<Vec<Monitor>> {
        let mut collector = Collector {
            monitors: Vec::new(),
            next_id: 0,
        };
        // SAFETY: we pass a null HDC and null clip rect (enumerate everything),
        // our own callback, and a pointer to `collector`, which outlives this
        // synchronous call.
        let ok = unsafe {
            EnumDisplayMonitors(
                HDC::default(),
                None,
                Some(enum_proc),
                LPARAM(&mut collector as *mut Collector as isize),
            )
        };
        if !ok.as_bool() && collector.monitors.is_empty() {
            return Err(Error::unavailable(
                "monitors",
                "EnumDisplayMonitors reported no monitors",
            ));
        }
        Ok(collector.monitors)
    }
}

/// The monitor under the cursor, used to decide where the notch belongs when
/// the user has not pinned it to a specific display.
pub fn monitor_under_cursor() -> Result<Option<u32>> {
    let mut pt = POINT::default();
    // SAFETY: `pt` is a valid local; GetCursorPos only writes to it.
    let ok = unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) };
    if ok.is_err() {
        return Ok(None);
    }
    let monitors = WindowsMonitors.monitors()?;
    Ok(perch_core::geometry::monitor_at(&monitors, pt.x, pt.y).map(|m| m.id))
}

/// Whether the given window handle is currently the foreground window.
pub(crate) fn is_foreground(hwnd: HWND) -> bool {
    // SAFETY: GetForegroundWindow takes no arguments and returns a handle we
    // only compare; no dereference happens.
    let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    fg == hwnd
}
