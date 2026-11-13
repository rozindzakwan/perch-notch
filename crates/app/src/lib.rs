#![forbid(unsafe_code)]
//! Perch's application layer: CLI, the event loop that wires the platform
//! backends to the notch state machine and the renderer, and the feature
//! coordinators.
//!
//! Everything OS-specific is reached through the traits in
//! `perch_core::platform`, so this crate contains no `unsafe` and no Win32 or
//! X11 types.

pub mod app;
pub mod cli;
pub mod features;
