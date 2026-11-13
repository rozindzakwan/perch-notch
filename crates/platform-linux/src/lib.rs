//! Linux backends for Perch (X11 primary, Wayland best-effort).
#![cfg_attr(not(target_os = "linux"), allow(unused))]

/// Crate-level marker so `cargo check` on non-Linux hosts still builds this crate.
pub const PLATFORM: &str = "linux";
