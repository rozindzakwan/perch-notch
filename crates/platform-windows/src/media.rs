//! Now playing via the System Media Transport Controls (SMTC).
//!
//! Event-driven: the session manager's `CurrentSessionChanged` and the current
//! session's `MediaPropertiesChanged` / `PlaybackInfoChanged` /
//! `TimelinePropertiesChanged` events fire on WinRT thread-pool threads, which
//! read the new state and push [`MediaEvent`]s through the sink. Nothing polls.
//! Works with any app that integrates with SMTC: Spotify, browsers (media
//! sessions), VLC, the Media Player app, and so on.
//!
//! Transport commands are fired without waiting for the async result, and the
//! blocking setup runs on its own MTA thread (see [`SmtcMedia::start`]), so the
//! UI thread never blocks on a player.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use perch_core::error::{Error, Result};
use perch_core::platform::{
    EventSink, MediaControl, MediaEvent, PlatformEvent, Track, TransportCommand,
};
use windows::Foundation::{EventRegistrationToken as Token, TypedEventHandler};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::Com::{CoIncrementMTAUsage, CoInitializeEx, COINIT_MULTITHREADED};

/// Largest cover image we read (bytes); bigger thumbnails are skipped.
const MAX_COVER: u32 = 4 * 1024 * 1024;

#[derive(Default)]
struct Bound {
    session: Option<Session>,
    tokens: Option<(Token, Token, Token)>,
}

/// The session manager and our `CurrentSessionChanged` registration, filled in
/// by the init thread once SMTC is connected.
struct Live {
    manager: Manager,
    token: Token,
}

#[derive(Default)]
struct Shared {
    live: Option<Live>,
    /// Set by `Drop`; the init thread must not register after this.
    stopped: bool,
}

/// SMTC-backed [`MediaControl`].
pub struct SmtcMedia {
    shared: Arc<Mutex<Shared>>,
    bound: Arc<Mutex<Bound>>,
}

impl SmtcMedia {
    /// Connect to the session manager and start listening.
    ///
    /// Returns at once. Connecting and the first track read (including the
    /// cover thumbnail) block on WinRT async operations, and blocking on those
    /// from the UI thread, a COM single-threaded apartment, deadlocks whenever
    /// the operation has to call back into that apartment (Spotify's cover art
    /// does). So that work runs on its own multithreaded-apartment thread, and
    /// the result arrives through the sink like every later change.
    pub fn start(sink: EventSink) -> Result<Self> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let bound = Arc::new(Mutex::new(Bound::default()));
        let (sh, b) = (shared.clone(), bound.clone());
        std::thread::Builder::new()
            .name("perch-smtc-init".into())
            .spawn(move || connect(&sh, &b, &sink))
            .map_err(|e| Error::backend(format!("SMTC init thread: {e}")))?;
        Ok(Self { shared, bound })
    }

    fn session(&self) -> Option<Session> {
        self.bound.lock().ok().and_then(|b| b.session.clone())
    }
}

/// Runs on the init thread: join the MTA, connect, subscribe, report.
fn connect(shared: &Arc<Mutex<Shared>>, bound: &Arc<Mutex<Bound>>, sink: &EventSink) {
    // SAFETY: this fresh thread joins the process-wide MTA; blocking `.get()`
    // calls are safe there. `CoIncrementMTAUsage` keeps the MTA alive after
    // this thread exits (event callbacks run on MTA thread-pool threads); its
    // cookie is deliberately never released.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = CoIncrementMTAUsage();
    }
    let manager = match Manager::RequestAsync().and_then(|op| op.get()) {
        Ok(m) => m,
        Err(e) => {
            log::warn!("now-playing unavailable: SMTC session manager: {e}");
            return;
        }
    };
    let (b, s, m) = (bound.clone(), sink.clone(), manager.clone());
    let token = match manager.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
        rebind(&m, &b, &s);
        Ok(())
    })) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("now-playing unavailable: CurrentSessionChanged: {e}");
            return;
        }
    };
    {
        let Ok(mut sh) = shared.lock() else { return };
        if sh.stopped {
            // The backend was dropped while we were connecting.
            let _ = manager.RemoveCurrentSessionChanged(token);
            return;
        }
        sh.live = Some(Live {
            manager: manager.clone(),
            token,
        });
    }
    rebind(&manager, bound, sink);
    // Dropped meanwhile? Undo the session subscription rebind just made.
    if shared.lock().map(|sh| sh.stopped).unwrap_or(true) {
        if let Ok(mut b) = bound.lock() {
            unbind(&mut b);
        }
    }
}

impl Drop for SmtcMedia {
    fn drop(&mut self) {
        if let Ok(mut sh) = self.shared.lock() {
            sh.stopped = true;
            if let Some(live) = sh.live.take() {
                let _ = live.manager.RemoveCurrentSessionChanged(live.token);
            }
        }
        if let Ok(mut b) = self.bound.lock() {
            unbind(&mut b);
        }
    }
}

fn unbind(b: &mut Bound) {
    if let (Some(s), Some((p, pb, t))) = (&b.session, b.tokens) {
        let _ = s.RemoveMediaPropertiesChanged(p);
        let _ = s.RemovePlaybackInfoChanged(pb);
        let _ = s.RemoveTimelinePropertiesChanged(t);
    }
    b.session = None;
    b.tokens = None;
}

/// Switch our listeners to the manager's current session and report it.
fn rebind(manager: &Manager, bound: &Arc<Mutex<Bound>>, sink: &EventSink) {
    let current = manager.GetCurrentSession().ok();
    let Ok(mut b) = bound.lock() else { return };
    let same = match (&b.session, &current) {
        (Some(a), Some(c)) => a == c,
        (None, None) => true,
        _ => false,
    };
    if same && b.tokens.is_some() {
        return;
    }
    unbind(&mut b);
    let Some(session) = current else {
        drop(b);
        sink(PlatformEvent::Media(MediaEvent::Gone));
        return;
    };

    let s1 = sink.clone();
    let props = session.MediaPropertiesChanged(&TypedEventHandler::new(
        move |sess: &Option<Session>, _| {
            if let Some(sess) = sess {
                report_track(sess, &s1);
            }
            Ok(())
        },
    ));
    let s2 = sink.clone();
    let playback =
        session.PlaybackInfoChanged(&TypedEventHandler::new(move |sess: &Option<Session>, _| {
            if let Some(sess) = sess {
                let playing = is_playing(sess);
                s2(PlatformEvent::Media(MediaEvent::PlaybackChanged(playing)));
                if let Some(pos) = position(sess, playing) {
                    s2(PlatformEvent::Media(MediaEvent::PositionChanged(pos)));
                }
            }
            Ok(())
        }));
    let s3 = sink.clone();
    let timeline = session.TimelinePropertiesChanged(&TypedEventHandler::new(
        move |sess: &Option<Session>, _| {
            if let Some(sess) = sess {
                if let Some(pos) = position(sess, is_playing(sess)) {
                    s3(PlatformEvent::Media(MediaEvent::PositionChanged(pos)));
                }
            }
            Ok(())
        },
    ));
    b.tokens = match (props, playback, timeline) {
        (Ok(p), Ok(pb), Ok(t)) => Some((p, pb, t)),
        _ => {
            log::warn!("could not subscribe to every SMTC session event");
            None
        }
    };
    b.session = Some(session.clone());
    drop(b);
    report_track(&session, sink);
}

fn is_playing(s: &Session) -> bool {
    s.GetPlaybackInfo()
        .and_then(|i| i.PlaybackStatus())
        .map(|st| st == Status::Playing)
        .unwrap_or(false)
}

/// 100 ns ticks to a `Duration`.
fn ticks(t: i64) -> Duration {
    Duration::from_nanos((t.max(0) as u64).saturating_mul(100))
}

/// Track length from the session timeline.
fn duration(s: &Session) -> Option<Duration> {
    let tl = s.GetTimelineProperties().ok()?;
    let end = tl.EndTime().ok()?.Duration;
    let start = tl.StartTime().ok()?.Duration;
    let d = end - start;
    (d > 0).then(|| ticks(d))
}

/// Current position. SMTC reports the position as of `LastUpdatedTime`; when
/// playing, add the time elapsed since then.
fn position(s: &Session, playing: bool) -> Option<Duration> {
    let tl = s.GetTimelineProperties().ok()?;
    let pos = tl.Position().ok()?.Duration;
    let mut p = ticks(pos);
    if playing {
        if let Ok(updated) = tl.LastUpdatedTime() {
            // DateTime is 100 ns ticks since 1601-01-01 UTC.
            let now_1601 = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64 / 100 + 116_444_736_000_000_000)
                .unwrap_or(0);
            let since = now_1601 - updated.UniversalTime;
            if (0..36_000_000_000).contains(&since) {
                p += ticks(since);
            }
        }
    }
    Some(p)
}

fn read_track(s: &Session) -> Option<Track> {
    let props = s.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let title = props.Title().map(|h| h.to_string()).unwrap_or_default();
    let artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();
    let album = props
        .AlbumTitle()
        .map(|h| h.to_string())
        .unwrap_or_default();
    if title.is_empty() && artist.is_empty() {
        return None;
    }
    let playing = is_playing(s);
    let cover = props.Thumbnail().ok().and_then(|t| {
        let stream = t.OpenReadAsync().ok()?.get().ok()?;
        let size = u32::try_from(stream.Size().ok()?).ok()?;
        if size == 0 || size > MAX_COVER {
            return None;
        }
        let reader = DataReader::CreateDataReader(&stream).ok()?;
        reader.LoadAsync(size).ok()?.get().ok()?;
        let mut buf = vec![0u8; size as usize];
        reader.ReadBytes(&mut buf).ok()?;
        Some(buf)
    });
    Some(Track {
        title,
        artist,
        album,
        duration: duration(s),
        position: position(s, playing),
        playing,
        cover,
    })
}

fn report_track(s: &Session, sink: &EventSink) {
    match read_track(s) {
        Some(t) => sink(PlatformEvent::Media(MediaEvent::TrackChanged(t))),
        None => sink(PlatformEvent::Media(MediaEvent::Gone)),
    }
}

impl MediaControl for SmtcMedia {
    fn now_playing(&self) -> Option<Track> {
        self.session().and_then(|s| read_track(&s))
    }

    fn send(&mut self, cmd: TransportCommand) -> Result<()> {
        let s = self
            .session()
            .ok_or_else(|| Error::unavailable("media", "no player"))?;
        // Fire and forget: the async operation completes on its own, and the
        // resulting state change arrives through the session events.
        let r = match cmd {
            TransportCommand::Play => s.TryPlayAsync().map(drop),
            TransportCommand::Pause => s.TryPauseAsync().map(drop),
            TransportCommand::PlayPause => s.TryTogglePlayPauseAsync().map(drop),
            TransportCommand::Next => s.TrySkipNextAsync().map(drop),
            TransportCommand::Previous => s.TrySkipPreviousAsync().map(drop),
            TransportCommand::SeekTo(pos) => {
                let t = i64::try_from(pos.as_nanos() / 100).unwrap_or(i64::MAX);
                s.TryChangePlaybackPositionAsync(t).map(drop)
            }
        };
        r.map_err(|e| Error::backend(format!("SMTC command {cmd:?}: {e}")))
    }
}
