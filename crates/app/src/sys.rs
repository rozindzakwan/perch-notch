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
