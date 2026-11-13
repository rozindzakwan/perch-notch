//! Feature gating and pill-status priority are pure logic; tested directly
//! because "a disabled feature must not run" is a hard requirement.

use std::time::Duration;

use perch_app::features::{pill_status, soonest, ActiveFeatures, Feature, PillStatus};
use perch_core::config::Config;

#[test]
fn a_default_config_activates_no_network_feature() {
    let cfg = Config::default();
    let active = ActiveFeatures::resolve(&cfg);
    assert!(
        !active.uses_network(),
        "a default install must never touch the network, active: {:?}",
        active.active()
    );
}

#[test]
fn disabled_features_are_never_active() {
    let cfg = Config::default();
    let active = ActiveFeatures::resolve(&cfg);
    for f in [
        Feature::SpeechToText,
        Feature::Screenshot,
        Feature::CameraMirror,
        Feature::Lyrics,
        Feature::ScreenTime,
        Feature::Clipboard,
    ] {
        assert!(!active.is_active(f), "{f:?} is off by default");
    }
}

#[test]
fn media_is_active_by_default() {
    let cfg = Config::default();
    assert!(ActiveFeatures::resolve(&cfg).is_active(Feature::Media));
}

#[test]
fn lyrics_require_media_to_be_meaningful() {
    let mut cfg = Config::default();
    cfg.features.media = false;
    cfg.features.lyrics = true;
    let active = ActiveFeatures::resolve(&cfg);
    assert!(
        !active.is_active(Feature::Lyrics),
        "lyrics without a track source would fetch nothing forever"
    );
    assert!(!active.uses_network());
}

#[test]
fn enabling_lyrics_with_media_turns_the_network_on_and_says_so() {
    let mut cfg = Config::default();
    cfg.features.lyrics = true;
    let active = ActiveFeatures::resolve(&cfg);
    assert!(active.is_active(Feature::Lyrics));
    assert!(
        active.uses_network(),
        "the settings window must be able to warn the user"
    );
}

#[test]
fn a_feature_enabled_but_not_compiled_in_is_reported_as_missing() {
    let mut cfg = Config::default();
    cfg.features.speech_to_text = true;
    let active = ActiveFeatures::resolve(&cfg);
    // The default build has no `speech` cargo feature, so this must land in
    // `missing` rather than pretending to work.
    if Feature::SpeechToText.compiled_in() {
        assert!(active.is_active(Feature::SpeechToText));
    } else {
        assert!(
            active.missing().contains(&Feature::SpeechToText),
            "missing: {:?}",
            active.missing()
        );
        assert!(!active.is_active(Feature::SpeechToText));
    }
}

#[test]
fn every_feature_has_a_label_and_a_stable_network_classification() {
    for f in Feature::ALL {
        assert!(!f.label().is_empty(), "{f:?} needs a label");
    }
    assert!(Feature::Lyrics.uses_network());
    assert!(Feature::SpeechToText.uses_network(), "model download");
    assert!(!Feature::Media.uses_network());
    assert!(!Feature::Pomodoro.uses_network());
    assert!(!Feature::ScreenTime.uses_network(), "screen time is local");
    assert!(!Feature::Clipboard.uses_network(), "clipboard is local");
}

#[test]
fn a_running_countdown_beats_now_playing_in_the_collapsed_pill() {
    assert_eq!(
        pill_status(Some("24:12".into()), Some("Senja - Gurindam".into())),
        PillStatus::Countdown("24:12".into())
    );
}

#[test]
fn now_playing_shows_when_no_countdown_is_running() {
    assert_eq!(
        pill_status(None, Some("Senja - Gurindam".into())),
        PillStatus::NowPlaying("Senja - Gurindam".into())
    );
}

#[test]
fn empty_strings_do_not_count_as_something_to_show() {
    assert_eq!(pill_status(Some(String::new()), None), PillStatus::Idle);
    assert_eq!(
        pill_status(Some(String::new()), Some(String::new())),
        PillStatus::Idle
    );
    assert_eq!(pill_status(None, None), PillStatus::Idle);
}

#[test]
fn soonest_deadline_is_none_when_everything_is_idle() {
    assert_eq!(soonest(&[]), None);
    assert_eq!(soonest(&[None, None, None]), None);
}

#[test]
fn soonest_deadline_picks_the_minimum() {
    let got = soonest(&[
        Some(Duration::from_secs(30)),
        None,
        Some(Duration::from_millis(250)),
        Some(Duration::from_secs(5)),
    ]);
    assert_eq!(got, Some(Duration::from_millis(250)));
}
