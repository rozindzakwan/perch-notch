//! Clipboard history with limits and password-manager exclusion.
//!
//! This module is pure: it never touches the OS clipboard and never reads the
//! clock. The platform layer observes clipboard changes, converts them into a
//! [`ClipboardContent`] plus a [`ContentFlags`] bundle, and calls
//! [`ClipboardHistory::push`] with the capture time.
//!
//! Timestamps are **`u64` seconds since the Unix epoch**. `u64` keeps the type
//! serde- and SQLite-friendly, and seconds are plenty for a UI that groups
//! entries by "a minute ago"; a monotonic clock is deliberately not used so the
//! value survives a restart.
//!
//! # Privacy
//!
//! Clipboard history is one of the easiest ways to leak a password, so anything
//! flagged as sensitive is dropped. [`ContentFlags`] carries the real-world
//! signals:
//!
//! * Windows sets the clipboard formats `ExcludeClipboardContentFromMonitorProcessing`,
//!   `CanIncludeInClipboardHistory` (value `0`) and `CanUploadToCloudClipboard`
//!   (value `0`) when an app asks history keepers to stay away.
//! * X11/Wayland password managers (KeePassXC, KWallet) set the selection
//!   target `x-kde-passwordManagerHint` to the value `secret`.
//! * The user can additionally deny-list source application names.
//!
//! Entries also redact themselves in [`std::fmt::Debug`]: Perch logs liberally,
//! and a `{:?}` of an entry must never print the clipboard payload.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::config::ClipboardConfig;

/// Encoding of a captured clipboard image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageFormat {
    /// PNG bytes.
    Png,
    /// Windows device-independent bitmap.
    Bmp,
    /// JPEG bytes.
    Jpeg,
}

/// The payload of one clipboard entry.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ClipboardContent {
    /// Plain UTF-8 text.
    Text {
        /// The text itself.
        value: String,
    },
    /// A raster image.
    Image {
        /// Encoded image bytes.
        bytes: Vec<u8>,
        /// Pixel width.
        width: u32,
        /// Pixel height.
        height: u32,
        /// Byte encoding of `bytes`.
        format: ImageFormat,
    },
}

impl ClipboardContent {
    /// Build a text payload.
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text { value: s.into() }
    }

    /// Build an image payload.
    pub fn image(bytes: Vec<u8>, width: u32, height: u32, format: ImageFormat) -> Self {
        Self::Image {
            bytes,
            width,
            height,
            format,
        }
    }

    /// The text of a [`ClipboardContent::Text`] payload, else `None`.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { value } => Some(value.as_str()),
            Self::Image { .. } => None,
        }
    }

    /// Size of the payload in bytes.
    pub fn byte_len(&self) -> usize {
        match self {
            Self::Text { value } => value.len(),
            Self::Image { bytes, .. } => bytes.len(),
        }
    }
}

/// Redacted: prints the shape of the payload, never the payload itself.
impl fmt::Debug for ClipboardContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { value } => write!(f, "Text(<redacted {} bytes>)", value.len()),
            Self::Image {
                bytes,
                width,
                height,
                format,
            } => write!(
                f,
                "Image {{ {width}x{height} {format:?}, <redacted {} bytes> }}",
                bytes.len()
            ),
        }
    }
}

/// Stable identifier for an entry, unique within one [`ClipboardHistory`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct EntryId(pub u64);

/// One retained clipboard entry.
///
/// Deserialization is tolerant: a missing `captured_at` defaults to `0` and
/// unknown fields from a newer Perch are ignored, so an old or new on-disk
/// payload still loads.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardEntry {
    #[serde(default)]
    id: EntryId,
    content: ClipboardContent,
    #[serde(default)]
    captured_at: u64,
}

impl ClipboardEntry {
    /// Stable id of this entry.
    pub fn id(&self) -> EntryId {
        self.id
    }

    /// The payload.
    pub fn content(&self) -> &ClipboardContent {
        &self.content
    }

    /// Capture time, Unix seconds.
    pub fn captured_at(&self) -> u64 {
        self.captured_at
    }

    /// The text payload, if this entry is text.
    pub fn as_text(&self) -> Option<&str> {
        self.content.as_text()
    }
}

/// Redacted: never prints the clipboard payload.
impl fmt::Debug for ClipboardEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClipboardEntry")
            .field("id", &self.id)
            .field("captured_at", &self.captured_at)
            .field("content", &self.content)
            .finish()
    }
}

/// Signals supplied by the platform layer about one clipboard change.
///
/// The platform layer fills in whatever its OS offers and leaves the rest at
/// the default. Nothing here contains the payload, so a `ContentFlags` is safe
/// to log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentFlags {
    /// A password manager marked this content as a secret (the generic form of
    /// X11's `x-kde-passwordManagerHint: secret`).
    pub password_manager_hint: bool,
    /// The source rendered the content concealed (a password field).
    pub concealed: bool,
    /// Process / application name that owned the selection, if known.
    pub source_app: Option<String>,
    /// Windows clipboard exclusion formats and their `u32` values, e.g.
    /// `("CanIncludeInClipboardHistory", 0)`.
    pub windows_formats: Vec<(String, u32)>,
    /// X11/Wayland selection targets and their textual values, e.g.
    /// `("x-kde-passwordManagerHint", "secret")`.
    pub x11_targets: Vec<(String, String)>,
}

/// Windows format whose mere presence means "do not record this".
const WIN_EXCLUDE_FORMAT: &str = "excludeclipboardcontentfrommonitorprocessing";
/// Windows formats where a value of `0` means "do not record this".
const WIN_OPT_OUT_ZERO_FORMATS: [&str; 2] =
    ["canincludeinclipboardhistory", "canuploadtocloudclipboard"];
/// X11/Wayland target used by KDE-family password managers.
const X11_PASSWORD_HINT_TARGET: &str = "x-kde-passwordmanagerhint";
/// The value of [`X11_PASSWORD_HINT_TARGET`] that marks a secret.
const X11_PASSWORD_HINT_SECRET: &str = "secret";

impl ContentFlags {
    /// Record a Windows clipboard format and its value.
    pub fn with_windows_format(mut self, name: impl Into<String>, value: u32) -> Self {
        self.windows_formats.push((name.into(), value));
        self
    }

    /// Record an X11/Wayland selection target and its textual value.
    pub fn with_x11_target(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.x11_targets.push((name.into(), value.into()));
        self
    }

    /// Record the owning application name.
    pub fn with_source_app(mut self, app: impl Into<String>) -> Self {
        self.source_app = Some(app.into());
        self
    }

    /// True when any OS-level signal asks history keepers to stay away.
    ///
    /// Covers the Windows exclusion formats and the KDE password-manager hint,
    /// plus the two generic booleans. Matching is case-insensitive because the
    /// format/target spellings differ between toolkits.
    pub fn is_excluded(&self) -> bool {
        if self.password_manager_hint || self.concealed {
            return true;
        }
        let win = self.windows_formats.iter().any(|(name, value)| {
            let lower = name.to_ascii_lowercase();
            if lower == WIN_EXCLUDE_FORMAT {
                return true;
            }
            WIN_OPT_OUT_ZERO_FORMATS.contains(&lower.as_str()) && *value == 0
        });
        if win {
            return true;
        }
        self.x11_targets.iter().any(|(name, value)| {
            name.to_ascii_lowercase() == X11_PASSWORD_HINT_TARGET
                && value.trim().to_ascii_lowercase() == X11_PASSWORD_HINT_SECRET
        })
    }
}

/// Why a push did not store a new entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// The source asked to be excluded from clipboard history.
    Excluded,
    /// The source app is on the user's deny-list.
    DenyListed,
    /// Image capture is disabled by config.
    ImagesDisabled,
    /// The image exceeds `max_image_kb`; it is rejected, never truncated.
    ImageTooLarge,
    /// The payload was empty.
    Empty,
}

/// Result of [`ClipboardHistory::push`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushOutcome {
    /// A new entry was added.
    Stored {
        /// Id of the new entry.
        id: EntryId,
        /// Ids evicted to respect `history_limit`, oldest first.
        evicted: Vec<EntryId>,
    },
    /// Identical content was already present; it was refreshed and promoted.
    Refreshed {
        /// Id of the pre-existing entry, which keeps its identity.
        id: EntryId,
    },
    /// Nothing was stored.
    Skipped(SkipReason),
}

/// A persistable snapshot of a board, most-recent-first.
///
/// `#[serde(default)]` on every field keeps old and new payloads loadable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardSnapshot {
    /// Retained entries, most-recent-first.
    pub entries: Vec<ClipboardEntry>,
}

/// A bounded, most-recent-first clipboard board.
#[derive(Debug, Clone)]
pub struct ClipboardHistory {
    config: ClipboardConfig,
    deny_list: Vec<String>,
    entries: Vec<ClipboardEntry>,
    next_id: u64,
}

impl ClipboardHistory {
    /// Create an empty history governed by `config`.
    pub fn new(config: ClipboardConfig) -> Self {
        Self {
            config,
            deny_list: Vec::new(),
            entries: Vec::new(),
            next_id: 1,
        }
    }

    /// Replace the user's deny-list of source application names.
    ///
    /// Entries are trimmed and compared case-insensitively, so
    /// `"  KeePassXC.exe "` matches a source app of `"keepassxc.exe"`.
    pub fn set_deny_list<I, S>(&mut self, apps: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.deny_list = apps
            .into_iter()
            .map(|a| a.as_ref().trim().to_ascii_lowercase())
            .filter(|a| !a.is_empty())
            .collect();
    }

    /// The configured deny-list, normalised to trimmed lowercase.
    pub fn deny_list(&self) -> &[String] {
        &self.deny_list
    }

    /// Decide whether content carrying `flags` must be dropped on privacy
    /// grounds, and why. `None` means it is safe to retain.
    pub fn should_skip(&self, flags: &ContentFlags) -> Option<SkipReason> {
        if flags.is_excluded() {
            return Some(SkipReason::Excluded);
        }
        let app = flags.source_app.as_deref()?.trim().to_ascii_lowercase();
        if !app.is_empty() && self.deny_list.contains(&app) {
            return Some(SkipReason::DenyListed);
        }
        None
    }

    /// Number of retained entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is retained.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries, most-recent-first.
    pub fn iter(&self) -> impl Iterator<Item = &ClipboardEntry> {
        self.entries.iter()
    }

    /// Remove every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Remove the entry with `id`. Returns `true` when something was removed.
    pub fn remove(&mut self, id: EntryId) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() != before
    }

    /// Take a persistable snapshot of the board.
    pub fn snapshot(&self) -> ClipboardSnapshot {
        ClipboardSnapshot {
            entries: self.entries.clone(),
        }
    }

    /// Rebuild a board from a persisted snapshot, re-applying `history_limit`
    /// and ensuring newly minted ids cannot collide with restored ones.
    pub fn from_snapshot(config: ClipboardConfig, snapshot: ClipboardSnapshot) -> Self {
        let next_id = snapshot
            .entries
            .iter()
            .map(|e| e.id.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let mut history = Self {
            config,
            deny_list: Vec::new(),
            entries: snapshot.entries,
            next_id,
        };
        history.trim();
        history
    }

    /// Offer `content` captured at `now` to the board.
    ///
    /// Deduplication: if an entry with byte-identical content already exists it
    /// is **refreshed** (its `captured_at` becomes `now`) and **promoted** to
    /// the front, keeping its original [`EntryId`] so UI selections and
    /// persisted rows stay valid. This holds whether the duplicate was the most
    /// recent entry or an older one, so re-copying something never grows the
    /// board.
    ///
    /// Oldest entries are evicted first so `history_limit` is never exceeded.
    pub fn push(&mut self, content: ClipboardContent, now: u64) -> PushOutcome {
        self.push_with_flags(content, now, &ContentFlags::default())
    }

    /// Like [`ClipboardHistory::push`], but consulting the platform-supplied
    /// privacy `flags` first. Flagged content is dropped before it is ever
    /// stored, so a secret never reaches the board.
    pub fn push_with_flags(
        &mut self,
        content: ClipboardContent,
        now: u64,
        flags: &ContentFlags,
    ) -> PushOutcome {
        if let Some(reason) = self.should_skip(flags) {
            return PushOutcome::Skipped(reason);
        }
        if let Some(reason) = self.rejection(&content) {
            return PushOutcome::Skipped(reason);
        }
        if let Some(pos) = self.entries.iter().position(|e| e.content == content) {
            let mut existing = self.entries.remove(pos);
            existing.captured_at = now;
            let id = existing.id;
            self.entries.insert(0, existing);
            return PushOutcome::Refreshed { id };
        }
        let id = EntryId(self.next_id);
        self.next_id += 1;
        self.entries.insert(
            0,
            ClipboardEntry {
                id,
                content,
                captured_at: now,
            },
        );
        PushOutcome::Stored {
            id,
            evicted: self.trim(),
        }
    }

    /// Why `content` cannot be retained under the current config, if so.
    ///
    /// Rejected content leaves the board completely untouched: a dropped image
    /// never evicts a retained text entry.
    fn rejection(&self, content: &ClipboardContent) -> Option<SkipReason> {
        if content.byte_len() == 0 {
            return Some(SkipReason::Empty);
        }
        if let ClipboardContent::Image { bytes, .. } = content {
            if !self.config.keep_images {
                return Some(SkipReason::ImagesDisabled);
            }
            if bytes.len() > self.config.max_image_kb.saturating_mul(1024) {
                return Some(SkipReason::ImageTooLarge);
            }
        }
        None
    }

    /// Drop the oldest entries until `history_limit` is respected, returning the
    /// evicted ids oldest-first.
    fn trim(&mut self) -> Vec<EntryId> {
        let limit = self.config.history_limit.max(1);
        if self.entries.len() <= limit {
            return Vec::new();
        }
        let mut evicted: Vec<EntryId> =
            self.entries.split_off(limit).iter().map(|e| e.id).collect();
        evicted.reverse();
        evicted
    }
}
