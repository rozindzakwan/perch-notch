//! The application model: every feature's state, and the single place where
//! actions, platform events and deadlines turn into [`Effect`]s.
//!
//! The model never touches the OS. It is fed events and the current time, and
//! answers with effects for the shell to carry out (notify, send a transport
//! command, write the clipboard, ...) plus a [`PanelView`] to paint. That keeps
//! all behaviour testable with fake backends and a fake clock, which is what
//! `tests/model.rs` does.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use perch_core::clipboard::{ClipboardContent, ClipboardHistory, ContentFlags, PushOutcome};
use perch_core::config::Config;
use perch_core::lrc::Lyrics;
use perch_core::mail::{MailBoard, MailUpdate};
use perch_core::media::{MediaState, TrackToken};
use perch_core::noise::{NoiseKind, NoiseMix};
use perch_core::platform::{AudioSession, MediaEvent, PlatformEvent, Track, TransportCommand};
use perch_core::pomodoro::{Phase, Pomodoro, PomodoroEvent, TimerState};
use perch_core::reminders::{ReminderEvent, ReminderKind, Reminders};
use perch_core::shelf::Shelf;
use perch_core::todo::TodoList;
use perch_core::usage::{DayId, UsageTracker, UtcOffset};
use perch_core::view::{
    clock, human_duration, Action, Badge, BadgeKind, Body, CollapsedView, Glyph, MediaView,
    PanelView, PomodoroView, Row, RowKind, RowLeading, SliderRow, SliderTarget, Tab, TasksView,
    UsageBar,
};

use crate::features::{ActiveFeatures, Feature};

/// How long an in-pill message stays up.
const MESSAGE_TTL: Duration = Duration::from_secs(6);

/// How often the place is looked up while the location feature is on.
pub const LOCATION_EVERY: Duration = Duration::from_secs(3600);

/// How soon a failed location lookup is tried again.
pub const LOCATION_RETRY: Duration = Duration::from_secs(600);

/// The current time, as both a monotonic instant (for durations) and unix
/// seconds (for wall-clock things like to-do due times and screen-time days).
#[derive(Debug, Clone, Copy)]
pub struct Now {
    /// Monotonic.
    pub instant: Instant,
    /// Wall clock, unix seconds.
    pub unix: u64,
}

impl Now {
    /// The real current time.
    pub fn real() -> Self {
        Self {
            instant: Instant::now(),
            unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }

    /// `self` moved forward by `d`.
    pub fn plus(self, d: Duration) -> Self {
        Self {
            instant: self.instant + d,
            unix: self.unix.saturating_add(d.as_secs()),
        }
    }
}

/// A lyrics lookup the shell should perform off the UI thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricsRequest {
    /// Opaque key the result must be returned with.
    pub key: String,
    /// Track title.
    pub title: String,
    /// Artist.
    pub artist: String,
    /// Album.
    pub album: String,
    /// Duration in seconds, when known.
    pub duration_secs: Option<u64>,
}

/// Something the shell must do on the model's behalf.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Repaint the notch.
    Redraw,
    /// Open the panel (a banner was clicked).
    OpenPanel,
    /// Bring up the app a mirrored notification came from (its banner was
    /// clicked); the id was checked with `launchable_id`.
    LaunchApp(String),
    /// Show a desktop notification.
    Notify {
        /// Title.
        summary: String,
        /// Body.
        body: String,
    },
    /// Send a transport command to the media backend.
    Transport(TransportCommand),
    /// Look lyrics up (network, opt-in).
    FetchLyrics(LyricsRequest),
    /// Find where the user is and name the place (location feature, network).
    Locate,
    /// Write content to the clipboard.
    ClipboardWrite(ClipboardContent),
    /// Put files on the clipboard.
    ClipboardWriteFiles(Vec<PathBuf>),
    /// Open a file/folder with the default handler.
    Open(PathBuf),
    /// Set an audio session's volume.
    SetVolume {
        /// Session id.
        id: String,
        /// 0..=1.
        volume: f32,
    },
    /// Mute/unmute an audio session.
    SetMute {
        /// Session id.
        id: String,
        /// Muted.
        muted: bool,
    },
    /// Apply a new white-noise mix (opens/closes the audio device as needed).
    Noise(NoiseMix),
    /// Persist the to-do list.
    SaveTodos,
    /// Persist screen-time totals.
    SaveUsage,
    /// Open the settings window.
    OpenSettings,
    /// Open the add-to-do dialog.
    OpenTodoDialog,
    /// Start a screenshot.
    Screenshot,
    /// Toggle the camera mirror.
    CameraToggle,
    /// Start/stop dictation.
    DictateToggle,
    /// The push-to-talk key was released.
    DictateRelease,
    /// Toggle the notch open/closed.
    ToggleNotch,
    /// Pin/unpin the notch open.
    PinToggle,
    /// Re-read the config.
    ReloadConfig,
    /// Exit.
    Quit,
    /// Measure idle time now and report it via [`Model::idle_measured`].
    QueryIdle,
}

/// All feature state.
#[derive(Debug)]
pub struct Model {
    config: Config,
    features: ActiveFeatures,
    tab: Option<Tab>,
    scroll: usize,
    /// The mail account tab picked (`None` = all accounts).
    mail_filter: Option<String>,

    media: MediaState,
    media_reported_at: Instant,
    /// Seek-bar position while it is being dragged (0..=1); the seek goes
    /// out once, on release.
    seek_preview: Option<f32>,
    /// The lyric line the collapsed pill shows (kept by `tick`, which runs at
    /// each line change while lyrics are on and a track plays).
    pill_lyric: Option<String>,
    /// The clock the idle pill shows ("14:05  Sun 15 Nov"), kept by `tick`.
    pill_clock: String,
    /// Local offset from UTC in seconds (the shell keeps it current).
    utc_offset: i32,
    /// Place name for the Today tab, from the location feature.
    location: Option<String>,
    /// The last lookup failed (and nothing was found before).
    location_failed: bool,
    /// When to look the place up next (only while the feature is on).
    location_due: Option<Instant>,
    cover: Option<(Arc<Vec<u8>>, u32)>,
    accent: Option<([u8; 3], [u8; 3])>,
    lyrics: Option<(String, Lyrics)>,

    todos: TodoList,
    pomodoro: Pomodoro,
    reminders: Reminders,
    clipboard: ClipboardHistory,
    shelf: Shelf,
    sessions: Vec<AudioSession>,
    noise: NoiseMix,
    usage: UsageTracker,
    mail: MailBoard,
    /// Small thumbnails of clipboard images, made once per entry.
    clip_thumbs: std::collections::HashMap<u64, (Arc<Vec<u8>>, u32)>,

    message: Option<(String, Instant)>,
    /// In-notch notification banners.
    banners: crate::banner::BannerQueue,
    /// Latest banner raised while hidden by a fullscreen app.
    deferred_banner: Option<perch_core::view::BannerView>,
    /// The panel is open (banners are not shown then).
    panel_expanded: bool,
    /// The notch is visible (not hidden by a fullscreen app).
    visible: bool,
    last_tick: Instant,
    /// A stretch reminder came due and is waiting for an idle measurement.
    pending_stretch: bool,
}

impl Model {
    /// Build a model from a config at `now`.
    pub fn new(config: Config, now: Now) -> Self {
        let features = ActiveFeatures::resolve(&config);
        let locate_now = features.is_active(Feature::Location);
        let mut m = Self {
            pomodoro: Pomodoro::new(config.pomodoro.clone()),
            reminders: Reminders::new(config.reminders.clone()),
            clipboard: ClipboardHistory::new(config.clipboard.clone()),
            usage: UsageTracker::new(local_offset()),
            config,
            features,
            tab: None,
            scroll: 0,
            mail_filter: None,
            media: MediaState::new(),
            media_reported_at: now.instant,
            seek_preview: None,
            pill_lyric: None,
            pill_clock: String::new(),
            utc_offset: 0,
            location: None,
            location_failed: false,
            // Looked up right away when on.
            location_due: locate_now.then_some(now.instant),
            cover: None,
            accent: None,
            lyrics: None,
            todos: TodoList::new(),
            shelf: Shelf::default(),
            sessions: Vec::new(),
            noise: NoiseMix::default(),
            mail: MailBoard::default(),
            clip_thumbs: std::collections::HashMap::new(),
            message: None,
            banners: Default::default(),
            deferred_banner: None,
            panel_expanded: false,
            visible: true,
            last_tick: now.instant,
            pending_stretch: false,
        };
        m.sync_mail_accounts();
        m.tab = m.tabs().first().copied();
        m
    }

    /// Replace the to-do list (loaded from disk at startup).
    pub fn set_todos(&mut self, todos: TodoList) {
        self.todos = todos;
    }

    /// The to-do list, for persisting.
    pub fn todos(&self) -> &TodoList {
        &self.todos
    }

    /// The live config.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Live features.
    pub fn features(&self) -> &ActiveFeatures {
        &self.features
    }

    /// Whether a feature is live.
    pub fn is_active(&self, f: Feature) -> bool {
        self.features.is_active(f)
    }

    /// Current media state.
    pub fn media(&self) -> &MediaState {
        &self.media
    }

    /// Audio sessions the mixer last reported.
    pub fn sessions(&self) -> &[AudioSession] {
        &self.sessions
    }

    /// Whether synced lyrics are loaded for the current track.
    pub fn has_lyrics(&self) -> bool {
        self.lyrics.is_some()
    }

    /// Track the configured mail accounts (or none when the feature is off).
    fn sync_mail_accounts(&mut self) {
        if self.is_active(Feature::Mail) {
            let names: Vec<String> = self
                .config
                .mail
                .accounts
                .iter()
                .map(|a| a.name.clone())
                .collect();
            self.mail.set_accounts(&names);
        } else {
            self.mail.clear();
        }
    }

    /// Mail state, for the status file and tests.
    pub fn mail(&self) -> &MailBoard {
        &self.mail
    }

    /// The IMAP client reported something.
    pub fn mail_update(&mut self, update: MailUpdate) -> Vec<Effect> {
        if !self.is_active(Feature::Mail) {
            return Vec::new();
        }
        let fresh = self.mail.apply(update);
        let mut fx = vec![Effect::Redraw];
        if self.config.mail.notify && !fresh.is_empty() {
            let (summary, body) = match fresh.as_slice() {
                [(_, m)] => (
                    format!("New mail from {}", m.from),
                    if self.config.mail.notify_subject {
                        m.subject.clone()
                    } else {
                        String::new()
                    },
                ),
                many => (
                    format!("{} new messages", many.len()),
                    many.iter()
                        .map(|(_, m)| m.from.as_str())
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            };
            let leading = match fresh.as_slice() {
                [(_, m)] => RowLeading::Avatar {
                    initials: perch_core::view::initials(&m.from),
                    seed: perch_core::view::seed(&m.from),
                },
                _ => RowLeading::Kind(RowKind::Mail),
            };
            let at = self.last_tick;
            fx.extend(self.alert_at(
                perch_core::view::BannerView {
                    leading,
                    title: summary,
                    body,
                    tab: Some(Tab::Mail),
                    app: None,
                    launch: None,
                },
                at,
            ));
        }
        fx
    }

    /// The clipboard board.
    pub fn clipboard(&self) -> &ClipboardHistory {
        &self.clipboard
    }

    /// The shelf.
    pub fn shelf(&self) -> &Shelf {
        &self.shelf
    }

    /// The noise mix.
    pub fn noise(&self) -> NoiseMix {
        self.noise
    }

    /// The screen-time tracker.
    pub fn usage(&self) -> &UsageTracker {
        &self.usage
    }

    /// Replace the screen-time tracker.
    pub fn set_usage(&mut self, usage: UsageTracker) {
        self.usage = usage;
    }

    /// Merge totals persisted by a previous run.
    pub fn restore_usage(&mut self, persisted: &perch_core::usage::UsageAggregate) {
        self.usage.restore(persisted);
    }

    /// Credit the in-flight slice to the current app (before saving on exit).
    pub fn flush_usage(&mut self, now: Now) {
        if self.is_active(Feature::ScreenTime) {
            self.usage.flush(now.unix);
        }
    }

    /// The Pomodoro.
    pub fn pomodoro(&self) -> &Pomodoro {
        &self.pomodoro
    }

    /// The reminders.
    pub fn reminders(&self) -> &Reminders {
        &self.reminders
    }

    /// Current tab.
    pub fn tab(&self) -> Option<Tab> {
        self.tab
    }

    /// Pinned open.
    pub fn pinned(&self) -> bool {
        self.config.notch.keep_pill
    }

    /// Visible tabs for the live feature set.
    pub fn tabs(&self) -> Vec<Tab> {
        let f = &self.features;
        let mut tabs = Vec::new();
        // Time, date and place: always first.
        tabs.push(Tab::Today);
        if f.is_active(Feature::Media) {
            tabs.push(Tab::Media);
        }
        if f.is_active(Feature::Todo)
            || f.is_active(Feature::Pomodoro)
            || f.is_active(Feature::Reminders)
        {
            tabs.push(Tab::Tasks);
        }
        if f.is_active(Feature::Clipboard) {
            tabs.push(Tab::Clipboard);
        }
        if f.is_active(Feature::Shelf) {
            tabs.push(Tab::Shelf);
        }
        if f.is_active(Feature::VolumeMixer) {
            tabs.push(Tab::Mixer);
        }
        if f.is_active(Feature::WhiteNoise) {
            tabs.push(Tab::Sounds);
        }
        if f.is_active(Feature::ScreenTime) {
            tabs.push(Tab::Time);
        }
        if f.is_active(Feature::Screenshot)
            || f.is_active(Feature::CameraMirror)
            || f.is_active(Feature::SpeechToText)
        {
            tabs.push(Tab::Tools);
        }
        if f.is_active(Feature::Mail) {
            tabs.push(Tab::Mail);
        }
        tabs
    }

    /// Apply a new config (live reload). Feature state survives; features that
    /// were switched off drop their state and emit the effects that stop them.
    pub fn apply_config(&mut self, config: Config) -> Vec<Effect> {
        let mut fx = vec![Effect::Redraw];
        let old = std::mem::replace(&mut self.config, config);
        let lyrics_were_on = self.features.is_active(Feature::Lyrics);
        self.features = ActiveFeatures::resolve(&self.config);
        if old.pomodoro != self.config.pomodoro && self.pomodoro.state() == TimerState::Idle {
            self.pomodoro = Pomodoro::new(self.config.pomodoro.clone());
        }
        if old.reminders != self.config.reminders {
            self.reminders = Reminders::new(self.config.reminders.clone());
        }
        if old.clipboard != self.config.clipboard {
            let snapshot = self.clipboard.snapshot();
            self.clipboard =
                ClipboardHistory::from_snapshot(self.config.clipboard.clone(), snapshot);
        }
        if !self.is_active(Feature::Clipboard) {
            self.clipboard.clear();
        }
        if !self.is_active(Feature::Media) {
            self.media = MediaState::new();
            self.cover = None;
            self.accent = None;
            self.lyrics = None;
        }
        if !self.is_active(Feature::Lyrics) {
            self.lyrics = None;
        } else if !lyrics_were_on && self.lyrics.is_none() {
            // Turned on mid-song: look up the track that is already playing
            // instead of waiting for the next track change.
            if let (Some(token), Some(track)) = (self.media.track_token(), self.media.track()) {
                if !track.title.is_empty() {
                    fx.push(Effect::FetchLyrics(lyrics_request(&token, track)));
                }
            }
        }
        self.sync_mail_accounts();
        if !self.is_active(Feature::WhiteNoise) && self.noise.is_audible() {
            self.noise = NoiseMix::default();
            fx.push(Effect::Noise(self.noise));
        }
        if !self.is_active(Feature::Pomodoro) {
            self.pomodoro.stop();
        }
        if !self.is_active(Feature::Location) {
            // Off: forget the place at once, never look it up again.
            self.location = None;
            self.location_failed = false;
            self.location_due = None;
        } else if self.location_due.is_none() {
            // Just turned on: look it up on the next tick.
            self.location_due = Some(self.last_tick);
        }
        let tabs = self.tabs();
        if !self.tab.is_some_and(|t| tabs.contains(&t)) {
            self.tab = tabs.first().copied();
            self.scroll = 0;
        }
        fx
    }

    /// Show a short message: a banner in the notch, or (with in-notch
    /// notifications off, or while the panel is open) text in the pill.
    pub fn flash(&mut self, text: impl Into<String>, now: Now) {
        let text = text.into();
        if self.config.notifications.in_notch && !self.panel_expanded {
            let b = perch_core::view::BannerView {
                leading: RowLeading::None,
                title: text,
                body: String::new(),
                tab: None,
                app: None,
                launch: None,
            };
            if self.visible {
                self.banners.push(b, now.instant);
            } else {
                self.deferred_banner = Some(b);
            }
        } else {
            self.message = Some((text, now.instant));
        }
    }

    /// Raise an alert: a banner (in-notch notifications) and/or a system
    /// notification, per `[notifications]`.
    fn alert(&mut self, b: perch_core::view::BannerView, now: Now) -> Vec<Effect> {
        self.alert_at(b, now.instant)
    }

    fn alert_at(&mut self, b: perch_core::view::BannerView, at: Instant) -> Vec<Effect> {
        let n = self.config.notifications;
        let mut fx = vec![Effect::Redraw];
        if n.system || !n.in_notch {
            fx.push(Effect::Notify {
                summary: b.title.clone(),
                body: b.body.clone(),
            });
        }
        if !n.in_notch {
            self.message = Some((b.title, at));
        } else if self.panel_expanded {
            // The open panel shows the change in place.
        } else if !self.visible {
            self.deferred_banner = Some(b);
        } else {
            self.banners.push(b, at);
        }
        fx
    }

    /// The shell reports whether the panel is open and whether the notch is
    /// visible. Opening clears banners; becoming visible shows the latest
    /// one raised meanwhile.
    pub fn set_panel_state(&mut self, expanded: bool, visible: bool, now: Now) {
        if expanded && !self.panel_expanded {
            self.banners.clear();
        }
        self.panel_expanded = expanded;
        self.visible = visible;
        if visible && !expanded {
            if let Some(b) = self.deferred_banner.take() {
                self.banners.push(b, now.instant);
            }
        }
    }

    /// The pointer is (or stopped being) over the banner: pause it.
    pub fn banner_hover(&mut self, inside: bool, now: Now) {
        self.banners.hover(inside, now.instant);
    }

    /// A banner is showing.
    pub fn has_banner(&self) -> bool {
        !self.panel_expanded && self.banners.current().is_some()
    }

    /// Advance all timers to `now` and return what came due.
    pub fn tick(&mut self, now: Now) -> Vec<Effect> {
        let dt = now.instant.saturating_duration_since(self.last_tick);
        self.last_tick = now.instant;
        let mut fx = Vec::new();

        if let Some((_, at)) = &self.message {
            if now.instant.saturating_duration_since(*at) >= MESSAGE_TTL {
                self.message = None;
                fx.push(Effect::Redraw);
            }
        }
        let clock = {
            let lt = perch_core::clock::LocalTime::at(now.unix as i64, self.utc_offset);
            format!("{}  {}", lt.time(), lt.date_short())
        };
        if clock != self.pill_clock {
            self.pill_clock = clock;
            fx.push(Effect::Redraw);
        }
        let lyric = self.current_lyric(now);
        if lyric != self.pill_lyric {
            self.pill_lyric = lyric;
            fx.push(Effect::Redraw);
        }
        if self.banners.tick(now.instant) {
            fx.push(Effect::Redraw);
        }
        if self
            .location_due
            .is_some_and(|due| due <= now.instant && self.is_active(Feature::Location))
        {
            // The answer re-arms the timer; until then, never ask twice.
            self.location_due = Some(now.instant + LOCATION_EVERY);
            fx.push(Effect::Locate);
        }

        if self.is_active(Feature::Pomodoro) && self.pomodoro.state() == TimerState::Running {
            for ev in self.pomodoro.tick(dt) {
                if let PomodoroEvent::PhaseCompleted(phase) = ev {
                    let (summary, body) = match phase {
                        Phase::Work => ("Focus session done", "Time for a break."),
                        Phase::ShortBreak | Phase::LongBreak => ("Break over", "Back to focus."),
                    };
                    fx.extend(self.alert(
                        perch_core::view::BannerView {
                            leading: RowLeading::None,
                            title: summary.into(),
                            body: body.into(),
                            tab: Some(Tab::Tasks),
                            app: None,
                            launch: None,
                        },
                        now,
                    ));
                }
            }
            fx.push(Effect::Redraw);
        }

        if self.is_active(Feature::Reminders) {
            for ReminderEvent::Due(kind) in self.reminders.tick(dt) {
                if kind == ReminderKind::Stretch && !self.reminders.is_idle() {
                    // The stretch timer counts *active* time, but we only learn
                    // about idleness when asked. Measure now; the shell answers
                    // through `idle_measured`, which fires or postpones it.
                    fx.push(Effect::QueryIdle);
                    self.pending_stretch = true;
                    continue;
                }
                fx.extend(self.reminder_due(kind, now));
            }
        }

        if self.is_active(Feature::Todo) {
            let due = self.todos.take_due(now.unix);
            for id in due {
                if let Some(item) = self.todos.get(id) {
                    let text = item.text.clone();
                    fx.extend(self.alert(
                        perch_core::view::BannerView {
                            leading: RowLeading::None,
                            title: "To-do due".into(),
                            body: text,
                            tab: Some(Tab::Tasks),
                            app: None,
                            launch: None,
                        },
                        now,
                    ));
                }
                self.todos.mark_notified(id);
                fx.push(Effect::SaveTodos);
            }
        }

        if !fx.is_empty() && !fx.contains(&Effect::Redraw) {
            fx.push(Effect::Redraw);
        }
        fx
    }

    fn reminder_due(&mut self, kind: ReminderKind, now: Now) -> Vec<Effect> {
        let (summary, body) = match kind {
            ReminderKind::Water => ("Drink some water", "Stay hydrated."),
            ReminderKind::Stretch => ("Time to stretch", "You've been at it a while; stand up."),
        };
        self.alert(
            perch_core::view::BannerView {
                leading: RowLeading::None,
                title: summary.into(),
                body: body.into(),
                tab: Some(Tab::Tasks),
                app: None,
                launch: None,
            },
            now,
        )
    }

    /// The shell measured idle time after a [`Effect::QueryIdle`].
    ///
    /// If the user has been idle for at least the threshold, the stretch
    /// reminder has not really been earned: push it back by the idle time.
    /// Otherwise it fires now.
    pub fn idle_measured(&mut self, idle_for: Duration, now: Now) -> Vec<Effect> {
        if !std::mem::take(&mut self.pending_stretch) {
            return Vec::new();
        }
        let threshold =
            Duration::from_secs(u64::from(self.config.reminders.idle_threshold_seconds));
        if idle_for >= threshold {
            // The timer was re-armed when it came due; the user still owes
            // exactly the time they were away.
            self.reminders.postpone_to(ReminderKind::Stretch, idle_for);
            Vec::new()
        } else {
            self.reminders.reset(ReminderKind::Stretch);
            self.reminder_due(ReminderKind::Stretch, now)
        }
    }

    /// The soonest moment the model needs to run again, relative to `now`.
    /// `None` means it can sleep until the next external event.
    pub fn next_deadline(&self, now: Now, expanded: bool) -> Option<Duration> {
        let mut ds: Vec<Option<Duration>> = Vec::new();
        // The clock in the pill and on the Today tab: the next whole minute.
        ds.push(Some(Duration::from_secs(
            perch_core::clock::until_next_minute(now.unix as i64),
        )));
        ds.push(self.banners.next_deadline(now.instant));
        if let Some(due) = self.location_due {
            ds.push(Some(due.saturating_duration_since(now.instant)));
        }
        if let Some((_, at)) = &self.message {
            ds.push(Some(
                MESSAGE_TTL.saturating_sub(now.instant.saturating_duration_since(*at)),
            ));
        }
        if self.pending_stretch {
            ds.push(None);
        }
        if self.is_active(Feature::Pomodoro) && self.pomodoro.state() == TimerState::Running {
            // The collapsed pill shows the countdown, so wake on the next
            // whole second (but never later than the phase end).
            let rem = self.pomodoro.remaining();
            let frac = Duration::from_nanos(u64::from(rem.subsec_nanos()));
            let next = if frac.is_zero() {
                Duration::from_secs(1)
            } else {
                frac
            };
            ds.push(Some(next.min(rem)));
        }
        if self.is_active(Feature::Reminders) {
            ds.push(self.reminders.next_deadline());
        }
        if self.is_active(Feature::Todo) {
            ds.push(self.todos.next_deadline(now.unix));
        }
        if expanded && self.tab == Some(Tab::Media) && self.media.is_playing() {
            // Seek bar and clock: once a second, only while visible.
            let mut d = Some(Duration::from_secs(1));
            if let Some(next) = self.next_lyric_change(now) {
                d = Some(next.min(Duration::from_secs(1)));
            }
            ds.push(d);
        } else if self.is_active(Feature::Media) && self.media.is_playing() && self.lyrics.is_some()
        {
            // Expanded or not: the collapsed pill shows the current line too.
            ds.push(self.next_lyric_change(now));
        }
        crate::features::soonest(&ds)
    }

    fn position_now(&self, now: Now) -> Option<Duration> {
        self.media.position_at(
            now.instant
                .saturating_duration_since(self.media_reported_at),
        )
    }

    /// The lyric line to show right now: lyrics on, a track playing, and a
    /// non-empty line at the playhead.
    fn current_lyric(&self, now: Now) -> Option<String> {
        if !self.is_active(Feature::Media) || !self.media.is_playing() {
            return None;
        }
        let (_, lyrics) = self.lyrics.as_ref()?;
        let line = lyrics.line_at(self.position_now(now)?)?;
        let text = line.text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }

    /// The local offset from UTC (seconds), for the clock and dates.
    pub fn set_utc_offset(&mut self, offset: i32) {
        self.utc_offset = offset;
    }

    /// The place name the Today tab shows (from the location feature).
    pub fn set_location(&mut self, place: Option<String>) -> Vec<Effect> {
        if self.location == place {
            return Vec::new();
        }
        self.location = place;
        vec![Effect::Redraw]
    }

    /// A location lookup finished. A failure keeps the last place found (a
    /// stale place beats none) and tries again sooner.
    pub fn location_found(&mut self, result: Result<String, String>, now: Now) -> Vec<Effect> {
        if !self.is_active(Feature::Location) {
            return Vec::new();
        }
        match result {
            Ok(place) => {
                self.location_failed = false;
                self.location_due = Some(now.instant + LOCATION_EVERY);
                self.set_location(Some(place));
            }
            Err(e) => {
                log::info!("location: {e}");
                self.location_failed = true;
                self.location_due = Some(now.instant + LOCATION_RETRY);
            }
        }
        vec![Effect::Redraw]
    }

    /// The collapsed pill is showing a lyric line (it then keeps a steady,
    /// wide size instead of resizing on every line).
    pub fn pill_shows_lyric(&self) -> bool {
        self.pill_lyric.is_some() && self.message.is_none()
    }

    fn next_lyric_change(&self, now: Now) -> Option<Duration> {
        let (_, lyrics) = self.lyrics.as_ref()?;
        let pos = self.position_now(now)?;
        let next = lyrics.next_time_after(pos)?;
        Some(next.saturating_sub(pos).max(Duration::from_millis(20)))
    }

    /// Handle a platform event. Events needing OS reads (clipboard, audio
    /// sessions, file stat) are completed by the shell, which then calls the
    /// matching `*_captured` method.
    pub fn handle_event(&mut self, event: &PlatformEvent, now: Now) -> Vec<Effect> {
        match event {
            PlatformEvent::Media(ev) if self.is_active(Feature::Media) => self.media_event(ev, now),
            PlatformEvent::ForeignNotification(n)
                if self.is_active(Feature::NotificationMirror)
                    && self.config.notifications.in_notch =>
            {
                // Already a system notification: only the in-notch banner,
                // never echoed back as a toast.
                // The app's own icon when the OS gave one, else its colour
                // disc or a lettered avatar.
                let icon = n.icon.as_deref().and_then(|b| decode_thumb(b, 64));
                let leading = match (icon, perch_core::view::app_colour(&n.app)) {
                    (Some((px, side)), _) => RowLeading::Thumb(px, side),
                    (None, Some(rgb)) => RowLeading::Disc {
                        initials: perch_core::view::initials(&n.app),
                        rgb,
                    },
                    (None, None) => RowLeading::Avatar {
                        initials: perch_core::view::initials(&n.app),
                        seed: perch_core::view::seed(&n.app),
                    },
                };
                let b = perch_core::view::BannerView {
                    leading,
                    title: n.title.clone(),
                    body: n.body.clone(),
                    tab: None,
                    app: Some(n.app.clone()),
                    // Clicking opens the app; never an id that could be a
                    // path or an option.
                    launch: n
                        .launch
                        .clone()
                        .filter(|id| perch_core::platform::launchable_id(id)),
                };
                if self.panel_expanded {
                    Vec::new()
                } else if !self.visible {
                    self.deferred_banner = Some(b);
                    Vec::new()
                } else {
                    self.banners.push(b, now.instant);
                    vec![Effect::Redraw]
                }
            }
            PlatformEvent::ActiveWindowChanged { app, .. }
                if self.is_active(Feature::ScreenTime) =>
            {
                self.usage.active_window_changed(app, now.unix);
                let mut fx = vec![Effect::SaveUsage];
                if self.tab == Some(Tab::Time) {
                    fx.push(Effect::Redraw);
                }
                fx
            }
            PlatformEvent::IdleChanged(idle) | PlatformEvent::SessionLocked(idle) => {
                self.reminders.set_idle(*idle);
                if self.is_active(Feature::ScreenTime) {
                    self.usage.set_idle(*idle, now.unix);
                    return vec![Effect::SaveUsage];
                }
                Vec::new()
            }
            PlatformEvent::Command(verb) => self.command(verb, now),
            _ => Vec::new(),
        }
    }

    fn media_event(&mut self, ev: &MediaEvent, now: Now) -> Vec<Effect> {
        let before = self.media.track_token();
        // A play/pause change does not carry a position, so freeze the
        // extrapolated position into the state before the anchor moves;
        // otherwise pausing would snap the seek bar back to the last report.
        if matches!(ev, MediaEvent::PlaybackChanged(_)) {
            if let Some(pos) = self.position_now(now) {
                self.media.apply(&MediaEvent::PositionChanged(pos));
            }
        }
        if !matches!(ev, MediaEvent::Gone) {
            self.media_reported_at = now.instant;
        }
        // Decoding cover art costs milliseconds; only redo it when the bytes
        // actually changed (players re-report the same track often).
        let new_cover = match ev {
            MediaEvent::TrackChanged(track) => (self.media.track().map(|t| &t.cover)
                != Some(&track.cover))
            .then(|| track.cover.clone()),
            _ => None,
        };
        let changed = self.media.apply(ev);
        let mut fx = Vec::new();
        if let Some(cover) = new_cover {
            self.cover = cover.as_deref().and_then(decode_cover);
            self.accent = self
                .cover
                .as_ref()
                .and_then(|(px, side)| perch_core::palette::dominant_pair(px, *side));
        }
        if matches!(ev, MediaEvent::Gone) {
            self.cover = None;
            self.accent = None;
            self.lyrics = None;
        }
        let after = self.media.track_token();
        if after != before {
            self.lyrics = None;
            if let (Some(token), Some(track)) = (after, self.media.track()) {
                if self.is_active(Feature::Lyrics) && !track.title.is_empty() {
                    fx.push(Effect::FetchLyrics(lyrics_request(&token, track)));
                }
            }
        }
        if changed {
            fx.push(Effect::Redraw);
        }
        fx
    }

    /// Lyrics arrived for the request with `key` (`None` = not found / error).
    pub fn lyrics_loaded(&mut self, key: &str, lrc: Option<&str>) -> Vec<Effect> {
        let current = self.media.track_token().map(|t| token_key(&t));
        if current.as_deref() != Some(key) {
            return Vec::new(); // stale: the track changed meanwhile
        }
        match lrc.map(Lyrics::parse).filter(|l| !l.is_empty()) {
            Some(l) => {
                self.lyrics = Some((key.to_string(), l));
                vec![Effect::Redraw]
            }
            None => Vec::new(),
        }
    }

    /// The shell read the clipboard after a [`PlatformEvent::ClipboardChanged`].
    pub fn clipboard_captured(
        &mut self,
        content: ClipboardContent,
        flags: &ContentFlags,
        now: Now,
    ) -> Vec<Effect> {
        if !self.is_active(Feature::Clipboard) {
            return Vec::new();
        }
        let thumb = match &content {
            ClipboardContent::Image { bytes, .. } => decode_thumb(bytes, 56),
            _ => None,
        };
        let outcome = self.clipboard.push_with_flags(content, now.unix, flags);
        if let (PushOutcome::Stored { id, .. }, Some(t)) = (&outcome, thumb) {
            self.clip_thumbs.insert(id.0, t);
        }
        let live: std::collections::HashSet<u64> =
            self.clipboard.iter().map(|e| e.id().0).collect();
        self.clip_thumbs.retain(|id, _| live.contains(id));
        match outcome {
            PushOutcome::Skipped(reason) => {
                log::debug!("clipboard entry skipped: {reason:?}");
                Vec::new()
            }
            _ if self.tab == Some(Tab::Clipboard) => vec![Effect::Redraw],
            _ => Vec::new(),
        }
    }

    /// A file was dropped on the notch (`is_dir` from the shell's stat).
    pub fn file_dropped(&mut self, path: PathBuf, is_dir: bool, now: Now) -> Vec<Effect> {
        if !self.is_active(Feature::Shelf) {
            self.flash("Turn on the file shelf to park files", now);
            return vec![Effect::Redraw];
        }
        self.shelf.add(path, is_dir);
        self.tab = Some(Tab::Shelf);
        self.scroll = 0;
        vec![Effect::Redraw]
    }

    /// The shell re-read the audio sessions.
    pub fn sessions_captured(&mut self, mut sessions: Vec<AudioSession>) -> Vec<Effect> {
        // The device's main volume first, then apps by name.
        sessions.sort_by_key(|s| {
            (
                s.id != perch_core::platform::SYSTEM_VOLUME_ID,
                s.name.to_lowercase(),
            )
        });
        if sessions == self.sessions {
            return Vec::new();
        }
        self.sessions = sessions;
        if self.tab == Some(Tab::Mixer) {
            vec![Effect::Redraw]
        } else {
            Vec::new()
        }
    }

    /// Prune shelf entries whose files vanished.
    pub fn prune_shelf(&mut self, exists: impl Fn(&std::path::Path) -> bool) -> Vec<Effect> {
        if self.shelf.prune(exists) > 0 {
            vec![Effect::Redraw]
        } else {
            Vec::new()
        }
    }

    /// Add a to-do (from the dialog or the CLI).
    pub fn add_todo(&mut self, text: &str, due: Option<u64>, now: Now) -> Vec<Effect> {
        let text = text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        self.todos.add(text, due, now.unix);
        vec![Effect::SaveTodos, Effect::Redraw]
    }

    /// Map an IPC / tray / hotkey verb.
    pub fn command(&mut self, verb: &str, now: Now) -> Vec<Effect> {
        let verb = verb.trim();
        let (head, rest) = verb.split_once(' ').unwrap_or((verb, ""));
        match head {
            "toggle-notch" | "toggle" => vec![Effect::ToggleNotch],
            "pin" => vec![Effect::PinToggle],
            "settings" => vec![Effect::OpenSettings],
            "reload" => vec![Effect::ReloadConfig],
            "quit" | "exit" => vec![Effect::Quit],
            "shot" | "screenshot" => self.action(Action::Screenshot, now),
            "dictate" => self.action(Action::DictateToggle, now),
            "dictate-release" if self.is_active(Feature::SpeechToText) => {
                vec![Effect::DictateRelease]
            }
            "dictate-release" => Vec::new(),
            "camera" => self.action(Action::CameraToggle, now),
            "pomodoro" => self.action(Action::PomodoroToggle, now),
            "play-pause" => self.action(Action::Transport(TransportCommand::PlayPause), now),
            "next" => self.action(Action::Transport(TransportCommand::Next), now),
            "previous" | "prev" => self.action(Action::Transport(TransportCommand::Previous), now),
            "todo" if !rest.is_empty() => {
                // An optional due time first: `+10m`, `+30s`, `+2h` or `14:30`.
                let (first, text) = rest.split_once(' ').unwrap_or((rest, ""));
                let looks_like_due = first.starts_with('+') || first.contains(':');
                let offset = if first.contains(':') {
                    crate::sys::utc_offset_seconds()
                } else {
                    0
                };
                match crate::forms::parse_due(first, now.unix, offset) {
                    Ok(Some(due)) if looks_like_due && !text.trim().is_empty() => {
                        self.add_todo(text.trim(), Some(due), now)
                    }
                    _ => self.add_todo(rest, None, now),
                }
            }
            "tab" => match parse_tab(rest) {
                Some(t) => self.action(Action::SelectTab(t), now),
                None => Vec::new(),
            },
            other => {
                log::warn!("ignoring unknown command {other:?}");
                Vec::new()
            }
        }
    }

    /// A slider is being dragged to `v`. Volume and noise levels apply live;
    /// the seek bar only previews until [`Model::end_drag`].
    pub fn drag_slider(&mut self, target: SliderTarget, v: f32, now: Now) -> Vec<Effect> {
        match target {
            SliderTarget::Seek => {
                self.seek_preview = Some(clamp01(v));
                vec![Effect::Redraw]
            }
            other => self.action(Action::Slider(other, v), now),
        }
    }

    /// The drag ended at `v`: commit it (one seek for the seek bar).
    pub fn end_drag(&mut self, target: SliderTarget, v: f32, now: Now) -> Vec<Effect> {
        self.seek_preview = None;
        let mut fx = self.action(Action::Slider(target, v), now);
        fx.push(Effect::Redraw);
        fx
    }

    /// Carry out a user action.
    pub fn action(&mut self, action: Action, now: Now) -> Vec<Effect> {
        let redraw = vec![Effect::Redraw];
        match action {
            Action::BannerOpen => {
                if let Some(b) = self.banners.dismiss(now.instant) {
                    if let Some(id) = b.launch {
                        return vec![Effect::LaunchApp(id), Effect::Redraw];
                    }
                    if let Some(t) = b.tab.filter(|t| self.tabs().contains(t)) {
                        self.tab = Some(t);
                        self.scroll = 0;
                    }
                }
                vec![Effect::OpenPanel, Effect::Redraw]
            }
            Action::SelectTab(t) => {
                if self.tabs().contains(&t) {
                    self.tab = Some(t);
                    self.scroll = 0;
                }
                redraw
            }
            Action::MailFilter(account) => {
                self.mail_filter = account;
                self.scroll = 0;
                redraw
            }
            Action::Scroll(d) => {
                let max = self.list_len().saturating_sub(1);
                let s = self.scroll as i64 + i64::from(d);
                self.scroll = s.clamp(0, max as i64) as usize;
                redraw
            }
            Action::Transport(cmd) => {
                if !self.is_active(Feature::Media) || self.media.is_idle() {
                    return Vec::new();
                }
                vec![Effect::Transport(cmd)]
            }
            Action::Slider(SliderTarget::Seek, v) => match self.media.seek_target(f64::from(v)) {
                Some(cmd) => vec![Effect::Transport(cmd)],
                None => Vec::new(),
            },
            Action::Slider(SliderTarget::Volume(id), v) => {
                let v = clamp01(v);
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
                    s.volume = v;
                }
                vec![Effect::SetVolume { id, volume: v }, Effect::Redraw]
            }
            Action::Slider(SliderTarget::Noise(kind), v) => {
                self.noise.set_level(kind, v);
                if !self.noise.is_on(kind) {
                    self.noise.set_on(kind, true);
                }
                vec![Effect::Noise(self.noise), Effect::Redraw]
            }
            Action::MuteToggle(id) => {
                let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) else {
                    return Vec::new();
                };
                s.muted = !s.muted;
                vec![Effect::SetMute { id, muted: s.muted }, Effect::Redraw]
            }
            Action::NoiseToggle(kind) => {
                self.noise.toggle(kind);
                vec![Effect::Noise(self.noise), Effect::Redraw]
            }
            Action::TodoAdd => vec![Effect::OpenTodoDialog],
            Action::TodoToggle(id) => {
                if self.todos.toggle(id) {
                    vec![Effect::SaveTodos, Effect::Redraw]
                } else {
                    Vec::new()
                }
            }
            Action::TodoRemove(id) => {
                if self.todos.remove(id) {
                    vec![Effect::SaveTodos, Effect::Redraw]
                } else {
                    Vec::new()
                }
            }
            Action::PomodoroToggle => {
                if !self.is_active(Feature::Pomodoro) {
                    return Vec::new();
                }
                match self.pomodoro.state() {
                    TimerState::Idle => {
                        self.pomodoro.start();
                    }
                    TimerState::Running => self.pomodoro.pause(),
                    TimerState::Paused => self.pomodoro.resume(),
                }
                self.last_tick = now.instant;
                redraw
            }
            Action::PomodoroSkip => {
                self.pomodoro.skip();
                redraw
            }
            Action::PomodoroReset => {
                self.pomodoro.reset();
                redraw
            }
            Action::ClipCopy(id) => {
                let content = self
                    .clipboard
                    .iter()
                    .find(|e| e.id() == id)
                    .map(|e| e.content().clone());
                match content {
                    Some(content) => {
                        self.flash("Copied", now);
                        vec![Effect::ClipboardWrite(content), Effect::Redraw]
                    }
                    None => Vec::new(),
                }
            }
            Action::ClipRemove(id) => {
                self.clipboard.remove(id);
                redraw
            }
            Action::ClipClear => {
                self.clipboard.clear();
                self.scroll = 0;
                redraw
            }
            Action::ShelfOpen(id) => match self.shelf.get(id) {
                Some(i) => vec![Effect::Open(i.path.clone())],
                None => Vec::new(),
            },
            Action::ShelfCopy(id) => match self.shelf.get(id).map(|i| i.path.clone()) {
                Some(path) => {
                    self.flash("File copied; paste it anywhere", now);
                    vec![Effect::ClipboardWriteFiles(vec![path]), Effect::Redraw]
                }
                None => Vec::new(),
            },
            Action::ShelfRemove(id) => {
                self.shelf.remove(id);
                redraw
            }
            Action::ShelfClear => {
                self.shelf.clear();
                self.scroll = 0;
                redraw
            }
            Action::Screenshot if self.is_active(Feature::Screenshot) => vec![Effect::Screenshot],
            Action::CameraToggle if self.is_active(Feature::CameraMirror) => {
                vec![Effect::CameraToggle]
            }
            Action::DictateToggle if self.is_active(Feature::SpeechToText) => {
                vec![Effect::DictateToggle]
            }
            Action::Screenshot | Action::CameraToggle | Action::DictateToggle => {
                self.flash("That feature is off; enable it in settings", now);
                redraw
            }
            Action::OpenSettings => vec![Effect::OpenSettings],
            // The lock: the shell flips `[notch] keep_pill`, saves and applies it.
            Action::PinToggle => vec![Effect::PinToggle],
        }
    }

    fn list_len(&self) -> usize {
        match self.tab {
            Some(Tab::Clipboard) => self.clipboard.len(),
            Some(Tab::Shelf) => self.shelf.len(),
            Some(Tab::Tasks) => self.todos.len(),
            Some(Tab::Mixer) => self.sessions.len(),
            Some(Tab::Mail) => self.mail_items().len(),
            _ => 0,
        }
    }

    /// The account the mail tab shows, if one is picked and still exists.
    fn mail_account(&self) -> Option<&str> {
        let name = self.mail_filter.as_deref()?;
        self.mail
            .accounts()
            .iter()
            .any(|a| a.name == name)
            .then_some(name)
    }

    /// Unread mail for the picked account tab (all accounts by default),
    /// newest first.
    fn mail_items(&self) -> Vec<(&str, &perch_core::mail::MailItem)> {
        let only = self.mail_account();
        let mut items = self.mail.items();
        if let Some(name) = only {
            items.retain(|(acct, _)| *acct == name);
        }
        items
    }

    /// The tabs above the mail list: "All" plus one per account. None with a
    /// single account (the tabs would say nothing new).
    fn mail_filters(&self) -> Vec<perch_core::view::ListFilter> {
        use perch_core::view::ListFilter;
        let accounts = self.mail.accounts();
        if accounts.len() < 2 {
            return Vec::new();
        }
        let only = self.mail_account();
        let mut out = vec![ListFilter {
            label: "All".into(),
            count: self.mail.total_unread(),
            selected: only.is_none(),
            action: Action::MailFilter(None),
        }];
        out.extend(accounts.iter().map(|a| ListFilter {
            label: a.name.clone(),
            count: a.unread,
            selected: only == Some(a.name.as_str()),
            action: Action::MailFilter(Some(a.name.clone())),
        }));
        out
    }

    /// What the collapsed pill shows: a message wins; otherwise the running
    /// Pomodoro or the playing track, plus badges (unread mail).
    pub fn collapsed(&self) -> CollapsedView {
        if let Some((m, _)) = &self.message {
            return CollapsedView::Message(m.clone());
        }
        let mut badges = Vec::new();
        if self.is_active(Feature::Mail) {
            let n = self.mail.total_unread();
            if n > 0 {
                badges.push(Badge {
                    kind: BadgeKind::Mail,
                    count: n,
                });
            }
        }
        let (thumb, glyph, text) =
            if self.is_active(Feature::Pomodoro) && self.pomodoro.state() != TimerState::Idle {
                (None, Some(Glyph::Timer), self.pomodoro.label())
            } else {
                match self.media.label() {
                    Some(label) if self.is_active(Feature::Media) && self.media.is_playing() => {
                        let thumb = self.cover.clone();
                        let glyph = thumb.is_none().then_some(Glyph::Music);
                        // With lyrics on, the current line replaces the title.
                        let text = self.pill_lyric.clone().unwrap_or(label);
                        (thumb, glyph, text)
                    }
                    // Nothing playing: the time and date.
                    _ => (None, Some(Glyph::Clock), self.pill_clock.clone()),
                }
            };
        if text.is_empty() && badges.is_empty() {
            return CollapsedView::Idle;
        }
        CollapsedView::Pill {
            thumb,
            glyph,
            text,
            badges,
        }
    }

    /// Logical height the expanded panel wants for the current tab, capped
    /// at `expanded_height`. Header (padding + tab bar + gap) is 56 px.
    pub fn panel_height_hint(&self) -> u32 {
        use perch_ui::tokens::space::{ROW1, ROW2};
        let rows = |n: usize, h: f32| (n.max(1) as f32 * h) as u32;
        let body: u32 = match self.tab {
            None => 60,
            Some(Tab::Today) => 124,
            Some(Tab::Media) => 168,
            Some(Tab::Tasks) => 80 + rows(self.todos.len() + 1, ROW1),
            Some(Tab::Clipboard) => 28 + rows(self.clipboard.len(), ROW2),
            Some(Tab::Shelf) => 28 + rows(self.shelf.len(), ROW2),
            Some(Tab::Mail) => 28 + rows(self.mail_items().len() + self.mail_problems(), ROW2),
            Some(Tab::Mixer) => rows(self.sessions.len(), 38.0),
            Some(Tab::Sounds) => rows(NoiseKind::ALL.len(), 38.0),
            Some(Tab::Time) => 140,
            Some(Tab::Tools) => 96,
        };
        let max = self.config.notch.expanded_height.max(120);
        (56 + body + 12).clamp(120, max)
    }

    fn mail_problems(&self) -> usize {
        self.mail
            .accounts()
            .iter()
            .filter(|a| a.status.is_problem())
            .count()
    }

    /// Build the view for painting. Settles in-flight screen time first so the
    /// Time tab is current.
    pub fn view(&mut self, now: Now) -> PanelView {
        if self.tab == Some(Tab::Time) {
            self.usage.flush(now.unix);
        }
        let tabs = self.tabs();
        let body = match self.tab {
            None => Body::Empty("Nothing enabled. Open settings to turn features on.".into()),
            Some(Tab::Today) => {
                let lt = perch_core::clock::LocalTime::at(now.unix as i64, self.utc_offset);
                Body::Today(perch_core::view::TodayView {
                    time: lt.time(),
                    date: lt.date_long(),
                    // Off: `None`, and the tab says how to turn it on.
                    location: self.is_active(Feature::Location).then(|| {
                        match (&self.location, self.location_failed) {
                            (Some(p), _) => p.clone(),
                            (None, true) => {
                                "Location unavailable: check the system's location setting".into()
                            }
                            (None, false) => "Finding where you are\u{2026}".into(),
                        }
                    }),
                })
            }
            Some(Tab::Media) => Body::Media(self.media_view(now)),
            Some(Tab::Tasks) => Body::Tasks(self.tasks_view(now)),
            Some(Tab::Clipboard) => Body::List {
                rows: self
                    .clipboard
                    .iter()
                    .skip(self.scroll)
                    .map(|e| Row {
                        text: match e.content() {
                            ClipboardContent::Text { .. } => e
                                .as_text()
                                .unwrap_or_default()
                                .replace(['\n', '\r', '\t'], " "),
                            other => format!("Image ({} KB)", other.byte_len() / 1024),
                        },
                        detail: ago(now.unix, e.captured_at()),
                        check: None,
                        primary: Some(Action::ClipCopy(e.id())),
                        remove: Some(Action::ClipRemove(e.id())),
                        dim: false,
                        leading: match e.content() {
                            ClipboardContent::Text { .. } => {
                                let t = e.as_text().unwrap_or_default();
                                let t = t.trim_start();
                                if t.starts_with("http://") || t.starts_with("https://") {
                                    RowLeading::Kind(RowKind::Link)
                                } else {
                                    RowLeading::Kind(RowKind::Text)
                                }
                            }
                            _ => match self.clip_thumbs.get(&e.id().0) {
                                Some((px, side)) => RowLeading::Thumb(px.clone(), *side),
                                None => RowLeading::Kind(RowKind::Image),
                            },
                        },
                        subtitle: None,
                        chip: Some(("Copy".into(), Action::ClipCopy(e.id()))),
                    })
                    .collect(),
                empty: "Copy something and it shows up here.".into(),
                header: (!self.clipboard.is_empty()).then(|| ("Clear".into(), Action::ClipClear)),
                filters: Vec::new(),
            },
            Some(Tab::Shelf) => Body::List {
                rows: self
                    .shelf
                    .items()
                    .iter()
                    .skip(self.scroll)
                    .map(|i| Row {
                        text: i.name.clone(),
                        detail: if i.is_dir {
                            "folder".into()
                        } else {
                            "copy".into()
                        },
                        check: None,
                        primary: Some(Action::ShelfCopy(i.id)),
                        remove: Some(Action::ShelfRemove(i.id)),
                        dim: false,
                        leading: RowLeading::Kind(if i.is_dir {
                            RowKind::Folder
                        } else {
                            RowKind::File
                        }),
                        subtitle: None,
                        chip: Some(("Copy".into(), Action::ShelfCopy(i.id))),
                    })
                    .collect(),
                empty: "Drop files here to park them.".into(),
                header: (!self.shelf.is_empty()).then(|| ("Clear".into(), Action::ShelfClear)),
                filters: Vec::new(),
            },
            Some(Tab::Mixer) => Body::Sliders {
                rows: self
                    .sessions
                    .iter()
                    .skip(self.scroll)
                    .map(|s| SliderRow {
                        label: s.name.clone(),
                        value: s.volume,
                        on: !s.muted,
                        target: SliderTarget::Volume(s.id.clone()),
                        toggle: Action::MuteToggle(s.id.clone()),
                        leading: match perch_core::view::app_colour(&s.name) {
                            _ if s.id == perch_core::platform::SYSTEM_VOLUME_ID => {
                                RowLeading::Kind(perch_core::view::RowKind::Speaker)
                            }
                            Some(rgb) => RowLeading::Disc {
                                initials: perch_core::view::initials(&s.name),
                                rgb,
                            },
                            None => RowLeading::Avatar {
                                initials: perch_core::view::initials(&s.name),
                                seed: perch_core::view::seed(&s.name),
                            },
                        },
                    })
                    .collect(),
                empty: "No app is playing sound.".into(),
            },
            Some(Tab::Sounds) => Body::Sliders {
                rows: NoiseKind::ALL
                    .iter()
                    .map(|k| SliderRow {
                        label: k.label().into(),
                        value: self.noise.level(*k),
                        on: self.noise.is_on(*k),
                        target: SliderTarget::Noise(*k),
                        toggle: Action::NoiseToggle(*k),
                        leading: RowLeading::Kind(RowKind::Sound),
                    })
                    .collect(),
                empty: String::new(),
            },
            Some(Tab::Time) => {
                let day = DayId::from_unix(now.unix, local_offset());
                let top = self.usage.aggregate().top_apps(day, 5);
                let max = top.first().map(|(_, s)| *s).unwrap_or(1).max(1);
                Body::Usage {
                    total: format!(
                        "Today {}",
                        human_duration(self.usage.aggregate().grand_total(day))
                    ),
                    bars: top
                        .into_iter()
                        .map(|(app, secs)| UsageBar {
                            label: human_duration(secs),
                            fraction: secs as f32 / max as f32,
                            app,
                        })
                        .collect(),
                }
            }
            Some(Tab::Mail) => {
                let mut rows: Vec<Row> = self
                    .mail
                    .accounts()
                    .iter()
                    .filter(|a| a.status.is_problem())
                    .filter(|a| self.mail_account().is_none_or(|n| n == a.name))
                    .map(|a| Row {
                        text: format!("{}: {}", a.name, a.status.label()),
                        detail: String::new(),
                        check: None,
                        primary: Some(Action::OpenSettings),
                        remove: None,
                        dim: true,
                        leading: RowLeading::Kind(RowKind::Warning),
                        subtitle: None,
                        chip: None,
                    })
                    .collect();
                // The account name only helps on the "All" tab.
                let several = self.mail.accounts().len() > 1 && self.mail_account().is_none();
                rows.extend(
                    self.mail_items()
                        .into_iter()
                        .skip(self.scroll)
                        .map(|(acct, m)| Row {
                            text: m.from.clone(),
                            detail: match (m.date_unix, several) {
                                (Some(t), true) => format!("{acct} \u{00b7} {}", ago(now.unix, t)),
                                (Some(t), false) => ago(now.unix, t),
                                (None, true) => acct.to_string(),
                                (None, false) => String::new(),
                            },
                            check: None,
                            primary: None,
                            remove: None,
                            dim: false,
                            leading: RowLeading::Avatar {
                                initials: perch_core::view::initials(&m.from),
                                seed: perch_core::view::seed(&m.from),
                            },
                            subtitle: Some(m.subject.clone()),
                            chip: None,
                        }),
                );
                let connecting = self
                    .mail
                    .accounts()
                    .iter()
                    .any(|a| a.status == perch_core::mail::AccountStatus::Connecting);
                Body::List {
                    rows,
                    empty: if self.config.mail.accounts.is_empty() {
                        "Add a mail account in settings.".into()
                    } else if connecting {
                        "Connecting\u{2026}".into()
                    } else {
                        "No unread mail.".into()
                    },
                    header: Some(("Accounts".into(), Action::OpenSettings)),
                    filters: self.mail_filters(),
                }
            }
            Some(Tab::Tools) => {
                let mut b = Vec::new();
                if self.is_active(Feature::Screenshot) {
                    b.push(("Screenshot".to_string(), Action::Screenshot));
                }
                if self.is_active(Feature::CameraMirror) {
                    b.push(("Mirror".to_string(), Action::CameraToggle));
                }
                if self.is_active(Feature::SpeechToText) {
                    b.push(("Dictate".to_string(), Action::DictateToggle));
                }
                Body::Buttons(b)
            }
        };
        PanelView {
            collapsed: self.collapsed(),
            tabs,
            tab: self.tab,
            body,
            pinned: self.config.notch.keep_pill,
            banner: if self.panel_expanded {
                None
            } else {
                self.banners.current().cloned()
            },
        }
    }

    fn media_view(&self, now: Now) -> MediaView {
        let track = self.media.track();
        let duration = track.and_then(|t| t.duration);
        // While the seek bar is dragged, show where it would land.
        let pos = match (self.seek_preview, duration) {
            (Some(f), Some(d)) => Some(d.mul_f64(f64::from(f))),
            _ => self.position_now(now),
        };
        let fraction = match (pos, duration) {
            (Some(p), Some(d)) if !d.is_zero() => {
                Some((p.as_secs_f64() / d.as_secs_f64()).clamp(0.0, 1.0))
            }
            _ => None,
        };
        let times = match (pos, duration) {
            (Some(p), Some(d)) if !d.is_zero() => Some((clock(p), clock(d))),
            _ => None,
        };
        let lyric = match (&self.lyrics, pos) {
            (Some((_, l)), Some(p)) => l
                .line_at(p)
                .map(|line| line.text.clone())
                .filter(|t| !t.trim().is_empty()),
            _ => None,
        };
        let next_lyric = match (&self.lyrics, pos) {
            (Some((_, l)), Some(p)) => l.index_at(p).and_then(|i| {
                l.lines()
                    .iter()
                    .skip(i + 1)
                    .find(|line| !line.is_blank())
                    .map(|line| line.text.clone())
            }),
            _ => None,
        };
        MediaView {
            title: track
                .map(|t| t.title.clone())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "Nothing playing".into()),
            artist: track.map(|t| t.artist.clone()).unwrap_or_default(),
            has_player: track.is_some(),
            playing: self.media.is_playing(),
            fraction,
            times,
            lyric,
            cover: self.cover.clone(),
            accent: self.accent,
            next_lyric,
        }
    }

    /// How much of the current Pomodoro phase has passed.
    fn pomodoro_fraction(&self) -> f32 {
        if self.pomodoro.state() == TimerState::Idle {
            return 0.0;
        }
        let c = &self.config.pomodoro;
        let minutes = match self.pomodoro.phase() {
            Phase::Work => c.work_minutes,
            Phase::ShortBreak => c.break_minutes,
            Phase::LongBreak => c.long_break_minutes,
        };
        let total = (u64::from(minutes) * 60).max(1) as f32;
        (1.0 - self.pomodoro.remaining().as_secs_f32() / total).clamp(0.0, 1.0)
    }

    /// "Session 2 of 4 - break next" / "Break - focus next".
    fn pomodoro_session(&self) -> String {
        let per = self.config.pomodoro.sessions_before_long_break.max(1);
        match self.pomodoro.phase() {
            Phase::Work => format!(
                "Session {} of {per} \u{b7} break next",
                self.pomodoro.completed_work_sessions() % per + 1
            ),
            Phase::ShortBreak => "Short break \u{b7} focus next".into(),
            Phase::LongBreak => "Long break \u{b7} focus next".into(),
        }
    }

    fn tasks_view(&self, now: Now) -> TasksView {
        let pomodoro = self.is_active(Feature::Pomodoro).then(|| PomodoroView {
            label: match self.pomodoro.state() {
                TimerState::Idle => format!(
                    "Focus {}",
                    clock(Duration::from_secs(
                        u64::from(self.config.pomodoro.work_minutes) * 60
                    ))
                ),
                _ => self.pomodoro.label(),
            },
            running: self.pomodoro.state() == TimerState::Running,
            fraction: self.pomodoro_fraction(),
            remaining: clock(match self.pomodoro.state() {
                TimerState::Idle => {
                    Duration::from_secs(u64::from(self.config.pomodoro.work_minutes) * 60)
                }
                _ => self.pomodoro.remaining(),
            }),
            session: self.pomodoro_session(),
        });
        let todos = self.is_active(Feature::Todo).then(|| {
            let mut items = self.todos.all();
            // Pending first, then by due time, then creation.
            items.sort_by_key(|i| (i.done, i.due.unwrap_or(u64::MAX), i.created_at));
            items
                .into_iter()
                .skip(self.scroll)
                .map(|i| Row {
                    text: i.text.clone(),
                    detail: i.due.map(|d| due_label(now.unix, d)).unwrap_or_default(),
                    check: Some(i.done),
                    primary: Some(Action::TodoToggle(i.id)),
                    remove: Some(Action::TodoRemove(i.id)),
                    dim: i.done,
                    ..Row::default()
                })
                .collect()
        });
        let reminder = self.is_active(Feature::Reminders).then(|| {
            let water = self.reminders.remaining(ReminderKind::Water);
            let stretch = self.reminders.remaining(ReminderKind::Stretch);
            match (water, stretch) {
                (Some(w), Some(s)) if w <= s => format!("Water in {}", human_duration(w.as_secs())),
                (_, Some(s)) => format!("Stretch in {}", human_duration(s.as_secs())),
                (Some(w), None) => format!("Water in {}", human_duration(w.as_secs())),
                (None, None) => "Reminders off".into(),
            }
        });
        TasksView {
            pomodoro,
            todos,
            reminder,
        }
    }
}

fn clamp01(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The key a lyrics result is matched against.
pub fn token_key(t: &TrackToken) -> String {
    format!("{t:?}")
}

fn lyrics_request(token: &TrackToken, track: &Track) -> LyricsRequest {
    LyricsRequest {
        key: token_key(token),
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone(),
        duration_secs: track.duration.map(|d| d.as_secs()),
    }
}

fn parse_tab(s: &str) -> Option<Tab> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "today" | "clock" | "time-date" => Tab::Today,
        "music" | "media" => Tab::Media,
        "tasks" | "todo" => Tab::Tasks,
        "clips" | "clipboard" => Tab::Clipboard,
        "shelf" => Tab::Shelf,
        "mixer" | "volume" => Tab::Mixer,
        "sounds" | "noise" => Tab::Sounds,
        "time" => Tab::Time,
        "tools" => Tab::Tools,
        "mail" | "email" | "inbox" => Tab::Mail,
        _ => return None,
    })
}

fn ago(now: u64, then: u64) -> String {
    let d = now.saturating_sub(then);
    if d < 60 {
        "now".into()
    } else if d < 3600 {
        format!("{}m", d / 60)
    } else if d < 86_400 {
        format!("{}h", d / 3600)
    } else {
        format!("{}d", d / 86_400)
    }
}

fn due_label(now: u64, due: u64) -> String {
    if due <= now {
        "overdue".into()
    } else {
        format!("in {}", human_duration(due - now))
    }
}

/// Decode cover art into a small square RGBA buffer for the renderer.
/// A square RGBA thumbnail of an encoded image (clipboard rows).
fn decode_thumb(bytes: &[u8], side: u32) -> Option<(Arc<Vec<u8>>, u32)> {
    let img = image::load_from_memory(bytes).ok()?;
    let thumb = img.resize_to_fill(side, side, image::imageops::FilterType::Triangle);
    Some((Arc::new(thumb.to_rgba8().into_raw()), side))
}

fn decode_cover(bytes: &[u8]) -> Option<(Arc<Vec<u8>>, u32)> {
    let img = image::load_from_memory(bytes).ok()?;
    let side = 128u32;
    let thumb = img.resize_to_fill(side, side, image::imageops::FilterType::Triangle);
    Some((Arc::new(thumb.to_rgba8().into_raw()), side))
}

/// The local UTC offset for screen-time day buckets.
///
/// Read once at startup from the platform (via the `PERCH_UTC_OFFSET_SECS`
/// override or the system), defaulting to UTC when unknown.
pub fn local_offset() -> UtcOffset {
    if let Some(v) = std::env::var("PERCH_UTC_OFFSET_SECS")
        .ok()
        .and_then(|s| s.parse::<i32>().ok())
    {
        return UtcOffset(v);
    }
    UtcOffset(crate::sys::utc_offset_seconds())
}
