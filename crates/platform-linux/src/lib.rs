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
pub mod session;
#[cfg(target_os = "linux")]
pub mod single_instance;
#[cfg(target_os = "linux")]
pub mod x11;

#[cfg(target_os = "linux")]
pub use autostart::XdgAutostart;
pub use session::{detect_session, SessionKind};
#[cfg(target_os = "linux")]
pub use single_instance::UnixSocketInstance;

/// Platform marker, so this crate also compiles (empty) on other hosts.
pub const PLATFORM: &str = "linux";
