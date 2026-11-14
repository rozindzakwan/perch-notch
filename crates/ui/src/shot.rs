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
use crate::icons::Icon;
use crate::text::TextRenderer;
use crate::theme::{Gradient, Palette};
use crate::tokens::Weight;

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
    toolbar: Option<(f32, f32, f32, f32)>,
    hover: Option<(f32, f32)>,
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
            toolbar: None,
            hover: None,
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

    /// The toolbar's outline from the last paint (capture pixels).
    pub fn toolbar_rect(&self) -> Option<(f32, f32, f32, f32)> {
        self.toolbar
    }

    /// The toolbar buttons from the last paint, with stable names:
    /// `pen`, `arrow`, `text`, `ocr`, `copy`, `save`, `cancel`.
    pub fn toolbar_buttons(&self) -> Vec<ToolbarButton> {
        self.buttons
            .iter()
            .map(|(x, y, w, h, b)| ((*x, *y, *w, *h), button_name(*b)))
            .collect()
    }

    /// The cursor moved without a button held. Returns true when the hovered
    /// toolbar button changed (a repaint shows its label).
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let before = self.hover.and_then(|(hx, hy)| self.button_at(hx, hy));
        self.hover = Some((x, y));
        before != self.button_at(x, y)
    }

    fn paint_toolbar(&mut self, pm: &mut Pixmap, text: &mut TextRenderer) {
        let Some((sx, sy, sw, sh)) = self.sel else {
            return;
        };
        // Groups: drawing tools | outputs | cancel.
        let groups: [&[Button]; 3] = [
            &[
                Button::Tool(Tool::Pen),
                Button::Tool(Tool::Arrow),
                Button::Tool(Tool::Text),
            ],
            &[Button::Ocr, Button::Copy, Button::Save],
            &[Button::Cancel],
        ];
        let (b, gap, pad, sep) = (TB_BUTTON, 4.0, 6.0, 13.0);
        let count: usize = groups.iter().map(|g| g.len()).sum();
        let total = 2.0 * pad
            + count as f32 * b
            + (count - groups.len()) as f32 * gap
            + (groups.len() - 1) as f32 * sep;
        let th = b + 2.0 * pad;
        let (pw, ph) = (pm.width() as f32, pm.height() as f32);
        let x0 = (sx + sw - total).clamp(4.0, (pw - total - 4.0).max(4.0));
        let y0 = if sy + sh + th + 16.0 < ph {
            sy + sh + 10.0
        } else if sy - th - 16.0 > 0.0 {
            sy - th - 10.0
        } else {
            sy + sh - th - 12.0
        };
        let pal = Palette::dark(false);
        glass_pill(pm, (x0, y0, total, th), &pal);
        self.toolbar = Some((x0, y0, total, th));
        let hovered = self.hover;
        let mut x = x0 + pad;
        let y = y0 + pad;
        let mut label: Option<(f32, &'static str)> = None;
        for (gi, group) in groups.iter().enumerate() {
            if gi > 0 {
                let lx = x - gap + (sep + gap) / 2.0 - 0.5;
                fill_rect(pm, lx, y + 7.0, 1.0, b - 14.0, pal.border);
                x += sep - gap;
            }
            for btn in group.iter() {
                let active = *btn == Button::Tool(self.tool);
                let hot =
                    hovered.is_some_and(|(hx, hy)| hx >= x && hy >= y && hx < x + b && hy < y + b);
                if active {
                    if let Some(p) = PathBuilder::from_circle(x + b / 2.0, y + b / 2.0, b / 2.0) {
                        fill_gradient(pm, &p, pal.accent, x, x + b);
                    }
                } else if hot {
                    if let Some(p) = PathBuilder::from_circle(x + b / 2.0, y + b / 2.0, b / 2.0) {
                        pm.fill_path(
                            &p,
                            &solid(pal.raised[1]),
                            FillRule::Winding,
                            Transform::identity(),
                            None,
                        );
                    }
                }
                if hot {
                    label = Some((x + b / 2.0, button_label(*btn)));
                }
                let icon = TB_ICON;
                let color = if *btn == Button::Cancel && !hot {
                    pal.muted
                } else {
                    pal.fg
                };
                crate::icons::draw(
                    pm,
                    button_icon(*btn),
                    x + (b - icon) / 2.0,
                    y + (b - icon) / 2.0,
                    icon,
                    color,
                );
                self.buttons.push((x, y, b, b, *btn));
                x += b + gap;
            }
        }
        // The hovered button's name, in a small pill above (or below) the bar.
        if let Some((cx, name)) = label {
            let px = 13.0;
            let m = text.measure_w(name, px, Weight::Medium);
            let (lw, lh) = (m.width + 20.0, 26.0);
            let lx = (cx - lw / 2.0).clamp(4.0, (pw - lw - 4.0).max(4.0));
            let ly = if y0 - lh - 6.0 > 0.0 {
                y0 - lh - 6.0
            } else {
                y0 + th + 6.0
            };
            glass_pill(pm, (lx, ly, lw, lh), &pal);
            text.draw_w(
                pm,
                name,
                lx + 10.0,
                ly + (lh + m.ascent - m.descent) / 2.0,
                px,
                pal.fg,
                lw,
                Weight::Medium,
            );
        }
    }
}

/// A toolbar button: its rect `(x, y, w, h)` and stable name.
pub type ToolbarButton = ((f32, f32, f32, f32), &'static str);

/// Toolbar button side and icon size (capture pixels).
const TB_BUTTON: f32 = 34.0;
const TB_ICON: f32 = 18.0;

fn button_name(b: Button) -> &'static str {
    match b {
        Button::Tool(Tool::Pen) => "pen",
        Button::Tool(Tool::Arrow) => "arrow",
        Button::Tool(Tool::Text) => "text",
        Button::Ocr => "ocr",
        Button::Copy => "copy",
        Button::Save => "save",
        Button::Cancel => "cancel",
    }
}

fn button_label(b: Button) -> &'static str {
    match b {
        Button::Tool(Tool::Pen) => "Pen",
        Button::Tool(Tool::Arrow) => "Arrow",
        Button::Tool(Tool::Text) => "Text",
        Button::Ocr => "Copy text (OCR)",
        Button::Copy => "Copy image",
        Button::Save => "Save",
        Button::Cancel => "Cancel",
    }
}

fn button_icon(b: Button) -> Icon {
    match b {
        Button::Tool(Tool::Pen) => Icon::Pen,
        Button::Tool(Tool::Arrow) => Icon::Arrow,
        Button::Tool(Tool::Text) => Icon::Type,
        Button::Ocr => Icon::Ocr,
        Button::Copy => Icon::Clipboard,
        Button::Save => Icon::Save,
        Button::Cancel => Icon::Close,
    }
}

fn solid(c: Color) -> Paint<'static> {
    let mut p = Paint {
        anti_alias: true,
        ..Default::default()
    };
    p.set_color(c);
    p
}

fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: Color) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pm.fill_rect(r, &solid(c), Transform::identity(), None);
    }
}

fn fill_gradient(pm: &mut Pixmap, path: &tiny_skia::Path, g: Gradient, x0: f32, x1: f32) {
    let shader = tiny_skia::LinearGradient::new(
        tiny_skia::Point::from_xy(x0, 0.0),
        tiny_skia::Point::from_xy(x1, 0.0),
        vec![
            tiny_skia::GradientStop::new(0.0, g.from),
            tiny_skia::GradientStop::new(1.0, g.to),
        ],
        tiny_skia::SpreadMode::Pad,
        Transform::identity(),
    );
    let paint = match shader {
        Some(shader) => Paint {
            shader,
            anti_alias: true,
            ..Default::default()
        },
        None => solid(g.from),
    };
    pm.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
}

/// A dark glass pill: the palette surface, a hairline border and a top
/// highlight, over a soft drop shadow.
fn glass_pill(pm: &mut Pixmap, (x, y, w, h): (f32, f32, f32, f32), pal: &Palette) {
    let r = h / 2.0;
    // Shadow rings drawn entirely outside the shape, so the translucent fill
    // shows the capture rather than the shadow.
    for (grow, a) in [(8.0, 0.06), (4.0, 0.10), (2.0, 0.14)] {
        let half = grow / 2.0;
        if let Some(p) = art::rounded_rect(x - half, y - half, w + grow, h + grow, r + half) {
            let st = Stroke {
                width: grow,
                ..Default::default()
            };
            let c = Color::from_rgba(0.0, 0.0, 0.0, a).unwrap_or(Color::TRANSPARENT);
            pm.stroke_path(&p, &solid(c), &st, Transform::identity(), None);
        }
    }
    if let Some(p) = art::rounded_rect(x, y, w, h, r) {
        pm.fill_path(
            &p,
            &solid(pal.surface),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    if let Some(p) = art::rounded_rect(x + 0.5, y + 0.5, w - 1.0, h - 1.0, r - 0.5) {
        let st = Stroke {
            width: 1.0,
            ..Default::default()
        };
        pm.stroke_path(&p, &solid(pal.border), &st, Transform::identity(), None);
    }
}

fn hint(pm: &mut Pixmap, text: &mut TextRenderer, s: &str) {
    let pal = Palette::dark(false);
    let px = 15.0;
    let m = text.measure_w(s, px, Weight::Medium);
    let (w, h) = (m.width + 36.0, 38.0);
    let x = (pm.width() as f32 - w) / 2.0;
    glass_pill(pm, (x, 24.0, w, h), &pal);
    text.draw_w(
        pm,
        s,
        x + 18.0,
        24.0 + (h + m.ascent - m.descent) / 2.0,
        px,
        pal.fg,
        w,
        Weight::Medium,
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
                    if self.state.hover(self.cursor.0, self.cursor.1) {
                        self.window.request_redraw();
                    }
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
