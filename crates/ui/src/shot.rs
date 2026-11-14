//! The screenshot overlay: a frozen capture of the monitor, region selection,
//! annotation (pen, arrow, text) and a toolbar (OCR, copy, save, cancel).
//!
//! [`ShotState`] holds all logic and painting and is driven by plain method
//! calls, so it is tested without a window; [`ShotWindow`] only feeds it winit
//! events.

use std::num::NonZeroU32;
use std::rc::Rc;

use perch_core::error::{Error, Result};
use softbuffer::{Context, Surface};
use tiny_skia::{
    Color, FillRule, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform,
};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

use crate::art;
use crate::text::TextRenderer;

/// Annotation tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Freehand.
    Pen,
    /// Straight arrow.
    Arrow,
    /// Typed label.
    Text,
}

/// One annotation, in screen (pixmap) coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    /// Freehand stroke.
    Pen(Vec<(f32, f32)>),
    /// Arrow from → to.
    Arrow((f32, f32), (f32, f32)),
    /// Text label anchored at its baseline-left.
    Text((f32, f32), String),
}

/// What the user asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum ShotEvent {
    /// Copy the annotated selection (straight RGBA, width, height).
    Copy(Vec<u8>, u32, u32),
    /// Save it as a file.
    Save(Vec<u8>, u32, u32),
    /// Recognise text in the (un-annotated) selection.
    Ocr(Vec<u8>, u32, u32),
    /// Close without doing anything.
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    Tool(Tool),
    Ocr,
    Copy,
    Save,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Selecting,
    Annotating,
}

const INK: (u8, u8, u8) = (235, 64, 52);
const ACCENT: (u8, u8, u8) = (255, 176, 66);
const TEXT_PX: f32 = 24.0;

/// Overlay state + painting.
pub struct ShotState {
    base: Pixmap,
    phase: Phase,
    anchor: Option<(f32, f32)>,
    sel: Option<(f32, f32, f32, f32)>,
    tool: Tool,
    marks: Vec<Mark>,
    drawing: Option<Mark>,
    typing: Option<((f32, f32), String)>,
    buttons: Vec<(f32, f32, f32, f32, Button)>,
}

fn paint(c: (u8, u8, u8), a: u8) -> Paint<'static> {
    let mut p = Paint {
        anti_alias: true,
        ..Default::default()
    };
    p.set_color(Color::from_rgba8(c.0, c.1, c.2, a));
    p
}

impl ShotState {
    /// Start from a straight-RGBA capture.
    pub fn new(rgba: &[u8], w: u32, h: u32) -> Option<Self> {
        let mut base = Pixmap::new(w, h)?;
        for (dst, src) in base.pixels_mut().iter_mut().zip(rgba.chunks_exact(4)) {
            if let Some(p) = tiny_skia::PremultipliedColorU8::from_rgba(src[0], src[1], src[2], 255)
            {
                *dst = p;
            }
        }
        Some(Self {
            base,
            phase: Phase::Selecting,
            anchor: None,
            sel: None,
            tool: Tool::Pen,
            marks: Vec::new(),
            drawing: None,
            typing: None,
            buttons: Vec::new(),
        })
    }

    /// Size of the capture.
    pub fn size(&self) -> (u32, u32) {
        (self.base.width(), self.base.height())
    }

    /// The selection `(x, y, w, h)`, once made.
    pub fn selection(&self) -> Option<(f32, f32, f32, f32)> {
        self.sel
    }

    /// Current tool.
    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// Choose a tool (the toolbar does this on click).
    pub fn set_tool(&mut self, tool: Tool) {
        self.commit_typing();
        self.tool = tool;
    }

    /// Committed annotations.
    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }

    fn inside_sel(&self, x: f32, y: f32) -> bool {
        self.sel
            .is_some_and(|(sx, sy, sw, sh)| x >= sx && y >= sy && x < sx + sw && y < sy + sh)
    }

    fn button_at(&self, x: f32, y: f32) -> Option<Button> {
        self.buttons
            .iter()
            .find(|(bx, by, bw, bh, _)| x >= *bx && y >= *by && x < bx + bw && y < by + bh)
            .map(|b| b.4)
    }

    fn commit_typing(&mut self) {
        if let Some((at, text)) = self.typing.take() {
            if !text.trim().is_empty() {
                self.marks.push(Mark::Text(at, text));
            }
        }
    }

    /// Mouse button down.
    pub fn press(&mut self, x: f32, y: f32) -> Vec<ShotEvent> {
        match self.phase {
            Phase::Selecting => {
                self.anchor = Some((x, y));
                self.sel = Some((x, y, 0.0, 0.0));
                Vec::new()
            }
            Phase::Annotating => {
                if let Some(b) = self.button_at(x, y) {
                    return self.click(b);
                }
                if !self.inside_sel(x, y) {
                    return Vec::new();
                }
                self.commit_typing();
                match self.tool {
                    Tool::Pen => self.drawing = Some(Mark::Pen(vec![(x, y)])),
                    Tool::Arrow => self.drawing = Some(Mark::Arrow((x, y), (x, y))),
                    Tool::Text => self.typing = Some(((x, y + TEXT_PX * 0.4), String::new())),
                }
                Vec::new()
            }
        }
    }

    /// Mouse moved with the button held.
    pub fn drag(&mut self, x: f32, y: f32) {
        match self.phase {
            Phase::Selecting => {
                if let Some((ax, ay)) = self.anchor {
                    let (w, h) = self.size();
                    let x = x.clamp(0.0, w as f32);
                    let y = y.clamp(0.0, h as f32);
                    self.sel = Some((ax.min(x), ay.min(y), (x - ax).abs(), (y - ay).abs()));
                }
            }
            Phase::Annotating => match &mut self.drawing {
                Some(Mark::Pen(points)) => points.push((x, y)),
                Some(Mark::Arrow(_, to)) => *to = (x, y),
                _ => {}
            },
        }
    }

    /// Mouse button up.
    pub fn release(&mut self, x: f32, y: f32) {
        match self.phase {
            Phase::Selecting => {
                if self.anchor.take().is_some() {
                    self.drag(x, y);
                    // A click without a drag selects the whole monitor.
                    if self.sel.is_none_or(|(_, _, w, h)| w < 4.0 || h < 4.0) {
                        let (w, h) = self.size();
                        self.sel = Some((0.0, 0.0, w as f32, h as f32));
                    }
                    self.phase = Phase::Annotating;
                }
            }
            Phase::Annotating => {
                if let Some(m) = self.drawing.take() {
                    let keep = match &m {
                        Mark::Pen(p) => p.len() > 1,
                        Mark::Arrow(a, b) => (a.0 - b.0).abs() + (a.1 - b.1).abs() > 3.0,
                        Mark::Text(..) => true,
                    };
                    if keep {
                        self.marks.push(m);
                    }
                }
            }
        }
    }

    fn click(&mut self, b: Button) -> Vec<ShotEvent> {
        match b {
            Button::Tool(t) => {
                self.commit_typing();
                self.tool = t;
                Vec::new()
            }
            Button::Copy => self.output(ShotEvent::Copy),
            Button::Save => self.output(ShotEvent::Save),
            Button::Ocr => match self.crop(false) {
                Some((rgba, w, h)) => vec![ShotEvent::Ocr(rgba, w, h)],
                None => Vec::new(),
            },
            Button::Cancel => vec![ShotEvent::Cancel],
        }
    }

    fn output(&mut self, make: fn(Vec<u8>, u32, u32) -> ShotEvent) -> Vec<ShotEvent> {
        self.commit_typing();
        match self.crop(true) {
            Some((rgba, w, h)) => vec![make(rgba, w, h)],
            None => Vec::new(),
        }
    }

    /// Typed text (for the text tool).
    pub fn type_text(&mut self, s: &str) {
        if let Some((_, text)) = &mut self.typing {
            text.extend(s.chars().filter(|c| !c.is_control()));
        }
    }

    /// Backspace.
    pub fn backspace(&mut self) {
        if let Some((_, text)) = &mut self.typing {
            text.pop();
        }
    }

    /// Escape: stop typing, or cancel the whole screenshot.
    pub fn escape(&mut self) -> Vec<ShotEvent> {
        if self.typing.take().is_some() {
            Vec::new()
        } else {
            vec![ShotEvent::Cancel]
        }
    }

    /// Enter: finish the label, or (when not typing) copy.
    pub fn enter(&mut self) -> Vec<ShotEvent> {
        if self.typing.is_some() {
            self.commit_typing();
            Vec::new()
        } else if self.phase == Phase::Annotating {
            self.output(ShotEvent::Copy)
        } else {
            Vec::new()
        }
    }

    /// Crop the selection, optionally with annotations, as straight RGBA.
    pub fn crop(&self, annotated: bool) -> Option<(Vec<u8>, u32, u32)> {
        let (sx, sy, sw, sh) = self.sel?;
        let (w, h) = (sw.round().max(1.0) as u32, sh.round().max(1.0) as u32);
        let mut out = Pixmap::new(w, h)?;
        out.draw_pixmap(
            -(sx.round() as i32),
            -(sy.round() as i32),
            self.base.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
        if annotated {
            let t = Transform::from_translate(-sx.round(), -sy.round());
            let mut text = TextRenderer::new().ok();
            for m in &self.marks {
                draw_mark(&mut out, m, t, text.as_mut());
            }
        }
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for p in out.pixels() {
            let c = p.demultiply();
            rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), 255]);
        }
        Some((rgba, w, h))
    }

    /// Paint the overlay into `pm` (same size as the capture).
    pub fn paint(&mut self, pm: &mut Pixmap, text: &mut TextRenderer) {
        pm.fill(Color::BLACK);
        pm.draw_pixmap(
            0,
            0,
            self.base.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
        // Dim everything, then re-draw the selection undimmed.
        if let Some(r) = Rect::from_xywh(0.0, 0.0, pm.width() as f32, pm.height() as f32) {
            pm.fill_rect(r, &paint((0, 0, 0), 110), Transform::identity(), None);
        }
        if let Some((sx, sy, sw, sh)) = self.sel.filter(|s| s.2 >= 1.0 && s.3 >= 1.0) {
            if let (Some(mut mask), Some(r)) = (
                Mask::new(pm.width(), pm.height()),
                Rect::from_xywh(sx, sy, sw, sh),
            ) {
                mask.fill_path(
                    &PathBuilder::from_rect(r),
                    FillRule::Winding,
                    false,
                    Transform::identity(),
                );
                pm.draw_pixmap(
                    0,
                    0,
                    self.base.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    Some(&mask),
                );
            }
            for m in self.marks.iter().chain(self.drawing.iter()) {
                draw_mark(pm, m, Transform::identity(), Some(text));
            }
            if let Some((at, t)) = &self.typing {
                let shown = format!("{t}|");
                draw_mark(
                    pm,
                    &Mark::Text(*at, shown),
                    Transform::identity(),
                    Some(text),
                );
            }
            if let Some(r) = Rect::from_xywh(sx, sy, sw, sh) {
                let stroke = Stroke {
                    width: 2.0,
                    ..Default::default()
                };
                pm.stroke_path(
                    &PathBuilder::from_rect(r),
                    &paint(ACCENT, 255),
                    &stroke,
                    Transform::identity(),
                    None,
                );
            }
        }
        self.buttons.clear();
        if self.phase == Phase::Selecting {
            hint(
                pm,
                text,
                "Drag to select an area  ·  click for the whole screen  ·  Esc to cancel",
            );
            return;
        }
        self.paint_toolbar(pm, text);
    }

    fn paint_toolbar(&mut self, pm: &mut Pixmap, text: &mut TextRenderer) {
        let Some((sx, sy, sw, sh)) = self.sel else {
            return;
        };
        let items: [(&str, Button); 7] = [
            ("Pen", Button::Tool(Tool::Pen)),
            ("Arrow", Button::Tool(Tool::Arrow)),
            ("Text", Button::Tool(Tool::Text)),
            ("Copy text", Button::Ocr),
            ("Copy", Button::Copy),
            ("Save", Button::Save),
            ("Cancel", Button::Cancel),
        ];
        let px = 14.0;
        let pad = 12.0;
        let bh = 30.0;
        let widths: Vec<f32> = items
            .iter()
            .map(|(l, _)| text.measure(l, px).width + 2.0 * pad)
            .collect();
        let total: f32 = widths.iter().sum::<f32>() + 6.0 * (items.len() as f32 - 1.0) + 12.0;
        let (pw, ph) = (pm.width() as f32, pm.height() as f32);
        let mut x = (sx + sw - total).clamp(4.0, (pw - total - 4.0).max(4.0));
        let y = if sy + sh + bh + 16.0 < ph {
            sy + sh + 8.0
        } else if sy - bh - 16.0 > 0.0 {
            sy - bh - 14.0
        } else {
            sy + sh - bh - 14.0
        };
        if let Some(p) = art::rounded_rect(x, y - 3.0, total, bh + 6.0, 10.0) {
            pm.fill_path(
                &p,
                &paint((22, 23, 28), 240),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        x += 6.0;
        for ((label, b), w) in items.iter().zip(widths) {
            let active = *b == Button::Tool(self.tool);
            if active {
                if let Some(p) = art::rounded_rect(x, y, w, bh, 7.0) {
                    pm.fill_path(
                        &p,
                        &paint(ACCENT, 255),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }
            let color = if active {
                Color::from_rgba8(22, 23, 28, 255)
            } else {
                Color::from_rgba8(242, 243, 247, 255)
            };
            let m = text.measure(label, px);
            text.draw(
                pm,
                label,
                x + pad,
                y + (bh + m.ascent - m.descent) / 2.0,
                px,
                color,
                w,
            );
            self.buttons.push((x, y, w, bh, *b));
            x += w + 6.0;
        }
    }
}

fn hint(pm: &mut Pixmap, text: &mut TextRenderer, s: &str) {
    let px = 16.0;
    let m = text.measure(s, px);
    let w = m.width + 32.0;
    let x = (pm.width() as f32 - w) / 2.0;
    if let Some(p) = art::rounded_rect(x, 24.0, w, 36.0, 12.0) {
        pm.fill_path(
            &p,
            &paint((22, 23, 28), 230),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    text.draw(
        pm,
        s,
        x + 16.0,
        24.0 + (36.0 + m.ascent - m.descent) / 2.0,
        px,
        Color::from_rgba8(242, 243, 247, 255),
        w,
    );
}

fn draw_mark(pm: &mut Pixmap, m: &Mark, t: Transform, text: Option<&mut TextRenderer>) {
    let ink = paint(INK, 255);
    let stroke = Stroke {
        width: 4.0,
        line_cap: tiny_skia::LineCap::Round,
        line_join: tiny_skia::LineJoin::Round,
        ..Default::default()
    };
    match m {
        Mark::Pen(points) => {
            let mut pb = PathBuilder::new();
            for (i, (x, y)) in points.iter().enumerate() {
                if i == 0 {
                    pb.move_to(*x, *y);
                } else {
                    pb.line_to(*x, *y);
                }
            }
            if let Some(p) = pb.finish() {
                pm.stroke_path(&p, &ink, &stroke, t, None);
            }
        }
        Mark::Arrow((x0, y0), (x1, y1)) => {
            let ang = (y1 - y0).atan2(x1 - x0);
            let head = 18.0;
            let mut pb = PathBuilder::new();
            pb.move_to(*x0, *y0);
            pb.line_to(*x1, *y1);
            for side in [0.45f32, -0.45] {
                pb.move_to(*x1, *y1);
                pb.line_to(
                    x1 - head * (ang + side).cos(),
                    y1 - head * (ang + side).sin(),
                );
            }
            if let Some(p) = pb.finish() {
                pm.stroke_path(&p, &ink, &stroke, t, None);
            }
        }
        Mark::Text((x, y), s) => {
            if let Some(text) = text {
                let (tx, ty) = (x + t.tx, y + t.ty);
                text.draw(
                    pm,
                    s,
                    tx,
                    ty,
                    TEXT_PX,
                    Color::from_rgba8(INK.0, INK.1, INK.2, 255),
                    4000.0,
                );
            }
        }
    }
}

/// The overlay as a borderless, always-on-top window covering one monitor.
pub struct ShotWindow {
    window: Rc<Window>,
    surface: Surface<Rc<Window>, Rc<Window>>,
    state: ShotState,
    text: TextRenderer,
    frame: Option<Pixmap>,
    cursor: (f32, f32),
    pressed: bool,
    mods: ModifiersState,
}

impl ShotWindow {
    /// Open over the monitor whose physical origin is `(x, y)`.
    pub fn open(el: &ActiveEventLoop, state: ShotState, x: i32, y: i32) -> Result<Self> {
        let (w, h) = state.size();
        let attrs = Window::default_attributes()
            .with_title("Perch screenshot")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(PhysicalPosition::new(x, y))
            .with_inner_size(PhysicalSize::new(w, h));
        let window = Rc::new(
            el.create_window(attrs)
                .map_err(|e| Error::backend(format!("screenshot window: {e}")))?,
        );
        let ctx =
            Context::new(window.clone()).map_err(|e| Error::backend(format!("softbuffer: {e}")))?;
        let surface = Surface::new(&ctx, window.clone())
            .map_err(|e| Error::backend(format!("softbuffer: {e}")))?;
        window.set_cursor(winit::window::CursorIcon::Crosshair);
        window.focus_window();
        Ok(Self {
            window,
            surface,
            state,
            text: TextRenderer::new()?,
            frame: None,
            cursor: (0.0, 0.0),
            pressed: false,
            mods: ModifiersState::empty(),
        })
    }

    /// Window id.
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// Handle a window event.
    pub fn handle(&mut self, event: &WindowEvent) -> Vec<ShotEvent> {
        let out = match event {
            WindowEvent::CloseRequested => return vec![ShotEvent::Cancel],
            WindowEvent::RedrawRequested => {
                self.paint();
                return Vec::new();
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                return Vec::new();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                if !self.pressed {
                    return Vec::new();
                }
                self.state.drag(self.cursor.0, self.cursor.1);
                Vec::new()
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => {
                    self.pressed = true;
                    self.state.press(self.cursor.0, self.cursor.1)
                }
                ElementState::Released => {
                    self.pressed = false;
                    self.state.release(self.cursor.0, self.cursor.1);
                    Vec::new()
                }
            },
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => vec![ShotEvent::Cancel],
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => self.state.escape(),
                    Key::Named(NamedKey::Enter) => self.state.enter(),
                    Key::Named(NamedKey::Backspace) => {
                        self.state.backspace();
                        Vec::new()
                    }
                    Key::Character(c) if self.mods.control_key() && c.eq_ignore_ascii_case("c") => {
                        self.state.enter()
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
        let (w, h) = self.state.size();
        let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else {
            return;
        };
        if self.frame.as_ref().map(|p| (p.width(), p.height())) != Some((w, h)) {
            self.frame = Pixmap::new(w, h);
        }
        let Some(pm) = self.frame.as_mut() else {
            return;
        };
        self.state.paint(pm, &mut self.text);
        if self.surface.resize(nw, nh).is_err() {
            return;
        }
        let Ok(mut buf) = self.surface.buffer_mut() else {
            return;
        };
        for (dst, src) in buf.iter_mut().zip(pm.pixels()) {
            *dst = (u32::from(src.red()) << 16)
                | (u32::from(src.green()) << 8)
                | u32::from(src.blue());
        }
        let _ = buf.present();
    }
}
