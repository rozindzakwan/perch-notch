//! MPRIS integration test against a mock player on a private session bus.
//!
//! Run with `dbus-run-session -- cargo test -p perch-platform-linux --test
//! mpris -- --include-ignored` (scripts/linux-ci.sh does this). Ignored by
//! default because it needs a D-Bus session.
#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use perch_core::platform::{EventSink, MediaControl, MediaEvent, PlatformEvent, TransportCommand};
use perch_platform_linux::mpris::MprisMedia;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const PATH: &str = "/org/mpris/MediaPlayer2";

struct FakePlayer {
    title: String,
    status: String,
    calls: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl FakePlayer {
    async fn play_pause(&mut self, #[zbus(signal_emitter)] em: SignalEmitter<'_>) {
        self.record("PlayPause");
        self.status = if self.status == "Playing" {
            "Paused"
        } else {
            "Playing"
        }
        .into();
        let _ = self.playback_status_changed(&em).await;
    }

    async fn next(&mut self) {
        self.record("Next");
    }

    async fn previous(&mut self) {
        self.record("Previous");
    }

    async fn set_position(&mut self, track: ObjectPath<'_>, position: i64) {
        self.record(&format!("SetPosition {} {position}", track.as_str()));
    }

    #[zbus(property)]
    async fn playback_status(&self) -> String {
        self.status.clone()
    }

    #[zbus(property)]
    async fn position(&self) -> i64 {
        12_000_000
    }

    #[zbus(property)]
    async fn metadata(&self) -> HashMap<String, OwnedValue> {
        let mut m = HashMap::new();
        let mut put = |k: &str, v: Value<'_>| {
            if let Ok(o) = v.try_into() {
                m.insert(k.to_string(), o);
            }
        };
        put("xesam:title", Value::from(self.title.clone()));
        put("xesam:artist", Value::from(vec!["Test Band"]));
        put("xesam:album", Value::from("Test LP"));
        put("mpris:length", Value::from(200_000_000i64));
        if let Ok(p) = ObjectPath::try_from("/org/perch/track/1") {
            put("mpris:trackid", Value::from(p));
        }
        m
    }
}

impl FakePlayer {
    fn record(&self, s: &str) {
        if let Ok(mut c) = self.calls.lock() {
            c.push(s.to_string());
        }
    }
}

fn sink() -> (EventSink, Receiver<MediaEvent>) {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let sink: EventSink = Arc::new(move |ev| {
        if let PlatformEvent::Media(m) = ev {
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(m);
            }
        }
    });
    (sink, rx)
}

/// Wait for the first event matching `pred`.
fn wait_for(
    rx: &Receiver<MediaEvent>,
    what: &str,
    pred: impl Fn(&MediaEvent) -> bool,
) -> MediaEvent {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(ev) if pred(&ev) => return ev,
            Ok(_) => continue,
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

#[test]
#[ignore = "needs a D-Bus session bus (run under dbus-run-session)"]
fn mpris_backend_follows_a_mock_player() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let player = FakePlayer {
        title: "Song A".into(),
        status: "Playing".into(),
        calls: calls.clone(),
    };
    let conn = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.name("org.mpris.MediaPlayer2.perchtest"))
        .and_then(|b| b.serve_at(PATH, player))
        .and_then(|b| b.build())
        .unwrap_or_else(|e| panic!("mock player: {e}"));

    let (sink, rx) = sink();
    let mut media = MprisMedia::start(sink).unwrap_or_else(|e| panic!("backend: {e}"));

    // Discovered at startup, with metadata and position.
    let ev = wait_for(
        &rx,
        "the initial track",
        |e| matches!(e, MediaEvent::TrackChanged(t) if t.title == "Song A"),
    );
    if let MediaEvent::TrackChanged(t) = ev {
        assert_eq!(t.artist, "Test Band");
        assert_eq!(t.album, "Test LP");
        assert_eq!(t.duration, Some(Duration::from_secs(200)));
        assert_eq!(t.position, Some(Duration::from_secs(12)));
        assert!(t.playing);
    }

    // Commands reach the player, and its signal comes back as a state change.
    media
        .send(TransportCommand::PlayPause)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(
        &rx,
        "paused",
        |e| matches!(e, MediaEvent::TrackChanged(t) if !t.playing),
    );
    media
        .send(TransportCommand::Next)
        .unwrap_or_else(|e| panic!("{e}"));
    media
        .send(TransportCommand::SeekTo(Duration::from_secs(30)))
        .unwrap_or_else(|e| panic!("{e}"));
    let seen = calls.lock().map(|c| c.clone()).unwrap_or_default();
    assert!(seen.contains(&"PlayPause".to_string()), "{seen:?}");
    assert!(seen.contains(&"Next".to_string()), "{seen:?}");
    assert!(
        seen.iter()
            .any(|c| c == "SetPosition /org/perch/track/1 30000000"),
        "seek uses SetPosition with the track id: {seen:?}"
    );

    // A metadata change arrives as a new track.
    {
        let iface = conn
            .object_server()
            .interface::<_, FakePlayer>(PATH)
            .unwrap_or_else(|e| panic!("{e}"));
        iface.get_mut().title = "Song B".into();
        let em = iface.signal_emitter().clone();
        zbus::block_on(async { iface.get().metadata_changed(&em).await })
            .unwrap_or_else(|e| panic!("{e}"));
    }
    wait_for(
        &rx,
        "the second track",
        |e| matches!(e, MediaEvent::TrackChanged(t) if t.title == "Song B"),
    );

    // The player quitting is reported.
    drop(conn);
    wait_for(&rx, "player gone", |e| matches!(e, MediaEvent::Gone));
    assert!(media.now_playing().is_none());
}

#[test]
fn file_url_decoding() {
    use perch_platform_linux::mpris::percent_decode;
    assert_eq!(
        percent_decode("/home/me/My%20Music/a%2Bb.jpg"),
        "/home/me/My Music/a+b.jpg"
    );
    assert_eq!(percent_decode("/bad%zzescape%"), "/bad%zzescape%");
    assert!(perch_platform_linux::mpris::read_local_art("https://example.com/a.jpg").is_none());
    assert!(
        perch_platform_linux::mpris::read_local_art("file:///definitely/not/here.png").is_none()
    );
}
