//! A tiny MPRIS player for end-to-end tests: owns
//! `org.mpris.MediaPlayer2.perchmock`, reports one track, and toggles
//! play/pause when asked (e.g. by `playerctl play-pause`). Runs until killed.
//!
//! Usage: `mock_player [TITLE] [ARTIST]`

#[cfg(target_os = "linux")]
fn main() {
    use std::collections::HashMap;
    use zbus::object_server::SignalEmitter;
    use zbus::zvariant::{OwnedValue, Value};

    struct Player {
        title: String,
        artist: String,
        playing: bool,
    }

    #[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
    impl Player {
        async fn play_pause(&mut self, #[zbus(signal_emitter)] em: SignalEmitter<'_>) {
            self.playing = !self.playing;
            let _ = self.playback_status_changed(&em).await;
        }
        async fn play(&mut self, #[zbus(signal_emitter)] em: SignalEmitter<'_>) {
            self.playing = true;
            let _ = self.playback_status_changed(&em).await;
        }
        async fn pause(&mut self, #[zbus(signal_emitter)] em: SignalEmitter<'_>) {
            self.playing = false;
            let _ = self.playback_status_changed(&em).await;
        }
        async fn next(&self) {}
        async fn previous(&self) {}
        #[zbus(property)]
        async fn playback_status(&self) -> String {
            if self.playing { "Playing" } else { "Paused" }.into()
        }
        #[zbus(property)]
        async fn position(&self) -> i64 {
            5_000_000
        }
        #[zbus(property)]
        async fn can_control(&self) -> bool {
            true
        }
        #[zbus(property)]
        async fn can_play(&self) -> bool {
            true
        }
        #[zbus(property)]
        async fn can_pause(&self) -> bool {
            true
        }
        #[zbus(property)]
        async fn can_go_next(&self) -> bool {
            true
        }
        #[zbus(property)]
        async fn can_go_previous(&self) -> bool {
            true
        }
        #[zbus(property)]
        async fn metadata(&self) -> HashMap<String, OwnedValue> {
            let mut m = HashMap::new();
            for (k, v) in [
                ("xesam:title", Value::from(self.title.clone())),
                ("xesam:artist", Value::from(vec![self.artist.clone()])),
                ("mpris:length", Value::from(180_000_000i64)),
            ] {
                if let Ok(o) = v.try_into() {
                    m.insert(k.to_string(), o);
                }
            }
            m
        }
    }

    struct Root;
    #[zbus::interface(name = "org.mpris.MediaPlayer2")]
    impl Root {
        #[zbus(property)]
        async fn identity(&self) -> String {
            "Perch mock player".into()
        }
    }

    let mut args = std::env::args().skip(1);
    let title = args.next().unwrap_or_else(|| "Mock Song".into());
    let artist = args.next().unwrap_or_else(|| "Mock Artist".into());
    let built = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.name("org.mpris.MediaPlayer2.perchmock"))
        .and_then(|b| b.serve_at("/org/mpris/MediaPlayer2", Root))
        .and_then(|b| {
            b.serve_at(
                "/org/mpris/MediaPlayer2",
                Player {
                    title,
                    artist,
                    playing: true,
                },
            )
        })
        .and_then(|b| b.build());
    match built {
        Ok(_conn) => loop {
            std::thread::park();
        },
        Err(e) => {
            eprintln!("mock_player: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("mock_player is Linux-only");
}
