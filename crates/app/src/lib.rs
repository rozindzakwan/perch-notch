#![forbid(unsafe_code)]
//! Perch's application layer: CLI, the model that holds every feature's
//! state, the winit shell that wires the platform backends to the notch, and
//! the feature coordinators.
//!
//! Everything OS-specific is reached through the traits in
//! `perch_core::platform` (wired up in [`glue`]), so this crate contains no
//! `unsafe` and no Win32 or X11 types.

pub mod backends;
#[cfg(feature = "camera")]
pub mod camera;
pub mod cli;
pub mod extras;
pub mod features;
pub mod forms;
pub mod glue;
pub mod icon;
pub mod lyrics;
#[cfg(feature = "mail")]
pub mod mail;
pub mod model;
pub mod motion;
pub mod persist;
pub mod shell;
#[cfg(feature = "speech")]
pub mod speech;
pub mod sys;
#[cfg(windows)]
pub mod tray;
