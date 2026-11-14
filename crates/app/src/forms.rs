//! The settings window and the add-to-do dialog: how a [`Config`] becomes a
//! [`FormSpec`], and how form events become config edits.
//!
//! Pure functions, tested in `tests/forms.rs`.

use perch_core::config::{Config, MailAccount, Theme};
use perch_core::geometry::{Monitor, NotchPosition};
use perch_core::mail::{AccountStatus, MailBoard, Security, PRESETS};
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
pub fn settings_spec(cfg: &Config, monitors: &[Monitor], mail: &MailBoard) -> FormSpec {
    let mut fields = vec![Field::Group("Notch".into())];
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

    fields.push(Field::Group("Notifications".into()));
    fields.push(Field::Toggle {
        id: "notify_in_notch".into(),
        label: "Show notifications in the notch".into(),
        value: cfg.notifications.in_notch,
        disabled: false,
    });
    fields.push(Field::Toggle {
        id: "notify_system".into(),
        label: "Also show system notifications".into(),
        value: cfg.notifications.system,
        disabled: false,
    });
    fields.push(Field::Group("Features".into()));
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
         \"uses network\": lyrics are fetched from lrclib.net, dictation downloads its \
         speech model once, and mail connects to the mail servers you add (read-only; \
         passwords stay in the system keychain)."
            .into(),
    ));
    if Feature::Mail.enabled_in(cfg) && Feature::Mail.compiled_in() {
        fields.push(Field::Group("Mail accounts".into()));
        if cfg.mail.accounts.is_empty() {
            fields.push(Field::Note(
                "No accounts yet. Perch only reads: it shows unread mail and never marks, \
                 moves or deletes anything."
                    .into(),
            ));
        }
        for (i, a) in cfg.mail.accounts.iter().enumerate() {
            let status = mail
                .accounts()
                .iter()
                .find(|s| s.name == a.name)
                .map(|s| s.status.clone())
                .unwrap_or(AccountStatus::Connecting);
            let ok = match status {
                AccountStatus::Online { .. } => Some(true),
                AccountStatus::Connecting => None,
                _ => Some(false),
            };
            fields.push(Field::Status {
                label: a.name.clone(),
                detail: format!("{} \u{b7} {} on {}", status.label(), a.username, a.host),
                ok,
                action: Some((format!("mail-remove:{i}"), "Remove".into())),
            });
        }
        fields.push(Field::Buttons(vec![(
            "mail-add".into(),
            "Add mail account…".into(),
        )]));
    }
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
    Changed(Box<Config>),
    /// Open the config file in an editor.
    OpenConfig,
    /// Close the window.
    Close,
    /// Open the add-mail-account dialog.
    AddMailAccount,
    /// Remove the mail account at this index (and its stored password).
    RemoveMailAccount(usize),
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
        FormEvent::Button(id) if id == "mail-add" => return SettingsOutcome::AddMailAccount,
        FormEvent::Button(id) if id.starts_with("mail-remove:") => {
            return match id["mail-remove:".len()..].parse() {
                Ok(i) => SettingsOutcome::RemoveMailAccount(i),
                Err(_) => SettingsOutcome::None,
            };
        }
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
            "notify_in_notch" => next.notifications.in_notch = *v,
            "notify_system" => next.notifications.system = *v,
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
        SettingsOutcome::Changed(Box::new(next))
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
        Feature::Mail => &mut flags.mail,
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
                secret: false,
            },
            Field::Text {
                id: "due".into(),
                label: "Remind me at (optional)".into(),
                value: String::new(),
                placeholder: "14:30, or +45m, or +2h".into(),
                secret: false,
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
/// `+Ns` / `+Nm` / `+Nh` (relative), or empty (no due time). `offset` is the
/// local UTC offset in seconds.
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
            "s" | "sec" | "secs" => n,
            "" | "m" | "min" | "mins" => n * 60,
            "h" | "hr" | "hrs" => n * 3600,
            _ => return Err(format!("unknown unit in \"{input}\"; use s, m or h")),
        };
        if secs == 0 || secs > 7 * 86_400 {
            return Err("pick something between 1 second and 7 days".into());
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

/// What is typed into the add-mail-account dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailForm {
    /// Index into [`PRESETS`].
    pub preset: usize,
    /// Name shown in Perch.
    pub name: String,
    /// Sign-in name (usually the address).
    pub email: String,
    /// Password or app password (never logged, never saved to the config).
    pub password: String,
    /// IMAP server.
    pub host: String,
    /// IMAP port, as typed.
    pub port: String,
}

impl Default for MailForm {
    fn default() -> Self {
        let p = &PRESETS[0];
        Self {
            preset: 0,
            name: String::new(),
            email: String::new(),
            password: String::new(),
            host: p.host.into(),
            port: p.port.to_string(),
        }
    }
}

impl MailForm {
    /// Read the dialog's current values.
    pub fn from_spec(spec: &FormSpec) -> Self {
        let mut f = MailForm::default();
        for field in &spec.fields {
            match field {
                Field::Choice { id, selected, .. } if id == "provider" => f.preset = *selected,
                Field::Text { id, value, .. } => match id.as_str() {
                    "name" => f.name = value.clone(),
                    "email" => f.email = value.clone(),
                    "password" => f.password = value.clone(),
                    "host" => f.host = value.clone(),
                    "port" => f.port = value.clone(),
                    _ => {}
                },
                _ => {}
            }
        }
        f
    }

    /// The provider changed: take its server and port ("Other" keeps what
    /// was typed).
    pub fn with_preset(mut self, preset: usize) -> Self {
        let Some(p) = PRESETS.get(preset) else {
            return self;
        };
        self.preset = preset;
        if !p.host.is_empty() {
            self.host = p.host.into();
            self.port = p.port.to_string();
        } else if PRESETS.iter().any(|q| q.host == self.host) {
            self.host.clear();
        }
        self
    }
}

/// The add-mail-account dialog.
pub fn mail_account_spec(f: &MailForm, error: Option<&str>) -> FormSpec {
    let hint = PRESETS.get(f.preset).map(|p| p.hint).unwrap_or("password");
    let text = |id: &str, label: &str, value: &str, placeholder: &str, secret: bool| Field::Text {
        id: id.into(),
        label: label.into(),
        value: value.into(),
        placeholder: placeholder.into(),
        secret,
    };
    let mut fields =
        vec![
        Field::Choice {
            id: "provider".into(),
            label: "Provider".into(),
            options: PRESETS.iter().map(|p| p.label.to_string()).collect(),
            selected: f.preset.min(PRESETS.len() - 1),
        },
        text("email", "Email address (sign-in name)", &f.email, "you@example.com", false),
        text("password", "Password", &f.password, hint, true),
        text("name", "Name shown in Perch (optional)", &f.name, "Work", false),
        text("host", "IMAP server", &f.host, "imap.example.com", false),
        text("port", "Port", &f.port, "993", false),
        Field::Note(
            "Read-only: Perch opens your inbox with EXAMINE and only reads sender, subject and \
             time of unread mail. It never marks, moves, deletes or downloads messages. The \
             password goes to the system keychain, not the config file."
                .into(),
        ),
    ];
    if let Some(e) = error {
        fields.push(Field::Note(format!("Problem: {e}")));
    }
    fields.push(Field::Buttons(vec![
        ("add".into(), "Add account".into()),
        ("cancel".into(), "Cancel".into()),
    ]));
    FormSpec {
        title: "Add mail account".into(),
        fields,
        size: (440, 560),
    }
}

/// Turn the dialog into an account and its password, or explain what is wrong.
pub fn parse_mail_form(
    f: &MailForm,
    existing: &[MailAccount],
) -> Result<(MailAccount, String), String> {
    let email = f.email.trim();
    if email.is_empty() {
        return Err("enter the email address you sign in with".into());
    }
    if f.password.is_empty() {
        return Err("enter the password".into());
    }
    let host = f.host.trim().to_ascii_lowercase();
    if host.is_empty() || host.contains(char::is_whitespace) {
        return Err("enter the IMAP server, e.g. imap.example.com".into());
    }
    let port: u16 = f
        .port
        .trim()
        .parse()
        .ok()
        .filter(|p| *p > 0)
        .ok_or("the port must be a number such as 993")?;
    if existing
        .iter()
        .any(|a| a.username.eq_ignore_ascii_case(email) && a.host.eq_ignore_ascii_case(&host))
    {
        return Err("that account is already added".into());
    }
    let name = match f.name.trim() {
        "" => email.to_string(),
        n => n.to_string(),
    };
    let security = if port == 143 {
        Security::Starttls
    } else {
        Security::Tls
    };
    let account = MailAccount {
        name,
        username: email.to_string(),
        host,
        port,
        security,
        ..MailAccount::default()
    };
    if let Some(p) = account.problem() {
        return Err(p);
    }
    Ok((account, f.password.clone()))
}

#[cfg(test)]
mod mail_form_tests {
    use super::*;

    #[test]
    fn spacemail_is_the_default_and_presets_fill_server_and_port() {
        let f = MailForm::default();
        assert_eq!(
            (f.host.as_str(), f.port.as_str()),
            ("mail.spacemail.com", "993")
        );
        let gmail = PRESETS.iter().position(|p| p.id == "gmail").unwrap_or(0);
        let g = f.clone().with_preset(gmail);
        assert_eq!(g.host, "imap.gmail.com");
        let other = PRESETS.iter().position(|p| p.id == "other").unwrap_or(0);
        assert_eq!(
            g.with_preset(other).host,
            "",
            "a preset host is not kept for Other"
        );
    }

    #[test]
    fn the_dialog_round_trips_and_validates() {
        let f = MailForm {
            email: " you@yourcompany.example ".into(),
            password: "p w".into(),
            ..MailForm::default()
        };
        let spec = mail_account_spec(&f, None);
        assert_eq!(MailForm::from_spec(&spec), f);
        let (a, pw) = parse_mail_form(&f, &[]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(a.username, "you@yourcompany.example");
        assert_eq!(a.name, "you@yourcompany.example");
        assert_eq!(
            (a.host.as_str(), a.port, a.security),
            ("mail.spacemail.com", 993, Security::Tls)
        );
        assert_eq!(pw, "p w");
        assert!(
            parse_mail_form(&f, std::slice::from_ref(&a)).is_err(),
            "duplicates are refused"
        );
        let mut bad = f.clone();
        bad.password.clear();
        assert!(parse_mail_form(&bad, &[]).is_err());
        let mut starttls = f.clone();
        starttls.port = "143".into();
        assert_eq!(
            parse_mail_form(&starttls, &[]).map(|(a, _)| a.security),
            Ok(Security::Starttls)
        );
        let secret_shown = mail_account_spec(&f, None)
            .fields
            .iter()
            .any(|x| matches!(x, Field::Text { id, secret: true, .. } if id == "password"));
        assert!(secret_shown, "the password field is masked");
    }
}
