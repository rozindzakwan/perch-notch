#![forbid(unsafe_code)]
//! Platform-neutral logic for Perch.
//!
//! This crate contains **no** OS calls and **no** UI code. Every OS capability
//! is expressed as a trait in `platform`, with one implementation per OS living
//! in `perch-platform-windows` / `perch-platform-linux`, plus a fake
//! implementation here used by tests.

pub mod anim;
pub mod clipboard;
pub mod config;
pub mod error;
pub mod geometry;
pub mod hotkey;
pub mod lrc;
pub mod mail;
pub mod media;
pub mod mirror;
pub mod noise;
pub mod notch;
pub mod palette;
pub mod platform;
pub mod pomodoro;
pub mod reminders;
pub mod shelf;
pub mod todo;
pub mod tz;
pub mod usage;
pub mod view;

pub use error::{Error, Result};

/// Application identifier used for config dirs, single-instance IPC and autostart.
pub const APP_ID: &str = "perch";
/// Human-readable application name.
pub const APP_NAME: &str = "Perch";
