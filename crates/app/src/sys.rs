//! Tiny per-OS bridges that do not deserve a trait.

/// Local UTC offset in seconds (local = UTC + offset).
#[cfg(windows)]
pub fn utc_offset_seconds() -> i32 {
    perch_platform_windows::services::utc_offset_seconds()
}

/// Local UTC offset in seconds (local = UTC + offset).
#[cfg(target_os = "linux")]
pub fn utc_offset_seconds() -> i32 {
    perch_platform_linux::services::utc_offset_seconds()
}

/// Local UTC offset in seconds; UTC on other hosts.
#[cfg(not(any(windows, target_os = "linux")))]
pub fn utc_offset_seconds() -> i32 {
    0
}

/// Give freed heap back to the OS after a big transient allocation (a form
/// window closing). Only does anything on glibc.
pub fn trim_heap() {
    #[cfg(target_os = "linux")]
    perch_platform_linux::trim_heap();
}

/// Capture a screen rectangle as straight RGBA.
pub fn capture(rect: perch_core::geometry::Rect) -> perch_core::Result<(Vec<u8>, u32, u32)> {
    #[cfg(windows)]
    {
        perch_platform_windows::capture::capture(rect)
    }
    #[cfg(target_os = "linux")]
    {
        perch_platform_linux::capture::capture(rect)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = rect;
        Err(perch_core::Error::unavailable("screenshot", "no backend"))
    }
}

/// Recognise text in a straight-RGBA image.
pub fn ocr(rgba: &[u8], w: u32, h: u32) -> perch_core::Result<String> {
    #[cfg(windows)]
    {
        perch_platform_windows::capture::ocr(rgba, w, h)
    }
    #[cfg(target_os = "linux")]
    {
        perch_platform_linux::capture::ocr(rgba, w, h)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (rgba, w, h);
        Err(perch_core::Error::unavailable("ocr", "no backend"))
    }
}

/// Installed RAM in GiB (0 when unknown).
pub fn total_ram_gb() -> f64 {
    #[cfg(windows)]
    {
        perch_platform_windows::services::total_ram_bytes() as f64 / (1u64 << 30) as f64
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("MemTotal:"))
                    .and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok())
            })
            .map(|kb| kb / (1u64 << 20) as f64)
            .unwrap_or(0.0)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        0.0
    }
}

/// Paste into the focused window with a synthetic Ctrl+V (SendInput / XTEST).
pub fn paste() -> perch_core::Result<()> {
    #[cfg(windows)]
    {
        perch_platform_windows::services::send_paste()
    }
    #[cfg(target_os = "linux")]
    {
        perch_platform_linux::services::send_paste()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Err(perch_core::Error::unavailable("paste", "no backend"))
    }
}

/// The OS asks apps to reduce motion (read at start and on config reload).
pub fn prefers_reduced_motion() -> bool {
    #[cfg(windows)]
    {
        perch_platform_windows::services::prefers_reduced_motion()
    }
    #[cfg(target_os = "linux")]
    {
        perch_platform_linux::services::prefers_reduced_motion()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}
