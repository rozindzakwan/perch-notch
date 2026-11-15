//! Windows backends for Perch.
//!
//! This crate is one of only two places where `unsafe` is permitted (the Linux
//! X11 crate is the other), and every block carries a `// SAFETY:` comment.
//! Everything here implements a trait from `perch_core::platform`, so the rest
//! of the app never sees a Win32 type.

#![cfg_attr(not(windows), allow(unused))]

#[cfg(windows)]
mod autostart;
#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod clipboard;
#[cfg(windows)]
pub mod dragdrop;
#[cfg(windows)]
pub mod events;
#[cfg(windows)]
mod fullscreen;
#[cfg(windows)]
pub mod location;
#[cfg(windows)]
pub mod media;
#[cfg(windows)]
pub mod mixer;
#[cfg(windows)]
mod monitors;
#[cfg(windows)]
pub mod notifications;
#[cfg(windows)]
pub mod secrets;
#[cfg(windows)]
pub mod services;
#[cfg(windows)]
mod single_instance;
#[cfg(windows)]
pub mod window;

#[cfg(windows)]
pub use autostart::RegistryAutostart;
#[cfg(windows)]
pub use clipboard::WindowsClipboard;
#[cfg(windows)]
pub use events::{EventFlags, WindowsEvents};
#[cfg(windows)]
pub use fullscreen::FullscreenDetector;
#[cfg(windows)]
pub use media::SmtcMedia;
#[cfg(windows)]
pub use monitors::{monitor_under_cursor, WindowsMonitors};
#[cfg(windows)]
pub use services::{LastInputIdle, ShellOpener, ToastNotifier};
#[cfg(windows)]
pub use single_instance::NamedPipeInstance;

/// Platform marker, so this crate also compiles (empty) on other hosts and CI
/// can `cargo check` the whole workspace from Linux.
pub const PLATFORM: &str = "windows";
