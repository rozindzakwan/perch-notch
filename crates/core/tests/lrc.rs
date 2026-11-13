//! Unit tests for the LRC synced-lyrics parser.

use std::time::Duration;

use perch_core::lrc::Lyrics;

/// Parse helper that keeps the tests terse.
fn p(text: &str) -> Lyrics {
    Lyrics::parse(text)
}

#[test]
fn parses_a_single_timestamped_line() {
    let lyrics = p("[00:12.34]hello world");
    assert_eq!(lyrics.len(), 1);
    let line = match lyrics.lines().first() {
        Some(l) => l,
        None => panic!("expected one parsed line"),
    };
    assert_eq!(line.text, "hello world");
    assert_eq!(line.time, Duration::from_millis(12_340));
}

#[test]
fn accepts_every_timestamp_format_seen_in_the_wild() {
    // [mm:ss] with no fraction at all.
    assert_eq!(times("[01:02]a"), vec![Duration::from_millis(62_000)]);
    // One, two and three fraction digits after a dot.
    assert_eq!(times("[00:01.5]a"), vec![Duration::from_millis(1_500)]);
    assert_eq!(times("[00:01.05]a"), vec![Duration::from_millis(1_050)]);
    assert_eq!(times("[00:01.005]a"), vec![Duration::from_millis(1_005)]);
    // Colon as the fraction separator.
    assert_eq!(times("[00:01:25]a"), vec![Duration::from_millis(1_250)]);
    // Minutes beyond 99 (long mixes).
    assert_eq!(
        times("[123:45.67]a"),
        vec![Duration::from_millis(7_425_670)]
    );
}

/// The parsed timestamps, for terse assertions.
fn times(text: &str) -> Vec<Duration> {
    p(text).lines().iter().map(|l| l.time).collect()
}

#[test]
fn one_line_with_several_timestamps_yields_one_entry_each() {
    let lyrics = p("[00:12.00][01:30.50]same words");
    assert_eq!(lyrics.len(), 2);
    assert_eq!(
        times("[00:12.00][01:30.50]same words"),
        vec![Duration::from_millis(12_000), Duration::from_millis(90_500)]
    );
    for line in lyrics.lines() {
        assert_eq!(line.text, "same words");
    }
}

#[test]
fn id_tags_become_metadata_not_lyric_lines() {
    let lyrics = p("[ar:Artist Name]\n[ti:Title]\n[al:Album]\n[by:ripper]\n[length:03:41]\n[00:01.00]real line\n");
    assert_eq!(lyrics.len(), 1, "only the timed line is a lyric");
    assert_eq!(lyrics.metadata("ar"), Some("Artist Name"));
    assert_eq!(lyrics.metadata("ti"), Some("Title"));
    assert_eq!(lyrics.metadata("al"), Some("Album"));
    assert_eq!(lyrics.metadata("by"), Some("ripper"));
    assert_eq!(lyrics.metadata("length"), Some("03:41"));
    assert_eq!(lyrics.metadata("nope"), None);
}

#[test]
fn positive_offset_shifts_every_line_later() {
    let lyrics = p("[offset:+250]\n[00:01.00]a\n[00:02.00]b\n");
    assert_eq!(
        lyrics.lines().iter().map(|l| l.time).collect::<Vec<_>>(),
        vec![Duration::from_millis(1_250), Duration::from_millis(2_250)]
    );
    assert_eq!(lyrics.offset_millis(), 250);
}

#[test]
fn negative_offset_shifts_earlier_and_clamps_at_zero() {
    let lyrics = p("[offset:-1500]\n[00:01.00]early\n[00:10.00]later\n");
    assert_eq!(
        lyrics.lines().iter().map(|l| l.time).collect::<Vec<_>>(),
        vec![Duration::ZERO, Duration::from_millis(8_500)],
        "a line pushed before zero clamps to zero instead of underflowing"
    );
    assert_eq!(lyrics.offset_millis(), -1_500);
}

#[test]
fn unsorted_input_is_returned_in_time_order() {
    assert_eq!(
        texts("[00:30.00]third\n[00:10.00]first\n[00:20.00]second\n"),
        vec!["first", "second", "third"]
    );
}

#[test]
fn equal_timestamps_keep_their_input_order() {
    assert_eq!(
        texts("[00:05.00]alpha\n[00:05.00]beta\n[00:05.00]gamma\n"),
        vec!["alpha", "beta", "gamma"],
        "the sort must be stable for duplicate times"
    );
}

/// The parsed line texts, in output order.
fn texts(text: &str) -> Vec<String> {
    p(text).lines().iter().map(|l| l.text.clone()).collect()
}

#[test]
fn lookup_is_binary_search_over_a_large_sheet() {
    // 100_000 lines: a linear scan per query would be visible here, and a
    // broken midpoint would show up as a wrong answer.
    let mut src = String::new();
    for i in 0..100_000u64 {
        src.push_str(&format!(
            "[{:02}:{:02}.{:02}]line{}\n",
            i / 6000,
            (i / 100) % 60,
            i % 100,
            i
        ));
    }
    let lyrics = Lyrics::parse(&src);
    assert_eq!(lyrics.len(), 100_000);

    // Spot-check across the whole range, including both ends.
    for probe in [0u64, 1, 500, 49_999, 50_000, 99_998, 99_999] {
        let time = Duration::from_millis(probe * 10);
        let line = match lyrics.line_at(time) {
            Some(l) => l,
            None => panic!("expected a line at {time:?}"),
        };
        assert_eq!(line.text, format!("line{probe}"));
        assert_eq!(lyrics.index_at(time), Some(probe as usize));
    }
    // 1 ms before a boundary still resolves to the previous line.
    assert_eq!(
        lyrics
            .line_at(Duration::from_millis(50_000 * 10 - 1))
            .map(|l| l.text.clone()),
        Some("line49999".to_string())
    );
    assert!(lyrics
        .next_time_after(Duration::from_secs(100_000))
        .is_none());
}

/// Three-line fixture at 10s / 20s / 30s.
fn three_lines() -> Lyrics {
    p("[00:10.00]one\n[00:20.00]two\n[00:30.00]three\n")
}

#[test]
fn line_at_returns_the_last_line_at_or_before_the_position() {
    let lyrics = three_lines();
    let at = |secs| {
        lyrics
            .line_at(Duration::from_secs(secs))
            .map(|l| l.text.clone())
    };
    assert_eq!(at(0), None, "before the first timestamp there is no line");
    assert_eq!(at(9), None);
    assert_eq!(at(10), Some("one".to_string()), "boundary is inclusive");
    assert_eq!(at(15), Some("one".to_string()));
    assert_eq!(at(20), Some("two".to_string()));
    assert_eq!(at(29), Some("two".to_string()));
    assert_eq!(at(30), Some("three".to_string()));
    assert_eq!(at(9_999), Some("three".to_string()), "sticks on the last");
}

#[test]
fn index_at_reports_the_active_line_index_for_scrolling() {
    let lyrics = three_lines();
    assert_eq!(lyrics.index_at(Duration::from_secs(5)), None);
    assert_eq!(lyrics.index_at(Duration::from_secs(10)), Some(0));
    assert_eq!(lyrics.index_at(Duration::from_secs(25)), Some(1));
    assert_eq!(lyrics.index_at(Duration::from_secs(99)), Some(2));
}

#[test]
fn next_time_after_lets_the_caller_schedule_an_exact_wakeup() {
    let lyrics = three_lines();
    // Before the first line: wake at the first line.
    assert_eq!(
        lyrics.next_time_after(Duration::from_secs(0)),
        Some(Duration::from_secs(10))
    );
    // Exactly on a line: the next distinct line.
    assert_eq!(
        lyrics.next_time_after(Duration::from_secs(10)),
        Some(Duration::from_secs(20))
    );
    assert_eq!(
        lyrics.next_time_after(Duration::from_secs(15)),
        Some(Duration::from_secs(20))
    );
    // Past the last line: nothing more to schedule, distinct from "no active".
    assert_eq!(lyrics.next_time_after(Duration::from_secs(30)), None);
    assert_eq!(lyrics.next_time_after(Duration::from_secs(60)), None);
}

#[test]
fn no_active_line_is_distinguishable_from_no_further_line() {
    let lyrics = three_lines();
    // Before the start: no active line, but a next line exists.
    assert!(lyrics.index_at(Duration::ZERO).is_none());
    assert!(lyrics.next_time_after(Duration::ZERO).is_some());
    // After the end: an active line exists, but no next line.
    assert!(lyrics.index_at(Duration::from_secs(99)).is_some());
    assert!(lyrics.next_time_after(Duration::from_secs(99)).is_none());
}

#[test]
fn empty_lyrics_answer_every_query_with_none() {
    let lyrics = p("");
    assert!(lyrics.is_empty());
    assert_eq!(lyrics.len(), 0);
    assert!(lyrics.line_at(Duration::from_secs(5)).is_none());
    assert!(lyrics.index_at(Duration::from_secs(5)).is_none());
    assert!(lyrics.next_time_after(Duration::from_secs(5)).is_none());
}

#[test]
fn a_timestamp_with_no_words_is_a_blank_interlude_line() {
    let lyrics = p("[00:01.00]sing\n[00:05.00]\n[00:09.00]again\n");
    assert_eq!(lyrics.len(), 3, "the interlude is kept as a line");
    let blank = match lyrics.line_at(Duration::from_secs(6)) {
        Some(l) => l,
        None => panic!("expected the interlude to be active at 6s"),
    };
    assert!(blank.is_blank(), "an empty interlude is queryable as blank");
    assert_eq!(blank.text, "");

    let sung = match lyrics.line_at(Duration::from_secs(2)) {
        Some(l) => l,
        None => panic!("expected a line at 2s"),
    };
    assert!(!sung.is_blank());
}

#[test]
fn whitespace_only_text_counts_as_blank() {
    let lyrics = p("[00:01.00]   \n");
    let line = match lyrics.lines().first() {
        Some(l) => l,
        None => panic!("expected one line"),
    };
    assert!(line.is_blank(), "whitespace-only lyrics are blank");
}

#[test]
fn a_run_of_blank_lines_does_not_break_lookup() {
    let lyrics = p("[00:01.00]a\n[00:02.00]\n[00:03.00]\n[00:04.00]\n[00:05.00]b\n");
    assert_eq!(lyrics.len(), 5);
    assert_eq!(lyrics.index_at(Duration::from_secs(2)), Some(1));
    assert_eq!(lyrics.index_at(Duration::from_secs(3)), Some(2));
    assert_eq!(lyrics.index_at(Duration::from_secs(4)), Some(3));
    assert_eq!(lyrics.index_at(Duration::from_secs(5)), Some(4));
    assert_eq!(
        lyrics.next_time_after(Duration::from_secs(2)),
        Some(Duration::from_secs(3))
    );
    let last = match lyrics.line_at(Duration::from_secs(5)) {
        Some(l) => l,
        None => panic!("expected the final line"),
    };
    assert_eq!(last.text, "b");
}

#[test]
fn crlf_line_endings_do_not_leak_into_the_text() {
    let lyrics = p("[00:01.00]first\r\n[00:02.00]second\r\n");
    assert_eq!(
        texts("[00:01.00]first\r\n[00:02.00]second\r\n"),
        ["first", "second"]
    );
    assert_eq!(lyrics.len(), 2);
}

#[test]
fn a_leading_bom_does_not_eat_the_first_line() {
    let lyrics = p("\u{feff}[00:01.00]first\n[00:02.00]second\n");
    assert_eq!(lyrics.len(), 2, "the BOM must not break the first stamp");
    assert_eq!(
        lyrics
            .line_at(Duration::from_secs(1))
            .map(|l| l.text.clone()),
        Some("first".to_string())
    );
}

#[test]
fn leading_whitespace_before_a_timestamp_is_tolerated() {
    let lyrics = p("   [00:01.00]indented\n\t[00:02.00]tabbed\n");
    assert_eq!(lyrics.len(), 2);
    assert_eq!(
        texts("   [00:01.00]indented\n\t[00:02.00]tabbed\n"),
        ["indented", "tabbed"]
    );
}

#[test]
fn malformed_lines_are_dropped_without_failing_the_parse() {
    let lyrics = p(concat!(
        "not a lyric line at all\n",
        "[not-a-time]text\n",
        "[99:99\n",
        "[]\n",
        "[:]\n",
        "[ab:cd]\n",
        "[00:xx.00]text\n",
        "[00:01.0000]too many digits\n",
        "[00:02.00]good\n",
    ));
    assert_eq!(lyrics.len(), 1, "only the well-formed line survives");
    assert_eq!(
        lyrics.lines().first().map(|l| l.text.clone()),
        Some("good".to_string())
    );
}

#[test]
fn text_after_a_timestamp_keeps_its_internal_brackets() {
    let lyrics = p("[00:01.00]shout [loudly] now\n");
    assert_eq!(
        lyrics.lines().first().map(|l| l.text.clone()),
        Some("shout [loudly] now".to_string())
    );
}

#[test]
fn unicode_text_survives_intact() {
    let src = "[00:01.00]君の名は\n[00:02.00]Selamat pagi, dunia\n[00:03.00]🎵 emoji 🎶\n";
    let lyrics = p(src);
    assert_eq!(lyrics.len(), 3);
    assert_eq!(
        texts(src),
        ["君の名は", "Selamat pagi, dunia", "🎵 emoji 🎶"]
    );
    // No byte-slicing damage: every line is still valid UTF-8 of the right length.
    assert_eq!(
        lyrics.lines().first().map(|l| l.text.chars().count()),
        Some(4)
    );
}
