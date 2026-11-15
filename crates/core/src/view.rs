//! The view model: plain data describing what the notch shows, plus the
//! [`Action`]s a click on it can produce.
//!
//! The app builds a [`PanelView`] from its state, the renderer paints it and
//! returns hit regions tagged with actions, and clicks come back as actions.
//! Nothing here knows about windows or pixels, so the whole interaction loop is
//! testable without a display.

use crate::clipboard::EntryId;
use crate::noise::NoiseKind;
use crate::platform::TransportCommand;
use crate::shelf::ShelfId;
use crate::todo::TodoId;

/// Tabs in the expanded notch. Only tabs whose feature is live are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    /// Time, date and location (always first).
    Today,
    /// Now playing.
    Media,
    /// To-do + Pomodoro + reminders.
    Tasks,
    /// Clipboard history.
    Clipboard,
    /// File shelf.
    Shelf,
    /// Per-app volume.
    Mixer,
    /// White noise.
    Sounds,
    /// Screen time.
    Time,
    /// Screenshot / camera / dictation.
    Tools,
    /// Unread mail (read-only).
    Mail,
}

impl Tab {
    /// Short label for the tab strip.
    pub fn label(&self) -> &'static str {
        match self {
            Tab::Today => "Today",
            Tab::Media => "Music",
            Tab::Tasks => "Tasks",
            Tab::Clipboard => "Clips",
            Tab::Shelf => "Shelf",
            Tab::Mixer => "Mixer",
            Tab::Sounds => "Sounds",
            Tab::Time => "Time",
            Tab::Tools => "Tools",
            Tab::Mail => "Mail",
        }
    }
}

/// What a slider controls; the click position supplies the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SliderTarget {
    /// The media seek bar.
    Seek,
    /// One audio session's volume.
    Volume(String),
    /// One noise layer's level.
    Noise(NoiseKind),
}

/// Everything a click (or the tray, CLI, or a hotkey) can ask for.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Switch the expanded panel to a tab.
    SelectTab(Tab),
    /// Scroll the current list by this many rows (negative = up).
    Scroll(i32),
    /// Media transport.
    Transport(TransportCommand),
    /// A slider was set to `value` (0..=1).
    Slider(SliderTarget, f32),
    /// Open the add-to-do dialog.
    TodoAdd,
    /// Toggle a to-do's done flag.
    TodoToggle(TodoId),
    /// Delete a to-do.
    TodoRemove(TodoId),
    /// Start / pause / resume the Pomodoro.
    PomodoroToggle,
    /// Skip to the next Pomodoro phase.
    PomodoroSkip,
    /// Stop and reset the Pomodoro.
    PomodoroReset,
    /// Copy a clipboard history entry back to the clipboard.
    ClipCopy(EntryId),
    /// Delete a clipboard history entry.
    ClipRemove(EntryId),
    /// Clear the clipboard history.
    ClipClear,
    /// Open a shelf file with its default app.
    ShelfOpen(ShelfId),
    /// Put a shelf file on the clipboard as a file.
    ShelfCopy(ShelfId),
    /// Remove a shelf entry.
    ShelfRemove(ShelfId),
    /// Empty the shelf.
    ShelfClear,
    /// Mute / unmute an audio session.
    MuteToggle(String),
    /// Toggle a noise layer.
    NoiseToggle(NoiseKind),
    /// Start a screenshot.
    Screenshot,
    /// Toggle the camera mirror.
    CameraToggle,
    /// Start/stop dictation.
    DictateToggle,
    /// Open the settings window.
    OpenSettings,
    /// Pin/unpin the notch open.
    PinToggle,
    /// The banner was clicked: open the panel on its tab.
    BannerOpen,
}

/// A small icon the pill can show when there is no cover art. The renderer
/// maps it to its own icon set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    /// Something is playing (no cover).
    Music,
    /// A timer is running.
    Timer,
    /// Mail.
    Mail,
    /// A clock.
    Clock,
    /// Attention needed.
    Warning,
}

/// What a badge counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind {
    /// Unread mail.
    Mail,
}

/// A count shown at the right end of the pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Badge {
    /// What it counts.
    pub kind: BadgeKind,
    /// How many.
    pub count: u32,
}

/// A notification shown in the notch: the pill stretches into this banner.
#[derive(Debug, Clone, PartialEq)]
pub struct BannerView {
    /// Avatar or icon.
    pub leading: RowLeading,
    /// Bold first line.
    pub title: String,
    /// Second line (may be empty).
    pub body: String,
    /// The tab a click opens.
    pub tab: Option<Tab>,
    /// The app it came from, as a small caption (mirrored notifications).
    pub app: Option<String>,
}

/// The collapsed pill's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollapsedView {
    /// Just the mascot.
    Idle,
    /// Cover thumbnail or glyph, one line of text (track or countdown), and
    /// badges (mail count).
    Pill {
        /// Cover art (straight RGBA8, side).
        thumb: Option<(std::sync::Arc<Vec<u8>>, u32)>,
        /// Icon when there is no cover.
        glyph: Option<Glyph>,
        /// Track, countdown or status; may be empty when only badges show.
        text: String,
        /// Counts on the right.
        badges: Vec<Badge>,
    },
    /// A transient message (reminder, notification fallback).
    Message(String),
}

/// Today tab contents.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TodayView {
    /// "14:05".
    pub time: String,
    /// "Sunday, 15 November".
    pub date: String,
    /// Place name, when the location feature knows it.
    pub location: Option<String>,
}

/// Media tab contents.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaView {
    /// Title, or a placeholder when nothing is playing.
    pub title: String,
    /// Artist.
    pub artist: String,
    /// Whether a player is present at all (controls are hidden otherwise).
    pub has_player: bool,
    /// Playing or paused.
    pub playing: bool,
    /// Seek bar fraction.
    pub fraction: Option<f64>,
    /// `elapsed`, `total`.
    pub times: Option<(String, String)>,
    /// Current lyric line.
    pub lyric: Option<String>,
    /// Cover art as straight RGBA8 plus side length.
    pub cover: Option<(std::sync::Arc<Vec<u8>>, u32)>,
    /// Two dominant colours of the cover (for the glow), if it has any.
    pub accent: Option<([u8; 3], [u8; 3])>,
    /// The lyric line after the current one.
    pub next_lyric: Option<String>,
}

/// What kind of thing a row shows (picks its icon).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RowKind {
    /// Text.
    #[default]
    Text,
    /// An image.
    Image,
    /// A link.
    Link,
    /// A file.
    File,
    /// A folder.
    Folder,
    /// Something needs attention.
    Warning,
    /// An ambient sound.
    Sound,
    /// Mail.
    Mail,
    /// The output device (system volume).
    Speaker,
}

/// What sits at the start of a row.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum RowLeading {
    /// Nothing (or a check, when the row has `check`).
    #[default]
    None,
    /// Initials on a gradient disc.
    Avatar {
        /// One or two letters.
        initials: String,
        /// Stable hash; picks the gradient.
        seed: u64,
    },
    /// A kind icon.
    Kind(RowKind),
    /// Initials on a solid disc in a known colour (app brand colours).
    Disc {
        /// One or two letters.
        initials: String,
        /// Disc colour.
        rgb: [u8; 3],
    },
    /// A small image (straight RGBA8, square side).
    Thumb(std::sync::Arc<Vec<u8>>, u32),
}

/// One row in a list.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    /// Main text.
    pub text: String,
    /// Secondary text on the right (time, size, ...).
    pub detail: String,
    /// Checkbox state, when the row has one.
    pub check: Option<bool>,
    /// Click on the row body.
    pub primary: Option<Action>,
    /// Small trailing "x" button.
    pub remove: Option<Action>,
    /// Dim the row (done items, missing files).
    pub dim: bool,
    /// Leading avatar, icon or thumbnail.
    pub leading: RowLeading,
    /// Second line under the main text.
    pub subtitle: Option<String>,
    /// A button shown while the row is hovered.
    pub chip: Option<(String, Action)>,
}

/// Initials for an avatar: the first letter of the first two words (emoji
/// and punctuation skipped, addresses use the part before @), or "?".
pub fn initials(name: &str) -> String {
    let name = name.split('@').next().unwrap_or(name);
    let letters: String = name
        .split(|c: char| c.is_whitespace() || c == '.' || c == '_' || c == '-')
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    if letters.is_empty() {
        "?".into()
    } else {
        letters
    }
}

/// Brand colours of common apps, for the mixer's discs (`None` = use a
/// hashed gradient).
pub fn app_colour(name: &str) -> Option<[u8; 3]> {
    let n = name.to_ascii_lowercase();
    let table: [(&str, [u8; 3]); 10] = [
        ("spotify", [0x1D, 0xB9, 0x54]),
        ("chrome", [0x42, 0x85, 0xF4]),
        ("firefox", [0xFF, 0x71, 0x39]),
        ("discord", [0x58, 0x65, 0xF2]),
        ("edge", [0x00, 0x78, 0xD7]),
        ("vlc", [0xFF, 0x88, 0x00]),
        ("teams", [0x62, 0x64, 0xA7]),
        ("zoom", [0x2D, 0x8C, 0xFF]),
        ("slack", [0x4A, 0x15, 0x4B]),
        ("steam", [0x1B, 0x28, 0x38]),
    ];
    table.iter().find(|(k, _)| n.contains(k)).map(|(_, c)| *c)
}

/// A stable 64-bit hash (FNV-1a) for picking avatar colours.
pub fn seed(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Tasks tab: Pomodoro header + to-do list.
#[derive(Debug, Clone, PartialEq)]
pub struct TasksView {
    /// Pomodoro shown at all.
    pub pomodoro: Option<PomodoroView>,
    /// To-do rows (already scrolled/cut to what fits).
    pub todos: Option<Vec<Row>>,
    /// Next reminder, e.g. "Water in 12 min".
    pub reminder: Option<String>,
}

/// Pomodoro header.
#[derive(Debug, Clone, PartialEq)]
pub struct PomodoroView {
    /// "Focus 24:13" style label.
    pub label: String,
    /// Whether it is running (button shows pause) or not (shows play).
    pub running: bool,
    /// How much of the current phase has passed, 0..=1 (the ring).
    pub fraction: f32,
    /// Time left in the phase, e.g. "16:12".
    pub remaining: String,
    /// Where we are, e.g. "Session 2 of 4 · break next".
    pub session: String,
}

/// One slider row (mixer, sounds).
#[derive(Debug, Clone, PartialEq)]
pub struct SliderRow {
    /// Label.
    pub label: String,
    /// Value 0..=1.
    pub value: f32,
    /// On/off or mute state; `true` = audible.
    pub on: bool,
    /// What the slider sets.
    pub target: SliderTarget,
    /// What clicking the label/toggle does.
    pub toggle: Action,
    /// Leading disc or icon.
    pub leading: RowLeading,
}

/// Screen-time bar.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageBar {
    /// App name.
    pub app: String,
    /// "1 h 12 min".
    pub label: String,
    /// Fraction of the top app's time.
    pub fraction: f32,
}

/// The body of the expanded panel.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// Time, date and location.
    Today(TodayView),
    /// Media.
    Media(MediaView),
    /// Tasks.
    Tasks(TasksView),
    /// A generic list with a header action (clipboard, shelf).
    List {
        /// Rows.
        rows: Vec<Row>,
        /// Shown when there are no rows.
        empty: String,
        /// Header button label + action (e.g. "Clear").
        header: Option<(String, Action)>,
    },
    /// Sliders (mixer, sounds).
    Sliders {
        /// Rows.
        rows: Vec<SliderRow>,
        /// Shown when there are no rows.
        empty: String,
    },
    /// Screen time.
    Usage {
        /// Total today.
        total: String,
        /// Top apps.
        bars: Vec<UsageBar>,
    },
    /// Big buttons (tools).
    Buttons(Vec<(String, Action)>),
    /// Nothing enabled.
    Empty(String),
}

/// Everything the renderer needs besides geometry and colors.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelView {
    /// Collapsed content.
    pub collapsed: CollapsedView,
    /// Visible tabs.
    pub tabs: Vec<Tab>,
    /// Selected tab.
    pub tab: Option<Tab>,
    /// Expanded body.
    pub body: Body,
    /// Whether the notch is pinned open (pin icon state).
    pub pinned: bool,
    /// A notification banner to show in the collapsed pill.
    pub banner: Option<BannerView>,
}

impl Default for PanelView {
    fn default() -> Self {
        Self {
            collapsed: CollapsedView::Idle,
            tabs: Vec::new(),
            tab: None,
            body: Body::Empty("Nothing enabled. Open settings to turn features on.".into()),
            pinned: false,
            banner: None,
        }
    }
}

/// A hit region produced by the renderer, in pixmap coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Left.
    pub x: f32,
    /// Top.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
    /// What it does.
    pub kind: HitKind,
}

/// What a hit region does.
#[derive(Debug, Clone, PartialEq)]
pub enum HitKind {
    /// A button.
    Button(Action),
    /// A horizontal slider spanning the hit's width.
    Slider(SliderTarget),
    /// A shelf item that can also be dragged out.
    ShelfItem(ShelfId),
}

impl Hit {
    fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
}

/// Resolve a click at `(px, py)` against hit regions. Later regions win, so
/// small buttons drawn on top of a row take precedence over the row itself.
pub fn resolve_click(hits: &[Hit], px: f32, py: f32) -> Option<Action> {
    let hit = hits.iter().rev().find(|h| h.contains(px, py))?;
    Some(match &hit.kind {
        HitKind::Button(a) => a.clone(),
        HitKind::Slider(target) => {
            let v = if hit.w > 0.0 {
                ((px - hit.x) / hit.w).clamp(0.0, 1.0)
            } else {
                0.0
            };
            Action::Slider(target.clone(), v)
        }
        HitKind::ShelfItem(id) => Action::ShelfOpen(*id),
    })
}

/// A slider grabbed by the pointer: where it lies, so a drag keeps mapping
/// the pointer to a value even outside the slider (or the window).
#[derive(Debug, Clone, PartialEq)]
pub struct SliderDrag {
    /// What the slider controls.
    pub target: SliderTarget,
    /// Left edge.
    pub x: f32,
    /// Width.
    pub w: f32,
}

impl SliderDrag {
    /// Value 0..=1 for the pointer at `px`, clamped at both ends.
    pub fn value_at(&self, px: f32) -> f32 {
        if self.w > 0.0 {
            ((px - self.x) / self.w).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// The slider under a point, for starting a drag (topmost region wins, as for
/// clicks).
pub fn slider_at(hits: &[Hit], px: f32, py: f32) -> Option<SliderDrag> {
    let hit = hits.iter().rev().find(|h| h.contains(px, py))?;
    match &hit.kind {
        HitKind::Slider(target) => Some(SliderDrag {
            target: target.clone(),
            x: hit.x,
            w: hit.w,
        }),
        _ => None,
    }
}

/// The shelf item under a point, for starting a drag.
pub fn shelf_item_at(hits: &[Hit], px: f32, py: f32) -> Option<ShelfId> {
    hits.iter()
        .rev()
        .find(|h| h.contains(px, py))
        .and_then(|h| match h.kind {
            HitKind::ShelfItem(id) => Some(id),
            _ => None,
        })
}

/// Format a duration as `m:ss` or `h:mm:ss`.
pub fn clock(d: std::time::Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Format seconds as "1 h 05 min" / "12 min" / "40 s".
pub fn human_duration(secs: u64) -> String {
    if secs >= 3600 {
        format!("{} h {:02} min", secs / 3600, (secs / 60) % 60)
    } else if secs >= 60 {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

#[cfg(test)]
mod initials_tests {
    use super::*;

    #[test]
    fn initials_handle_names_emoji_and_empty() {
        assert_eq!(initials("Ann Lee"), "AL");
        assert_eq!(initials("Jürgen"), "J");
        assert_eq!(initials("😀 Bob"), "B");
        assert_eq!(initials("bob@example.com"), "B");
        assert_eq!(initials(""), "?");
        assert_eq!(initials("   "), "?");
    }

    #[test]
    fn seeds_are_stable_and_spread() {
        assert_eq!(seed("ann@example.com"), seed("ann@example.com"));
        let distinct: std::collections::HashSet<u64> = ["a", "b", "c", "d", "e", "f"]
            .iter()
            .map(|s| seed(s) % 8)
            .collect();
        assert!(distinct.len() >= 3, "{distinct:?}");
    }
}
