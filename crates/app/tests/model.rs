//! Integration tests for the application model, driven by the fake platform
//! (`perch_core::platform::fake`) and a fake clock. These cover the behaviour
//! the window shell relies on without needing a display.

use std::time::{Duration, Instant};

use perch_app::model::{Effect, Model, Now};
#[cfg(feature = "clipboard")]
use perch_core::clipboard::{ClipboardContent, ContentFlags};
use perch_core::config::Config;
use perch_core::noise::NoiseKind;
use perch_core::platform::fake::{FakePlatform, FakeScript};
use perch_core::platform::{
    AudioSession, EventSource, MediaControl, MediaEvent, Notifier, PlatformEvent, Track,
    TransportCommand,
};
use perch_core::view::{resolve_click, Action, Body, CollapsedView, SliderTarget, Tab};

fn t0() -> Now {
    Now {
        instant: Instant::now(),
        unix: 1_700_000_000,
    }
}

fn config_all() -> Config {
    let mut c = Config::default();
    let f = &mut c.features;
    f.media = true;
    f.lyrics = true;
    f.todo = true;
    f.pomodoro = true;
    f.clipboard = true;
    f.shelf = true;
    f.reminders = true;
    f.volume_mixer = true;
    c
}

/// Apply effects the way the shell would, against the fake platform.
fn carry_out(fx: &[Effect], fake: &mut FakePlatform) {
    for e in fx {
        match e {
            Effect::Transport(cmd) => {
                let _ = fake.send(*cmd);
            }
            Effect::Notify { summary, body } => {
                let _ = fake.notify(summary, body);
            }
            _ => {}
        }
    }
}

fn track(title: &str, secs: u64, playing: bool) -> Track {
    Track {
        title: title.into(),
        artist: "Artist".into(),
        album: "Album".into(),
        duration: Some(Duration::from_secs(secs)),
        position: Some(Duration::ZERO),
        playing,
        cover: None,
    }
}

#[test]
fn fake_media_backend_drives_the_collapsed_pill_and_the_media_tab() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    let mut fake = FakePlatform::default();
    fake.script(FakeScript::Track {
        title: "Song".into(),
        artist: "Band".into(),
        album: "LP".into(),
        duration: Duration::from_secs(200),
        playing: true,
    });
    let mut fx = Vec::new();
    for ev in fake.drain_events() {
        fx.extend(m.handle_event(&ev, now));
    }
    assert!(fx.contains(&Effect::Redraw));
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::FetchLyrics(r) if r.title == "Song")),
        "lyrics are fetched once per new track when the feature is on: {fx:?}"
    );
    assert_eq!(
        m.collapsed(),
        CollapsedView::NowPlaying("Band - Song".into())
    );

    let view = m.view(now.plus(Duration::from_secs(50)));
    let Body::Media(mv) = view.body else {
        panic!("media tab expected first");
    };
    assert_eq!(mv.title, "Song");
    let f = mv.fraction.unwrap_or_default();
    assert!((f - 0.25).abs() < 0.01, "50 s into 200 s is 25%, got {f}");
    assert_eq!(mv.times, Some(("0:50".into(), "3:20".into())));
}

#[test]
fn transport_actions_reach_the_backend_and_pause_freezes_the_seek_bar() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    let mut fake = FakePlatform::default();
    m.handle_event(
        &PlatformEvent::Media(MediaEvent::TrackChanged(track("A", 100, true))),
        now,
    );

    let fx = m.action(Action::Transport(TransportCommand::PlayPause), now);
    carry_out(&fx, &mut fake);
    assert_eq!(fake.sent_commands(), vec![TransportCommand::PlayPause]);

    // The player reports the pause 10 s later; the bar must stay at 10 s.
    let later = now.plus(Duration::from_secs(10));
    m.handle_event(
        &PlatformEvent::Media(MediaEvent::PlaybackChanged(false)),
        later,
    );
    let Body::Media(mv) = m.view(later.plus(Duration::from_secs(30))).body else {
        panic!("media tab");
    };
    assert_eq!(mv.times.map(|t| t.0), Some("0:10".into()));
}

#[test]
fn seek_slider_maps_to_an_absolute_position() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    m.handle_event(
        &PlatformEvent::Media(MediaEvent::TrackChanged(track("A", 200, true))),
        now,
    );
    let fx = m.action(Action::Slider(SliderTarget::Seek, 0.5), now);
    assert_eq!(
        fx,
        vec![Effect::Transport(TransportCommand::SeekTo(
            Duration::from_secs(100)
        ))]
    );
}

#[test]
fn stale_lyrics_for_a_previous_track_are_ignored() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    let fx = m.handle_event(
        &PlatformEvent::Media(MediaEvent::TrackChanged(track("A", 100, true))),
        now,
    );
    let key_a = fx
        .iter()
        .find_map(|e| match e {
            Effect::FetchLyrics(r) => Some(r.key.clone()),
            _ => None,
        })
        .unwrap_or_default();
    m.handle_event(
        &PlatformEvent::Media(MediaEvent::TrackChanged(track("B", 100, true))),
        now,
    );
    assert!(m
        .lyrics_loaded(&key_a, Some("[00:01.00]old song"))
        .is_empty());
}

#[test]
fn turning_lyrics_on_mid_song_fetches_them_for_the_current_track() {
    let now = t0();
    let mut cfg = config_all();
    cfg.features.lyrics = false;
    let mut m = Model::new(cfg.clone(), now);
    let fx = m.handle_event(
        &PlatformEvent::Media(MediaEvent::TrackChanged(track(
            "Already Playing",
            200,
            true,
        ))),
        now,
    );
    assert!(!fx.iter().any(|e| matches!(e, Effect::FetchLyrics(_))));
    cfg.features.lyrics = true;
    let fx = m.apply_config(cfg.clone());
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::FetchLyrics(r) if r.title == "Already Playing")),
        "enabling lyrics must look up the song that is already playing: {fx:?}"
    );
    // Re-applying an unchanged config must not fetch again.
    let fx = m.apply_config(cfg);
    assert!(!fx.iter().any(|e| matches!(e, Effect::FetchLyrics(_))));
}

#[test]
fn lyrics_follow_the_playhead_and_schedule_the_next_line() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    let fx = m.handle_event(
        &PlatformEvent::Media(MediaEvent::TrackChanged(track("A", 100, true))),
        now,
    );
    let key = fx
        .iter()
        .find_map(|e| match e {
            Effect::FetchLyrics(r) => Some(r.key.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let fx = m.lyrics_loaded(&key, Some("[00:00.50]first\n[00:03.00]second\n"));
    assert_eq!(fx, vec![Effect::Redraw]);
    let at = now.plus(Duration::from_secs(1));
    let Body::Media(mv) = m.view(at).body else {
        panic!()
    };
    assert_eq!(mv.lyric.as_deref(), Some("first"));
    let next = m.next_deadline(at, true).unwrap_or_default();
    assert!(
        next <= Duration::from_secs(1),
        "wakes for the seek bar / next line: {next:?}"
    );
    let Body::Media(mv) = m.view(now.plus(Duration::from_secs(4))).body else {
        panic!()
    };
    assert_eq!(mv.lyric.as_deref(), Some("second"));
}

#[test]
fn an_idle_model_has_no_deadline_so_the_loop_can_sleep() {
    let mut cfg = Config::default();
    cfg.features.todo = true;
    let now = t0();
    let m = Model::new(cfg, now);
    assert_eq!(m.next_deadline(now, false), None);
    assert_eq!(
        m.next_deadline(now, true),
        None,
        "nothing playing: no seek-bar ticks"
    );
}

#[test]
fn pomodoro_countdown_shows_in_the_pill_and_notifies_on_completion() {
    let mut cfg = config_all();
    cfg.pomodoro.work_minutes = 1;
    let now = t0();
    let mut m = Model::new(cfg, now);
    m.action(Action::PomodoroToggle, now);
    assert!(
        matches!(m.collapsed(), CollapsedView::Status(s) if s.contains("1:00") || s.contains("0:59"))
    );
    let d = m.next_deadline(now, false).unwrap_or_default();
    assert!(
        d <= Duration::from_secs(1),
        "ticks once a second while running: {d:?}"
    );

    let mut fake = FakePlatform::default();
    let fx = m.tick(now.plus(Duration::from_secs(61)));
    carry_out(&fx, &mut fake);
    assert!(
        fake.notifications()
            .iter()
            .any(|(s, _)| s.contains("Focus")),
        "{:?}",
        fake.notifications()
    );
}

#[test]
fn todo_due_time_fires_one_notification_and_is_persisted() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    let fx = m.add_todo("Call the dentist", Some(now.unix + 60), now);
    assert!(fx.contains(&Effect::SaveTodos));
    assert_eq!(
        m.next_deadline(now, false).map(|d| d.as_secs() <= 60),
        Some(true)
    );

    let mut fake = FakePlatform::default();
    let at = now.plus(Duration::from_secs(61));
    carry_out(&m.tick(at), &mut fake);
    carry_out(&m.tick(at.plus(Duration::from_secs(5))), &mut fake);
    let due: Vec<_> = fake
        .notifications()
        .into_iter()
        .filter(|(s, _)| s == "To-do due")
        .collect();
    assert_eq!(due.len(), 1, "exactly one notification: {due:?}");
}

#[test]
fn stretch_reminder_is_postponed_while_the_user_was_idle() {
    let mut cfg = config_all();
    cfg.reminders.water_minutes = 0;
    cfg.reminders.stretch_minutes = 1;
    let now = t0();
    let mut m = Model::new(cfg, now);
    let at = now.plus(Duration::from_secs(61));
    let fx = m.tick(at);
    assert!(fx.contains(&Effect::QueryIdle), "{fx:?}");
    // The user has been away for 10 minutes: not earned yet.
    let fx = m.idle_measured(Duration::from_secs(600), at);
    assert!(!fx.iter().any(|e| matches!(e, Effect::Notify { .. })));

    // Later, actively working: it fires.
    let at2 = at.plus(Duration::from_secs(601));
    let fx = m.tick(at2);
    assert!(fx.contains(&Effect::QueryIdle));
    let fx = m.idle_measured(Duration::from_secs(2), at2);
    assert!(fx
        .iter()
        .any(|e| matches!(e, Effect::Notify { summary, .. } if summary.contains("stretch"))));
}

#[test]
fn session_lock_counts_as_idle_for_reminders() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    m.handle_event(&PlatformEvent::SessionLocked(true), now);
    assert!(m.reminders().is_idle());
    m.handle_event(&PlatformEvent::SessionLocked(false), now);
    assert!(!m.reminders().is_idle());
}

#[cfg(feature = "clipboard")]
#[test]
fn password_manager_clipboard_content_is_never_recorded() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    let flags = ContentFlags::default().with_x11_target("x-kde-passwordManagerHint", "secret");
    m.clipboard_captured(ClipboardContent::text("hunter2"), &flags, now);
    assert_eq!(m.clipboard().len(), 0);
    m.clipboard_captured(
        ClipboardContent::text("hello"),
        &ContentFlags::default(),
        now,
    );
    assert_eq!(m.clipboard().len(), 1);
}

#[cfg(feature = "clipboard")]
#[test]
fn clipboard_view_rows_copy_back_and_clear() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    m.clipboard_captured(
        ClipboardContent::text("one\ntwo"),
        &ContentFlags::default(),
        now,
    );
    m.action(Action::SelectTab(Tab::Clipboard), now);
    let view = m.view(now);
    let Body::List { rows, header, .. } = view.body else {
        panic!("list")
    };
    assert_eq!(
        rows[0].text, "one two",
        "newlines are flattened for display"
    );
    let Some(Action::ClipCopy(id)) = rows[0].primary.clone() else {
        panic!()
    };
    let fx = m.action(Action::ClipCopy(id), now);
    assert!(fx
        .iter()
        .any(|e| matches!(e, Effect::ClipboardWrite(c) if c.as_text() == Some("one\ntwo"))));
    assert!(header.is_some());
    m.action(Action::ClipClear, now);
    assert!(m.clipboard().is_empty());
}

#[test]
fn dropped_files_land_on_the_shelf_and_switch_to_it() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    m.file_dropped("/tmp/report.pdf".into(), false, now);
    assert_eq!(m.shelf().len(), 1);
    assert_eq!(m.tab(), Some(Tab::Shelf));
    let id = m.shelf().items()[0].id;
    let fx = m.action(Action::ShelfCopy(id), now);
    assert!(fx
        .iter()
        .any(|e| matches!(e, Effect::ClipboardWriteFiles(p) if p.len() == 1)));
    let fx = m.prune_shelf(|_| false);
    assert_eq!(fx, vec![Effect::Redraw]);
    assert!(m.shelf().is_empty());
}

#[test]
fn dropping_a_file_with_the_shelf_off_explains_instead_of_failing() {
    let now = t0();
    let mut m = Model::new(Config::default(), now);
    m.file_dropped("/tmp/x".into(), false, now);
    assert!(matches!(m.collapsed(), CollapsedView::Message(_)));
}

#[test]
fn mixer_sliders_update_optimistically_and_emit_set_volume() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    m.sessions_captured(vec![
        AudioSession {
            id: "2".into(),
            name: "spotify".into(),
            volume: 0.5,
            muted: false,
        },
        AudioSession {
            id: "1".into(),
            name: "Firefox".into(),
            volume: 1.0,
            muted: false,
        },
    ]);
    m.action(Action::SelectTab(Tab::Mixer), now);
    let Body::Sliders { rows, .. } = m.view(now).body else {
        panic!()
    };
    assert_eq!(rows[0].label, "Firefox", "sorted case-insensitively");
    let fx = m.action(Action::Slider(SliderTarget::Volume("2".into()), 0.25), now);
    assert!(fx.contains(&Effect::SetVolume {
        id: "2".into(),
        volume: 0.25
    }));
    let fx = m.action(Action::MuteToggle("1".into()), now);
    assert!(fx.contains(&Effect::SetMute {
        id: "1".into(),
        muted: true
    }));
}

#[cfg(feature = "clipboard")]
#[test]
fn turning_a_feature_off_drops_its_state_and_tab() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    m.clipboard_captured(ClipboardContent::text("x"), &ContentFlags::default(), now);
    m.action(Action::SelectTab(Tab::Clipboard), now);
    let mut cfg = config_all();
    cfg.features.clipboard = false;
    m.apply_config(cfg);
    assert!(
        m.clipboard().is_empty(),
        "history must not linger after disabling"
    );
    assert!(!m.tabs().contains(&Tab::Clipboard));
    assert_ne!(m.tab(), Some(Tab::Clipboard));
}

#[test]
fn disabling_white_noise_silences_it() {
    let mut cfg = config_all();
    cfg.features.white_noise = true;
    let now = t0();
    let mut m = Model::new(cfg.clone(), now);
    m.action(Action::NoiseToggle(NoiseKind::Rain), now);
    // Only meaningful when the feature is compiled in; otherwise it is inert.
    cfg.features.white_noise = false;
    let fx = m.apply_config(cfg);
    assert!(!m.noise().is_audible());
    if m.noise().is_audible() {
        assert!(fx.iter().any(|e| matches!(e, Effect::Noise(_))));
    }
}

#[test]
fn cli_verbs_map_to_effects() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    assert_eq!(m.command("toggle-notch", now), vec![Effect::ToggleNotch]);
    assert_eq!(m.command("settings", now), vec![Effect::OpenSettings]);
    assert_eq!(m.command("quit", now), vec![Effect::Quit]);
    assert!(m.command("todo buy milk", now).contains(&Effect::SaveTodos));
    assert_eq!(m.todos().len(), 1);
    m.command("tab tasks", now);
    assert_eq!(m.tab(), Some(Tab::Tasks));
    assert!(m.command("definitely-not-a-verb", now).is_empty());
}

#[test]
fn clicks_resolve_through_the_rendered_hit_regions() {
    use perch_core::view::{Hit, HitKind};
    let hits = vec![
        Hit {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 20.0,
            kind: HitKind::Button(Action::OpenSettings),
        },
        Hit {
            x: 80.0,
            y: 0.0,
            w: 20.0,
            h: 20.0,
            kind: HitKind::Button(Action::PinToggle),
        },
        Hit {
            x: 0.0,
            y: 30.0,
            w: 200.0,
            h: 10.0,
            kind: HitKind::Slider(SliderTarget::Seek),
        },
    ];
    assert_eq!(resolve_click(&hits, 10.0, 10.0), Some(Action::OpenSettings));
    assert_eq!(
        resolve_click(&hits, 85.0, 10.0),
        Some(Action::PinToggle),
        "topmost wins"
    );
    assert_eq!(
        resolve_click(&hits, 150.0, 35.0),
        Some(Action::Slider(SliderTarget::Seek, 0.75))
    );
    assert_eq!(resolve_click(&hits, 500.0, 500.0), None);
}

#[test]
fn notification_failure_is_degradable() {
    let mut fake = FakePlatform::default();
    fake.fail_notifications("no daemon");
    assert!(fake.notify("x", "y").is_err());
    // The model still flashed the message in the pill, so nothing is lost.
    let now = t0();
    let mut cfg = config_all();
    cfg.reminders.water_minutes = 1;
    let mut m = Model::new(cfg, now);
    carry_out(&m.tick(now.plus(Duration::from_secs(61))), &mut fake);
    assert!(matches!(m.collapsed(), CollapsedView::Message(s) if s.contains("water")));
}
