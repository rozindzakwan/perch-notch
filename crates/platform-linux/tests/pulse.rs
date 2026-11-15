//! PulseAudio mixer against a real sound server: run via
//! `scripts/pulse-test.sh`, which starts `pulseaudio` and runs this with
//! `--include-ignored`. A `pacat` stream playing silence stands in for an app.
#![cfg(target_os = "linux")]

use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use perch_core::platform::{AudioMixer, EventSink, PlatformEvent};
use perch_platform_linux::pulse::PulseMixer;

fn wait_until(
    mixer: &mut PulseMixer,
    what: &str,
    pred: impl Fn(&[perch_core::platform::AudioSession]) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = mixer.sessions().unwrap_or_else(|e| panic!("{e}"));
        if pred(&s) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {s:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "needs a running PulseAudio/PipeWire server (scripts/pulse-test.sh)"]
fn pulse_mixer_lists_and_controls_app_streams() {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let sink: EventSink = Arc::new(move |ev| {
        if ev == PlatformEvent::AudioSessionsChanged {
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(());
            }
        }
    });
    let mut mixer = PulseMixer::start(sink).unwrap_or_else(|e| panic!("connect: {e}"));

    let mut tone = Command::new("pacat")
        .args([
            "--raw",
            "--property=application.name=PerchTone",
            "/dev/zero",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("pacat: {e}"));

    // The new stream is announced as an event (not discovered by polling).
    rx.recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|e| panic!("no AudioSessionsChanged event: {e}"));
    wait_until(&mut mixer, "the PerchTone stream", |s| {
        s.iter().any(|x| x.name == "PerchTone")
    });

    mixer
        .set_volume("perchtone", 0.25)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_until(&mut mixer, "volume 25%", |s| {
        s.iter()
            .any(|x| x.id == "perchtone" && (x.volume - 0.25).abs() < 0.02)
    });
    mixer
        .set_mute("perchtone", true)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_until(&mut mixer, "muted", |s| {
        s.iter().any(|x| x.id == "perchtone" && x.muted)
    });
    assert!(mixer.set_volume("no-such-app", 0.5).is_err());

    // The output device's main volume is the first row and follows changes
    // made elsewhere (as with the volume keys), announced as an event.
    use perch_core::platform::SYSTEM_VOLUME_ID;
    wait_until(&mut mixer, "the System row first", |s| {
        s.first().is_some_and(|x| x.id == SYSTEM_VOLUME_ID)
    });
    mixer
        .set_volume(SYSTEM_VOLUME_ID, 0.40)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_until(&mut mixer, "system volume 40%", |s| {
        s.iter()
            .any(|x| x.id == SYSTEM_VOLUME_ID && (x.volume - 0.40).abs() < 0.02)
    });
    while rx.try_recv().is_ok() {}
    let status = Command::new("pactl")
        .args(["set-sink-volume", "@DEFAULT_SINK@", "70%"])
        .status()
        .unwrap_or_else(|e| panic!("pactl: {e}"));
    assert!(status.success());
    rx.recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|e| panic!("no event for an outside volume change: {e}"));
    wait_until(&mut mixer, "system volume 70% from pactl", |s| {
        s.iter()
            .any(|x| x.id == SYSTEM_VOLUME_ID && (x.volume - 0.70).abs() < 0.02)
    });
    mixer
        .set_mute(SYSTEM_VOLUME_ID, true)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_until(&mut mixer, "system muted", |s| {
        s.iter().any(|x| x.id == SYSTEM_VOLUME_ID && x.muted)
    });
    mixer
        .set_mute(SYSTEM_VOLUME_ID, false)
        .unwrap_or_else(|e| panic!("{e}"));

    let _ = tone.kill();
    let _ = tone.wait();
    wait_until(&mut mixer, "the stream to go away", |s| {
        !s.iter().any(|x| x.id == "perchtone")
    });
}
