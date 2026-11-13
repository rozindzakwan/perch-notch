//! Optional feature backends (media, clipboard, mixer). Each constructor is
//! only called when its feature is live, so a disabled feature never starts
//! a thread or opens a device.

#[allow(unused_imports)]
use perch_core::error::{Error, Result};
use perch_core::platform::{AudioMixer, ClipboardAccess, EventSink, MediaControl};

/// Clipboard read/write.
pub fn clipboard() -> Result<Box<dyn ClipboardAccess>> {
    Err(Error::unavailable(
        "clipboard",
        "not implemented in this build yet",
    ))
}

/// Now-playing: SMTC on Windows, MPRIS on Linux.
pub fn media(sink: EventSink) -> Result<Box<dyn MediaControl>> {
    #[cfg(windows)]
    {
        Ok(Box::new(perch_platform_windows::SmtcMedia::start(sink)?))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(perch_platform_linux::mpris::MprisMedia::start(
            sink,
        )?))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = sink;
        Err(Error::unavailable("media", "no backend for this OS"))
    }
}

/// Per-app volume.
pub fn mixer(_sink: EventSink) -> Result<Box<dyn AudioMixer>> {
    Err(Error::unavailable(
        "mixer",
        "not implemented in this build yet",
    ))
}
