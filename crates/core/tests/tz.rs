use perch_core::tz::offset_at;

/// Build a v1 TZif file with the given transitions and types.
fn tzif_v1(transitions: &[(i32, u8)], types: &[(i32, u8)]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"TZif");
    b.push(0); // version 1
    b.extend_from_slice(&[0u8; 15]);
    let counts = [0u32, 0, 0, transitions.len() as u32, types.len() as u32, 4];
    for c in counts {
        b.extend_from_slice(&c.to_be_bytes());
    }
    for (t, _) in transitions {
        b.extend_from_slice(&t.to_be_bytes());
    }
    for (_, i) in transitions {
        b.push(*i);
    }
    for (off, dst) in types {
        b.extend_from_slice(&off.to_be_bytes());
        b.push(*dst);
        b.push(0);
    }
    b.extend_from_slice(b"ABC\0");
    b
}

#[test]
fn fixed_zone_without_transitions() {
    let f = tzif_v1(&[], &[(7 * 3600, 0)]);
    assert_eq!(offset_at(&f, 1_700_000_000), Some(25200));
}

#[test]
fn picks_the_last_transition_before_now() {
    // Standard +1h until t=1000, then DST +2h until t=2000, then +1h.
    let f = tzif_v1(&[(1000, 1), (2000, 0)], &[(3600, 0), (7200, 1)]);
    assert_eq!(offset_at(&f, 500), Some(3600));
    assert_eq!(offset_at(&f, 1000), Some(7200));
    assert_eq!(offset_at(&f, 1999), Some(7200));
    assert_eq!(offset_at(&f, 5000), Some(3600));
}

#[test]
fn before_first_transition_uses_first_standard_type() {
    let f = tzif_v1(&[(1000, 0)], &[(7200, 1), (3600, 0)]);
    assert_eq!(offset_at(&f, 0), Some(3600));
}

#[test]
fn garbage_is_rejected_not_panicking() {
    assert_eq!(offset_at(b"", 0), None);
    assert_eq!(offset_at(b"TZif", 0), None);
    assert_eq!(
        offset_at(b"not a zone file at all, nope nope nope nope nope", 0),
        None
    );
    let mut f = tzif_v1(&[(1000, 5)], &[(3600, 0)]);
    assert_eq!(offset_at(&f, 2000), None, "type index out of range");
    f.truncate(50);
    assert_eq!(offset_at(&f, 2000), None, "truncated");
}

#[test]
fn reads_the_real_system_zone_when_present() {
    // Only meaningful on Linux; elsewhere the file does not exist.
    if let Ok(bytes) = std::fs::read("/etc/localtime") {
        let off = offset_at(&bytes, 1_700_000_000);
        assert!(off.is_some(), "the system zone file must parse");
        let off = off.unwrap_or(0);
        assert!((-14 * 3600..=14 * 3600).contains(&off));
    }
}
