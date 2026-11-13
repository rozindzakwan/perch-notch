//! Real audio: the white-noise player opens the output device only while
//! audible, and the platform mixer sees and controls that stream.
//! It makes (very quiet) sound, so it is opt-in:
//! `cargo test -p perch --test audio -- --ignored`.
#![cfg(feature = "white-noise")]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use perch_app::extras::{noise_stream_open, set_noise};
use perch_core::noise::{NoiseKind, NoiseMix};
use perch_core::platform::{EventSink, PlatformEvent};

#[test]
#[ignore = "plays quiet noise on the default output device"]
fn noise_opens_and_closes_the_device_and_the_mixer_controls_it() {
    let events = Arc::new(Mutex::new(0u32));
    let e2 = events.clone();
    let sink: EventSink = Arc::new(move |ev| {
        if ev == PlatformEvent::AudioSessionsChanged {
            if let Ok(mut n) = e2.lock() {
                *n += 1;
            }
        }
    });
    let mut mixer = perch_app::backends::mixer(sink).unwrap_or_else(|e| panic!("mixer: {e}"));

    assert!(!noise_stream_open(), "no stream before anything is audible");
    let mut mix = NoiseMix::default();
    mix.master = 0.02;
    mix.set_on(NoiseKind::Rain, true);
    set_noise(mix);
    assert!(noise_stream_open(), "audible mix opens the device");

    // Our own process shows up as a session.
    let me = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let deadline = Instant::now() + Duration::from_secs(8);
    let session = loop {
        let sessions = mixer.sessions().unwrap_or_else(|e| panic!("{e}"));
        if let Some(s) = sessions
            .iter()
            .find(|s| s.id == me || s.name.to_lowercase().contains("perch"))
        {
            break s.clone();
        }
        assert!(
            Instant::now() < deadline,
            "own session not listed: {sessions:?} (looking for {me})"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    mixer
        .set_volume(&session.id, 0.4)
        .unwrap_or_else(|e| panic!("{e}"));
    let after = mixer.sessions().unwrap_or_else(|e| panic!("{e}"));
    let v = after
        .iter()
        .find(|s| s.id == session.id)
        .map(|s| s.volume)
        .unwrap_or(-1.0);
    assert!((v - 0.4).abs() < 0.02, "volume applied: {v}");

    mix.set_on(NoiseKind::Rain, false);
    set_noise(mix);
    assert!(!noise_stream_open(), "going silent closes the device");
    assert!(
        events.lock().map(|n| *n).unwrap_or(0) > 0,
        "session events were delivered"
    );
}
