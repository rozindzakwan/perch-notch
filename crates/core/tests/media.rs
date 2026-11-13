//! Unit tests for the media session state.

use std::time::Duration;

use perch_core::media::MediaState;
use perch_core::platform::{MediaEvent, Track, TransportCommand};

/// A track fixture with a known duration and position.
fn track(title: &str, artist: &str, album: &str) -> Track {
    Track {
        title: title.to_string(),
        artist: artist.to_string(),
        album: album.to_string(),
        duration: Some(Duration::from_secs(200)),
        position: Some(Duration::from_secs(10)),
        playing: true,
        cover: None,
    }
}

#[test]
fn a_fresh_state_has_no_track() {
    let state = MediaState::new();
    assert!(state.track().is_none());
    assert!(state.is_idle(), "with no player the UI shows the idle pill");
}

#[test]
fn applying_a_track_changed_event_reports_a_change_and_stores_the_track() {
    let mut state = MediaState::new();
    let changed = state.apply(&MediaEvent::TrackChanged(track(
        "Sail",
        "Awolnation",
        "Megalithic",
    )));
    assert!(changed, "the first track is observably new");
    assert_eq!(
        state.track().map(|t| t.title.clone()),
        Some("Sail".to_string())
    );
    assert!(!state.is_idle());
}

#[test]
fn applying_the_same_playback_state_twice_reports_no_change_the_second_time() {
    let mut state = sail(); // starts playing
    assert!(
        state.apply(&MediaEvent::PlaybackChanged(false)),
        "playing -> paused is a change"
    );
    assert!(
        !state.apply(&MediaEvent::PlaybackChanged(false)),
        "paused -> paused is NOT a change: the UI must skip the redraw"
    );
    assert!(state.apply(&MediaEvent::PlaybackChanged(true)));
    assert!(!state.apply(&MediaEvent::PlaybackChanged(true)));
    assert_eq!(state.track().map(|t| t.playing), Some(true));
}

#[test]
fn playback_changed_with_no_track_is_not_a_change() {
    let mut state = MediaState::new();
    assert!(
        !state.apply(&MediaEvent::PlaybackChanged(true)),
        "no track means nothing observable changed"
    );
    assert!(state.is_idle());
}

#[test]
fn position_changed_updates_the_reported_position_and_reports_a_change() {
    let mut state = sail();
    assert!(state.apply(&MediaEvent::PositionChanged(Duration::from_secs(90))));
    assert_eq!(
        state.track().and_then(|t| t.position),
        Some(Duration::from_secs(90))
    );
    assert!(
        !state.apply(&MediaEvent::PositionChanged(Duration::from_secs(90))),
        "the same position twice is not a change"
    );
}

#[test]
fn gone_clears_the_state_and_falls_back_to_the_idle_pill() {
    let mut state = sail();
    assert!(
        state.apply(&MediaEvent::Gone),
        "losing the player is a change"
    );
    assert!(state.track().is_none());
    assert!(state.is_idle());
    assert!(state.track_token().is_none());
    assert!(state.progress_fraction().is_none(), "no track, no progress");
    assert!(
        !state.apply(&MediaEvent::Gone),
        "already gone: nothing more to clear"
    );
}

/// A state already playing the given track.
fn state_with(title: &str, artist: &str, album: &str) -> MediaState {
    let mut state = MediaState::new();
    state.apply(&MediaEvent::TrackChanged(track(title, artist, album)));
    state
}

/// The default fixture: "Sail" by Awolnation on "Megalithic".
fn sail() -> MediaState {
    state_with("Sail", "Awolnation", "Megalithic")
}

#[test]
fn position_advances_by_the_injected_elapsed_time_while_playing() {
    // Reported at 10s of a 200s track, playing.
    let state = sail();
    assert_eq!(
        state.position_at(Duration::ZERO),
        Some(Duration::from_secs(10)),
        "with no elapsed time the estimate is the last report"
    );
    assert_eq!(
        state.position_at(Duration::from_secs(5)),
        Some(Duration::from_secs(15))
    );
    assert_eq!(
        state.position_at(Duration::from_millis(2_500)),
        Some(Duration::from_millis(12_500))
    );
}

#[test]
fn position_does_not_advance_while_paused() {
    let mut state = sail();
    state.apply(&MediaEvent::PlaybackChanged(false));
    assert_eq!(
        state.position_at(Duration::from_secs(3_600)),
        Some(Duration::from_secs(10)),
        "a paused player's position is frozen no matter how much time passes"
    );
}

#[test]
fn the_estimated_position_clamps_to_the_track_duration() {
    let state = sail(); // 10s into a 200s track
    assert_eq!(
        state.position_at(Duration::from_secs(190)),
        Some(Duration::from_secs(200)),
        "exactly at the end"
    );
    assert_eq!(
        state.position_at(Duration::from_secs(10_000)),
        Some(Duration::from_secs(200)),
        "the estimate must never exceed the duration"
    );
}

#[test]
fn a_track_without_a_duration_still_estimates_a_position() {
    let mut t = track("Live Stream", "Someone", "");
    t.duration = None;
    t.position = Some(Duration::from_secs(4));
    let mut state = MediaState::new();
    state.apply(&MediaEvent::TrackChanged(t));
    assert_eq!(
        state.position_at(Duration::from_secs(6)),
        Some(Duration::from_secs(10)),
        "with no duration there is nothing to clamp against"
    );
}

#[test]
fn a_track_without_a_reported_position_estimates_from_zero() {
    let mut t = track("Sail", "Awolnation", "Megalithic");
    t.position = None;
    let mut state = MediaState::new();
    state.apply(&MediaEvent::TrackChanged(t));
    assert_eq!(
        state.position_at(Duration::from_secs(7)),
        Some(Duration::from_secs(7))
    );
}

#[test]
fn an_idle_state_has_no_estimated_position() {
    assert!(MediaState::new()
        .position_at(Duration::from_secs(5))
        .is_none());
}

#[test]
fn progress_fraction_is_the_position_over_the_duration() {
    let mut state = sail(); // 10s of 200s
    assert_eq!(state.progress_fraction(), Some(0.05));
    state.apply(&MediaEvent::PositionChanged(Duration::from_secs(100)));
    assert_eq!(state.progress_fraction(), Some(0.5));
    state.apply(&MediaEvent::PositionChanged(Duration::from_secs(200)));
    assert_eq!(state.progress_fraction(), Some(1.0));
}

#[test]
fn progress_fraction_is_none_when_the_duration_is_unknown() {
    let mut t = track("Live Stream", "Someone", "");
    t.duration = None;
    let mut state = MediaState::new();
    state.apply(&MediaEvent::TrackChanged(t));
    assert_eq!(state.progress_fraction(), None);
}

#[test]
fn progress_fraction_is_none_when_the_duration_is_zero() {
    let mut t = track("Glitch", "Someone", "");
    t.duration = Some(Duration::ZERO);
    let mut state = MediaState::new();
    state.apply(&MediaEvent::TrackChanged(t));
    assert_eq!(
        state.progress_fraction(),
        None,
        "a zero duration must not divide by zero"
    );
}

#[test]
fn progress_fraction_stays_within_zero_to_one_when_the_player_overshoots() {
    let mut t = track("Sail", "Awolnation", "Megalithic");
    t.duration = Some(Duration::from_secs(100));
    t.position = Some(Duration::from_secs(150)); // player reported past the end
    let mut state = MediaState::new();
    state.apply(&MediaEvent::TrackChanged(t));
    let f = match state.progress_fraction() {
        Some(f) => f,
        None => panic!("expected a fraction"),
    };
    assert!((0.0..=1.0).contains(&f), "fraction {f} escaped 0.0..=1.0");
    assert_eq!(f, 1.0);
}

#[test]
fn seek_target_turns_a_seek_bar_click_into_a_seek_command() {
    let state = sail(); // 200s track
    assert_eq!(
        state.seek_target(0.5),
        Some(TransportCommand::SeekTo(Duration::from_secs(100)))
    );
    assert_eq!(
        state.seek_target(0.0),
        Some(TransportCommand::SeekTo(Duration::ZERO))
    );
    assert_eq!(
        state.seek_target(1.0),
        Some(TransportCommand::SeekTo(Duration::from_secs(200)))
    );
}

#[test]
fn seek_target_clamps_out_of_range_fractions() {
    let state = sail();
    assert_eq!(
        state.seek_target(-3.0),
        Some(TransportCommand::SeekTo(Duration::ZERO)),
        "a negative fraction clamps to the start"
    );
    assert_eq!(
        state.seek_target(12.5),
        Some(TransportCommand::SeekTo(Duration::from_secs(200))),
        "a fraction above 1 clamps to the end"
    );
    assert_eq!(
        state.seek_target(f64::NAN),
        Some(TransportCommand::SeekTo(Duration::ZERO)),
        "NaN must not panic or produce a wild seek"
    );
    assert_eq!(
        state.seek_target(f64::INFINITY),
        Some(TransportCommand::SeekTo(Duration::from_secs(200)))
    );
}

#[test]
fn seek_target_is_none_without_a_known_duration() {
    let mut t = track("Live Stream", "Someone", "");
    t.duration = None;
    let mut state = MediaState::new();
    state.apply(&MediaEvent::TrackChanged(t));
    assert_eq!(state.seek_target(0.5), None, "cannot seek without a length");
    assert_eq!(
        MediaState::new().seek_target(0.5),
        None,
        "cannot seek with no player"
    );
}

#[test]
fn a_metadata_refresh_of_the_same_track_is_not_a_track_change() {
    let mut state = sail();
    let token_before = state.track_token();

    // Same title/artist/album, but the player revised position and added art.
    let mut refreshed = track("Sail", "Awolnation", "Megalithic");
    refreshed.position = Some(Duration::from_secs(42));
    refreshed.playing = false;
    refreshed.cover = Some(vec![1, 2, 3]);

    assert!(
        state.is_same_track(&refreshed),
        "identity ignores position, playing state and cover"
    );
    assert!(
        state.apply(&MediaEvent::TrackChanged(refreshed)),
        "the UI still redraws: position and art are observable"
    );
    assert_eq!(
        state.track_token(),
        token_before,
        "but the identity token is unchanged, so no lyric re-fetch"
    );
}

#[test]
fn a_different_title_is_a_track_change() {
    let mut state = sail();
    let before = state.track_token();
    let other = track("Not My Thing", "Awolnation", "Megalithic");
    assert!(!state.is_same_track(&other));
    state.apply(&MediaEvent::TrackChanged(other));
    assert_ne!(state.track_token(), before);
}

#[test]
fn a_different_album_is_a_track_change() {
    let mut state = sail();
    let before = state.track_token();
    // Same song title and artist, different album: a live version, say.
    let live = track("Sail", "Awolnation", "Live at Red Rocks");
    assert!(
        !state.is_same_track(&live),
        "album is part of the identity, so lyrics are re-fetched"
    );
    state.apply(&MediaEvent::TrackChanged(live));
    assert_ne!(state.track_token(), before);
}

#[test]
fn a_different_artist_is_a_track_change() {
    let state = sail();
    assert!(!state.is_same_track(&track("Sail", "Someone Else", "Megalithic")));
}

#[test]
fn an_idle_state_is_never_the_same_track() {
    let state = MediaState::new();
    assert!(state.track_token().is_none());
    assert!(
        !state.is_same_track(&track("Sail", "Awolnation", "Megalithic")),
        "with no current track, anything is new"
    );
}

#[test]
fn a_degraded_no_player_state_answers_every_query_without_panicking() {
    let mut state = MediaState::new();
    // Every accessor on the empty/degraded state.
    assert!(state.track().is_none());
    assert!(state.is_idle());
    assert!(!state.is_playing());
    assert!(state.track_token().is_none());
    assert!(state.position_at(Duration::from_secs(99)).is_none());
    assert!(state.progress_fraction().is_none());
    assert!(state.seek_target(0.5).is_none());
    assert!(state.label().is_none());
    // And every event applied to it.
    for event in [
        MediaEvent::PlaybackChanged(true),
        MediaEvent::PlaybackChanged(false),
        MediaEvent::PositionChanged(Duration::from_secs(5)),
        MediaEvent::Gone,
    ] {
        assert!(
            !state.apply(&event),
            "{event:?} changed nothing on an idle state"
        );
        assert!(state.is_idle());
    }
}

#[test]
fn is_playing_follows_the_track_and_is_false_when_idle() {
    let mut state = sail();
    assert!(state.is_playing());
    state.apply(&MediaEvent::PlaybackChanged(false));
    assert!(!state.is_playing());
    state.apply(&MediaEvent::Gone);
    assert!(!state.is_playing(), "no player is not playing");
}

#[test]
fn label_is_the_tracks_display_label() {
    let state = sail();
    assert_eq!(state.label(), Some("Awolnation - Sail".to_string()));
}

#[test]
fn an_empty_track_from_a_backend_is_accepted_without_panicking() {
    // Some backends briefly report a blank track; it must be representable.
    let mut state = MediaState::new();
    assert!(state.apply(&MediaEvent::TrackChanged(Track::default())));
    assert!(!state.is_idle(), "a blank track is still a track");
    assert!(state.progress_fraction().is_none());
    assert!(state.seek_target(0.5).is_none());
    assert_eq!(
        state.position_at(Duration::from_secs(3)),
        Some(Duration::ZERO)
    );
}

#[test]
fn a_track_change_resets_the_position_estimate_to_the_new_report() {
    let mut state = sail();
    let mut next = track("Megalithic Symphony", "Awolnation", "Megalithic");
    next.position = Some(Duration::from_secs(1));
    state.apply(&MediaEvent::TrackChanged(next));
    assert_eq!(
        state.position_at(Duration::from_secs(2)),
        Some(Duration::from_secs(3)),
        "the estimate is based on the NEW track's report, not the old one"
    );
}
