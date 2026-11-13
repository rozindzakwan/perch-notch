//! Synced lyrics from LRCLIB (<https://lrclib.net>), opt-in.
//!
//! A lookup happens at most once per track change, on a short-lived worker
//! thread, and the answer (including "not found") is cached on disk, so
//! replaying a song never touches the network again. The request carries only
//! track metadata and a generic user agent: nothing about the user.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::model::LyricsRequest;

/// FNV-1a 64-bit, for cache file names.
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn cache_key(req: &LyricsRequest) -> String {
    format!(
        "{:016x}",
        fnv1a(&format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{:?}",
            req.title.to_lowercase(),
            req.artist.to_lowercase(),
            req.album.to_lowercase(),
            req.duration_secs
        ))
    )
}

/// What a lookup produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// Synced LRC text.
    Found(String),
    /// LRCLIB has no synced lyrics for this track.
    NotFound,
    /// Network or parse failure (not cached; retried on the next track change).
    Failed(String),
}

/// Read a cached answer: `Some(Some(lrc))`, `Some(None)` = cached miss,
/// `None` = not cached.
pub fn read_cache(dir: &Path, req: &LyricsRequest) -> Option<Option<String>> {
    let base = dir.join(cache_key(req));
    if let Ok(text) = std::fs::read_to_string(base.with_extension("lrc")) {
        return Some(Some(text));
    }
    if base.with_extension("none").exists() {
        return Some(None);
    }
    None
}

/// Store an answer.
pub fn write_cache(dir: &Path, req: &LyricsRequest, lookup: &Lookup) {
    let base = dir.join(cache_key(req));
    let r = match lookup {
        Lookup::Found(lrc) => {
            crate::persist::write_atomic(&base.with_extension("lrc"), lrc.as_bytes())
        }
        Lookup::NotFound => crate::persist::write_atomic(&base.with_extension("none"), b""),
        Lookup::Failed(_) => return,
    };
    if let Err(e) = r {
        log::debug!("lyrics cache write: {e}");
    }
}

/// Parse an LRCLIB `/api/get` JSON body.
pub fn parse_response(body: &str) -> Lookup {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v) => match v.get("syncedLyrics").and_then(|s| s.as_str()) {
            Some(s) if !s.trim().is_empty() => Lookup::Found(s.to_string()),
            _ => Lookup::NotFound,
        },
        Err(e) => Lookup::Failed(format!("bad JSON from lrclib: {e}")),
    }
}

#[cfg(feature = "lyrics")]
fn fetch_network(req: &LyricsRequest) -> Lookup {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .user_agent(concat!(
            "Perch/",
            env!("CARGO_PKG_VERSION"),
            " (desktop notch app)"
        ))
        .build();
    let mut call = agent
        .get("https://lrclib.net/api/get")
        .query("track_name", &req.title)
        .query("artist_name", &req.artist);
    if !req.album.is_empty() {
        call = call.query("album_name", &req.album);
    }
    if let Some(d) = req.duration_secs {
        call = call.query("duration", &d.to_string());
    }
    match call.call() {
        Ok(resp) => match resp.into_string() {
            Ok(body) => parse_response(&body),
            Err(e) => Lookup::Failed(e.to_string()),
        },
        Err(ureq::Error::Status(404, _)) => Lookup::NotFound,
        Err(e) => Lookup::Failed(e.to_string()),
    }
}

#[cfg(not(feature = "lyrics"))]
fn fetch_network(_req: &LyricsRequest) -> Lookup {
    Lookup::Failed("lyrics are not compiled into this build".into())
}

/// Resolve a request (cache first, then the network) on a worker thread and
/// hand the result to `done`.
pub fn fetch_async(
    req: LyricsRequest,
    cache: Option<PathBuf>,
    done: impl FnOnce(String, Option<String>) + Send + 'static,
) {
    let spawn = std::thread::Builder::new()
        .name("perch-lyrics".into())
        .spawn(move || {
            if let Some(dir) = &cache {
                if let Some(hit) = read_cache(dir, &req) {
                    done(req.key.clone(), hit);
                    return;
                }
            }
            let lookup = fetch_network(&req);
            if let Some(dir) = &cache {
                write_cache(dir, &req, &lookup);
            }
            match lookup {
                Lookup::Found(lrc) => done(req.key.clone(), Some(lrc)),
                Lookup::NotFound => done(req.key.clone(), None),
                Lookup::Failed(e) => {
                    log::info!("lyrics lookup failed: {e}");
                    done(req.key.clone(), None);
                }
            }
        });
    if let Err(e) = spawn {
        log::warn!("cannot start the lyrics worker: {e}");
    }
}

/// Blocking lookup (no cache), for the opt-in live test and diagnostics.
pub fn lookup(req: &LyricsRequest) -> Lookup {
    fetch_network(req)
}
