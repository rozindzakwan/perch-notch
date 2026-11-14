//! Secret Service round trip. Needs a session bus with an unlocked keyring;
//! CI starts a throwaway GNOME Keyring on a private bus:
//! `dbus-run-session -- sh -c 'echo -n x | gnome-keyring-daemon --unlock --components=secrets; cargo test ... --test secrets -- --ignored'`.
#![cfg(target_os = "linux")]

use perch_core::platform::SecretStore;
use perch_platform_linux::secrets::SecretService;

#[test]
#[ignore = "needs a running, unlocked Secret Service"]
fn round_trip_in_the_keyring() {
    let s = SecretService::connect().unwrap_or_else(|e| panic!("{e}"));
    let key = format!("test:roundtrip-{}@localhost", std::process::id());
    assert_eq!(s.get(&key).unwrap_or_else(|e| panic!("{e}")), None);
    s.set(&key, "Perch test credential", "pä55 wörd \"x\"")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        s.get(&key).ok().flatten().as_deref(),
        Some("pä55 wörd \"x\"")
    );
    s.set(&key, "Perch test credential", "second")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        s.get(&key).ok().flatten().as_deref(),
        Some("second"),
        "replaced, not duplicated"
    );
    s.delete(&key).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(s.get(&key).ok().flatten(), None);
}
