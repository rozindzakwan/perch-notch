#![forbid(unsafe_code)]
//! Perch's UI: rendering (`tiny-skia` into a `softbuffer` surface) and the
//! per-OS window shell driven by `winit`.
//!
//! The crate itself forbids `unsafe`; the few OS style calls it needs are
//! delegated to `perch-platform-windows` / `perch-platform-linux`, which own
//! their `unsafe` blocks and document each one.

pub mod art;
pub mod form;
pub mod render;
pub mod shell;
pub mod text;
pub mod theme;

pub use form::{Field, FormEvent, FormSpec, FormState, FormWindow};
pub use render::{Collapsed, Frame, Renderer};
pub use shell::{NotchWindows, ShellEvent};
pub use theme::Palette;

/// Which renderer this build uses, reported in logs and `--version` output.
pub const RENDERER: &str = "tiny-skia/softbuffer";
