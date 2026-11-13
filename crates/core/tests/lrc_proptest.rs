//! Property-based tests for the LRC parser.
//!
//! These cover the invariants that example-based tests cannot: that parsing is
//! total (never panics) on arbitrary input, that it is order-preserving, and
//! that [`Lyrics::line_at`] always agrees with a brute-force linear scan.

use std::time::Duration;

use perch_core::lrc::{LyricLine, Lyrics};
use proptest::prelude::*;

/// Brute-force reference implementation of `line_at`: the last line whose time
/// is `<= position`, found by a linear scan. The real one is a binary search.
fn line_at_linear(lines: &[LyricLine], position: Duration) -> Option<&LyricLine> {
    let mut found = None;
    for line in lines {
        if line.time <= position {
            found = Some(line);
        }
    }
    found
}

/// Bracket-heavy junk: the shapes a malformed LRC file actually takes.
fn bracket_junk() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("[".to_string()),
        Just("]".to_string()),
        Just(":".to_string()),
        Just(".".to_string()),
        Just("+".to_string()),
        Just("-".to_string()),
        Just("\n".to_string()),
        Just("\r\n".to_string()),
        Just("\u{feff}".to_string()),
        Just("offset".to_string()),
        Just("ar".to_string()),
        Just("99".to_string()),
        Just("00".to_string()),
        Just("mm".to_string()),
        Just("ü".to_string()),
        Just("🎵".to_string()),
        "[a-z0-9 ]{0,4}",
    ];
    proptest::collection::vec(piece, 0..40).prop_map(|parts| parts.concat())
}

/// A well-formed `(minutes, seconds, centis, text)` lyric entry.
fn entry() -> impl Strategy<Value = (u64, u64, u64, String)> {
    (0u64..200, 0u64..60, 0u64..100, "[a-zA-Z0-9 ]{0,20}")
}

/// Render entries as LRC text.
fn render(entries: &[(u64, u64, u64, String)]) -> String {
    let mut out = String::new();
    for (m, s, cs, text) in entries {
        out.push_str(&format!("[{m:02}:{s:02}.{cs:02}]{text}\n"));
    }
    out
}

/// The millisecond time an entry should parse to.
fn entry_millis(&(m, s, cs, _): &(u64, u64, u64, String)) -> u64 {
    m * 60_000 + s * 1_000 + cs * 10
}

proptest! {
    /// (a) Parsing is total: arbitrary text never panics.
    #[test]
    fn parse_never_panics_on_arbitrary_text(text in any::<String>()) {
        let lyrics = Lyrics::parse(&text);
        // Touch every query path so none of them can panic either.
        let _ = lyrics.len();
        let _ = lyrics.is_empty();
        let _ = lyrics.offset_millis();
        let _ = lyrics.line_at(Duration::from_secs(1));
        let _ = lyrics.index_at(Duration::ZERO);
        let _ = lyrics.next_time_after(Duration::from_secs(60));
    }

    /// (a) Parsing is total for bracket-heavy junk, which is where a parser is
    /// most likely to slice a string mid-character or index out of bounds.
    #[test]
    fn parse_never_panics_on_bracket_junk(text in bracket_junk()) {
        let lyrics = Lyrics::parse(&text);
        for line in lyrics.lines() {
            // Every produced line must be coherent.
            prop_assert!(line.text.is_char_boundary(0));
            prop_assert_eq!(line.is_blank(), line.text.trim().is_empty());
        }
        let _ = lyrics.line_at(Duration::from_millis(12_345));
    }

    /// (b) Well-formed input yields exactly one line per timestamp, sorted.
    #[test]
    fn well_formed_input_keeps_every_timestamp_and_sorts(
        entries in proptest::collection::vec(entry(), 0..40)
    ) {
        let lyrics = Lyrics::parse(&render(&entries));
        prop_assert_eq!(lyrics.len(), entries.len());

        let times: Vec<Duration> = lyrics.lines().iter().map(|l| l.time).collect();
        for pair in times.windows(2) {
            prop_assert!(pair[0] <= pair[1], "output must be non-decreasing by time");
        }

        // Same multiset of times as the input, just reordered.
        let mut want: Vec<u64> = entries.iter().map(entry_millis).collect();
        want.sort_unstable();
        let got: Vec<u64> = times.iter().map(|t| t.as_millis() as u64).collect();
        prop_assert_eq!(got, want);
    }

    /// (b2) Several timestamps on one line produce one entry each.
    #[test]
    fn repeated_timestamps_on_one_line_each_produce_an_entry(
        stamps in proptest::collection::vec((0u64..90, 0u64..60, 0u64..100), 1..8),
        text in "[a-z ]{0,12}",
    ) {
        let mut src = String::new();
        for (m, s, cs) in &stamps {
            src.push_str(&format!("[{m:02}:{s:02}.{cs:02}]"));
        }
        src.push_str(&text);

        let lyrics = Lyrics::parse(&src);
        prop_assert_eq!(lyrics.len(), stamps.len());
        for line in lyrics.lines() {
            prop_assert_eq!(&line.text, &text);
        }
    }

    /// (c) Round-trip: each rendered timestamp is findable, and `line_at` at
    /// that exact time reports the generated time back.
    #[test]
    fn round_trip_line_at_finds_each_generated_time(
        entries in proptest::collection::vec(entry(), 1..30)
    ) {
        let lyrics = Lyrics::parse(&render(&entries));
        for e in &entries {
            let at = Duration::from_millis(entry_millis(e));
            let line = match lyrics.line_at(at) {
                Some(l) => l,
                None => return Err(TestCaseError::fail("every generated time must be active")),
            };
            // Texts may repeat across entries, so assert on the time: the
            // active line at a generated timestamp is that timestamp.
            prop_assert_eq!(line.time, at);
        }
    }

    /// (c2) Round-trip with unique texts: the text comes back intact, even
    /// through unicode and an applied offset.
    #[test]
    fn round_trip_preserves_unique_texts(
        times in proptest::collection::hash_set(0u64..6_000, 1..25),
        offset in -5_000i64..5_000,
    ) {
        let mut entries: Vec<(u64, String)> = times
            .into_iter()
            .enumerate()
            .map(|(i, cs)| (cs * 10, format!("líne🎵{i}")))
            .collect();
        entries.sort_unstable();

        let mut src = format!("[offset:{offset}]\n");
        for (ms, text) in &entries {
            src.push_str(&format!("[{:02}:{:02}.{:03}]{}\r\n", ms / 60_000, (ms / 1000) % 60, ms % 1000, text));
        }

        let lyrics = Lyrics::parse(&src);
        prop_assert_eq!(lyrics.len(), entries.len());
        prop_assert_eq!(lyrics.offset_millis(), offset);

        for (ms, text) in &entries {
            // Where the line should now sit, with the offset applied and clamped.
            let shifted = (*ms as i64 + offset).max(0) as u64;
            let line = match lyrics.line_at(Duration::from_millis(shifted)) {
                Some(l) => l,
                None => return Err(TestCaseError::fail("offset line must be active")),
            };
            // Lines clamped to zero collapse together; only the un-clamped ones
            // are uniquely addressable by time.
            if *ms as i64 + offset > 0 {
                prop_assert_eq!(&line.text, text);
            }
        }
    }

    /// (d) `line_at` agrees with a linear scan for arbitrary query positions.
    #[test]
    fn line_at_agrees_with_linear_scan(
        entries in proptest::collection::vec(entry(), 0..40),
        queries in proptest::collection::vec(0u64..13_000_000, 1..30),
    ) {
        let lyrics = Lyrics::parse(&render(&entries));
        for q in queries {
            let at = Duration::from_millis(q);
            let fast = lyrics.line_at(at);
            let slow = line_at_linear(lyrics.lines(), at);
            prop_assert_eq!(fast, slow, "binary search disagreed with linear scan at {:?}", at);

            // The index must point at the very line `line_at` returned.
            match lyrics.index_at(at) {
                Some(i) => prop_assert_eq!(lyrics.lines().get(i), fast),
                None => prop_assert!(fast.is_none()),
            }
        }
    }

    /// (d2) `next_time_after` is the first time strictly greater than the query.
    #[test]
    fn next_time_after_agrees_with_linear_scan(
        entries in proptest::collection::vec(entry(), 0..40),
        queries in proptest::collection::vec(0u64..13_000_000, 1..30),
    ) {
        let lyrics = Lyrics::parse(&render(&entries));
        for q in queries {
            let at = Duration::from_millis(q);
            let want = lyrics.lines().iter().map(|l| l.time).find(|t| *t > at);
            prop_assert_eq!(lyrics.next_time_after(at), want);
        }
    }
}
