//! Regression test: starting the SMTC backend on a UI thread must not block.
//!
//! The UI thread is a COM single-threaded apartment (winit initialises OLE
//! there). Blocking on a WinRT async operation from it deadlocks whenever the
//! operation has to call back into that apartment, which is what reading
//! Spotify's cover thumbnail does. That froze Perch at startup whenever Spotify
//! was the current media session (reproduced: blocked > 5 s before the fix).
//!
//! By default the test publishes a session with a cover through the `mock_smtc`
//! example (deterministic, but its cover does not trigger the deadlock); set
//! `PERCH_SMTC_NO_MOCK=1` to test against the player that is current, e.g.
//! Spotify, which does. It needs an interactive desktop with
//! the SMTC service, so it is opt-in:
//! `cargo test -p perch-platform-windows --test smtc_sta -- --ignored`.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use perch_core::platform::{EventSink, MediaEvent, PlatformEvent};
use perch_platform_windows::SmtcMedia;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn mock_player(cover: &std::path::Path) -> Kill {
    // Examples live in target/<profile>/examples; the test binary sits somewhere
    // below target/<profile> depending on the cargo version, so walk up.
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe
        .ancestors()
        .map(|d| d.join("examples").join("mock_smtc.exe"))
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("mock_smtc.exe"));
    assert!(dir.exists(), "build the example first: {}", dir.display());
    let child = Command::new(&dir)
        .args(["STA Song", "STA Band", "--cover"])
        .arg(cover)
        .spawn()
        .unwrap_or_else(|e| panic!("starting {}: {e}", dir.display()));
    Kill(child)
}

#[test]
#[ignore = "needs an interactive desktop with the SMTC service"]
fn starting_on_a_ui_thread_does_not_block_on_the_player() {
    let cover = std::env::temp_dir().join("perch-smtc-sta-cover.png");
    let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 120, 40, 255]));
    img.save(&cover)
        .unwrap_or_else(|e| panic!("writing cover: {e}"));
    // PERCH_SMTC_NO_MOCK=1: use whatever player is current (e.g. Spotify) instead.
    let _player = (std::env::var_os("PERCH_SMTC_NO_MOCK").is_none()).then(|| mock_player(&cover));
    std::thread::sleep(Duration::from_millis(1500));

    let tracks = Arc::new(Mutex::new(Vec::new()));
    let t2 = tracks.clone();
    let sink: EventSink = Arc::new(move |ev| {
        if let PlatformEvent::Media(MediaEvent::TrackChanged(t)) = ev {
            if let Ok(mut v) = t2.lock() {
                v.push((t.title.clone(), t.cover.is_some()));
            }
        }
    });

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // SAFETY: make this thread a single-threaded apartment, like the UI thread.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let started = Instant::now();
        let media = SmtcMedia::start(sink);
        let _ = tx.send((started.elapsed(), media.is_ok()));
        // Keep the backend alive while the main thread watches for events.
        std::thread::sleep(Duration::from_secs(4));
        drop(media);
    });
    let (took, ok) = rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|_| panic!("SmtcMedia::start blocked the STA thread for more than 5 s"));
    assert!(ok, "the backend started");
    assert!(
        took < Duration::from_millis(500),
        "start returned quickly: {took:?}"
    );

    // The track (whichever session is current) still arrives, off the UI thread.
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && tracks.lock().map(|v| v.is_empty()).unwrap_or(true) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let seen = tracks.lock().map(|v| v.clone()).unwrap_or_default();
    eprintln!("start took {took:?}; tracks seen (title, has cover): {seen:?}");
    assert!(!seen.is_empty(), "a TrackChanged event was delivered");
}
