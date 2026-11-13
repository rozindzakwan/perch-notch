use perch_app::lyrics::{parse_response, read_cache, write_cache, Lookup};
use perch_app::model::LyricsRequest;

fn req(title: &str) -> LyricsRequest {
    LyricsRequest {
        key: "k".into(),
        title: title.into(),
        artist: "Artist".into(),
        album: "Album".into(),
        duration_secs: Some(200),
    }
}

#[test]
fn lrclib_responses_are_parsed() {
    let found = r#"{"id":1,"trackName":"x","syncedLyrics":"[00:01.00]hello\n[00:02.00]world","plainLyrics":"hello"}"#;
    assert_eq!(
        parse_response(found),
        Lookup::Found("[00:01.00]hello\n[00:02.00]world".into())
    );
    assert_eq!(
        parse_response(r#"{"syncedLyrics":null,"plainLyrics":"x"}"#),
        Lookup::NotFound
    );
    assert_eq!(
        parse_response(r#"{"syncedLyrics":"   "}"#),
        Lookup::NotFound
    );
    assert!(matches!(parse_response("<html>"), Lookup::Failed(_)));
}

#[test]
fn cache_remembers_hits_and_misses_but_not_failures() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let a = req("Song A");
    assert_eq!(read_cache(dir.path(), &a), None, "nothing cached yet");
    write_cache(dir.path(), &a, &Lookup::Found("[00:01.00]x".into()));
    assert_eq!(read_cache(dir.path(), &a), Some(Some("[00:01.00]x".into())));

    let b = req("Song B");
    write_cache(dir.path(), &b, &Lookup::NotFound);
    assert_eq!(
        read_cache(dir.path(), &b),
        Some(None),
        "a miss is remembered: no repeat network call"
    );

    let c = req("Song C");
    write_cache(dir.path(), &c, &Lookup::Failed("offline".into()));
    assert_eq!(
        read_cache(dir.path(), &c),
        None,
        "failures are retried next time"
    );

    // Case-insensitive identity.
    let mut a2 = req("SONG A");
    a2.artist = "ARTIST".into();
    a2.album = "ALBUM".into();
    assert_eq!(
        read_cache(dir.path(), &a2),
        Some(Some("[00:01.00]x".into()))
    );
}

/// Talks to lrclib.net, so it is opt-in: `cargo test -p perch --test lyrics --
/// --ignored`. Sends only the song's public metadata.
#[test]
#[ignore = "network: queries lrclib.net"]
fn lrclib_live_lookup_returns_synced_lyrics_for_a_well_known_song() {
    let r = LyricsRequest {
        key: "live".into(),
        title: "Bohemian Rhapsody".into(),
        artist: "Queen".into(),
        album: "A Night at the Opera".into(),
        duration_secs: Some(354),
    };
    match perch_app::lyrics::lookup(&r) {
        Lookup::Found(lrc) => {
            let parsed = perch_core::lrc::Lyrics::parse(&lrc);
            assert!(
                parsed.len() > 10,
                "synced lines expected, got {}",
                parsed.len()
            );
        }
        other => panic!("expected synced lyrics, got {other:?}"),
    }
}
