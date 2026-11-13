//! Per-app volume through WASAPI audio sessions on the default render device.
//!
//! Sessions are grouped by application (a browser with several streams is one
//! row). Changes arrive as events: `IAudioSessionNotification::OnSessionCreated`
//! for new sessions and each session's `IAudioSessionEvents` for volume, mute,
//! state and disconnects. Every callback just emits `AudioSessionsChanged`;
//! the app re-reads the list. Nothing polls.

use std::collections::HashSet;
use std::sync::Mutex;

use perch_core::error::{Error, Result};
use perch_core::platform::{AudioMixer, AudioSession, EventSink, PlatformEvent};
use windows::core::{implement, Interface, GUID, PCWSTR};
use windows::Win32::Foundation::{BOOL, S_OK};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, AudioSessionDisconnectReason, AudioSessionState,
    AudioSessionStateExpired, IAudioSessionControl, IAudioSessionControl2, IAudioSessionEvents,
    IAudioSessionEvents_Impl, IAudioSessionManager2, IAudioSessionNotification,
    IAudioSessionNotification_Impl, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};

/// Tags our own volume changes so their echo events can be ignored.
const PERCH_CONTEXT: GUID = GUID::from_u128(0x5045_5243_4800_4d49_5845_5200_0000_0001);

#[implement(IAudioSessionNotification)]
struct NewSessions {
    sink: EventSink,
}

impl IAudioSessionNotification_Impl for NewSessions_Impl {
    fn OnSessionCreated(&self, _new: Option<&IAudioSessionControl>) -> windows::core::Result<()> {
        (self.sink)(PlatformEvent::AudioSessionsChanged);
        Ok(())
    }
}

#[implement(IAudioSessionEvents)]
struct SessionEvents {
    sink: EventSink,
}

impl IAudioSessionEvents_Impl for SessionEvents_Impl {
    fn OnDisplayNameChanged(&self, _: &PCWSTR, _: *const GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnIconPathChanged(&self, _: &PCWSTR, _: *const GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnSimpleVolumeChanged(
        &self,
        _: f32,
        _: BOOL,
        ctx: *const GUID,
    ) -> windows::core::Result<()> {
        // SAFETY: `ctx` is either null or points at the GUID passed to the
        // setter, valid for the duration of the callback.
        let ours = !ctx.is_null() && unsafe { *ctx } == PERCH_CONTEXT;
        if !ours {
            (self.sink)(PlatformEvent::AudioSessionsChanged);
        }
        Ok(())
    }
    fn OnChannelVolumeChanged(
        &self,
        _: u32,
        _: *const f32,
        _: u32,
        _: *const GUID,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnGroupingParamChanged(&self, _: *const GUID, _: *const GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnStateChanged(&self, _: AudioSessionState) -> windows::core::Result<()> {
        (self.sink)(PlatformEvent::AudioSessionsChanged);
        Ok(())
    }
    fn OnSessionDisconnected(&self, _: AudioSessionDisconnectReason) -> windows::core::Result<()> {
        (self.sink)(PlatformEvent::AudioSessionsChanged);
        Ok(())
    }
}

struct Entry {
    name: String,
    volume: ISimpleAudioVolume,
}

/// WASAPI-backed [`AudioMixer`].
pub struct WasapiMixer {
    sink: EventSink,
    manager: IAudioSessionManager2,
    notification: IAudioSessionNotification,
    registered: Mutex<Vec<(String, IAudioSessionControl, IAudioSessionEvents)>>,
}

impl WasapiMixer {
    /// Attach to the default output device and start listening.
    pub fn start(sink: EventSink) -> Result<Self> {
        // SAFETY: initialise COM for this thread if nobody did yet. On the UI
        // thread winit already initialised OLE, and this returns S_FALSE /
        // RPC_E_CHANGED_MODE harmlessly; the apartment lives as long as the
        // thread, so it is deliberately not uninitialised.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        // SAFETY: COM is initialised on this (UI) thread by winit/OLE; the
        // enumerator and manager are plain COM interfaces we own.
        let manager: IAudioSessionManager2 = unsafe {
            let en: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| Error::unavailable("mixer", format!("device enumerator: {e}")))?;
            let dev = en
                .GetDefaultAudioEndpoint(eRender, eMultimedia)
                .map_err(|e| Error::unavailable("mixer", format!("no output device: {e}")))?;
            dev.Activate(CLSCTX_ALL, None)
                .map_err(|e| Error::unavailable("mixer", format!("session manager: {e}")))?
        };
        // Documented quirk: enumerate once before registering, or the
        // session-created notification never fires.
        // SAFETY: valid manager interface.
        let _ = unsafe { manager.GetSessionEnumerator() };
        let notification: IAudioSessionNotification = NewSessions { sink: sink.clone() }.into();
        // SAFETY: registering our own COM object; unregistered in Drop.
        unsafe { manager.RegisterSessionNotification(&notification) }
            .map_err(|e| Error::backend(format!("session notifications: {e}")))?;
        let me = Self {
            sink,
            manager,
            notification,
            registered: Mutex::new(Vec::new()),
        };
        let _ = me.entries();
        Ok(me)
    }

    /// Current sessions, registering for events on any we have not seen.
    fn entries(&self) -> Result<Vec<Entry>> {
        // SAFETY: plain COM calls on interfaces we own; every returned string
        // is freed with CoTaskMemFree.
        unsafe {
            let en = self
                .manager
                .GetSessionEnumerator()
                .map_err(|e| Error::backend(format!("session enumerator: {e}")))?;
            let n = en.GetCount().unwrap_or(0);
            let mut out = Vec::new();
            let mut seen = HashSet::new();
            for i in 0..n {
                let Ok(control) = en.GetSession(i) else {
                    continue;
                };
                if control
                    .GetState()
                    .map(|s| s == AudioSessionStateExpired)
                    .unwrap_or(true)
                {
                    continue;
                }
                let Ok(c2) = control.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                let Ok(volume) = control.cast::<ISimpleAudioVolume>() else {
                    continue;
                };
                let name = if c2.IsSystemSoundsSession() == S_OK {
                    "System sounds".to_string()
                } else {
                    let pid = c2.GetProcessId().unwrap_or(0);
                    crate::events::process_display_name(pid).unwrap_or_else(|| format!("pid {pid}"))
                };
                let instance = c2
                    .GetSessionInstanceIdentifier()
                    .ok()
                    .map(|p| {
                        let s = p.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(p.0 as *const _));
                        s
                    })
                    .unwrap_or_default();
                seen.insert(instance.clone());
                self.register(&instance, &control);
                out.push(Entry { name, volume });
            }
            // Forget registrations for sessions that are gone.
            if let Ok(mut reg) = self.registered.lock() {
                reg.retain(|(id, control, events)| {
                    let keep = seen.contains(id);
                    if !keep {
                        let _ = control.UnregisterAudioSessionNotification(events);
                    }
                    keep
                });
            }
            Ok(out)
        }
    }

    fn register(&self, instance: &str, control: &IAudioSessionControl) {
        let Ok(mut reg) = self.registered.lock() else {
            return;
        };
        if reg.iter().any(|(id, _, _)| id == instance) {
            return;
        }
        let events: IAudioSessionEvents = SessionEvents {
            sink: self.sink.clone(),
        }
        .into();
        // SAFETY: registering our own COM object on a live session control;
        // unregistered when the session goes away or the mixer is dropped.
        if unsafe { control.RegisterAudioSessionNotification(&events) }.is_ok() {
            reg.push((instance.to_string(), control.clone(), events));
        }
    }

    fn for_app(
        &self,
        id: &str,
        f: impl Fn(&ISimpleAudioVolume) -> windows::core::Result<()>,
    ) -> Result<()> {
        let mut any = false;
        for e in self.entries()? {
            if e.name.to_lowercase() == id {
                any = true;
                f(&e.volume).map_err(|err| Error::backend(format!("session volume: {err}")))?;
            }
        }
        if any {
            Ok(())
        } else {
            Err(Error::backend(format!("no audio session for {id}")))
        }
    }
}

impl Drop for WasapiMixer {
    fn drop(&mut self) {
        // SAFETY: undoing the registrations made in `start` / `register`.
        unsafe {
            let _ = self
                .manager
                .UnregisterSessionNotification(&self.notification);
            if let Ok(mut reg) = self.registered.lock() {
                for (_, control, events) in reg.drain(..) {
                    let _ = control.UnregisterAudioSessionNotification(&events);
                }
            }
        }
    }
}

impl AudioMixer for WasapiMixer {
    fn sessions(&mut self) -> Result<Vec<AudioSession>> {
        let mut grouped: Vec<AudioSession> = Vec::new();
        for e in self.entries()? {
            // SAFETY: reading volume/mute from a live session.
            let (vol, muted) = unsafe {
                (
                    e.volume.GetMasterVolume().unwrap_or(1.0),
                    e.volume.GetMute().map(|b| b.as_bool()).unwrap_or(false),
                )
            };
            let id = e.name.to_lowercase();
            match grouped.iter_mut().find(|g| g.id == id) {
                Some(g) => {
                    g.volume = g.volume.max(vol);
                    g.muted = g.muted && muted;
                }
                None => grouped.push(AudioSession {
                    id,
                    name: e.name,
                    volume: vol,
                    muted,
                }),
            }
        }
        Ok(grouped)
    }

    fn set_volume(&mut self, id: &str, volume: f32) -> Result<()> {
        let v = volume.clamp(0.0, 1.0);
        // SAFETY: setting volume on a live session with our context GUID.
        self.for_app(id, |s| unsafe { s.SetMasterVolume(v, &PERCH_CONTEXT) })
    }

    fn set_mute(&mut self, id: &str, muted: bool) -> Result<()> {
        // SAFETY: as above.
        self.for_app(id, |s| unsafe { s.SetMute(muted, &PERCH_CONTEXT) })
    }
}
