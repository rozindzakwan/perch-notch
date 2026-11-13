use perch_core::config::HotkeysConfig;
use perch_core::hotkey::{Accel, AccelError, Key};
use proptest::prelude::*;

#[test]
fn parses_common_spellings() {
    let a = Accel::parse("Ctrl+Alt+N").unwrap_or_else(|e| panic!("{e}"));
    assert!(a.ctrl && a.alt && !a.shift && !a.super_key);
    assert_eq!(a.key, Key::Letter('N'));
    assert_eq!(
        Accel::parse(" super - shift - s "),
        Accel::parse("Win+Shift+S")
    );
    assert_eq!(Accel::parse("Print").map(|a| a.key), Ok(Key::PrintScreen));
    assert_eq!(Accel::parse("ctrl+F12").map(|a| a.key), Ok(Key::F(12)));
    assert_eq!(Accel::parse("Alt+7").map(|a| a.key), Ok(Key::Digit(7)));
}

#[test]
fn rejects_bad_shortcuts() {
    assert_eq!(Accel::parse(""), Err(AccelError::Empty));
    assert_eq!(Accel::parse("Ctrl+Alt"), Err(AccelError::NoKey));
    assert_eq!(Accel::parse("Ctrl+A+B"), Err(AccelError::TwoKeys));
    assert_eq!(Accel::parse("Shift+A"), Err(AccelError::NeedsModifier));
    assert_eq!(Accel::parse("A"), Err(AccelError::NeedsModifier));
    assert!(matches!(
        Accel::parse("Ctrl+Banana"),
        Err(AccelError::Unknown(_))
    ));
    assert!(matches!(Accel::parse("F25"), Err(AccelError::Unknown(_))));
}

#[test]
fn display_round_trips() {
    for s in [
        "Ctrl+Alt+N",
        "Super+Shift+S",
        "Print",
        "Ctrl+F5",
        "Alt+Space",
    ] {
        let a = Accel::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert_eq!(Accel::parse(&a.to_string()), Ok(a));
    }
}

#[test]
fn config_bindings_skip_empty_and_report_invalid() {
    let mut h = HotkeysConfig::default();
    assert_eq!(h.bindings().0.len(), 3);
    h.dictate = String::new();
    h.screenshot = "Shift+Q".into();
    let (ok, bad) = h.bindings();
    assert_eq!(ok.len(), 1);
    assert_eq!(ok[0].id, "toggle-notch");
    assert_eq!(bad.len(), 1);
    assert!(bad[0].contains("hotkeys.shot"));
}

proptest! {
    #[test]
    fn parsing_never_panics(s in ".{0,40}") {
        let _ = Accel::parse(&s);
    }
}
