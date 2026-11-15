//! Per-app volume through PulseAudio sink inputs, plus the default output's
//! main volume as the System row (works unchanged on
//! PipeWire's `pipewire-pulse`, the default on Mint 22).
//!
//! A threaded PulseAudio mainloop runs the protocol; a subscription to
//! sink-input events turns every new/removed/changed stream into
//! `AudioSessionsChanged`. Callbacks hand data back through `std` channels
//! (never shared `Rc`s), and the mainloop lock is only held while a request is
//! issued. Streams are grouped by application name.

use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::Duration;

use libpulse_binding::callbacks::ListResult;
use libpulse_binding::context::subscribe::{Facility, InterestMaskSet};
use libpulse_binding::context::{Context, FlagSet, State};
use libpulse_binding::mainloop::threaded::Mainloop;
use libpulse_binding::proplist::{properties, Proplist};
use libpulse_binding::volume::{ChannelVolumes, Volume};
use perch_core::error::{Error, Result};
use perch_core::platform::{AudioMixer, AudioSession, EventSink, PlatformEvent, SYSTEM_VOLUME_ID};

/// PulseAudio's name for whatever the default output currently is.
const DEFAULT_SINK: &str = "@DEFAULT_SINK@";

const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
struct Stream {
    index: u32,
    name: String,
    channels: u8,
    volume: f32,
    muted: bool,
}

/// PulseAudio-backed [`AudioMixer`].
pub struct PulseMixer {
    ml: Mainloop,
    ctx: Context,
    last: Vec<Stream>,
}

impl PulseMixer {
    /// Connect to the user's sound server and subscribe to stream events.
    pub fn start(sink: EventSink) -> Result<Self> {
        let mut ml =
            Mainloop::new().ok_or_else(|| Error::unavailable("mixer", "no PulseAudio mainloop"))?;
        let mut props = Proplist::new().ok_or_else(|| Error::backend("proplist"))?;
        let _ = props.set_str(properties::APPLICATION_NAME, "Perch");
        let mut ctx = Context::new_with_proplist(&ml, "Perch", &props)
            .ok_or_else(|| Error::unavailable("mixer", "cannot create a PulseAudio context"))?;

        let (tx, rx) = channel::<State>();
        let ctx_ptr_state = std::sync::Mutex::new(tx);
        // The state callback only reports that the state changed; we read it
        // under the lock below.
        ctx.set_state_callback(Some(Box::new(move || {
            if let Ok(tx) = ctx_ptr_state.lock() {
                let _ = tx.send(State::Unconnected);
            }
        })));
        ctx.connect(None, FlagSet::NOFLAGS, None)
            .map_err(|e| Error::unavailable("mixer", format!("PulseAudio connect: {e}")))?;
        ml.start()
            .map_err(|e| Error::backend(format!("PulseAudio mainloop: {e}")))?;
        loop {
            ml.lock();
            let state = ctx.get_state();
            ml.unlock();
            match state {
                State::Ready => break,
                State::Failed | State::Terminated => {
                    ml.stop();
                    return Err(Error::unavailable(
                        "mixer",
                        "no PulseAudio/PipeWire sound server",
                    ));
                }
                _ => match rx.recv_timeout(TIMEOUT) {
                    Ok(_) => continue,
                    Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
                        ml.stop();
                        return Err(Error::unavailable(
                            "mixer",
                            "timed out connecting to the sound server",
                        ));
                    }
                },
            }
        }

        ml.lock();
        ctx.set_state_callback(None);
        ctx.set_subscribe_callback(Some(Box::new(move |facility, _op, _idx| {
            // Streams, the output device (main volume) and the server (a new
            // default output) all change what the mixer shows.
            if matches!(
                facility,
                Some(Facility::SinkInput | Facility::Sink | Facility::Server)
            ) {
                sink(PlatformEvent::AudioSessionsChanged);
            }
        })));
        let _op = ctx.subscribe(
            InterestMaskSet::SINK_INPUT | InterestMaskSet::SINK | InterestMaskSet::SERVER,
            |_| {},
        );
        ml.unlock();
        Ok(Self {
            ml,
            ctx,
            last: Vec::new(),
        })
    }

    fn streams(&mut self) -> Result<Vec<Stream>> {
        let (tx, rx) = channel::<Option<Stream>>();
        self.ml.lock();
        let _op = self
            .ctx
            .introspect()
            .get_sink_input_info_list(move |res| match res {
                ListResult::Item(info) => {
                    let name = info
                        .proplist
                        .get_str(properties::APPLICATION_NAME)
                        .or_else(|| info.name.as_ref().map(|n| n.to_string()))
                        .unwrap_or_else(|| format!("stream {}", info.index));
                    let avg = info.volume.avg().0 as f32 / Volume::NORMAL.0 as f32;
                    let _ = tx.send(Some(Stream {
                        index: info.index,
                        name,
                        channels: info.volume.len(),
                        volume: if avg.is_finite() { avg } else { 0.0 },
                        muted: info.mute,
                    }));
                }
                ListResult::End | ListResult::Error => {
                    let _ = tx.send(None);
                }
            });
        self.ml.unlock();
        let mut out = Vec::new();
        loop {
            match rx.recv_timeout(TIMEOUT) {
                Ok(Some(s)) => out.push(s),
                Ok(None) => break,
                Err(_) => return Err(Error::backend("PulseAudio did not answer the stream list")),
            }
        }
        self.last = out.clone();
        Ok(out)
    }

    /// The default output's main volume, mute state and channel count.
    fn device(&mut self) -> Option<(f32, bool, u8)> {
        let (tx, rx) = channel::<Option<(f32, bool, u8)>>();
        self.ml.lock();
        let _op = self
            .ctx
            .introspect()
            .get_sink_info_by_name(DEFAULT_SINK, move |res| match res {
                ListResult::Item(info) => {
                    let avg = info.volume.avg().0 as f32 / Volume::NORMAL.0 as f32;
                    let v = if avg.is_finite() { avg } else { 0.0 };
                    let _ = tx.send(Some((v, info.mute, info.volume.len())));
                }
                ListResult::End | ListResult::Error => {
                    let _ = tx.send(None);
                }
            });
        self.ml.unlock();
        rx.recv_timeout(TIMEOUT).ok().flatten()
    }

    fn indices(&mut self, id: &str) -> Result<Vec<(u32, u8)>> {
        let streams = self.streams()?;
        let v: Vec<(u32, u8)> = streams
            .iter()
            .filter(|s| s.name.to_lowercase() == id)
            .map(|s| (s.index, s.channels.max(1)))
            .collect();
        if v.is_empty() {
            Err(Error::backend(format!("no audio stream for {id}")))
        } else {
            Ok(v)
        }
    }
}

impl Drop for PulseMixer {
    fn drop(&mut self) {
        self.ml.lock();
        self.ctx.set_subscribe_callback(None);
        self.ctx.disconnect();
        self.ml.unlock();
        self.ml.stop();
    }
}

/// Group streams per application. Pure.
fn group(streams: &[Stream]) -> Vec<AudioSession> {
    let mut out: Vec<AudioSession> = Vec::new();
    for s in streams {
        let id = s.name.to_lowercase();
        let vol = s.volume.clamp(0.0, 1.0);
        match out.iter_mut().find(|g| g.id == id) {
            Some(g) => {
                g.volume = g.volume.max(vol);
                g.muted = g.muted && s.muted;
            }
            None => out.push(AudioSession {
                id,
                name: s.name.clone(),
                volume: vol,
                muted: s.muted,
            }),
        }
    }
    out
}

impl AudioMixer for PulseMixer {
    fn sessions(&mut self) -> Result<Vec<AudioSession>> {
        let streams = self.streams()?;
        let mut out = Vec::new();
        if let Some((volume, muted, _)) = self.device() {
            out.push(AudioSession {
                id: SYSTEM_VOLUME_ID.to_string(),
                name: "System volume".to_string(),
                volume: volume.clamp(0.0, 1.0),
                muted,
            });
        }
        out.extend(group(&streams));
        Ok(out)
    }

    fn set_volume(&mut self, id: &str, volume: f32) -> Result<()> {
        let v = volume.clamp(0.0, 1.0);
        if id == SYSTEM_VOLUME_ID {
            let channels = self
                .device()
                .map(|(_, _, c)| c.max(1))
                .ok_or_else(|| Error::backend("no default output"))?;
            let mut cv = ChannelVolumes::default();
            cv.set(
                channels,
                Volume((Volume::NORMAL.0 as f32 * v).round() as u32),
            );
            self.ml.lock();
            let _ = self
                .ctx
                .introspect()
                .set_sink_volume_by_name(DEFAULT_SINK, &cv, None);
            self.ml.unlock();
            return Ok(());
        }
        let targets = self.indices(id)?;
        self.ml.lock();
        let mut introspect = self.ctx.introspect();
        for (index, channels) in targets {
            let mut cv = ChannelVolumes::default();
            cv.set(
                channels,
                Volume((Volume::NORMAL.0 as f32 * v).round() as u32),
            );
            let _ = introspect.set_sink_input_volume(index, &cv, None);
        }
        self.ml.unlock();
        Ok(())
    }

    fn set_mute(&mut self, id: &str, muted: bool) -> Result<()> {
        if id == SYSTEM_VOLUME_ID {
            self.ml.lock();
            let _ = self
                .ctx
                .introspect()
                .set_sink_mute_by_name(DEFAULT_SINK, muted, None);
            self.ml.unlock();
            return Ok(());
        }
        let targets = self.indices(id)?;
        self.ml.lock();
        let mut introspect = self.ctx.introspect();
        for (index, _) in targets {
            let _ = introspect.set_sink_input_mute(index, muted, None);
        }
        self.ml.unlock();
        Ok(())
    }
}
