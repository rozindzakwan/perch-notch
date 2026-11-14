//! Linux backends for Perch.
//!
//! X11 (Cinnamon/Muffin) is the first-class path and is implemented here in
//! full. Wayland support is detected at runtime and falls back with a logged,
//! documented limitation rather than failing.
//!
//! `unsafe` is permitted in this crate only because `x11rb`'s
//! `allow-unsafe-code` feature is required for some of its helpers; Perch's own
//! code here contains no `unsafe` blocks.

#![cfg_attr(not(target_os = "linux"), allow(unused))]

#[cfg(target_os = "linux")]
pub mod autostart;
// Pure environment classification: compiled on every host so its unit tests
// run in CI on Windows as well as on Linux.
#[cfg(target_os = "linux")]
pub mod capture;
#[cfg(target_os = "linux")]
pub mod clipboard;
#[cfg(target_os = "linux")]
pub mod mpris;
#[cfg(target_os = "linux")]
pub mod pulse;
#[cfg(target_os = "linux")]
pub mod screensaver;
#[cfg(target_os = "linux")]
pub mod services;
pub mod session;
#[cfg(target_os = "linux")]
pub mod single_instance;
#[cfg(target_os = "linux")]
pub mod tray;
#[cfg(target_os = "linux")]
pub mod x11;
#[cfg(target_os = "linux")]
mod x11_watch;

#[cfg(target_os = "linux")]
pub use autostart::XdgAutostart;
pub use session::{detect_session, SessionKind};
#[cfg(target_os = "linux")]
pub use single_instance::UnixSocketInstance;

/// Platform marker, so this crate also compiles (empty) on other hosts.
pub const PLATFORM: &str = "linux";

/// Return freed heap memory to the OS (glibc `malloc_trim`). Called after a
/// large transient allocation goes away (closing the settings window), so
/// idle RSS reflects what Perch actually uses rather than glibc's cache.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn trim_heap() {
    extern "C" {
        fn malloc_trim(pad: usize) -> i32;
    }
    // SAFETY: `malloc_trim` is a thread-safe glibc function that only
    // releases free memory at the top of / inside the heap; it takes a plain
    // integer and touches no Rust-owned memory.
    unsafe {
        malloc_trim(0);
    }
}

/// No-op where the allocator is not glibc.
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn trim_heap() {}
