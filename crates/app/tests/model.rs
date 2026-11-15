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
use perch_core::view::{
    resolve_click, Action, Badge, BadgeKind, Body, CollapsedView, Glyph, SliderTarget, Tab,
};

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
    // Existing tests assert the system notification effect as well.
    c.notifications.system = true;
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
    assert!(
        matches!(m.collapsed(), CollapsedView::Pill { ref text, glyph: Some(Glyph::Music), .. } if text == "Band - Song"),
        "{:?}",
        m.collapsed()
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
        matches!(m.collapsed(), CollapsedView::Pill { ref text, glyph: Some(Glyph::Timer), .. } if text.contains("1:00") || text.contains("0:59"))
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
    assert!(
        m.view(now).banner.is_some(),
        "the explanation shows as a banner"
    );
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
    // The model still shows the reminder in the notch, so nothing is lost.
    let now = t0();
    let mut cfg = config_all();
    cfg.reminders.water_minutes = 1;
    let mut m = Model::new(cfg, now);
    carry_out(&m.tick(now.plus(Duration::from_secs(61))), &mut fake);
    let title = m.view(now).banner.map(|b| b.title).unwrap_or_default();
    assert!(title.contains("water"), "{title}");
}

#[cfg(feature = "mail")]
mod mail {
    use super::*;
    use perch_core::config::MailAccount;
    use perch_core::mail::{AccountStatus, MailItem, MailUpdate};

    fn cfg() -> Config {
        let mut c = config_all();
        c.features.mail = true;
        c.mail.accounts.push(MailAccount {
            name: "Work".into(),
            username: "user@example.com".into(),
            host: "mail.example.com".into(),
            ..MailAccount::default()
        });
        c
    }

    fn snap(uids: &[u32]) -> MailUpdate {
        MailUpdate::Snapshot {
            account: "Work".into(),
            uid_validity: 1,
            unread: uids.len() as u32,
            items: uids
                .iter()
                .rev()
                .map(|u| MailItem {
                    uid: *u,
                    from: format!("Sender {u}"),
                    subject: format!("Subject {u}"),
                    date_unix: Some(1_700_000_000),
                })
                .collect(),
        }
    }

    fn notifications(fx: &[Effect]) -> Vec<(String, String)> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::Notify { summary, body } => Some((summary.clone(), body.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_inbox_shows_in_the_tab_and_pill_and_only_new_mail_notifies() {
        let now = t0();
        let mut m = Model::new(cfg(), now);
        assert!(m.tabs().contains(&Tab::Mail));
        assert!(
            notifications(&m.mail_update(snap(&[1, 2]))).is_empty(),
            "startup is quiet"
        );
        assert!(
            matches!(m.collapsed(), CollapsedView::Pill { ref badges, .. } if badges == &[Badge { kind: BadgeKind::Mail, count: 2 }]),
            "{:?}",
            m.collapsed()
        );
        let fx = m.mail_update(snap(&[1, 2, 3]));
        assert_eq!(
            notifications(&fx),
            vec![("New mail from Sender 3".into(), "Subject 3".into())]
        );
        let fx = m.mail_update(snap(&[1, 2, 3, 4, 5]));
        assert_eq!(notifications(&fx)[0].0, "2 new messages");

        m.action(Action::SelectTab(Tab::Mail), now);
        let Body::List { rows, .. } = m.view(now).body else {
            panic!("mail tab is a list")
        };
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].text, "Sender 5");
        assert_eq!(rows[0].subtitle.as_deref(), Some("Subject 5"));
        assert!(
            rows.iter()
                .all(|r| r.primary.is_none() && r.remove.is_none()),
            "read-only rows"
        );
    }

    #[test]
    fn problems_are_listed_and_subjects_can_stay_out_of_notifications() {
        let now = t0();
        let mut c = cfg();
        c.mail.notify_subject = false;
        let mut m = Model::new(c, now);
        m.mail_update(MailUpdate::Status {
            account: "Work".into(),
            status: AccountStatus::AuthFailed("Invalid credentials".into()),
        });
        m.action(Action::SelectTab(Tab::Mail), now);
        let Body::List { rows, .. } = m.view(now).body else {
            panic!("mail tab is a list")
        };
        assert!(
            rows[0].dim && rows[0].text.contains("sign-in failed"),
            "{:?}",
            rows[0]
        );
        m.mail_update(snap(&[1]));
        let fx = m.mail_update(snap(&[1, 2]));
        assert_eq!(
            notifications(&fx),
            vec![("New mail from Sender 2".into(), String::new())]
        );
    }

    #[test]
    fn turning_mail_off_forgets_everything() {
        let now = t0();
        let mut m = Model::new(cfg(), now);
        m.mail_update(snap(&[1, 2]));
        let mut c = cfg();
        c.features.mail = false;
        m.apply_config(c);
        assert_eq!(m.mail().total_unread(), 0);
        assert!(m.mail().accounts().is_empty());
        assert!(!m.tabs().contains(&Tab::Mail));
        assert!(
            m.mail_update(snap(&[1, 2, 3])).is_empty(),
            "updates are ignored while off"
        );
    }
}

#[test]
fn cover_change_computes_accent_once_and_lyrics_expose_the_next_line() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    let mut img = image::RgbaImage::new(32, 32);
    for (x, _, p) in img.enumerate_pixels_mut() {
        *p = if x < 16 {
            image::Rgba([0xFF, 0x7A, 0x59, 255])
        } else {
            image::Rgba([0xC4, 0x31, 0x7A, 255])
        };
    }
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap_or_else(|e| panic!("{e}"));
    let mut t = track("Song", 200, true);
    t.cover = Some(png);
    let fx = m.handle_event(&PlatformEvent::Media(MediaEvent::TrackChanged(t)), now);
    let key = fx
        .iter()
        .find_map(|e| match e {
            Effect::FetchLyrics(r) => Some(r.key.clone()),
            _ => None,
        })
        .unwrap_or_default();
    m.lyrics_loaded(
        &key,
        Some("[00:00.00]line one\n[00:10.00]line two\n[00:20.00]line three"),
    );
    m.action(Action::SelectTab(Tab::Media), now);
    let Body::Media(v) = m.view(now).body else {
        panic!("media tab")
    };
    assert!(v.accent.is_some(), "accent from the cover");
    assert_eq!(v.lyric.as_deref(), Some("line one"));
    assert_eq!(v.next_lyric.as_deref(), Some("line two"));
}

#[test]
fn panel_height_follows_the_tab_within_the_maximum() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    m.action(Action::SelectTab(Tab::Media), now);
    let media = m.panel_height_hint();
    for i in 0..30 {
        m.clipboard_captured(
            ClipboardContent::text(format!("clip {i}")),
            &ContentFlags::default(),
            now,
        );
    }
    m.action(Action::SelectTab(Tab::Clipboard), now);
    let clips = m.panel_height_hint();
    assert!((200..=300).contains(&media), "media {media}");
    assert_eq!(clips, 300, "a long list is capped at expanded_height");
}

#[cfg(feature = "mail")]
#[test]
fn mail_rows_have_avatar_sender_subject_age() {
    use perch_core::config::MailAccount;
    use perch_core::mail::{MailItem, MailUpdate};
    use perch_core::view::RowLeading;
    let now = t0();
    let mut cfg = config_all();
    cfg.features.mail = true;
    cfg.mail.accounts.push(MailAccount {
        name: "Work".into(),
        username: "user@example.com".into(),
        host: "mail.example.com".into(),
        ..MailAccount::default()
    });
    let mut m = Model::new(cfg, now);
    m.mail_update(MailUpdate::Snapshot {
        account: "Work".into(),
        uid_validity: 1,
        unread: 1,
        items: vec![MailItem {
            uid: 1,
            from: "Ann Lee".into(),
            subject: "Quarterly report".into(),
            date_unix: Some(now.unix - 120),
        }],
    });
    m.action(Action::SelectTab(Tab::Mail), now);
    let Body::List { rows, .. } = m.view(now).body else {
        panic!("mail is a list")
    };
    let r = &rows[0];
    assert_eq!(r.text, "Ann Lee");
    assert_eq!(r.subtitle.as_deref(), Some("Quarterly report"));
    assert_eq!(r.detail, "2m");
    assert!(
        matches!(&r.leading, RowLeading::Avatar { initials, .. } if initials == "AL"),
        "{:?}",
        r.leading
    );
}

#[cfg(feature = "mail")]
mod banners {
    use super::*;
    use perch_core::config::MailAccount;
    use perch_core::mail::{MailItem, MailUpdate};

    fn cfg(system: bool, subject: bool) -> Config {
        let mut c = config_all();
        c.features.mail = true;
        c.notifications.in_notch = true;
        c.notifications.system = system;
        c.mail.notify_subject = subject;
        c.mail.accounts.push(MailAccount {
            name: "Work".into(),
            username: "user@example.com".into(),
            host: "mail.example.com".into(),
            ..MailAccount::default()
        });
        c
    }

    fn snap(uids: &[u32]) -> MailUpdate {
        MailUpdate::Snapshot {
            account: "Work".into(),
            uid_validity: 1,
            unread: uids.len() as u32,
            items: uids
                .iter()
                .rev()
                .map(|u| MailItem {
                    uid: *u,
                    from: format!("Sender {u}"),
                    subject: format!("Subject {u}"),
                    date_unix: Some(1_700_000_000),
                })
                .collect(),
        }
    }

    fn banner_title(m: &mut Model, now: Now) -> Option<String> {
        m.view(now).banner.map(|b| b.title)
    }

    #[test]
    fn no_banner_while_expanded_but_badges_update() {
        let now = t0();
        let mut m = Model::new(cfg(false, true), now);
        m.mail_update(snap(&[1]));
        m.set_panel_state(true, true, now);
        m.mail_update(snap(&[1, 2]));
        assert_eq!(
            banner_title(&mut m, now),
            None,
            "the open panel shows it in place"
        );
        assert!(
            matches!(m.collapsed(), CollapsedView::Pill { ref badges, .. } if badges[0].count == 2)
        );
    }

    #[test]
    fn fullscreen_defers_only_the_latest_banner() {
        let now = t0();
        let mut m = Model::new(cfg(false, true), now);
        m.set_panel_state(false, false, now);
        m.flash("first", now);
        m.flash("second", now);
        assert_eq!(
            banner_title(&mut m, now),
            None,
            "nothing pops up over a fullscreen app"
        );
        m.set_panel_state(false, true, now);
        assert_eq!(banner_title(&mut m, now).as_deref(), Some("second"));
        m.tick(now.plus(Duration::from_secs(5)));
        assert_eq!(
            banner_title(&mut m, now.plus(Duration::from_secs(5))),
            None,
            "only the latest was kept"
        );
    }

    #[test]
    fn banner_click_opens_its_tab() {
        let now = t0();
        let mut m = Model::new(cfg(false, true), now);
        m.mail_update(snap(&[1]));
        m.mail_update(snap(&[1, 2]));
        assert_eq!(
            banner_title(&mut m, now).as_deref(),
            Some("New mail from Sender 2")
        );
        let fx = m.action(Action::BannerOpen, now);
        assert!(fx.contains(&Effect::OpenPanel), "{fx:?}");
        assert_eq!(m.tab(), Some(Tab::Mail));
        assert_eq!(banner_title(&mut m, now), None, "the banner is dismissed");
    }

    #[test]
    fn notify_subject_false_hides_subject_in_banner() {
        let now = t0();
        let mut m = Model::new(cfg(false, false), now);
        m.mail_update(snap(&[1]));
        m.mail_update(snap(&[1, 2]));
        let b = m.view(now).banner.unwrap_or_else(|| panic!("banner"));
        assert_eq!(b.title, "New mail from Sender 2");
        assert_eq!(b.body, "", "no subject");
    }

    #[test]
    fn system_toast_only_when_enabled() {
        let now = t0();
        let toast = |fx: &[Effect]| fx.iter().any(|e| matches!(e, Effect::Notify { .. }));
        let mut quiet = Model::new(cfg(false, true), now);
        quiet.mail_update(snap(&[1]));
        let fx = quiet.mail_update(snap(&[1, 2]));
        assert!(!toast(&fx), "in-notch only by default");
        assert!(banner_title(&mut quiet, now).is_some());
        let mut both = Model::new(cfg(true, true), now);
        both.mail_update(snap(&[1]));
        let fx = both.mail_update(snap(&[1, 2]));
        assert!(toast(&fx), "system notifications when enabled");
        assert!(banner_title(&mut both, now).is_some());
    }
}

#[test]
fn ipc_todo_takes_an_optional_relative_due_time() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    m.command("todo +2s Tea is ready", now);
    m.command("todo Plain item", now);
    m.command("todo +lots of work", now);
    let items: Vec<(String, Option<u64>)> = m
        .todos()
        .all()
        .iter()
        .map(|i| (i.text.clone(), i.due))
        .collect();
    assert!(
        items.contains(&("Tea is ready".to_string(), Some(now.unix + 2))),
        "{items:?}"
    );
    assert!(
        items.contains(&("Plain item".to_string(), None)),
        "{items:?}"
    );
    assert!(
        items.contains(&("+lots of work".to_string(), None)),
        "not a due time: kept as text: {items:?}"
    );
}

#[test]
fn dragging_the_seek_bar_previews_and_seeks_once_on_release() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    m.handle_event(
        &PlatformEvent::Media(MediaEvent::TrackChanged(track("A", 200, true))),
        now,
    );
    for v in [0.2, 0.4, 0.6] {
        let fx = m.drag_slider(SliderTarget::Seek, v, now);
        assert!(
            !fx.iter().any(|e| matches!(e, Effect::Transport(_))),
            "no seek while dragging: {fx:?}"
        );
    }
    m.action(Action::SelectTab(Tab::Media), now);
    match m.view(now).body {
        Body::Media(mv) => {
            assert!(
                (mv.fraction.unwrap_or(0.0) - 0.6).abs() < 1e-6,
                "preview shown"
            );
            assert_eq!(mv.times.map(|t| t.0), Some("2:00".to_string()));
        }
        other => panic!("{other:?}"),
    }
    let fx = m.end_drag(SliderTarget::Seek, 0.75, now);
    assert_eq!(
        fx.iter()
            .filter(|e| matches!(e, Effect::Transport(_)))
            .collect::<Vec<_>>(),
        vec![&Effect::Transport(TransportCommand::SeekTo(
            Duration::from_secs(150)
        ))]
    );
}

#[test]
fn dragging_a_volume_slider_applies_live() {
    let now = t0();
    let mut m = Model::new(config_all(), now);
    m.sessions_captured(vec![AudioSession {
        id: "spotify".into(),
        name: "Spotify".into(),
        volume: 0.8,
        muted: false,
    }]);
    let fx = m.drag_slider(SliderTarget::Volume("spotify".into()), 0.3, now);
    assert!(fx.contains(&Effect::SetVolume {
        id: "spotify".into(),
        volume: 0.3
    }));
    let fx = m.end_drag(SliderTarget::Volume("spotify".into()), 0.35, now);
    assert!(fx.contains(&Effect::SetVolume {
        id: "spotify".into(),
        volume: 0.35
    }));
}

#[test]
fn system_volume_is_the_first_mixer_row_with_a_speaker_icon() {
    use perch_core::platform::SYSTEM_VOLUME_ID;
    use perch_core::view::{RowKind, RowLeading};
    let now = t0();
    let mut m = Model::new(config_all(), now);
    let s = |id: &str, name: &str, v: f32| AudioSession {
        id: id.into(),
        name: name.into(),
        volume: v,
        muted: false,
    };
    m.sessions_captured(vec![
        s("chrome", "Chrome", 1.0),
        s(SYSTEM_VOLUME_ID, "System volume", 0.3),
        s("aimp", "AIMP", 0.5),
    ]);
    m.action(Action::SelectTab(Tab::Mixer), now);
    let Body::Sliders { rows, .. } = m.view(now).body else {
        panic!("mixer body")
    };
    assert_eq!(rows[0].label, "System volume");
    assert!((rows[0].value - 0.3).abs() < 1e-6);
    assert_eq!(rows[0].leading, RowLeading::Kind(RowKind::Speaker));
    assert_eq!(rows[1].label, "AIMP", "apps follow, sorted by name");
}

#[test]
fn the_collapsed_pill_shows_the_current_lyric_line_live() {
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
    m.lyrics_loaded(&key, Some("[00:00.50]first line\n[00:03.00]second line\n"));
    let text = |m: &Model| match m.collapsed() {
        CollapsedView::Pill { text, .. } => text,
        other => panic!("{other:?}"),
    };
    let at = now.plus(Duration::from_secs(1));
    m.tick(at);
    assert_eq!(text(&m), "first line");
    assert!(m.pill_shows_lyric());
    // Collapsed, the loop still wakes for the next line (and only then).
    let next = m.next_deadline(at, false).unwrap_or_default();
    assert!(
        next > Duration::from_millis(1500) && next <= Duration::from_secs(2),
        "next line at 3 s: {next:?}"
    );
    let fx = m.tick(now.plus(Duration::from_millis(3_100)));
    assert!(fx.contains(&Effect::Redraw), "a new line redraws the pill");
    assert_eq!(text(&m), "second line");
    // Paused: back to the track title, no lyric wake-ups.
    m.handle_event(
        &PlatformEvent::Media(MediaEvent::PlaybackChanged(false)),
        now.plus(Duration::from_secs(4)),
    );
    m.tick(now.plus(Duration::from_secs(4)));
    assert!(!m.pill_shows_lyric());
}

fn foreign(app: &str, title: &str, body: &str) -> PlatformEvent {
    PlatformEvent::ForeignNotification(perch_core::platform::ForeignNotification {
        app: app.into(),
        title: title.into(),
        body: body.into(),
    })
}

#[test]
fn other_apps_notifications_become_banners_when_mirroring_is_on() {
    let now = t0();
    let mut cfg = config_all();
    cfg.features.notification_mirror = true;
    let mut m = Model::new(cfg.clone(), now);
    m.set_panel_state(false, true, now);
    let fx = m.handle_event(&foreign("WhatsApp", "Ann", "See you at 8"), now);
    assert!(fx.contains(&Effect::Redraw));
    let b = m.view(now).banner.unwrap_or_else(|| panic!("a banner"));
    assert_eq!(b.app.as_deref(), Some("WhatsApp"));
    assert_eq!((b.title.as_str(), b.body.as_str()), ("Ann", "See you at 8"));
    assert!(
        !fx.iter().any(|e| matches!(e, Effect::Notify { .. })),
        "never echoed back as a system toast"
    );

    // Off (the default), or with in-notch notifications off: nothing.
    for c in [config_all(), {
        let mut c = cfg.clone();
        c.notifications.in_notch = false;
        c
    }] {
        let mut m = Model::new(c, now);
        m.set_panel_state(false, true, now);
        m.handle_event(&foreign("WhatsApp", "Ann", "hi"), now);
        assert!(m.view(now).banner.is_none());
    }
}
