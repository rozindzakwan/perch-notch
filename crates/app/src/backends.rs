//! Optional feature backends (media, clipboard, mixer). Each constructor is
//! only called when its feature is live, so a disabled feature never starts
//! a thread or opens a device.

#[allow(unused_imports)]
use perch_core::error::{Error, Result};
use perch_core::platform::{AudioMixer, EventSink, MediaControl};

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

/// Per-app volume: WASAPI audio sessions on Windows, PulseAudio sink inputs
/// (also served by pipewire-pulse) on Linux.
pub fn mixer(sink: EventSink) -> Result<Box<dyn AudioMixer>> {
    #[cfg(windows)]
    {
        Ok(Box::new(perch_platform_windows::mixer::WasapiMixer::start(
            sink,
        )?))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(perch_platform_linux::pulse::PulseMixer::start(
            sink,
        )?))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = sink;
        Err(Error::unavailable("mixer", "no backend for this OS"))
    }
}

/// The system keychain for mail passwords. Without one (no Secret Service on
/// Linux), passwords are kept in memory until Perch exits.
#[cfg(feature = "mail")]
pub fn secrets() -> std::sync::Arc<dyn perch_core::platform::SecretStore> {
    #[cfg(windows)]
    {
        std::sync::Arc::new(perch_platform_windows::secrets::WindowsCredentials)
    }
    #[cfg(target_os = "linux")]
    {
        match perch_platform_linux::secrets::SecretService::connect() {
            Ok(s) => std::sync::Arc::new(s),
            Err(e) => {
                log::warn!("no keyring ({e}); mail passwords are kept until Perch exits");
                std::sync::Arc::new(perch_core::platform::MemorySecrets::default())
            }
        }
    }
}
