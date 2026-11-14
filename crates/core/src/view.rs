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

/// One row in a list.
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PomodoroView {
    /// "Focus 24:13" style label.
    pub label: String,
    /// Whether it is running (button shows pause) or not (shows play).
    pub running: bool,
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
}

impl Default for PanelView {
    fn default() -> Self {
        Self {
            collapsed: CollapsedView::Idle,
            tabs: Vec::new(),
            tab: None,
            body: Body::Empty("Nothing enabled. Open settings to turn features on.".into()),
            pinned: false,
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
