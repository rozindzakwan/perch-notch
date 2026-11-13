//! Optional feature backends (media, clipboard, mixer). Each constructor is
//! only called when its feature is live, so a disabled feature never starts
//! a thread or opens a device.

use perch_core::error::{Error, Result};
use perch_core::platform::{AudioMixer, ClipboardAccess, EventSink, MediaControl};

/// Clipboard read/write.
pub fn clipboard() -> Result<Box<dyn ClipboardAccess>> {
    Err(Error::unavailable(
        "clipboard",
        "not implemented in this build yet",
    ))
}

/// Now-playing (SMTC / MPRIS).
pub fn media(_sink: EventSink) -> Result<Box<dyn MediaControl>> {
    Err(Error::unavailable(
        "media",
        "not implemented in this build yet",
    ))
}

/// Per-app volume.
pub fn mixer(_sink: EventSink) -> Result<Box<dyn AudioMixer>> {
    Err(Error::unavailable(
        "mixer",
        "not implemented in this build yet",
    ))
}
