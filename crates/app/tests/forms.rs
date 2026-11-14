use perch_app::features::Feature;
use perch_app::forms::{
    apply_settings_event, parse_due, set_feature, settings_spec, todo_spec, SettingsOutcome,
};
use perch_core::config::{Config, MailAccount, Theme};
use perch_core::geometry::{Monitor, NotchPosition};
use perch_core::mail::{AccountStatus, MailBoard, MailUpdate};
use perch_ui::form::{Field, FormEvent, FormState};

fn monitors() -> Vec<Monitor> {
    vec![
        Monitor {
            id: 0,
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            primary: true,
            work: None,
        },
        Monitor {
            id: 1,
            x: 1920,
            y: 0,
            width: 2560,
            height: 1440,
            scale: 1.5,
            primary: false,
            work: None,
        },
    ]
}

#[test]
fn settings_form_lists_every_feature_and_marks_network_and_missing_ones() {
    let spec = settings_spec(&Config::default(), &monitors(), &MailBoard::default());
    let toggles: Vec<&str> = spec
        .fields
        .iter()
        .filter_map(|f| match f {
            Field::Toggle { label, .. } => Some(label.as_str()),
            _ => None,
        })
        .collect();
    assert!(toggles.len() >= Feature::ALL.len() + 2);
    assert!(toggles
        .iter()
        .any(|t| t.starts_with("Synced lyrics") && t.contains("uses network")));
}

#[test]
fn choosing_a_position_monitor_and_theme_edits_the_config() {
    let cfg = Config::default();
    let m = monitors();
    let SettingsOutcome::Changed(c) =
        apply_settings_event(&cfg, &m, &FormEvent::Chosen("position".into(), 4))
    else {
        panic!("position change")
    };
    assert_eq!(c.notch.position, NotchPosition::BottomLeft);
    let SettingsOutcome::Changed(c) =
        apply_settings_event(&c, &m, &FormEvent::Chosen("monitor".into(), 2))
    else {
        panic!("monitor change")
    };
    assert_eq!(c.notch.monitor, Some(1));
    let SettingsOutcome::Changed(c) =
        apply_settings_event(&c, &m, &FormEvent::Chosen("theme".into(), 2))
    else {
        panic!("theme change")
    };
    assert_eq!(c.theme, Theme::Light);
}

#[test]
fn feature_toggles_round_trip_and_lyrics_implies_media() {
    let mut cfg = Config::default();
    cfg.features.media = false;
    let out = apply_settings_event(
        &cfg,
        &monitors(),
        &FormEvent::Toggled("feature:Lyrics".into(), true),
    );
    let SettingsOutcome::Changed(c) = out else {
        panic!()
    };
    assert!(c.features.lyrics && c.features.media);
    assert_eq!(
        apply_settings_event(
            &c,
            &monitors(),
            &FormEvent::Toggled("feature:Lyrics".into(), true)
        ),
        SettingsOutcome::None,
        "no-op changes do not rewrite the file"
    );
}

#[test]
fn buttons_and_close() {
    let cfg = Config::default();
    assert_eq!(
        apply_settings_event(&cfg, &[], &FormEvent::Closed),
        SettingsOutcome::Close
    );
    assert_eq!(
        apply_settings_event(&cfg, &[], &FormEvent::Button("open-config".into())),
        SettingsOutcome::OpenConfig
    );
}

#[test]
fn todo_dialog_typing_and_enter() {
    let mut st = FormState::new(todo_spec());
    st.type_text("Buy milk\n");
    assert_eq!(
        st.text("text"),
        Some("Buy milk"),
        "control characters are dropped"
    );
    st.backspace();
    assert_eq!(st.text("text"), Some("Buy mil"));
    st.tab();
    st.type_text("+30m");
    assert_eq!(st.text("due"), Some("+30m"));
    assert_eq!(st.enter(), vec![FormEvent::Button("add".into())]);
}

#[test]
fn due_parsing() {
    // 2023-11-14 22:13:20 UTC
    let now = 1_700_000_000u64;
    assert_eq!(parse_due("", now, 0), Ok(None));
    assert_eq!(parse_due("+30m", now, 0), Ok(Some(now + 1800)));
    assert_eq!(parse_due("+2h", now, 0), Ok(Some(now + 7200)));
    assert_eq!(parse_due(" +5 ", now, 0), Ok(Some(now + 300)));
    // 23:00 UTC today is later than 22:13.
    assert_eq!(parse_due("23:00", now, 0), Ok(Some(1_700_002_800)));
    // 08:00 already passed today -> tomorrow.
    assert_eq!(parse_due("08:00", now, 0), Ok(Some(1_700_035_200)));
    // In UTC+2 it is 00:13 on the 15th locally; 08:00 local = 06:00 UTC.
    assert_eq!(parse_due("08:00", now, 7200), Ok(Some(1_700_028_000)));
    assert!(parse_due("25:00", now, 0).is_err());
    assert!(parse_due("noon", now, 0).is_err());
    assert!(parse_due("+0m", now, 0).is_err());
    assert!(parse_due("+3x", now, 0).is_err());
}

#[test]
fn notification_toggles_edit_the_config() {
    let cfg = Config::default();
    assert!(
        cfg.notifications.in_notch && !cfg.notifications.system,
        "in-notch only by default"
    );
    let SettingsOutcome::Changed(c) = apply_settings_event(
        &cfg,
        &monitors(),
        &FormEvent::Toggled("notify_system".into(), true),
    ) else {
        panic!("system toggle")
    };
    assert!(c.notifications.system);
    let SettingsOutcome::Changed(c) = apply_settings_event(
        &c,
        &monitors(),
        &FormEvent::Toggled("notify_in_notch".into(), false),
    ) else {
        panic!("in-notch toggle")
    };
    assert!(!c.notifications.in_notch);
}

fn group_of<'a>(spec: &'a perch_ui::form::FormSpec, id: &str) -> Option<&'a str> {
    let mut group = None;
    for f in &spec.fields {
        match f {
            Field::Group(g) => group = Some(g.as_str()),
            Field::Toggle { id: fid, .. } if fid == id => return group,
            _ => {}
        }
    }
    None
}

#[test]
fn settings_has_notification_rows_in_their_own_group() {
    let spec = settings_spec(&Config::default(), &monitors(), &MailBoard::default());
    assert_eq!(group_of(&spec, "notify_in_notch"), Some("Notifications"));
    assert_eq!(group_of(&spec, "notify_system"), Some("Notifications"));
    assert_eq!(group_of(&spec, "autostart"), Some("Notch"));
    assert!(spec.fields.iter().any(
        |f| matches!(f, Field::Choice { id, options, .. } if id == "theme" && options.len() == 3)
    ));
}

#[test]
fn mail_account_rows_show_status() {
    let mut cfg = Config::default();
    set_feature(&mut cfg, Feature::Mail, true);
    for name in ["Work", "Home"] {
        cfg.mail.accounts.push(MailAccount {
            name: name.into(),
            host: "imap.example.com".into(),
            port: 993,
            username: format!("{}@example.com", name.to_lowercase()),
            ..MailAccount::default()
        });
    }
    let mut board = MailBoard::default();
    board.set_accounts(&["Work".into(), "Home".into()]);
    board.apply(MailUpdate::Status {
        account: "Work".into(),
        status: AccountStatus::AuthFailed("bad password".into()),
    });
    let spec = settings_spec(&cfg, &monitors(), &board);
    let rows: Vec<(&str, &str, Option<bool>, Option<&str>)> = spec
        .fields
        .iter()
        .filter_map(|f| match f {
            Field::Status {
                label,
                detail,
                ok,
                action,
            } => Some((
                label.as_str(),
                detail.as_str(),
                *ok,
                action.as_ref().map(|(id, _)| id.as_str()),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows[0].0.contains("Work"));
    assert!(rows[0].1.contains("sign-in failed"), "{:?}", rows[0]);
    assert_eq!(rows[0].2, Some(false));
    assert_eq!(rows[0].3, Some("mail-remove:0"));
    assert!(rows[1].1.contains("connecting"), "{:?}", rows[1]);
    assert_eq!(rows[1].2, None, "still connecting: neither ok nor failed");
    assert_eq!(rows[1].3, Some("mail-remove:1"));
}
