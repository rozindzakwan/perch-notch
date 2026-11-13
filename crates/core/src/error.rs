//! Error types shared by every Perch crate.
//!
//! Runtime paths return [`Result`]; nothing in Perch is allowed to `unwrap()`
//! or `expect()` outside of tests.

use std::path::PathBuf;

/// Every fallible Perch operation returns this.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors raised by Perch's platform-neutral logic.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The config file could not be read or written.
    #[error("config i/o failed for {path}: {source}")]
    ConfigIo {
        /// File we were trying to touch.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// The config file exists but is not valid TOML / has invalid values.
    #[error("config at {path} is invalid: {message}")]
    ConfigInvalid {
        /// File that failed to parse.
        path: PathBuf,
        /// Human-readable reason.
        message: String,
    },

    /// A platform directory (config/data) could not be determined.
    #[error("could not determine the {kind} directory for this user")]
    NoDirectory {
        /// `"config"` or `"data"`.
        kind: &'static str,
    },

    /// An optional capability is not available; callers degrade gracefully.
    #[error("{capability} is unavailable: {reason}")]
    Unavailable {
        /// Capability name, e.g. `"media"`, `"microphone"`.
        capability: &'static str,
        /// Why it is unavailable.
        reason: String,
    },

    /// Generic I/O failure with context.
    #[error("{context}: {source}")]
    Io {
        /// What we were doing.
        context: String,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// A backend (OS API, D-Bus, SQLite, ...) reported a failure.
    #[error("{0}")]
    Backend(String),
}

impl Error {
    /// Build an [`Error::Unavailable`] for a degraded optional feature.
    pub fn unavailable(capability: &'static str, reason: impl Into<String>) -> Self {
        Self::Unavailable {
            capability,
            reason: reason.into(),
        }
    }

    /// Build an [`Error::Io`] carrying a human-readable context string.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    /// Build an [`Error::Backend`] from any displayable value.
    pub fn backend(msg: impl std::fmt::Display) -> Self {
        Self::Backend(msg.to_string())
    }

    /// True when the failure means "optional capability missing", so the caller
    /// should disable the feature rather than abort.
    pub fn is_degradable(&self) -> bool {
        matches!(self, Error::Unavailable { .. })
    }
}
