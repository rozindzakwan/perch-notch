//! Windows backends for Perch.
#![cfg_attr(not(windows), allow(unused))]

/// Crate-level marker so `cargo check` on non-Windows hosts still builds this crate.
pub const PLATFORM: &str = "windows";
