//! Now playing via the System Media Transport Controls (SMTC).
//!
//! Event-driven: the session manager's `CurrentSessionChanged` and the current
//! session's `MediaPropertiesChanged` / `PlaybackInfoChanged` /
//! `TimelinePropertiesChanged` events fire on WinRT thread-pool threads, which
//! read the new state and push [`MediaEvent`]s through the sink. Nothing polls.
//! Works with any app that integrates with SMTC: Spotify, browsers (media
//! sessions), VLC, the Media Player app, and so on.
//!
//! Transport commands are fired without waiting for the async result, so the
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

/// Largest cover image we read (bytes); bigger thumbnails are skipped.
const MAX_COVER: u32 = 4 * 1024 * 1024;

#[derive(Default)]
struct Bound {
    session: Option<Session>,
    tokens: Option<(Token, Token, Token)>,
}

/// SMTC-backed [`MediaControl`].
pub struct SmtcMedia {
    manager: Manager,
    manager_token: Token,
    bound: Arc<Mutex<Bound>>,
}

impl SmtcMedia {
    /// Connect to the session manager and start listening.
    pub fn start(sink: EventSink) -> Result<Self> {
        let manager = Manager::RequestAsync()
            .and_then(|op| op.get())
            .map_err(|e| Error::unavailable("media", format!("SMTC session manager: {e}")))?;
        let bound = Arc::new(Mutex::new(Bound::default()));

        let (b, s, m) = (bound.clone(), sink.clone(), manager.clone());
        let manager_token = manager
            .CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
                rebind(&m, &b, &s);
                Ok(())
            }))
            .map_err(|e| Error::backend(format!("CurrentSessionChanged: {e}")))?;

        rebind(&manager, &bound, &sink);
        Ok(Self {
            manager,
            manager_token,
            bound,
        })
    }

    fn session(&self) -> Option<Session> {
        self.bound.lock().ok().and_then(|b| b.session.clone())
    }
}

impl Drop for SmtcMedia {
    fn drop(&mut self) {
        let _ = self.manager.RemoveCurrentSessionChanged(self.manager_token);
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
