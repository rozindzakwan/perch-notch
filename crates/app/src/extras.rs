//! Hooks for the heavier, optional features (white noise, screen-time
//! persistence, screenshot, camera, dictation, hotkeys, shelf drag-out).
//!
//! Each hook is a no-op that logs when its feature is not compiled in, so the
//! shell can call them unconditionally and a slim build simply lacks the code.

use std::path::Path;

use perch_core::noise::NoiseMix;
use perch_core::usage::UsageTracker;
use winit::event_loop::ActiveEventLoop;

use crate::model::{Effect, Model, Now};

/// Apply a white-noise mix. The audio device is opened when the mix becomes
/// audible and closed (dropped) as soon as it goes silent, so an idle Perch
/// never has an audio stream running.
pub fn set_noise(mix: NoiseMix) {
    #[cfg(feature = "white-noise")]
    noise::set(mix);
    #[cfg(not(feature = "white-noise"))]
    log::debug!(
        "white noise not compiled in; ignoring mix (audible: {})",
        mix.is_audible()
    );
}

/// Whether a noise stream is open right now (for the status file / tests).
pub fn noise_stream_open() -> bool {
    #[cfg(feature = "white-noise")]
    {
        noise::is_open()
    }
    #[cfg(not(feature = "white-noise"))]
    {
        false
    }
}

#[cfg(feature = "white-noise")]
mod noise {
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use perch_core::noise::{NoiseGenerator, NoiseMix};

    const RATE: u32 = 48_000;

    /// A rodio source pulling from the procedural generator. The mix is
    /// shared and re-read every 1024 samples, so UI changes apply within
    /// ~20 ms without taking a lock per sample.
    struct NoiseSource {
        gen: NoiseGenerator,
        shared: Arc<Mutex<NoiseMix>>,
        countdown: u32,
    }

    impl Iterator for NoiseSource {
        type Item = f32;
        fn next(&mut self) -> Option<f32> {
            if self.countdown == 0 {
                self.countdown = 1024;
                if let Ok(m) = self.shared.lock() {
                    self.gen.set_mix(*m);
                }
            }
            self.countdown -= 1;
            Some(self.gen.next_sample())
        }
    }

    impl rodio::Source for NoiseSource {
        fn current_frame_len(&self) -> Option<usize> {
            None
        }
        fn channels(&self) -> u16 {
            1
        }
        fn sample_rate(&self) -> u32 {
            RATE
        }
        fn total_duration(&self) -> Option<Duration> {
            None
        }
    }

    struct Player {
        _stream: rodio::OutputStream,
        _sink: rodio::Sink,
        shared: Arc<Mutex<NoiseMix>>,
    }

    thread_local! {
        static PLAYER: RefCell<Option<Player>> = const { RefCell::new(None) };
    }

    pub fn set(mix: NoiseMix) {
        PLAYER.with(|p| {
            let mut p = p.borrow_mut();
            if !mix.is_audible() {
                *p = None; // closes the device
                return;
            }
            if let Some(player) = p.as_ref() {
                if let Ok(mut m) = player.shared.lock() {
                    *m = mix;
                }
                return;
            }
            match rodio::OutputStream::try_default() {
                Ok((stream, handle)) => match rodio::Sink::try_new(&handle) {
                    Ok(sink) => {
                        let shared = Arc::new(Mutex::new(mix));
                        let mut gen = NoiseGenerator::new(RATE, 0x5eed_1234);
                        gen.set_mix(mix);
                        sink.append(NoiseSource {
                            gen,
                            shared: shared.clone(),
                            countdown: 1024,
                        });
                        *p = Some(Player {
                            _stream: stream,
                            _sink: sink,
                            shared,
                        });
                    }
                    Err(e) => log::warn!("white noise: cannot open a sink: {e}"),
                },
                Err(e) => log::warn!("white noise: no audio output device: {e}"),
            }
        });
    }

    pub fn is_open() -> bool {
        PLAYER.with(|p| p.borrow().is_some())
    }
}

/// Persist screen-time totals (today's and yesterday's rows, as absolutes).
pub fn save_usage(usage: &UsageTracker) {
    #[cfg(feature = "screen-time")]
    usage_db::save(usage);
    #[cfg(not(feature = "screen-time"))]
    let _ = usage;
}

/// Load persisted screen-time totals for the last 30 days.
pub fn load_usage() -> Option<perch_core::usage::UsageAggregate> {
    #[cfg(feature = "screen-time")]
    {
        usage_db::load()
    }
    #[cfg(not(feature = "screen-time"))]
    {
        None
    }
}

#[cfg(feature = "screen-time")]
mod usage_db {
    use std::cell::RefCell;

    use perch_core::usage::{DayId, UsageAggregate, UsageTracker};

    thread_local! {
        static DB: RefCell<Option<rusqlite::Connection>> = const { RefCell::new(None) };
    }

    fn with_db<R>(f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<R>) -> Option<R> {
        DB.with(|db| {
            let mut db = db.borrow_mut();
            if db.is_none() {
                let dir = crate::persist::data_dir()?;
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    log::warn!("screen time: {e}");
                    return None;
                }
                match open(&dir.join("usage.sqlite")) {
                    Ok(c) => *db = Some(c),
                    Err(e) => {
                        log::warn!("screen time database unavailable: {e}");
                        return None;
                    }
                }
            }
            let conn = db.as_ref()?;
            match f(conn) {
                Ok(r) => Some(r),
                Err(e) => {
                    log::warn!("screen time database: {e}");
                    None
                }
            }
        })
    }

    /// Open (and migrate) the database.
    pub fn open(path: &std::path::Path) -> rusqlite::Result<rusqlite::Connection> {
        let c = rusqlite::Connection::open(path)?;
        c.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS usage (
                 day INTEGER NOT NULL,
                 app TEXT NOT NULL,
                 seconds INTEGER NOT NULL,
                 PRIMARY KEY (day, app)
             );",
        )?;
        Ok(c)
    }

    /// Upsert the absolute totals for `days`.
    pub fn write(
        conn: &rusqlite::Connection,
        agg: &UsageAggregate,
        days: &[DayId],
    ) -> rusqlite::Result<()> {
        let mut stmt = conn.prepare_cached(
            "INSERT INTO usage (day, app, seconds) VALUES (?1, ?2, ?3)
             ON CONFLICT(day, app) DO UPDATE SET seconds = excluded.seconds",
        )?;
        for day in days {
            for (app, secs) in agg.apps_for(*day) {
                stmt.execute(rusqlite::params![day.0, app, secs as i64])?;
            }
        }
        Ok(())
    }

    /// Read rows from `since` on.
    pub fn read(conn: &rusqlite::Connection, since: DayId) -> rusqlite::Result<UsageAggregate> {
        let mut agg = UsageAggregate::new();
        let mut stmt = conn.prepare("SELECT day, app, seconds FROM usage WHERE day >= ?1")?;
        let rows = stmt.query_map([since.0], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (day, app, secs) = row?;
            agg.add(&app, DayId(day), secs.max(0) as u64);
        }
        Ok(agg)
    }

    pub fn save(usage: &UsageTracker) {
        let now = crate::model::Now::real();
        let today = DayId::from_unix(now.unix, crate::model::local_offset());
        let days = [DayId(today.0 - 1), today];
        let _ = with_db(|c| write(c, usage.aggregate(), &days));
    }

    pub fn load() -> Option<UsageAggregate> {
        let now = crate::model::Now::real();
        let today = DayId::from_unix(now.unix, crate::model::local_offset());
        with_db(|c| read(c, DayId(today.0 - 30)))
    }
}

/// Round-trip helpers exposed for the integration tests.
#[cfg(feature = "screen-time")]
pub mod usage_store {
    pub use super::usage_db::{open, read, write};
}

/// Start a screenshot.
pub fn screenshot(_el: &ActiveEventLoop, model: &mut Model) {
    model.flash("Screenshot is not in this build", Now::real());
}

/// Toggle the camera mirror.
pub fn camera_toggle(_el: &ActiveEventLoop, model: &mut Model) {
    model.flash("Camera mirror is not in this build", Now::real());
}

/// Toggle dictation.
pub fn dictate_toggle(model: &mut Model) {
    model.flash("Dictation is not in this build", Now::real());
}

/// A global hotkey fired. Presses run the bound command; releasing the
/// dictation key ends push-to-talk (X11 reports releases; Windows only
/// presses, where dictation toggles instead).
pub fn hotkey(model: &mut Model, id: &str, pressed: bool, now: Now) -> Vec<Effect> {
    match (id, pressed) {
        (_, true) => model.command(id, now),
        ("dictate", false) => model.command("dictate-release", now),
        _ => Vec::new(),
    }
}

/// Start dragging a shelf file out of the notch (Windows: OLE drag source).
/// Elsewhere the file can be copied to the clipboard and pasted instead.
pub fn drag_out(path: &Path) {
    #[cfg(windows)]
    {
        if let Err(e) = perch_platform_windows::dragdrop::drag_files(&[path.to_path_buf()]) {
            log::warn!("dragging {} out failed: {e}", path.display());
        }
    }
    #[cfg(not(windows))]
    log::debug!(
        "drag-out is Windows-only; click the item to copy {} and paste it",
        path.display()
    );
}

/// Flush anything that must survive exit.
pub fn shutdown(usage: &UsageTracker) {
    save_usage(usage);
}
