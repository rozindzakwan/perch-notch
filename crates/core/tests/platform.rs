//! RED-phase tests for the platform trait layer, driven through the fakes.

use std::time::Duration;

use perch_core::platform::fake::{FakePlatform, FakeScript};
use perch_core::platform::{
    Autostart, EventSource, FullscreenWatch, HotZoneWatch, MediaControl, MediaEvent, MonitorSource,
    Notifier, PlatformEvent, SingleInstance, TransportCommand,
};

#[test]
fn fake_platform_reports_the_monitors_it_was_given() {
    let p = FakePlatform::with_monitors(vec![
        perch_core::geometry::Monitor {
            id: 0,
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            primary: true,
            work: None,
        },
        perch_core::geometry::Monitor {
            id: 1,
            x: 1920,
            y: 0,
            width: 2560,
            height: 1440,
            scale: 1.5,
            primary: false,
            work: None,
        },
    ]);
    let monitors = match p.monitors() {
        Ok(m) => m,
        Err(e) => panic!("monitors: {e}"),
    };
    assert_eq!(monitors.len(), 2);
    assert!(monitors[0].primary);
    assert_eq!(monitors[1].scale, 1.5);
}

#[test]
fn primary_monitor_is_chosen_when_no_index_is_configured() {
    let p = FakePlatform::default();
    let m = match p.monitor_for(None) {
        Ok(m) => m,
        Err(e) => panic!("monitor_for: {e}"),
    };
    assert!(m.primary);
}

#[test]
fn configured_monitor_index_out_of_range_falls_back_to_primary() {
    let p = FakePlatform::default();
    let m = match p.monitor_for(Some(99)) {
        Ok(m) => m,
        Err(e) => panic!("must not fail on a stale monitor index: {e}"),
    };
    assert!(m.primary, "falls back instead of erroring");
}

#[test]
fn hot_zone_watch_delivers_enter_and_leave_as_events() {
    let mut p = FakePlatform::default();
    p.script(FakeScript::HotZoneEnter);
    p.script(FakeScript::HotZoneLeave);
    let events = p.drain_events();
    assert_eq!(
        events,
        vec![PlatformEvent::HotZoneEntered, PlatformEvent::HotZoneLeft]
    );
}

#[test]
fn hot_zone_watch_never_reports_a_cursor_position_poll() {
    let p = FakePlatform::default();
    assert_eq!(
        p.cursor_polls(),
        0,
        "the hot-zone implementation must be event-driven"
    );
}

#[test]
fn fullscreen_watch_reports_transitions_only() {
    let mut p = FakePlatform::default();
    assert!(!p.is_fullscreen_active());
    p.script(FakeScript::Fullscreen(true));
    p.script(FakeScript::Fullscreen(true));
    p.script(FakeScript::Fullscreen(false));
    let events = p.drain_events();
    assert_eq!(
        events,
        vec![
            PlatformEvent::FullscreenChanged(true),
            PlatformEvent::FullscreenChanged(false)
        ],
        "a repeated identical state is not an event"
    );
}

#[test]
fn media_backend_starts_empty_and_reports_now_playing_changes() {
    let mut p = FakePlatform::default();
    assert!(p.now_playing().is_none());

    p.script(FakeScript::Track {
        title: "Gurindam".into(),
        artist: "Senja".into(),
        album: "Pelangi".into(),
        duration: Duration::from_secs(215),
        playing: true,
    });
    let events = p.drain_events();
    assert_eq!(events.len(), 1);
    match &events[0] {
        PlatformEvent::Media(MediaEvent::TrackChanged(t)) => {
            assert_eq!(t.title, "Gurindam");
            assert_eq!(t.artist, "Senja");
            assert_eq!(t.duration, Some(Duration::from_secs(215)));
            assert!(t.playing);
        }
        other => panic!("expected a track change, got {other:?}"),
    }
    let np = match p.now_playing() {
        Some(np) => np,
        None => panic!("now_playing should be populated"),
    };
    assert_eq!(np.title, "Gurindam");
}

#[test]
fn transport_commands_are_recorded_and_reflected_in_state() {
    let mut p = FakePlatform::default();
    p.script(FakeScript::Track {
        title: "A".into(),
        artist: "B".into(),
        album: "C".into(),
        duration: Duration::from_secs(100),
        playing: true,
    });
    p.drain_events();

    if let Err(e) = p.send(TransportCommand::Pause) {
        panic!("pause: {e}");
    }
    assert_eq!(p.sent_commands(), vec![TransportCommand::Pause]);
    let np = match p.now_playing() {
        Some(np) => np,
        None => panic!("still playing something"),
    };
    assert!(!np.playing, "pause is reflected in the state");

    if let Err(e) = p.send(TransportCommand::Next) {
        panic!("next: {e}");
    }
    assert_eq!(
        p.sent_commands(),
        vec![TransportCommand::Pause, TransportCommand::Next]
    );
}

#[test]
fn media_backend_absence_degrades_instead_of_failing_the_app() {
    let mut p = FakePlatform::default();
    p.fail_media("no player running");
    let err = match p.send(TransportCommand::Play) {
        Ok(()) => panic!("expected a failure"),
        Err(e) => e,
    };
    assert!(
        err.is_degradable(),
        "a missing player is a degradable condition, not fatal: {err}"
    );
}

#[test]
fn notifications_are_recorded_and_failures_are_degradable() {
    let mut p = FakePlatform::default();
    if let Err(e) = p.notify("Perch", "Time to drink water") {
        panic!("notify: {e}");
    }
    assert_eq!(
        p.notifications(),
        vec![("Perch".to_string(), "Time to drink water".to_string())]
    );

    p.fail_notifications("no notification daemon");
    let err = match p.notify("Perch", "nope") {
        Ok(()) => panic!("expected failure"),
        Err(e) => e,
    };
    assert!(err.is_degradable());
}

#[test]
fn autostart_toggles_and_reads_back() {
    let mut p = FakePlatform::default();
    assert!(!match p.is_enabled() {
        Ok(v) => v,
        Err(e) => panic!("is_enabled: {e}"),
    });
    if let Err(e) = p.set_enabled(true) {
        panic!("set_enabled: {e}");
    }
    assert!(match p.is_enabled() {
        Ok(v) => v,
        Err(e) => panic!("is_enabled: {e}"),
    });
    if let Err(e) = p.set_enabled(false) {
        panic!("set_enabled: {e}");
    }
    assert!(!match p.is_enabled() {
        Ok(v) => v,
        Err(e) => panic!("is_enabled: {e}"),
    });
}

#[test]
fn single_instance_first_acquire_wins_and_second_is_told_to_forward() {
    let mut p = FakePlatform::default();
    assert!(
        match p.acquire() {
            Ok(v) => v,
            Err(e) => panic!("acquire: {e}"),
        },
        "first instance owns the lock"
    );
    assert!(
        !match p.acquire() {
            Ok(v) => v,
            Err(e) => panic!("acquire: {e}"),
        },
        "second instance must not own it"
    );
}

#[test]
fn single_instance_forwards_cli_commands_to_the_running_instance() {
    let mut p = FakePlatform::default();
    if let Err(e) = p.forward("toggle-notch") {
        panic!("forward: {e}");
    }
    assert_eq!(p.forwarded(), vec!["toggle-notch".to_string()]);
}

#[test]
fn ipc_commands_arrive_as_platform_events() {
    let mut p = FakePlatform::default();
    p.script(FakeScript::Ipc("toggle-notch".into()));
    let events = p.drain_events();
    assert_eq!(
        events,
        vec![PlatformEvent::Command("toggle-notch".to_string())]
    );
}

#[test]
fn active_window_changes_are_reported_for_screen_time() {
    let mut p = FakePlatform::default();
    p.script(FakeScript::ActiveWindow {
        app: "firefox".into(),
        title: "Perch - GitHub".into(),
    });
    let events = p.drain_events();
    match &events[0] {
        PlatformEvent::ActiveWindowChanged { app, title } => {
            assert_eq!(app, "firefox");
            assert_eq!(title, "Perch - GitHub");
        }
        other => panic!("expected an active-window change, got {other:?}"),
    }
}
