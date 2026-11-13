//! The Windows window shell: borderless, transparent, topmost, non-activating
//! tool windows, hidden from the taskbar and Alt-Tab.
//!
//! These helpers take a raw window handle from `winit` and apply the style bits
//! `winit` does not expose. Every call is a style/position change on a window we
//! own.

use perch_core::error::{Error, Result};
use perch_core::geometry::Rect;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_EXCLUDED_FROM_PEEK};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};

/// Rebuild an `HWND` from the integer handle `raw-window-handle` gives us.
///
/// The caller must only pass a handle obtained from a live window it owns.
pub fn hwnd_from_raw(raw: isize) -> HWND {
    HWND(raw as *mut std::ffi::c_void)
}

/// Make a window a proper notch surface: no activation, no taskbar entry, no
/// Alt-Tab entry, always on top.
///
/// Returns the resulting extended style so callers (and the smoke test) can
/// verify the bits actually landed.
pub fn apply_notch_styles(hwnd: HWND) -> Result<isize> {
    if hwnd.is_invalid() {
        return Err(Error::backend("apply_notch_styles got an invalid HWND"));
    }
    // SAFETY: `hwnd` is a live window handle supplied by the caller;
    // Get/SetWindowLongPtrW only read and write that window's style bits.
    let style = unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        // WS_EX_APPWINDOW must be *cleared*: winit sets it, and it forces a
        // taskbar and Alt-Tab entry even alongside WS_EX_TOOLWINDOW.
        // WS_EX_LAYERED is deliberately absent. A layered window is
        // hit-tested per pixel against its alpha channel, so the hover strip
        // (which never paints anything, and whose pixels are therefore fully
        // transparent) would receive no WM_MOUSEMOVE at all and never fire
        // CursorEntered. Transparency comes from winit's `with_transparent`
        // via DWM composition instead, which keeps hit-testing intact.
        let wanted = (current & !(WS_EX_APPWINDOW.0 as isize))
            | WS_EX_TOOLWINDOW.0 as isize
            | WS_EX_NOACTIVATE.0 as isize
            | WS_EX_TOPMOST.0 as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
        GetWindowLongPtrW(hwnd, GWL_EXSTYLE)
    };

    // Keep the pill out of Aero Peek's "show desktop" preview.
    let excluded: u32 = 1;
    // SAFETY: `hwnd` is live, the attribute id is a documented constant, and we
    // pass a pointer to a correctly sized u32 with its length. Failure is
    // cosmetic, so it is ignored.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_EXCLUDED_FROM_PEEK,
            &excluded as *const u32 as *const std::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        );
    }

    Ok(style)
}

/// Additionally make a window click-through, for the invisible hover strip: it
/// must receive cursor enter/leave but never swallow a click meant for whatever
/// is underneath.
pub fn make_click_through(hwnd: HWND) -> Result<isize> {
    if hwnd.is_invalid() {
        return Err(Error::backend("make_click_through got an invalid HWND"));
    }
    // SAFETY: `hwnd` is a live window handle; we only add WS_EX_TRANSPARENT to
    // its existing extended style.
    let style = unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, current | WS_EX_TRANSPARENT.0 as isize);
        GetWindowLongPtrW(hwnd, GWL_EXSTYLE)
    };
    Ok(style)
}

/// Move/resize a window without activating it, keeping it topmost.
pub fn place_topmost(hwnd: HWND, rect: Rect, show: bool) -> Result<()> {
    if hwnd.is_invalid() {
        return Err(Error::backend("place_topmost got an invalid HWND"));
    }
    let mut flags = SWP_NOACTIVATE;
    if show {
        flags |= SWP_SHOWWINDOW;
    }
    // SAFETY: `hwnd` is a live window handle and the geometry values are plain
    // integers; SetWindowPos only repositions that window.
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            rect.x,
            rect.y,
            rect.w as i32,
            rect.h as i32,
            flags,
        )
    }
    .map_err(|e| Error::backend(format!("SetWindowPos: {e}")))
}

/// Re-assert topmost without moving the window. Some shells (and some games on
/// exit) steal the Z-order; this is the cheap fix, called on a foreground event
/// rather than on a timer.
pub fn reassert_topmost(hwnd: HWND) -> Result<()> {
    if hwnd.is_invalid() {
        return Err(Error::backend("reassert_topmost got an invalid HWND"));
    }
    // SAFETY: `hwnd` is live; SWP_NOMOVE | SWP_NOSIZE mean the position and
    // size arguments are ignored, so this only changes the Z-order band.
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        )
    }
    .map_err(|e| Error::backend(format!("SetWindowPos(topmost): {e}")))
}

/// Query the extended style bits of a window; used by tests and diagnostics.
pub fn ex_style(hwnd: HWND) -> isize {
    if hwnd.is_invalid() {
        return 0;
    }
    // SAFETY: `hwnd` is a live window handle; this only reads a style value.
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) }
}

/// Whether all the notch style bits are present *and* the taskbar/Alt-Tab bit
/// is absent. The UI smoke test asserts exactly this against the live window.
pub fn has_notch_styles(style: isize) -> bool {
    let want = WS_EX_TOOLWINDOW.0 as isize | WS_EX_NOACTIVATE.0 as isize | WS_EX_TOPMOST.0 as isize;
    style & want == want && style & WS_EX_APPWINDOW.0 as isize == 0
}
