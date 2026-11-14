//! Small form windows (settings, add-to-do, add mail account), drawn with the
//! same software renderer and glass tokens as the notch.
//!
//! These are ordinary decorated, focusable windows: the pill itself never
//! takes keyboard focus (it is `WS_EX_NOACTIVATE` / a dock), so anything that
//! needs typing happens here. The form is described by a [`FormSpec`] and
//! reports what the user did as [`FormEvent`]s; it holds no app logic.
//!
//! Layout follows the settings style of the OS: rows after a
//! [`Field::Group`] sit in one rounded box with hairlines between them,
//! notes and buttons sit outside the boxes.

use std::num::NonZeroU32;
use std::rc::Rc;

use perch_core::error::{Error, Result};
use softbuffer::{Context, Surface};
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point, SpreadMode,
    Stroke, Transform,
};
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::art;
use crate::text::TextRenderer;
use crate::theme::{Gradient, Palette};
use crate::tokens::{text as ty, Weight};

/// One row of a form.
#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    /// Large heading (dialog sections); ends any open group.
    Heading(String),
    /// Section caption; the rows after it, up to the next group, heading,
    /// note or buttons, share one rounded background.
    Group(String),
    /// Muted explanatory text; ends any open group.
    Note(String),
    /// On/off switch.
    Toggle {
        /// Id reported back.
        id: String,
        /// Label.
        label: String,
        /// Current value.
        value: bool,
        /// Greyed out (e.g. not compiled into this build).
        disabled: bool,
    },
    /// Pick one of several options: a segmented control when the options
    /// fit (at most four), otherwise a stepper whose arrows cycle.
    Choice {
        /// Id reported back.
        id: String,
        /// Label.
        label: String,
        /// Options.
        options: Vec<String>,
        /// Selected index.
        selected: usize,
    },
    /// Single-line text input.
    Text {
        /// Id reported back.
        id: String,
        /// Label.
        label: String,
        /// Current text.
        value: String,
        /// Shown when empty.
        placeholder: String,
        /// Mask the value (passwords): drawn as bullets, never as text.
        secret: bool,
    },
    /// A status line: coloured dot, label, detail and an optional button
    /// `(id, label)` (mail accounts).
    Status {
        /// Main text.
        label: String,
        /// Secondary text under it.
        detail: String,
        /// Green when `Some(true)`, red when `Some(false)`, grey while unknown.
        ok: Option<bool>,
        /// Trailing button.
        action: Option<(String, String)>,
    },
    /// A row of buttons `(id, label)`; the first is the primary action. Ends
    /// any open group.
    Buttons(Vec<(String, String)>),
}

impl Field {
    /// Whether this field sits inside an open group's box.
    fn groupable(&self) -> bool {
        matches!(
            self,
            Field::Toggle { .. } | Field::Choice { .. } | Field::Text { .. } | Field::Status { .. }
        )
    }
}

/// A whole form.
#[derive(Debug, Clone, PartialEq)]
pub struct FormSpec {
    /// Window title.
    pub title: String,
    /// Rows.
    pub fields: Vec<Field>,
    /// Logical size of the window.
    pub size: (u32, u32),
}

/// What the user did.
#[derive(Debug, Clone, PartialEq)]
pub enum FormEvent {
    /// A toggle flipped.
    Toggled(String, bool),
    /// A choice changed.
    Chosen(String, usize),
    /// A button was pressed (Enter in a text field presses the first button).
    Button(String),
    /// The window was closed (or Escape pressed).
    Closed,
}

#[derive(Debug, Clone, PartialEq)]
enum Region {
    Toggle(usize),
    ChoiceNext(usize),
    ChoicePrev(usize),
    /// Segment `k` of the segmented choice at field `i`.
    Segment(usize, usize),
    Text(usize),
    Button(String),
}

/// Logical layout constants.
const PAD: f32 = 16.0;
const INSET: f32 = 14.0;
const ROW: f32 = 42.0;
const STATUS_ROW: f32 = 52.0;
const CONTROL_H: f32 = 28.0;
const GROUP_RADIUS: f32 = 12.0;
const LABEL_MIN: f32 = 110.0;
const MAX_SEGMENTS: usize = 4;

/// Where pass one put a field.
#[derive(Debug, Clone, Copy)]
struct Place {
    y: f32,
    h: f32,
    /// Inside a group box, and not its first row (draws a hairline above).
    divider: bool,
}

/// Pure form state + layout, separated from the window so it can be tested.
#[derive(Debug, Clone)]
pub struct FormState {
    spec: FormSpec,
    focus: Option<usize>,
    scroll: f32,
    content_h: f32,
    regions: Vec<(f32, f32, f32, f32, Region)>,
    groups: Vec<(f32, f32, f32, f32)>,
    hover: Option<(f32, f32)>,
}

impl FormState {
    /// Fresh state; focuses the first text field, if any.
    pub fn new(spec: FormSpec) -> Self {
        let focus = spec
            .fields
            .iter()
            .position(|f| matches!(f, Field::Text { .. }));
        Self {
            spec,
            focus,
            scroll: 0.0,
            content_h: 0.0,
            regions: Vec::new(),
            groups: Vec::new(),
            hover: None,
        }
    }

    /// The spec, with any edits applied.
    pub fn spec(&self) -> &FormSpec {
        &self.spec
    }

    /// Group boxes from the last paint, in logical content coordinates
    /// `(x, y, w, h)` (before scrolling).
    pub fn group_rects(&self) -> &[(f32, f32, f32, f32)] {
        &self.groups
    }

    /// Replace the spec (e.g. after the app re-read the config), keeping text
    /// the user is typing and the scroll position.
    pub fn replace(&mut self, spec: FormSpec) {
        self.spec = spec;
        if let Some(f) = self.focus {
            if !matches!(self.spec.fields.get(f), Some(Field::Text { .. })) {
                self.focus = None;
            }
        }
    }

    /// Current value of a text field.
    pub fn text(&self, id: &str) -> Option<&str> {
        self.spec.fields.iter().find_map(|f| match f {
            Field::Text { id: fid, value, .. } if fid == id => Some(value.as_str()),
            _ => None,
        })
    }

    fn region_at(&self, x: f32, y: f32) -> Option<&Region> {
        let y = y + self.scroll;
        self.regions
            .iter()
            .find(|(rx, ry, rw, rh, _)| x >= *rx && y >= *ry && x < rx + rw && y < ry + rh)
            .map(|r| &r.4)
    }

    /// The cursor moved (logical coordinates, `None` = left the window).
    /// Returns true when the hovered control changed and a repaint is due.
    pub fn set_hover(&mut self, pos: Option<(f32, f32)>) -> bool {
        let before = self.hover.and_then(|(x, y)| self.region_at(x, y)).cloned();
        self.hover = pos;
        let after = pos.and_then(|(x, y)| self.region_at(x, y)).cloned();
        before != after
    }

    fn hovered(&self, region: &Region) -> bool {
        self.hover
            .and_then(|(x, y)| self.region_at(x, y))
            .is_some_and(|r| r == region)
    }

    /// Feed a click at logical coordinates.
    pub fn click(&mut self, x: f32, y: f32) -> Vec<FormEvent> {
        let Some(region) = self.region_at(x, y).cloned() else {
            return Vec::new();
        };
        match region {
            Region::Toggle(i) => match self.spec.fields.get_mut(i) {
                Some(Field::Toggle {
                    id,
                    value,
                    disabled: false,
                    ..
                }) => {
                    *value = !*value;
                    vec![FormEvent::Toggled(id.clone(), *value)]
                }
                _ => Vec::new(),
            },
            Region::ChoiceNext(i) | Region::ChoicePrev(i) => {
                let forward = matches!(region, Region::ChoiceNext(_));
                match self.spec.fields.get_mut(i) {
                    Some(Field::Choice {
                        id,
                        options,
                        selected,
                        ..
                    }) if !options.is_empty() => {
                        let n = options.len();
                        *selected = if forward {
                            (*selected + 1) % n
                        } else {
                            (*selected + n - 1) % n
                        };
                        vec![FormEvent::Chosen(id.clone(), *selected)]
                    }
                    _ => Vec::new(),
                }
            }
            Region::Segment(i, k) => match self.spec.fields.get_mut(i) {
                Some(Field::Choice {
                    id,
                    options,
                    selected,
                    ..
                }) if k < options.len() => {
                    if *selected == k {
                        return Vec::new();
                    }
                    *selected = k;
                    vec![FormEvent::Chosen(id.clone(), k)]
                }
                _ => Vec::new(),
            },
            Region::Text(i) => {
                self.focus = Some(i);
                Vec::new()
            }
            Region::Button(id) => vec![FormEvent::Button(id)],
        }
    }

    /// Scroll by `dy` logical pixels (positive = down).
    pub fn scroll(&mut self, dy: f32, view_h: f32) {
        let max = (self.content_h - view_h).max(0.0);
        self.scroll = (self.scroll + dy).clamp(0.0, max);
    }

    /// Typed text goes to the focused field.
    pub fn type_text(&mut self, s: &str) {
        if let Some(Field::Text { value, .. }) =
            self.focus.and_then(|f| self.spec.fields.get_mut(f))
        {
            for ch in s.chars().filter(|c| !c.is_control()) {
                if value.chars().count() < 500 {
                    value.push(ch);
                }
            }
        }
    }

    /// Backspace in the focused field.
    pub fn backspace(&mut self) {
        if let Some(Field::Text { value, .. }) =
            self.focus.and_then(|f| self.spec.fields.get_mut(f))
        {
            value.pop();
        }
    }

    /// Move focus to the next text field.
    pub fn tab(&mut self) {
        let texts: Vec<usize> = self
            .spec
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| matches!(f, Field::Text { .. }))
            .map(|(i, _)| i)
            .collect();
        if texts.is_empty() {
            return;
        }
        let next = match self.focus.and_then(|f| texts.iter().position(|t| *t == f)) {
            Some(p) => texts[(p + 1) % texts.len()],
            None => texts[0],
        };
        self.focus = Some(next);
    }

    /// Enter: press the first button of the form.
    pub fn enter(&self) -> Vec<FormEvent> {
        self.spec
            .fields
            .iter()
            .find_map(|f| match f {
                Field::Buttons(b) => b.first().map(|(id, _)| FormEvent::Button(id.clone())),
                _ => None,
            })
            .into_iter()
            .collect()
    }

    /// Segment widths for a choice, or `None` when it should be a stepper.
    fn segments(text: &mut TextRenderer, options: &[String], avail: f32) -> Option<Vec<f32>> {
        if options.is_empty() || options.len() > MAX_SEGMENTS {
            return None;
        }
        let widths: Vec<f32> = options
            .iter()
            .map(|o| (measure(text, o, ty::BODY) + 22.0).max(46.0))
            .collect();
        (widths.iter().sum::<f32>() + 4.0 <= avail).then_some(widths)
    }

    /// Pass one: where every field goes, and the group boxes.
    fn layout(&mut self, text: &mut TextRenderer, w: f32) -> Vec<Place> {
        let mut places = Vec::with_capacity(self.spec.fields.len());
        self.groups.clear();
        let mut y = 12.0;
        // (top of the open box, rows in it so far)
        let mut open: Option<(f32, usize)> = None;
        let close = |y: &mut f32, open: &mut Option<(f32, usize)>, groups: &mut Vec<_>| {
            if let Some((top, rows)) = open.take() {
                if rows > 0 {
                    groups.push((PAD, top, w - 2.0 * PAD, *y - top));
                    *y += 10.0;
                }
            }
        };
        for field in &self.spec.fields {
            if !field.groupable() {
                close(&mut y, &mut open, &mut self.groups);
            }
            let (h, divider) = match field {
                Field::Heading(_) => {
                    y += 6.0;
                    (30.0, false)
                }
                Field::Group(_) => {
                    close(&mut y, &mut open, &mut self.groups);
                    y += 4.0;
                    let h = 24.0;
                    open = Some((y + h, 0));
                    (h, false)
                }
                Field::Note(t) => {
                    let lines = wrap(text, t, ty::SECONDARY, w - 2.0 * PAD - 8.0).len();
                    (lines as f32 * 16.0 + 8.0, false)
                }
                Field::Buttons(_) => {
                    y += 6.0;
                    (40.0, false)
                }
                Field::Toggle { .. } | Field::Choice { .. } => (ROW, false),
                Field::Text { .. } => (70.0, false),
                Field::Status { .. } => (STATUS_ROW, false),
            };
            let divider = divider
                || match &mut open {
                    Some((_, rows)) if field.groupable() => {
                        *rows += 1;
                        *rows > 1
                    }
                    _ => false,
                };
            places.push(Place { y, h, divider });
            y += h;
        }
        close(&mut y, &mut open, &mut self.groups);
        self.content_h = y + 12.0;
        places
    }

    /// Paint into `pm` at `scale` and record click regions.
    pub fn paint(&mut self, pm: &mut Pixmap, text: &mut TextRenderer, pal: &Palette, scale: f32) {
        let s = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let w = pm.width() as f32 / s;
        pm.fill(window_bg(pal));
        self.regions.clear();
        let places = self.layout(text, w);
        let scroll = self.scroll;
        let mut p = Painter { pm, text, s };
        let box_bg = group_bg(pal);
        for &(gx, gy, gw, gh) in &self.groups {
            p.rrect(gx, gy - scroll, gw, gh, GROUP_RADIUS, box_bg);
        }
        let fields = self.spec.fields.clone();
        let (x0, x1) = (PAD + INSET, w - PAD - INSET);
        for (i, (field, place)) in fields.iter().zip(&places).enumerate() {
            let (y, h) = (place.y, place.h);
            let oy = y - scroll;
            if place.divider {
                p.rect(x0, oy, x1 - x0 + INSET, 0.5, with_alpha(pal.border, 0.8));
            }
            match field {
                Field::Heading(t) => {
                    p.text(t, PAD, oy, h, ty::TITLE, pal.fg, w - 2.0 * PAD);
                }
                Field::Group(t) => {
                    p.text(
                        &t.to_uppercase(),
                        PAD + 4.0,
                        oy + 4.0,
                        h - 4.0,
                        ty::CAPTION,
                        pal.muted,
                        w - 2.0 * PAD,
                    );
                }
                Field::Note(t) => {
                    let mut ly = oy + 4.0;
                    for line in wrap(p.text, t, ty::SECONDARY, w - 2.0 * PAD - 8.0) {
                        p.text(&line, PAD + 4.0, ly, 16.0, ty::SECONDARY, pal.muted, w);
                        ly += 16.0;
                    }
                }
                Field::Toggle {
                    label,
                    value,
                    disabled,
                    ..
                } => {
                    let region = Region::Toggle(i);
                    if !*disabled && self.hovered(&region) {
                        p.row_hover(PAD, oy, w - 2.0 * PAD, h, pal);
                    }
                    let color = if *disabled { pal.muted } else { pal.fg };
                    p.text(label, x0, oy, h, ty::BODY, color, x1 - x0 - 52.0);
                    let (sw, sh) = (40.0, 24.0);
                    p.switch(x1 - sw, oy + (h - sh) / 2.0, sw, sh, *value, *disabled, pal);
                    self.regions.push((PAD, y, w - 2.0 * PAD, h, region));
                }
                Field::Choice {
                    label,
                    options,
                    selected,
                    ..
                } => {
                    let cy = oy + (h - CONTROL_H) / 2.0;
                    let avail = x1 - x0 - LABEL_MIN;
                    if let Some(widths) = Self::segments(p.text, options, avail) {
                        let total: f32 = widths.iter().sum::<f32>() + 4.0;
                        let label_w = x1 - x0 - total - 10.0;
                        p.text(label, x0, oy, h, ty::BODY, pal.fg, label_w);
                        let bx = x1 - total;
                        p.rrect(bx, cy, total, CONTROL_H, CONTROL_H / 2.0, pal.raised[1]);
                        let mut sx = bx + 2.0;
                        for (k, (opt, sw)) in options.iter().zip(&widths).enumerate() {
                            let region = Region::Segment(i, k);
                            let on = k == *selected;
                            let (sy, sh) = (cy + 2.0, CONTROL_H - 4.0);
                            if on {
                                p.rrect(sx, sy, *sw, sh, sh / 2.0, segment_bg(pal));
                            } else if self.hovered(&region) {
                                p.rrect(sx, sy, *sw, sh, sh / 2.0, pal.raised[0]);
                            }
                            let tw = measure(p.text, opt, ty::BODY).min(*sw - 8.0);
                            let color = if on { pal.fg } else { pal.muted };
                            p.text(opt, sx + (sw - tw) / 2.0, sy, sh, ty::BODY, color, sw - 8.0);
                            self.regions
                                .push((sx, y + (cy - oy), *sw, CONTROL_H, region));
                            sx += sw;
                        }
                    } else {
                        let bw = ((x1 - x0) * 0.5).clamp(120.0, 230.0);
                        let bx = x1 - bw;
                        p.text(label, x0, oy, h, ty::BODY, pal.fg, x1 - x0 - bw - 10.0);
                        p.rrect(bx, cy, bw, CONTROL_H, CONTROL_H / 2.0, pal.raised[1]);
                        let value = options.get(*selected).cloned().unwrap_or_default();
                        let prev = Region::ChoicePrev(i);
                        let next = Region::ChoiceNext(i);
                        let arrow = |p: &mut Painter<'_>, x: f32, hot: bool| {
                            if hot {
                                p.rrect(x, cy + 2.0, 24.0, CONTROL_H - 4.0, 12.0, pal.raised[1]);
                            }
                        };
                        arrow(&mut p, bx + 2.0, self.hovered(&prev));
                        arrow(&mut p, bx + bw - 26.0, self.hovered(&next));
                        p.chevron(bx + 14.0, cy + CONTROL_H / 2.0, false, pal.muted);
                        p.chevron(bx + bw - 14.0, cy + CONTROL_H / 2.0, true, pal.muted);
                        let tw = measure(p.text, &value, ty::BODY).min(bw - 56.0);
                        p.text(
                            &value,
                            bx + (bw - tw) / 2.0,
                            cy,
                            CONTROL_H,
                            ty::BODY,
                            pal.fg,
                            bw - 56.0,
                        );
                        let ry = y + (cy - oy);
                        self.regions.push((bx, ry, 28.0, CONTROL_H, prev));
                        self.regions
                            .push((bx + 28.0, ry, bw - 28.0, CONTROL_H, next));
                    }
                }
                Field::Text {
                    label,
                    value,
                    placeholder,
                    secret,
                    ..
                } => {
                    let (lx, lw) = (x0, x1 - x0);
                    p.text(label, lx, oy + 4.0, 22.0, ty::SECONDARY, pal.muted, lw);
                    let (by, bh) = (oy + 28.0, 34.0);
                    let focused = self.focus == Some(i);
                    p.rrect(lx, by, lw, bh, 10.0, pal.raised[1]);
                    if focused {
                        p.stroke_rrect(lx, by, lw, bh, 10.0, pal.mail.from, 1.5);
                    } else {
                        p.stroke_rrect(lx, by, lw, bh, 10.0, pal.border, 1.0);
                    }
                    let (shown, color) = if value.is_empty() {
                        (placeholder.clone(), pal.muted)
                    } else if *secret {
                        (mask(value), pal.fg)
                    } else {
                        (value.clone(), pal.fg)
                    };
                    let tw = p.text_tail(&shown, lx + 10.0, by, bh, ty::BODY, color, lw - 20.0);
                    if focused {
                        let cx = if value.is_empty() {
                            lx + 10.0
                        } else {
                            lx + 10.0 + tw
                        };
                        p.rect(cx, by + 8.0, 1.5, bh - 16.0, pal.mail.from);
                    }
                    self.regions.push((lx, y + 28.0, lw, bh, Region::Text(i)));
                }
                Field::Status {
                    label,
                    detail,
                    ok,
                    action,
                } => {
                    let dot = match ok {
                        Some(true) => pal.ok,
                        Some(false) => pal.error,
                        None => with_alpha(pal.fg, 0.35),
                    };
                    p.circle(x0 + 4.0, oy + h / 2.0, 4.0, dot);
                    let mut right = x1;
                    if let Some((id, blabel)) = action {
                        let region = Region::Button(id.clone());
                        let bw = measure(p.text, blabel, ty::BODY) + 24.0;
                        let (bx, by) = (x1 - bw, oy + (h - CONTROL_H) / 2.0);
                        let bg = if self.hovered(&region) {
                            pal.raised[2]
                        } else {
                            pal.raised[1]
                        };
                        p.rrect(bx, by, bw, CONTROL_H, CONTROL_H / 2.0, bg);
                        p.text(blabel, bx + 12.0, by, CONTROL_H, ty::BODY, pal.fg, bw);
                        self.regions
                            .push((bx, y + (by - oy), bw, CONTROL_H, region));
                        right = bx - 10.0;
                    }
                    let tx = x0 + 18.0;
                    p.text(label, tx, oy + 7.0, 20.0, ty::BODY, pal.fg, right - tx);
                    p.text(
                        detail,
                        tx,
                        oy + 26.0,
                        18.0,
                        ty::SECONDARY,
                        pal.muted,
                        right - tx,
                    );
                }
                Field::Buttons(buttons) => {
                    let bh = 32.0;
                    let by = oy + (h - bh) / 2.0;
                    let mut bx = w - PAD;
                    for (n, (id, label)) in buttons.iter().enumerate().rev() {
                        let region = Region::Button(id.clone());
                        let lw = measure(p.text, label, ty::BODY) + 32.0;
                        bx -= lw;
                        let hot = self.hovered(&region);
                        if n == 0 {
                            p.gradient_rrect(bx, by, lw, bh, bh / 2.0, pal.mail);
                            if hot {
                                p.rrect(bx, by, lw, bh, bh / 2.0, with_alpha(Color::WHITE, 0.12));
                            }
                            p.text(label, bx + 16.0, by, bh, ty::BODY, Color::WHITE, lw);
                        } else {
                            let bg = if hot { pal.raised[2] } else { pal.raised[1] };
                            p.rrect(bx, by, lw, bh, bh / 2.0, bg);
                            p.text(label, bx + 16.0, by, bh, ty::BODY, pal.fg, lw);
                        }
                        self.regions.push((bx, y + (by - oy), lw, bh, region));
                        bx -= 8.0;
                    }
                }
            }
        }
    }
}

/// Opaque window background.
fn window_bg(pal: &Palette) -> Color {
    if pal.dark {
        opaque(pal.surface)
    } else {
        Color::from_rgba8(240, 241, 245, 255)
    }
}

/// Fill of a group box.
fn group_bg(pal: &Palette) -> Color {
    if pal.dark {
        pal.raised[0]
    } else {
        Color::WHITE
    }
}

/// The selected segment's pill.
fn segment_bg(pal: &Palette) -> Color {
    if pal.dark {
        pal.raised[2]
    } else {
        Color::WHITE
    }
}

fn opaque(c: Color) -> Color {
    let mut c = c;
    c.set_alpha(1.0);
    c
}

fn with_alpha(c: Color, a: f32) -> Color {
    let mut c = c;
    c.apply_opacity(a);
    c
}

/// Logical width of `s` (scale-independent).
fn measure(text: &mut TextRenderer, s: &str, (px, weight): (f32, Weight)) -> f32 {
    // Measure at a fixed 2x and halve: layout must not depend on the scale.
    text.measure_w(s, px * 2.0, weight).width / 2.0
}

/// Draws in logical coordinates onto a physical pixmap.
struct Painter<'a> {
    pm: &'a mut Pixmap,
    text: &'a mut TextRenderer,
    s: f32,
}

impl Painter<'_> {
    fn paint(color: Color) -> Paint<'static> {
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(color);
        paint
    }

    fn fill(&mut self, path: Option<tiny_skia::Path>, color: Color) {
        if let Some(path) = path {
            self.pm.fill_path(
                &path,
                &Self::paint(color),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        let s = self.s;
        let path = tiny_skia::Rect::from_xywh(x * s, y * s, w * s, (h * s).max(1.0))
            .map(PathBuilder::from_rect);
        self.fill(path, color);
    }

    fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: Color) {
        let s = self.s;
        self.fill(art::rounded_rect(x * s, y * s, w * s, h * s, r * s), color);
    }

    #[allow(clippy::too_many_arguments)]
    fn stroke_rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: Color, width: f32) {
        let s = self.s;
        let half = width * s / 2.0;
        if let Some(path) = art::rounded_rect(
            x * s + half,
            y * s + half,
            w * s - 2.0 * half,
            h * s - 2.0 * half,
            r * s,
        ) {
            let st = Stroke {
                width: width * s,
                ..Default::default()
            };
            self.pm
                .stroke_path(&path, &Self::paint(color), &st, Transform::identity(), None);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn gradient_rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, g: Gradient) {
        let s = self.s;
        let Some(path) = art::rounded_rect(x * s, y * s, w * s, h * s, r * s) else {
            return;
        };
        let shader = LinearGradient::new(
            Point::from_xy(x * s, y * s),
            Point::from_xy((x + w) * s, y * s),
            vec![GradientStop::new(0.0, g.from), GradientStop::new(1.0, g.to)],
            SpreadMode::Pad,
            Transform::identity(),
        );
        let paint = match shader {
            Some(shader) => Paint {
                shader,
                anti_alias: true,
                ..Default::default()
            },
            None => Self::paint(g.from),
        };
        self.pm.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    fn circle(&mut self, cx: f32, cy: f32, r: f32, color: Color) {
        let s = self.s;
        self.fill(PathBuilder::from_circle(cx * s, cy * s, r * s), color);
    }

    /// A small chevron pointing left or right, centred on `(cx, cy)`.
    fn chevron(&mut self, cx: f32, cy: f32, right: bool, color: Color) {
        let s = self.s;
        let d = if right { 1.0 } else { -1.0 };
        let mut pb = PathBuilder::new();
        pb.move_to((cx - 2.0 * d) * s, (cy - 4.5) * s);
        pb.line_to((cx + 2.5 * d) * s, cy * s);
        pb.line_to((cx - 2.0 * d) * s, (cy + 4.5) * s);
        if let Some(path) = pb.finish() {
            let st = Stroke {
                width: 1.6 * s,
                line_cap: tiny_skia::LineCap::Round,
                line_join: tiny_skia::LineJoin::Round,
                ..Default::default()
            };
            self.pm
                .stroke_path(&path, &Self::paint(color), &st, Transform::identity(), None);
        }
    }

    /// An on/off switch.
    #[allow(clippy::too_many_arguments)]
    fn switch(&mut self, x: f32, y: f32, w: f32, h: f32, on: bool, disabled: bool, pal: &Palette) {
        let fade = if disabled { 0.4 } else { 1.0 };
        if on {
            let mut g = pal.mail;
            g.from.apply_opacity(fade);
            g.to.apply_opacity(fade);
            self.gradient_rrect(x, y, w, h, h / 2.0, g);
        } else {
            self.rrect(x, y, w, h, h / 2.0, with_alpha(pal.fg, 0.18 * fade));
        }
        let r = h / 2.0 - 2.0;
        let kx = if on { x + w - h / 2.0 } else { x + h / 2.0 };
        self.circle(
            kx,
            y + h / 2.0 + 0.5,
            r,
            with_alpha(Color::BLACK, 0.18 * fade),
        );
        self.circle(kx, y + h / 2.0, r, with_alpha(Color::WHITE, fade));
    }

    /// A subtle highlight behind a hovered row.
    fn row_hover(&mut self, x: f32, y: f32, w: f32, h: f32, pal: &Palette) {
        self.rrect(x, y, w, h, 4.0, with_alpha(pal.fg, 0.04));
    }

    /// Text in a logical band, elided to `max_w`; returns the logical width drawn.
    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        s_: &str,
        x: f32,
        top: f32,
        band: f32,
        (px, weight): (f32, Weight),
        color: Color,
        max_w: f32,
    ) -> f32 {
        if max_w <= 0.0 || s_.is_empty() {
            return 0.0;
        }
        let s = self.s;
        let pxs = px * s;
        let shown = self.text.elide_w(s_, pxs, max_w * s, weight);
        let m = self.text.measure_w(&shown, pxs, weight);
        let baseline = top * s + (band * s + m.ascent - m.descent) / 2.0;
        self.text.draw_w(
            self.pm,
            &shown,
            x * s,
            baseline,
            pxs,
            color,
            max_w * s + 1.0,
            weight,
        ) / s
    }

    /// Like `text`, but keeps the *end* of the string visible (for inputs).
    #[allow(clippy::too_many_arguments)]
    fn text_tail(
        &mut self,
        s_: &str,
        x: f32,
        top: f32,
        band: f32,
        style: (f32, Weight),
        color: Color,
        max_w: f32,
    ) -> f32 {
        let pxs = style.0 * self.s;
        let chars: Vec<char> = s_.chars().collect();
        let mut start = 0;
        while start < chars.len() {
            let tail: String = chars[start..].iter().collect();
            if self.text.measure_w(&tail, pxs, style.1).width <= max_w * self.s {
                break;
            }
            start += 1;
        }
        let tail: String = chars[start..].iter().collect();
        self.text(&tail, x, top, band, style, color, max_w)
    }
}

fn wrap(text: &mut TextRenderer, s: &str, style: (f32, Weight), max_w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if measure(text, &candidate, style) > max_w && !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        } else {
            cur = candidate;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// A form in its own window.
pub struct FormWindow {
    window: Rc<Window>,
    surface: Surface<Rc<Window>, Rc<Window>>,
    state: FormState,
    text: TextRenderer,
    palette: Palette,
    cursor: (f64, f64),
    /// Reused between paints; reallocated only when the window size changes.
    pixmap: Option<Pixmap>,
}

impl FormWindow {
    /// Open a form window.
    pub fn open(el: &ActiveEventLoop, spec: FormSpec, palette: Palette) -> Result<Self> {
        let attrs = Window::default_attributes()
            .with_title(spec.title.clone())
            .with_inner_size(LogicalSize::new(spec.size.0, spec.size.1))
            .with_resizable(true)
            .with_min_inner_size(LogicalSize::new(320, 200));
        let window = Rc::new(
            el.create_window(attrs)
                .map_err(|e| Error::backend(format!("creating the form window: {e}")))?,
        );
        let ctx = Context::new(window.clone())
            .map_err(|e| Error::backend(format!("softbuffer context: {e}")))?;
        let surface = Surface::new(&ctx, window.clone())
            .map_err(|e| Error::backend(format!("softbuffer surface: {e}")))?;
        #[cfg(windows)]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = window.window_handle() {
                if let RawWindowHandle::Win32(h) = h.as_raw() {
                    perch_platform_windows::window::apply_mica(
                        perch_platform_windows::window::hwnd_from_raw(isize::from(h.hwnd)),
                        palette.dark,
                    );
                }
            }
        }
        window.focus_window();
        Ok(Self {
            window,
            surface,
            state: FormState::new(spec),
            text: TextRenderer::new()?,
            palette,
            cursor: (0.0, 0.0),
            pixmap: None,
        })
    }

    /// The window id.
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// Form state (for reading text fields).
    pub fn state(&self) -> &FormState {
        &self.state
    }

    /// Replace the spec and repaint.
    pub fn replace(&mut self, spec: FormSpec) {
        self.state.replace(spec);
        self.window.request_redraw();
    }

    /// Bring to front.
    pub fn focus(&self) {
        self.window.focus_window();
    }

    /// Handle a window event; returns what the user did.
    pub fn handle(&mut self, event: &WindowEvent) -> Vec<FormEvent> {
        let scale = self.window.scale_factor() as f32;
        let out = match event {
            WindowEvent::CloseRequested => return vec![FormEvent::Closed],
            WindowEvent::RedrawRequested => {
                self.paint();
                return Vec::new();
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.window.request_redraw();
                return Vec::new();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                let at = (position.x as f32 / scale, position.y as f32 / scale);
                if self.state.set_hover(Some(at)) {
                    self.window.request_redraw();
                }
                return Vec::new();
            }
            WindowEvent::CursorLeft { .. } => {
                if self.state.set_hover(None) {
                    self.window.request_redraw();
                }
                return Vec::new();
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self
                .state
                .click(self.cursor.0 as f32 / scale, self.cursor.1 as f32 / scale),
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * 40.0,
                    MouseScrollDelta::PixelDelta(p) => -(p.y as f32) / scale,
                };
                let view_h = self.window.inner_size().height as f32 / scale;
                self.state.scroll(dy, view_h);
                Vec::new()
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => return vec![FormEvent::Closed],
                    Key::Named(NamedKey::Enter) => self.state.enter(),
                    Key::Named(NamedKey::Backspace) => {
                        self.state.backspace();
                        Vec::new()
                    }
                    Key::Named(NamedKey::Tab) => {
                        self.state.tab();
                        Vec::new()
                    }
                    _ => {
                        if let Some(t) = &event.text {
                            self.state.type_text(t);
                        }
                        Vec::new()
                    }
                }
            }
            _ => return Vec::new(),
        };
        self.window.request_redraw();
        out
    }

    fn paint(&mut self) {
        let size = self.window.inner_size();
        let (Some(nw), Some(nh)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        if self.pixmap.as_ref().map(|p| (p.width(), p.height())) != Some((size.width, size.height))
        {
            self.pixmap = Pixmap::new(size.width, size.height);
        }
        let Some(pm) = self.pixmap.as_mut() else {
            return;
        };
        let scale = self.window.scale_factor() as f32;
        self.state.paint(pm, &mut self.text, &self.palette, scale);
        if self.surface.resize(nw, nh).is_err() {
            return;
        }
        let Ok(mut buffer) = self.surface.buffer_mut() else {
            return;
        };
        for (dst, src) in buffer.iter_mut().zip(pm.pixels()) {
            *dst = (u32::from(src.red()) << 16)
                | (u32::from(src.green()) << 8)
                | u32::from(src.blue());
        }
        if let Err(e) = buffer.present() {
            log::warn!("presenting the form: {e}");
        }
    }
}

/// What a secret field shows instead of its value: one bullet per character.
pub fn mask(value: &str) -> String {
    "\u{2022}".repeat(value.chars().count())
}

#[cfg(test)]
mod mask_tests {
    use super::mask;

    #[test]
    fn secrets_become_bullets_one_per_character() {
        assert_eq!(mask("pä55"), "\u{2022}\u{2022}\u{2022}\u{2022}");
        assert_eq!(mask(""), "");
        assert!(!mask("hunter2").contains("hunter2"));
    }
}

#[cfg(test)]
mod glass_tests {
    use super::*;

    fn toggle(id: &str) -> Field {
        Field::Toggle {
            id: id.into(),
            label: id.into(),
            value: false,
            disabled: false,
        }
    }

    fn painted(fields: Vec<Field>) -> (FormState, Pixmap) {
        let mut st = FormState::new(FormSpec {
            title: "t".into(),
            fields,
            size: (400, 400),
        });
        let mut pm = Pixmap::new(400, 400).unwrap_or_else(|| panic!("pixmap"));
        let mut text = TextRenderer::new().unwrap_or_else(|e| panic!("{e}"));
        st.paint(&mut pm, &mut text, &Palette::dark(false), 1.0);
        (st, pm)
    }

    fn region_of(st: &FormState, want: impl Fn(&Region) -> bool) -> (f32, f32, f32, f32) {
        st.regions
            .iter()
            .find(|r| want(&r.4))
            .map(|r| (r.0, r.1, r.2, r.3))
            .unwrap_or_else(|| panic!("region not found"))
    }

    #[test]
    fn groups_draw_one_background_per_section() {
        let (st, pm) = painted(vec![
            Field::Group("A".into()),
            toggle("a1"),
            toggle("a2"),
            Field::Group("B".into()),
            toggle("b1"),
        ]);
        let groups = st.group_rects();
        assert_eq!(groups.len(), 2, "{groups:?}");
        let inside = |g: &(f32, f32, f32, f32), r: (f32, f32, f32, f32)| {
            r.1 >= g.1 && r.1 + r.3 <= g.1 + g.3 + 0.5
        };
        assert!(inside(
            &groups[0],
            region_of(&st, |r| matches!(r, Region::Toggle(1)))
        ));
        assert!(inside(
            &groups[0],
            region_of(&st, |r| matches!(r, Region::Toggle(2)))
        ));
        assert!(inside(
            &groups[1],
            region_of(&st, |r| matches!(r, Region::Toggle(4)))
        ));
        // The group box is lighter than the window; the gap between groups is not.
        let px = |x: f32, y: f32| {
            let p = pm
                .pixel(x as u32, y as u32)
                .unwrap_or_else(|| panic!("pixel"));
            u32::from(p.red()) + u32::from(p.green()) + u32::from(p.blue())
        };
        let g = groups[0];
        let window = px(3.0, 3.0);
        assert!(
            px(g.0 + 6.0, g.1 + g.3 / 2.0) > window + 6,
            "group box shows"
        );
        let gap = (g.1 + g.3 + groups[1].1) / 2.0;
        assert_eq!(px(g.0 + 6.0, gap), window, "nothing between the groups");
    }

    #[test]
    fn segmented_choice_reports_the_clicked_option() {
        let choice = |n: usize| Field::Choice {
            id: "c".into(),
            label: "Pick".into(),
            options: (0..n).map(|i| format!("O{i}")).collect(),
            selected: 0,
        };
        let (mut st, _) = painted(vec![choice(3)]);
        let (x, y, w, h) = region_of(&st, |r| matches!(r, Region::Segment(0, 2)));
        assert_eq!(
            st.click(x + w / 2.0, y + h / 2.0),
            vec![FormEvent::Chosen("c".into(), 2)]
        );
        // Too many options for segments: the stepper steps forward.
        let (mut st, _) = painted(vec![choice(8)]);
        assert!(!st
            .regions
            .iter()
            .any(|r| matches!(r.4, Region::Segment(..))));
        let (x, y, w, h) = region_of(&st, |r| matches!(r, Region::ChoiceNext(0)));
        assert_eq!(
            st.click(x + w / 2.0, y + h / 2.0),
            vec![FormEvent::Chosen("c".into(), 1)]
        );
    }

    #[test]
    fn secret_field_still_masks() {
        let field = |value: &str, secret: bool| Field::Text {
            id: "p".into(),
            label: "Password".into(),
            value: value.into(),
            placeholder: String::new(),
            secret,
        };
        let (_, secret) = painted(vec![field("hunter2", true)]);
        let (_, bullets) = painted(vec![field(&mask("hunter2"), false)]);
        let (_, plain) = painted(vec![field("hunter2", false)]);
        assert_eq!(secret.data(), bullets.data(), "drawn exactly as bullets");
        assert_ne!(secret.data(), plain.data(), "never as the text");
    }

    #[test]
    fn status_rows_draw_a_dot_and_their_button_is_clickable() {
        let (mut st, _) = painted(vec![
            Field::Group("Mail".into()),
            Field::Status {
                label: "Work".into(),
                detail: "online".into(),
                ok: Some(true),
                action: Some(("rm".into(), "Remove".into())),
            },
        ]);
        let (x, y, w, h) = region_of(&st, |r| matches!(r, Region::Button(id) if id == "rm"));
        assert_eq!(
            st.click(x + w / 2.0, y + h / 2.0),
            vec![FormEvent::Button("rm".into())]
        );
        assert_eq!(st.group_rects().len(), 1);
    }
}
