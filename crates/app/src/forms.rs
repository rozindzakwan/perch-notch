//! The settings window and the add-to-do dialog: how a [`Config`] becomes a
//! [`FormSpec`], and how form events become config edits.
//!
//! Pure functions, tested in `tests/forms.rs`.

use perch_core::config::{Config, Theme};
use perch_core::geometry::{Monitor, NotchPosition};
use perch_ui::form::{Field, FormEvent, FormSpec};

use crate::features::Feature;

const POSITIONS: &[(NotchPosition, &str)] = &[
    (NotchPosition::TopCenter, "Top center"),
    (NotchPosition::TopLeft, "Top left"),
    (NotchPosition::TopRight, "Top right"),
    (NotchPosition::BottomCenter, "Bottom center"),
    (NotchPosition::BottomLeft, "Bottom left"),
    (NotchPosition::BottomRight, "Bottom right"),
    (NotchPosition::LeftCenter, "Left center"),
    (NotchPosition::RightCenter, "Right center"),
];

const THEMES: &[(Theme, &str)] = &[
    (Theme::System, "System"),
    (Theme::Dark, "Dark"),
    (Theme::Light, "Light"),
];

fn feature_id(f: Feature) -> String {
    format!("feature:{f:?}")
}

/// Build the settings form for a config and the current monitors.
pub fn settings_spec(cfg: &Config, monitors: &[Monitor]) -> FormSpec {
    let mut fields = vec![Field::Heading("Notch".into())];
    fields.push(Field::Choice {
        id: "position".into(),
        label: "Position".into(),
        options: POSITIONS.iter().map(|(_, l)| l.to_string()).collect(),
        selected: POSITIONS
            .iter()
            .position(|(p, _)| *p == cfg.notch.position)
            .unwrap_or(0),
    });
    let mut monitor_options = vec!["Primary".to_string()];
    monitor_options.extend(
        monitors
            .iter()
            .map(|m| format!("Monitor {} ({}x{})", m.id + 1, m.width, m.height)),
    );
    let selected_monitor = cfg
        .notch
        .monitor
        .and_then(|id| monitors.iter().position(|m| m.id == id))
        .map(|i| i + 1)
        .unwrap_or(0);
    fields.push(Field::Choice {
        id: "monitor".into(),
        label: "Monitor".into(),
        options: monitor_options,
        selected: selected_monitor,
    });
    fields.push(Field::Choice {
        id: "theme".into(),
        label: "Theme".into(),
        options: THEMES.iter().map(|(_, l)| l.to_string()).collect(),
        selected: THEMES
            .iter()
            .position(|(t, _)| *t == cfg.theme)
            .unwrap_or(0),
    });
    fields.push(Field::Toggle {
        id: "hide_on_fullscreen".into(),
        label: "Hide while a fullscreen app is active".into(),
        value: cfg.notch.hide_on_fullscreen,
        disabled: false,
    });
    fields.push(Field::Toggle {
        id: "autostart".into(),
        label: "Start at login".into(),
        value: cfg.autostart,
        disabled: false,
    });

    fields.push(Field::Heading("Features".into()));
    for f in Feature::ALL {
        let mut label = f.label().to_string();
        if f.uses_network() {
            label.push_str("  (uses network)");
        }
        if !f.compiled_in() {
            label.push_str("  (not in this build)");
        }
        fields.push(Field::Toggle {
            id: feature_id(*f),
            label,
            value: f.enabled_in(cfg),
            disabled: !f.compiled_in(),
        });
    }
    fields.push(Field::Note(
        "Everything runs locally. Nothing is sent anywhere unless you turn on a feature marked \
         \"uses network\": lyrics are fetched from lrclib.net, and dictation downloads its \
         speech model once."
            .into(),
    ));
    fields.push(Field::Buttons(vec![
        ("close".into(), "Done".into()),
        ("open-config".into(), "Open config file".into()),
    ]));
    FormSpec {
        title: "Perch settings".into(),
        fields,
        size: (460, 640),
    }
}

/// What a settings form event asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsOutcome {
    /// The config changed; save and apply it.
    Changed(Config),
    /// Open the config file in an editor.
    OpenConfig,
    /// Close the window.
    Close,
    /// Nothing to do.
    None,
}

/// Apply one settings form event to a config.
pub fn apply_settings_event(cfg: &Config, monitors: &[Monitor], ev: &FormEvent) -> SettingsOutcome {
    let mut next = cfg.clone();
    match ev {
        FormEvent::Closed => return SettingsOutcome::Close,
        FormEvent::Button(id) if id == "close" => return SettingsOutcome::Close,
        FormEvent::Button(id) if id == "open-config" => return SettingsOutcome::OpenConfig,
        FormEvent::Button(_) => return SettingsOutcome::None,
        FormEvent::Chosen(id, i) => match id.as_str() {
            "position" => {
                if let Some((p, _)) = POSITIONS.get(*i) {
                    next.notch.position = *p;
                }
            }
            "monitor" => {
                next.notch.monitor = if *i == 0 {
                    None
                } else {
                    monitors.get(i - 1).map(|m| m.id)
                };
            }
            "theme" => {
                if let Some((t, _)) = THEMES.get(*i) {
                    next.theme = *t;
                }
            }
            _ => return SettingsOutcome::None,
        },
        FormEvent::Toggled(id, v) => match id.as_str() {
            "hide_on_fullscreen" => next.notch.hide_on_fullscreen = *v,
            "autostart" => next.autostart = *v,
            other => {
                let Some(f) = Feature::ALL.iter().find(|f| feature_id(**f) == other) else {
                    return SettingsOutcome::None;
                };
                set_feature(&mut next, *f, *v);
            }
        },
    }
    if next == *cfg {
        SettingsOutcome::None
    } else {
        SettingsOutcome::Changed(next)
    }
}

/// Set one feature flag.
pub fn set_feature(cfg: &mut Config, f: Feature, on: bool) {
    let flags = &mut cfg.features;
    let slot = match f {
        Feature::Media => &mut flags.media,
        Feature::Lyrics => &mut flags.lyrics,
        Feature::Todo => &mut flags.todo,
        Feature::Pomodoro => &mut flags.pomodoro,
        Feature::Clipboard => &mut flags.clipboard,
        Feature::Shelf => &mut flags.shelf,
        Feature::Reminders => &mut flags.reminders,
        Feature::VolumeMixer => &mut flags.volume_mixer,
        Feature::WhiteNoise => &mut flags.white_noise,
        Feature::ScreenTime => &mut flags.screen_time,
        Feature::SpeechToText => &mut flags.speech_to_text,
        Feature::Screenshot => &mut flags.screenshot,
        Feature::CameraMirror => &mut flags.camera_mirror,
    };
    *slot = on;
    // Lyrics need something to sync to.
    if f == Feature::Lyrics && on {
        flags.media = true;
    }
}

/// The add-to-do dialog.
pub fn todo_spec() -> FormSpec {
    FormSpec {
        title: "Add to-do".into(),
        fields: vec![
            Field::Text {
                id: "text".into(),
                label: "What needs doing?".into(),
                value: String::new(),
                placeholder: "e.g. Call the dentist".into(),
            },
            Field::Text {
                id: "due".into(),
                label: "Remind me at (optional)".into(),
                value: String::new(),
                placeholder: "14:30, or +45m, or +2h".into(),
            },
            Field::Buttons(vec![
                ("add".into(), "Add".into()),
                ("cancel".into(), "Cancel".into()),
            ]),
        ],
        size: (380, 230),
    }
}

/// Parse a due-time field into unix seconds.
///
/// Accepts `HH:MM` (24 h; today, or tomorrow if that time already passed),
/// `+Nm` / `+Nh` (relative), or empty (no due time). `offset` is the local UTC
/// offset in seconds.
pub fn parse_due(input: &str, now_unix: u64, offset: i32) -> Result<Option<u64>, String> {
    let s = input.trim().to_ascii_lowercase();
    if s.is_empty() {
        return Ok(None);
    }
    if let Some(rel) = s.strip_prefix('+') {
        let (num, unit) =
            rel.split_at(rel.find(|c: char| !c.is_ascii_digit()).unwrap_or(rel.len()));
        let n: u64 = num
            .parse()
            .map_err(|_| format!("\"{input}\" is not a duration"))?;
        let secs = match unit.trim() {
            "" | "m" | "min" | "mins" => n * 60,
            "h" | "hr" | "hrs" => n * 3600,
            _ => return Err(format!("unknown unit in \"{input}\"; use m or h")),
        };
        if secs == 0 || secs > 7 * 86_400 {
            return Err("pick something between 1 minute and 7 days".into());
        }
        return Ok(Some(now_unix + secs));
    }
    let (h, m) = s
        .split_once(':')
        .ok_or_else(|| format!("\"{input}\" is not a time; use HH:MM or +30m"))?;
    let h: i64 = h.parse().map_err(|_| format!("bad hour in \"{input}\""))?;
    let m: i64 = m
        .parse()
        .map_err(|_| format!("bad minute in \"{input}\""))?;
    if !(0..24).contains(&h) || !(0..60).contains(&m) {
        return Err(format!("\"{input}\" is not a valid time of day"));
    }
    let local_now = now_unix as i64 + i64::from(offset);
    let day_start = local_now - local_now.rem_euclid(86_400);
    let mut target_local = day_start + h * 3600 + m * 60;
    if target_local <= local_now {
        target_local += 86_400;
    }
    Ok(Some((target_local - i64::from(offset)).max(0) as u64))
}
