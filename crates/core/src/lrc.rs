//! Synced-lyrics (`.lrc`) parsing.
//!
//! LRCLIB (<https://lrclib.net>) returns a `syncedLyrics` string; Perch parses
//! it once per track and then asks [`Lyrics::line_at`] which line is current.
//! Parsing never fails: malformed input simply yields fewer lines.

use std::time::Duration;

/// One timed lyric line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    /// Offset from the start of the track, with any `[offset:]` tag applied.
    pub time: Duration,
    /// The words sung at [`LyricLine::time`]; empty for an interlude.
    pub text: String,
}

impl LyricLine {
    /// True when this line has no words — an interlude the UI renders as a
    /// gap or a pulsing dot rather than text.
    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// A parsed LRC document: timed lines in time order, plus the ID tags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lyrics {
    lines: Vec<LyricLine>,
    metadata: Vec<(String, String)>,
    offset_millis: i64,
}

impl Lyrics {
    /// Parse LRC text. Never fails and never panics.
    pub fn parse(text: &str) -> Self {
        // A UTF-8 BOM would otherwise make the first `[` unreachable.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut lines = Vec::new();
        let mut metadata = Vec::new();
        for raw in text.lines() {
            // `str::lines` already drops a trailing `\r` from CRLF input.
            let raw = raw.trim_start();
            let (stamps, body) = split_stamps(raw);
            if stamps.is_empty() {
                if let Some((key, value)) = parse_id_tag(raw) {
                    metadata.push((key.to_string(), value.to_string()));
                }
                continue;
            }
            for time in stamps {
                lines.push(LyricLine {
                    time,
                    text: body.to_string(),
                });
            }
        }

        let offset_millis = metadata
            .iter()
            .find(|(k, _)| k == "offset")
            .and_then(|(_, v)| parse_offset(v))
            .unwrap_or(0);
        if offset_millis != 0 {
            for line in &mut lines {
                line.time = shift(line.time, offset_millis);
            }
        }

        // `sort_by_key` is a stable sort, so lines sharing a timestamp keep the
        // order they appeared in the file.
        lines.sort_by_key(|line| line.time);

        Self {
            lines,
            metadata,
            offset_millis,
        }
    }

    /// The `[offset:]` tag in milliseconds (positive = lyrics shown later),
    /// already applied to every line's time. `0` when the tag is absent.
    pub fn offset_millis(&self) -> i64 {
        self.offset_millis
    }

    /// Every parsed line, ordered by time.
    pub fn lines(&self) -> &[LyricLine] {
        &self.lines
    }

    /// The value of an ID tag such as `ar`, `ti`, `al`, `by` or `length`.
    pub fn metadata(&self, key: &str) -> Option<&str> {
        self.metadata
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Number of parsed lines.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// True when there are no timed lines to display.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Index of the line being sung at `position`: the LAST line whose time is
    /// `<= position`. `None` when `position` is before the first timestamp (or
    /// there are no lines).
    ///
    /// O(log n): the lines are sorted by time, so this is a binary search —
    /// the UI may call it on every frame without scanning the lyric sheet.
    pub fn index_at(&self, position: Duration) -> Option<usize> {
        // `partition_point` is a binary search: it returns the number of lines
        // with `time <= position`, so the active line is the one before it.
        let count = self.lines.partition_point(|line| line.time <= position);
        count.checked_sub(1)
    }

    /// The line being sung at `position`, or `None` before the first timestamp.
    ///
    /// O(log n); see [`Lyrics::index_at`].
    pub fn line_at(&self, position: Duration) -> Option<&LyricLine> {
        self.lines.get(self.index_at(position)?)
    }

    /// Time of the first line strictly after `position`, so the caller can
    /// sleep until exactly then instead of polling. `None` when no later line
    /// exists — which is distinct from [`Lyrics::index_at`] returning `None`
    /// for "nothing active yet".
    ///
    /// O(log n): binary search over the sorted times.
    pub fn next_time_after(&self, position: Duration) -> Option<Duration> {
        let index = self.lines.partition_point(|line| line.time <= position);
        self.lines.get(index).map(|line| line.time)
    }
}

/// Parse an `[offset:]` value in milliseconds. Accepts a leading `+` or `-`.
fn parse_offset(value: &str) -> Option<i64> {
    let value = value.trim();
    let (negative, digits) = match value.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, value.strip_prefix('+').unwrap_or(value)),
    };
    let magnitude = i64::try_from(parse_digits(digits.trim())?).ok()?;
    Some(if negative { -magnitude } else { magnitude })
}

/// Apply a signed millisecond offset to a time, clamping at zero so a lyric
/// pulled before the start of the track never underflows.
fn shift(time: Duration, offset_millis: i64) -> Duration {
    if offset_millis >= 0 {
        time.saturating_add(Duration::from_millis(offset_millis.unsigned_abs()))
    } else {
        time.saturating_sub(Duration::from_millis(offset_millis.unsigned_abs()))
    }
}

/// Parse an ID tag line such as `[ar:Artist]` into its key and value. The key
/// must be non-empty and alphabetic, which keeps junk like `[???]` out.
fn parse_id_tag(raw: &str) -> Option<(&str, &str)> {
    let trimmed = raw.trim();
    let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    let (key, value) = inner.split_once(':')?;
    let key = key.trim();
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    Some((key, value.trim()))
}

/// Split a raw line into its run of leading `[...]` timestamps and the text
/// that follows. Non-timestamp brackets (ID tags) end the run and are dropped.
fn split_stamps(raw: &str) -> (Vec<Duration>, &str) {
    let mut stamps = Vec::new();
    let mut rest = raw;
    while let Some(after) = rest.strip_prefix('[') {
        let Some(close) = after.find(']') else { break };
        let Some(time) = parse_timestamp(&after[..close]) else {
            break;
        };
        stamps.push(time);
        rest = &after[close + 1..];
    }
    (stamps, rest)
}

/// Parse the inside of a `[...]` timestamp into a [`Duration`].
///
/// Accepts `mm:ss`, `mm:ss.x`, `mm:ss.xx`, `mm:ss.xxx` and the in-the-wild
/// `mm:ss:xx` variant that uses a colon as the fraction separator. Minutes are
/// unbounded. Returns `None` for anything else.
fn parse_timestamp(stamp: &str) -> Option<Duration> {
    let stamp = stamp.trim();
    let (minutes, rest) = stamp.split_once(':')?;
    let minutes: u64 = parse_digits(minutes)?;
    let (seconds, frac) = match rest.find(['.', ':']) {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    let seconds: u64 = parse_digits(seconds)?;
    let millis = match frac {
        Some(f) => parse_fraction(f)?,
        None => 0,
    };
    Some(Duration::from_millis(
        minutes * 60_000 + seconds * 1_000 + millis,
    ))
}

/// Parse an all-ASCII-digit field, rejecting signs, spaces and empties.
fn parse_digits(field: &str) -> Option<u64> {
    if field.is_empty() || !field.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    field.parse().ok()
}

/// Turn 1-3 fraction digits into milliseconds (`5` -> 500, `05` -> 50, `005` -> 5).
fn parse_fraction(frac: &str) -> Option<u64> {
    let value = parse_digits(frac)?;
    match frac.len() {
        1 => Some(value * 100),
        2 => Some(value * 10),
        3 => Some(value),
        _ => None,
    }
}
