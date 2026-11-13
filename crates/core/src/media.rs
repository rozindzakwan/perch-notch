//! Media session state shared by the SMTC (Windows) and MPRIS (Linux) backends.
//!
//! The platform layer reports [`MediaEvent`]s; this module folds them into the
//! state the UI renders from. Two properties matter for Perch's "do nothing
//! when nothing changed" rule:
//!
//! * [`MediaState::apply`] returns whether anything observable changed, so the
//!   event loop can skip a redraw.
//! * [`MediaState::is_same_track`] distinguishes a genuine track change from a
//!   metadata refresh, so lyrics are fetched over the network only once per
//!   track.
//!
//! Position is *estimated* from the last report plus elapsed time rather than
//! polled, and the estimator is a pure function so tests can inject elapsed
//! time instead of waiting on a real clock.

use std::time::Duration;

use crate::platform::{MediaEvent, Track, TransportCommand};

/// Identity of a track for "is this a different song?" purposes.
///
/// Deliberately built from title + artist + album only: position, playing
/// state and cover art all change constantly for the *same* song, and a change
/// in them must not trigger another lyrics fetch.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrackToken {
    title: String,
    artist: String,
    album: String,
}

impl TrackToken {
    /// The identity of `track`.
    pub fn of(track: &Track) -> Self {
        Self {
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
        }
    }
}

/// Current media session as folded from [`MediaEvent`]s.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaState {
    track: Option<Track>,
}

impl MediaState {
    /// A state with no player attached: the UI shows the idle pill.
    pub fn new() -> Self {
        Self::default()
    }

    /// The current track, or `None` when no player is attached.
    pub fn track(&self) -> Option<&Track> {
        self.track.as_ref()
    }

    /// Identity of the current track, or `None` when idle. Compare two tokens
    /// to decide whether to re-fetch lyrics.
    pub fn track_token(&self) -> Option<TrackToken> {
        self.track.as_ref().map(TrackToken::of)
    }

    /// Whether `candidate` is the same song as the current track — i.e. a
    /// metadata refresh rather than a track change. Always `false` when idle.
    pub fn is_same_track(&self, candidate: &Track) -> bool {
        self.track_token() == Some(TrackToken::of(candidate))
    }

    /// True when there is no track to render (no player, or the player went
    /// away). This is the degraded state and is always safe to be in.
    pub fn is_idle(&self) -> bool {
        self.track.is_none()
    }

    /// Whether a track is currently playing. `false` when idle.
    pub fn is_playing(&self) -> bool {
        self.track.as_ref().is_some_and(|t| t.playing)
    }

    /// `"Artist - Title"` for the current track, or `None` when idle.
    pub fn label(&self) -> Option<String> {
        self.track.as_ref().map(Track::label)
    }

    /// Fold an event into the state, returning `true` when something the UI
    /// can observe changed. `false` means the caller may skip a redraw.
    pub fn apply(&mut self, event: &MediaEvent) -> bool {
        match event {
            MediaEvent::TrackChanged(track) => {
                if self.track.as_ref() == Some(track) {
                    return false;
                }
                self.track = Some(track.clone());
                true
            }
            MediaEvent::PlaybackChanged(playing) => match self.track.as_mut() {
                Some(track) if track.playing != *playing => {
                    track.playing = *playing;
                    true
                }
                // No track, or already in that state: nothing to redraw.
                _ => false,
            },
            MediaEvent::PositionChanged(position) => match self.track.as_mut() {
                Some(track) if track.position != Some(*position) => {
                    track.position = Some(*position);
                    true
                }
                _ => false,
            },
            MediaEvent::Gone => self.track.take().is_some(),
        }
    }

    /// Estimated playback position `elapsed` after the player's last report,
    /// without polling the player.
    ///
    /// Pure: `elapsed` is injected by the caller (who owns the clock), so this
    /// is fully testable. While paused the estimate does not advance, and it is
    /// always clamped to the track duration when one is known.
    pub fn position_at(&self, elapsed: Duration) -> Option<Duration> {
        let track = self.track.as_ref()?;
        let reported = track.position.unwrap_or(Duration::ZERO);
        let estimate = if track.playing {
            reported.saturating_add(elapsed)
        } else {
            // Paused: time passing does not move the playhead.
            reported
        };
        Some(match track.duration {
            Some(duration) => estimate.min(duration),
            None => estimate,
        })
    }

    /// Seek-bar fill in `0.0..=1.0`, or `None` when the duration is unknown or
    /// zero (which would divide by zero) or there is no track.
    pub fn progress_fraction(&self) -> Option<f64> {
        let track = self.track.as_ref()?;
        let duration = track.duration?;
        if duration.is_zero() {
            return None;
        }
        let position = track.position.unwrap_or(Duration::ZERO).min(duration);
        Some(position.as_secs_f64() / duration.as_secs_f64())
    }

    /// Turn a click at `fraction` of the seek bar into a [`TransportCommand`].
    ///
    /// `fraction` is clamped to `0.0..=1.0` (NaN is treated as `0.0`, since
    /// `f64::clamp` would panic on it). Returns `None` when the duration is
    /// unknown, because there is nothing to seek within.
    pub fn seek_target(&self, fraction: f64) -> Option<TransportCommand> {
        let duration = self.track.as_ref()?.duration?;
        // Hand-rolled clamp: `f64::clamp` panics on NaN, and NaN here would
        // otherwise produce a garbage seek.
        let clamped = if fraction.is_nan() {
            0.0
        } else {
            fraction.clamp(0.0, 1.0)
        };
        Some(TransportCommand::SeekTo(duration.mul_f64(clamped)))
    }
}
