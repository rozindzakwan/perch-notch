//! Small form windows (settings, add-to-do), drawn with the same software
//! renderer as the pill.
//!
//! These are ordinary decorated, focusable windows: the pill itself never
//! takes keyboard focus (it is `WS_EX_NOACTIVATE` / a dock), so anything that
//! needs typing happens here. The form is described by a [`FormSpec`] and
//! reports what the user did as [`FormEvent`]s; it holds no app logic.

use std::num::NonZeroU32;
use std::rc::Rc;

use perch_core::error::{Error, Result};
use softbuffer::{Context, Surface};
use tiny_skia::{Color, FillRule, Paint, Pixmap, Stroke, Transform};
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::art;
use crate::text::TextRenderer;
use crate::theme::Palette;

/// One row of a form.
#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    /// Section heading.
    Heading(String),
    /// Muted explanatory text.
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
    /// Pick one of several options; clicking cycles forward, the arrows step.
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
    /// A row of buttons `(id, label)`; the first is the primary action.
    Buttons(Vec<(String, String)>),
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

#[derive(Debug, Clone)]
enum Region {
    Toggle(usize),
    ChoiceNext(usize),
    ChoicePrev(usize),
    Text(usize),
    Button(String),
}

/// Pure form state + layout, separated from the window so it can be tested.
#[derive(Debug, Clone)]
pub struct FormState {
    spec: FormSpec,
    focus: Option<usize>,
    scroll: f32,
    content_h: f32,
    regions: Vec<(f32, f32, f32, f32, Region)>,
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
        }
    }

    /// The spec, with any edits applied.
    pub fn spec(&self) -> &FormSpec {
        &self.spec
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

    /// Feed a click at logical coordinates.
    pub fn click(&mut self, x: f32, y: f32) -> Vec<FormEvent> {
        let y = y + self.scroll;
        let Some(region) = self
            .regions
            .iter()
            .find(|(rx, ry, rw, rh, _)| x >= *rx && y >= *ry && x < rx + rw && y < ry + rh)
            .map(|r| r.4.clone())
        else {
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

    /// Paint into `pm` at `scale` and record click regions.
    pub fn paint(&mut self, pm: &mut Pixmap, text: &mut TextRenderer, pal: &Palette, scale: f32) {
        let s = scale;
        let w = pm.width() as f32 / s;
        pm.fill(opaque(pal.bg));
        self.regions.clear();
        let pad = 18.0;
        let mut y = 14.0;
        let row_h = 30.0;
        let fields = self.spec.fields.clone();
        for (i, field) in fields.iter().enumerate() {
            let oy = y - self.scroll;
            match field {
                Field::Heading(t) => {
                    y += 8.0;
                    draw_text(
                        pm,
                        text,
                        t,
                        pad,
                        y - self.scroll,
                        22.0,
                        14.5,
                        pal.fg,
                        w - 2.0 * pad,
                        s,
                    );
                    y += 26.0;
                }
                Field::Note(t) => {
                    for line in wrap(text, t, 11.5 * s, (w - 2.0 * pad) * s) {
                        draw_text(
                            pm,
                            text,
                            &line,
                            pad,
                            y - self.scroll,
                            16.0,
                            11.5,
                            pal.muted,
                            w - 2.0 * pad,
                            s,
                        );
                        y += 16.0;
                    }
                    y += 4.0;
                }
                Field::Toggle {
                    label,
                    value,
                    disabled,
                    ..
                } => {
                    let color = if *disabled { pal.muted } else { pal.fg };
                    draw_text(
                        pm,
                        text,
                        label,
                        pad,
                        oy,
                        row_h,
                        13.0,
                        color,
                        w - 2.0 * pad - 50.0,
                        s,
                    );
                    let sw = 36.0;
                    let sh = 20.0;
                    let sx = w - pad - sw;
                    let sy = oy + (row_h - sh) / 2.0;
                    let track = if *value && !*disabled {
                        pal.accent
                    } else {
                        with_alpha(pal.fg, 0.18)
                    };
                    fill(
                        pm,
                        art::rounded_rect(sx * s, sy * s, sw * s, sh * s, sh / 2.0 * s),
                        track,
                    );
                    let k = sh - 6.0;
                    let kx = if *value { sx + sw - k - 3.0 } else { sx + 3.0 };
                    fill(
                        pm,
                        art::rounded_rect(kx * s, (sy + 3.0) * s, k * s, k * s, k / 2.0 * s),
                        opaque(pal.bg),
                    );
                    self.regions
                        .push((pad, y, w - 2.0 * pad, row_h, Region::Toggle(i)));
                    y += row_h;
                }
                Field::Choice {
                    label,
                    options,
                    selected,
                    ..
                } => {
                    draw_text(pm, text, label, pad, oy, row_h, 13.0, pal.fg, w * 0.45, s);
                    let value = options.get(*selected).cloned().unwrap_or_default();
                    let bw = (w * 0.48).min(220.0);
                    let bx = w - pad - bw;
                    fill(
                        pm,
                        art::rounded_rect(
                            bx * s,
                            (oy + 4.0) * s,
                            bw * s,
                            (row_h - 8.0) * s,
                            6.0 * s,
                        ),
                        with_alpha(pal.fg, 0.10),
                    );
                    draw_text(
                        pm,
                        text,
                        "\u{2039}",
                        bx + 8.0,
                        oy,
                        row_h,
                        15.0,
                        pal.muted,
                        12.0,
                        s,
                    );
                    draw_text(
                        pm,
                        text,
                        &value,
                        bx + 24.0,
                        oy,
                        row_h,
                        12.5,
                        pal.fg,
                        bw - 48.0,
                        s,
                    );
                    draw_text(
                        pm,
                        text,
                        "\u{203a}",
                        bx + bw - 16.0,
                        oy,
                        row_h,
                        15.0,
                        pal.muted,
                        12.0,
                        s,
                    );
                    self.regions
                        .push((bx, y, 24.0, row_h, Region::ChoicePrev(i)));
                    self.regions
                        .push((bx + 24.0, y, bw - 24.0, row_h, Region::ChoiceNext(i)));
                    y += row_h;
                }
                Field::Text {
                    label,
                    value,
                    placeholder,
                    secret,
                    ..
                } => {
                    draw_text(
                        pm,
                        text,
                        label,
                        pad,
                        oy,
                        20.0,
                        12.0,
                        pal.muted,
                        w - 2.0 * pad,
                        s,
                    );
                    let by = oy + 20.0;
                    let bh = 30.0;
                    let focused = self.focus == Some(i);
                    let border = if focused {
                        pal.accent
                    } else {
                        with_alpha(pal.fg, 0.25)
                    };
                    stroke(
                        pm,
                        art::rounded_rect(pad * s, by * s, (w - 2.0 * pad) * s, bh * s, 6.0 * s),
                        border,
                        1.2 * s,
                    );
                    let (shown, color) = if value.is_empty() {
                        (placeholder.clone(), pal.muted)
                    } else if *secret {
                        (mask(value), pal.fg)
                    } else {
                        (value.clone(), pal.fg)
                    };
                    let tw = draw_text_tail(
                        pm,
                        text,
                        &shown,
                        pad + 8.0,
                        by,
                        bh,
                        13.0,
                        color,
                        w - 2.0 * pad - 16.0,
                        s,
                    );
                    if focused {
                        let cx = if value.is_empty() {
                            pad + 8.0
                        } else {
                            pad + 8.0 + tw
                        };
                        fill(
                            pm,
                            tiny_skia::Rect::from_xywh(
                                cx * s,
                                (by + 7.0) * s,
                                1.5 * s,
                                (bh - 14.0) * s,
                            )
                            .map(tiny_skia::PathBuilder::from_rect),
                            pal.accent,
                        );
                    }
                    self.regions
                        .push((pad, y + 20.0, w - 2.0 * pad, bh, Region::Text(i)));
                    y += 20.0 + bh + 8.0;
                }
                Field::Buttons(buttons) => {
                    y += 6.0;
                    let oy = y - self.scroll;
                    let mut bx = w - pad;
                    for (n, (id, label)) in buttons.iter().enumerate().rev() {
                        let lw = text.measure(label, 13.0 * s).width / s + 28.0;
                        bx -= lw;
                        let bg = if n == 0 {
                            pal.accent
                        } else {
                            with_alpha(pal.fg, 0.12)
                        };
                        let fg = if n == 0 { opaque(pal.bg) } else { pal.fg };
                        fill(
                            pm,
                            art::rounded_rect(bx * s, oy * s, lw * s, 30.0 * s, 8.0 * s),
                            bg,
                        );
                        draw_text(pm, text, label, bx + 14.0, oy, 30.0, 13.0, fg, lw, s);
                        self.regions
                            .push((bx, y, lw, 30.0, Region::Button(id.clone())));
                        bx -= 8.0;
                    }
                    y += 38.0;
                }
            }
        }
        self.content_h = y + 10.0;
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

fn fill(pm: &mut Pixmap, path: Option<tiny_skia::Path>, color: Color) {
    if let Some(p) = path {
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(color);
        pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

fn stroke(pm: &mut Pixmap, path: Option<tiny_skia::Path>, color: Color, width: f32) {
    if let Some(p) = path {
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(color);
        let st = Stroke {
            width,
            ..Default::default()
        };
        pm.stroke_path(&p, &paint, &st, Transform::identity(), None);
    }
}

/// Text in a logical band; returns the logical width drawn.
#[allow(clippy::too_many_arguments)]
fn draw_text(
    pm: &mut Pixmap,
    text: &mut TextRenderer,
    s_: &str,
    x: f32,
    top: f32,
    band: f32,
    px: f32,
    color: Color,
    max_w: f32,
    scale: f32,
) -> f32 {
    if max_w <= 0.0 || s_.is_empty() {
        return 0.0;
    }
    let px = px * scale;
    let shown = text.elide(s_, px, max_w * scale);
    let m = text.measure(&shown, px);
    let baseline = top * scale + (band * scale + m.ascent - m.descent) / 2.0;
    text.draw(pm, &shown, x * scale, baseline, px, color, max_w * scale) / scale
}

/// Like `draw_text`, but keeps the *end* of the string visible (for inputs).
#[allow(clippy::too_many_arguments)]
fn draw_text_tail(
    pm: &mut Pixmap,
    text: &mut TextRenderer,
    s_: &str,
    x: f32,
    top: f32,
    band: f32,
    px: f32,
    color: Color,
    max_w: f32,
    scale: f32,
) -> f32 {
    let pxs = px * scale;
    let mut start = 0;
    let chars: Vec<char> = s_.chars().collect();
    while start < chars.len() {
        let tail: String = chars[start..].iter().collect();
        if text.measure(&tail, pxs).width <= max_w * scale {
            break;
        }
        start += 1;
    }
    let tail: String = chars[start..].iter().collect();
    draw_text(pm, text, &tail, x, top, band, px, color, max_w, scale)
}

fn wrap(text: &mut TextRenderer, s: &str, px: f32, max_w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if text.measure(&candidate, px).width > max_w && !cur.is_empty() {
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
