//! RED-phase tests for config load/save/merge semantics.

use std::io::Write;

use perch_core::config::{Config, ConfigStore, Theme};
use perch_core::geometry::NotchPosition;

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => panic!("tempdir: {e}"),
    };
    let store = ConfigStore::at(dir.path().join("config.toml"));
    (dir, store)
}

#[test]
fn missing_file_yields_defaults_and_does_not_error() {
    let (_d, s) = store();
    let cfg = match s.load() {
        Ok(c) => c,
        Err(e) => panic!("load should succeed on a missing file: {e}"),
    };
    assert_eq!(cfg, Config::default());
    assert_eq!(cfg.notch.position, NotchPosition::TopCenter);
    assert!(!cfg.features.lyrics, "network features are opt-in");
    assert!(
        !cfg.features.speech_to_text,
        "heavy features are off by default"
    );
}

#[test]
fn save_then_load_round_trips_every_field() {
    let (_d, s) = store();
    let mut cfg = Config::default();
    cfg.notch.position = NotchPosition::BottomRight;
    cfg.notch.collapsed_width = 240;
    cfg.notch.expanded_height = 180;
    cfg.notch.hide_on_fullscreen = false;
    cfg.notch.animation_ms = 150;
    cfg.theme = Theme::Light;
    cfg.features.media = false;
    cfg.features.lyrics = true;
    cfg.features.pomodoro = true;
    cfg.clipboard.history_limit = 42;
    cfg.autostart = true;

    if let Err(e) = s.save(&cfg) {
        panic!("save: {e}");
    }
    let back = match s.load() {
        Ok(c) => c,
        Err(e) => panic!("load: {e}"),
    };
    assert_eq!(back, cfg);
}

#[test]
fn partial_file_keeps_defaults_for_absent_keys() {
    let (_d, s) = store();
    let mut f = match std::fs::File::create(s.path()) {
        Ok(f) => f,
        Err(e) => panic!("create: {e}"),
    };
    if let Err(e) = writeln!(f, "[notch]\nposition = \"bottom-center\"\n") {
        panic!("write: {e}");
    }
    drop(f);

    let cfg = match s.load() {
        Ok(c) => c,
        Err(e) => panic!("partial config must load: {e}"),
    };
    assert_eq!(cfg.notch.position, NotchPosition::BottomCenter);
    assert_eq!(
        cfg.notch.collapsed_width,
        Config::default().notch.collapsed_width,
        "absent key keeps its default"
    );
    assert_eq!(cfg.theme, Config::default().theme);
}

#[test]
fn invalid_toml_reports_an_error_naming_the_file() {
    let (_d, s) = store();
    if let Err(e) = std::fs::write(s.path(), b"this is { not toml") {
        panic!("write: {e}");
    }
    let err = match s.load() {
        Ok(c) => panic!("expected a parse error, got {c:?}"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("config.toml"),
        "error should name the file: {msg}"
    );
}

#[test]
fn load_or_default_never_fails_even_on_garbage() {
    let (_d, s) = store();
    if let Err(e) = std::fs::write(s.path(), b"\x00\x01 garbage [[[") {
        panic!("write: {e}");
    }
    let (cfg, problem) = s.load_or_default();
    assert_eq!(cfg, Config::default(), "falls back to defaults");
    assert!(problem.is_some(), "and reports what went wrong");
}

#[test]
fn unknown_position_string_falls_back_to_default_instead_of_failing() {
    let (_d, s) = store();
    if let Err(e) = std::fs::write(s.path(), b"[notch]\nposition = \"middle-of-nowhere\"\n") {
        panic!("write: {e}");
    }
    let (cfg, problem) = s.load_or_default();
    assert_eq!(cfg.notch.position, NotchPosition::TopCenter);
    assert!(problem.is_some(), "the bad value is reported");
}

#[test]
fn out_of_range_values_are_clamped_to_sane_bounds() {
    let (_d, s) = store();
    let toml = br#"
[notch]
collapsed_width = 10
collapsed_height = 0
expanded_width = 99999
animation_ms = 100000
hot_zone_thickness = 400

[clipboard]
history_limit = 0
"#;
    if let Err(e) = std::fs::write(s.path(), toml) {
        panic!("write: {e}");
    }
    let cfg = match s.load() {
        Ok(c) => c,
        Err(e) => panic!("load: {e}"),
    };
    assert!(cfg.notch.collapsed_width >= 80, "min width enforced");
    assert!(cfg.notch.collapsed_height >= 16, "min height enforced");
    assert!(cfg.notch.expanded_width <= 4000, "max width enforced");
    assert!(cfg.notch.animation_ms <= 2000, "animation capped");
    assert!(
        (1..=8).contains(&cfg.notch.hot_zone_thickness),
        "hot zone must stay thin, got {}",
        cfg.notch.hot_zone_thickness
    );
    assert!(cfg.clipboard.history_limit >= 1, "history limit min 1");
}

#[test]
fn save_creates_missing_parent_directories() {
    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => panic!("tempdir: {e}"),
    };
    let s = ConfigStore::at(dir.path().join("deep").join("nested").join("config.toml"));
    if let Err(e) = s.save(&Config::default()) {
        panic!("save should create parents: {e}");
    }
    assert!(s.path().exists());
}

#[test]
fn saved_file_is_human_editable_toml_with_comments_preserved_on_rewrite() {
    let (_d, s) = store();
    if let Err(e) = s.save(&Config::default()) {
        panic!("save: {e}");
    }
    let text = match std::fs::read_to_string(s.path()) {
        Ok(t) => t,
        Err(e) => panic!("read: {e}"),
    };
    assert!(text.contains("[notch]"), "has sections: {text}");
    assert!(text.contains("position"), "has keys: {text}");
    assert!(
        text.contains("top-center"),
        "uses the kebab-case spelling: {text}"
    );
}

#[test]
fn geometry_view_reflects_configured_sizes() {
    let mut cfg = Config::default();
    cfg.notch.collapsed_width = 222;
    cfg.notch.collapsed_height = 34;
    cfg.notch.expanded_width = 444;
    cfg.notch.expanded_height = 140;
    cfg.notch.margin = 7;
    cfg.notch.hot_zone_thickness = 2;
    let geo = cfg.notch.geometry();
    assert_eq!(geo.collapsed, (222, 34));
    assert_eq!(geo.expanded, (444, 140));
    assert_eq!(geo.margin, 7);
    assert_eq!(geo.hot_zone_thickness, 2);
}

#[test]
fn enabled_feature_count_is_zero_for_heavy_features_by_default() {
    let cfg = Config::default();
    assert!(!cfg.features.speech_to_text);
    assert!(!cfg.features.camera_mirror);
    assert!(!cfg.features.screenshot);
    assert!(!cfg.features.lyrics);
    assert!(
        cfg.features.media,
        "media is the headline feature, on by default"
    );
}
