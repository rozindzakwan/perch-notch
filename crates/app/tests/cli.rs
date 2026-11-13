//! CLI parsing is pure logic, so it is tested directly.

use perch_app::cli::{parse, usage, Command};

#[test]
fn no_arguments_means_run() {
    let inv = parse(Vec::<String>::new());
    assert_eq!(inv.command, Command::Run);
    assert!(inv.config.is_none());
    assert!(!inv.verbose);
    assert!(inv.unknown.is_empty());
}

#[test]
fn every_documented_subcommand_parses() {
    let cases = [
        ("run", Command::Run),
        ("toggle-notch", Command::ToggleNotch),
        ("toggle", Command::ToggleNotch),
        ("shot", Command::Shot),
        ("screenshot", Command::Shot),
        ("dictate", Command::Dictate),
        ("pomodoro", Command::Pomodoro),
        ("settings", Command::Settings),
        ("quit", Command::Quit),
        ("exit", Command::Quit),
    ];
    for (arg, want) in cases {
        assert_eq!(parse([arg]).command, want, "parsing {arg}");
    }
}

#[test]
fn version_and_help_flags_win_over_a_subcommand() {
    assert_eq!(parse(["run", "--version"]).command, Command::Version);
    assert_eq!(parse(["toggle-notch", "-h"]).command, Command::Help);
}

#[test]
fn config_accepts_both_spellings() {
    assert_eq!(
        parse(["--config", "/tmp/a.toml"]).config.as_deref(),
        Some("/tmp/a.toml")
    );
    assert_eq!(
        parse(["--config=/tmp/b.toml"]).config.as_deref(),
        Some("/tmp/b.toml")
    );
}

#[test]
fn a_config_flag_without_a_path_is_reported_not_ignored() {
    let inv = parse(["--config"]);
    assert!(inv.config.is_none());
    assert!(
        inv.unknown.iter().any(|u| u.contains("--config")),
        "the user must be told: {:?}",
        inv.unknown
    );
}

#[test]
fn verbose_is_position_independent() {
    assert!(parse(["-v", "shot"]).verbose);
    assert!(parse(["shot", "--verbose"]).verbose);
}

#[test]
fn unknown_arguments_are_collected_rather_than_silently_dropped() {
    let inv = parse(["--frobnicate", "shot"]);
    assert_eq!(inv.command, Command::Shot);
    assert_eq!(inv.unknown, vec!["--frobnicate".to_string()]);
}

#[test]
fn the_first_subcommand_wins_so_a_typo_cannot_change_the_action() {
    let inv = parse(["shot", "dictate"]);
    assert_eq!(inv.command, Command::Shot);
}

#[test]
fn forwardable_commands_have_an_ipc_verb_and_local_ones_do_not() {
    assert_eq!(Command::ToggleNotch.ipc_verb(), Some("toggle-notch"));
    assert_eq!(Command::Shot.ipc_verb(), Some("shot"));
    assert_eq!(Command::Dictate.ipc_verb(), Some("dictate"));
    assert_eq!(Command::Pomodoro.ipc_verb(), Some("pomodoro"));
    assert_eq!(Command::Settings.ipc_verb(), Some("settings"));
    assert_eq!(Command::Quit.ipc_verb(), Some("quit"));
    assert_eq!(Command::Run.ipc_verb(), None);
    assert_eq!(Command::Version.ipc_verb(), None);
    assert_eq!(Command::Help.ipc_verb(), None);
}

#[test]
fn usage_text_documents_every_subcommand() {
    let text = usage();
    for verb in [
        "run",
        "toggle-notch",
        "shot",
        "dictate",
        "pomodoro",
        "settings",
        "quit",
        "--config",
        "--verbose",
    ] {
        assert!(text.contains(verb), "usage must mention {verb}:\n{text}");
    }
}
