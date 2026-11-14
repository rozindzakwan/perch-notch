//! Glass components: the building blocks every Perch screen is drawn from.
//!
//! All coordinates are physical pixels (logical × scale); sizes inside a
//! component (type, padding) come from [`crate::tokens`] times `ctx.s`.
//! Components draw into the context's pixmap and record hit regions; they
//! never decide what an action does.

mod surface;
mod widgets;

use std::sync::Arc;

use perch_core::view::{Action, Hit, HitKind};
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, Path, Pixmap, Point, SpreadMode, Stroke,
    Transform,
};

use crate::effects::Effects;
use crate::icons::Icon;
use crate::text::TextRenderer;
use crate::theme::{Gradient, Palette};
use crate::tokens::Weight;

pub use surface::{banner, surface};
pub use widgets::{avatar, chip, image, progress, ring, round_button, row, slider, tabs, toggle};

/// What sits at the start of a list row or banner.
#[derive(Debug, Clone, PartialEq)]
pub enum Leading {
    /// Initials on a gradient disc; `seed` picks the gradient.
    Avatar {
        /// One or two letters.
        initials: String,
        /// Stable hash of the sender/app.
        seed: u64,
    },
    /// An icon, optionally on a gradient square.
    Icon(Icon, Option<Gradient>),
    /// A small image (straight RGBA8, square side).
    Thumb(Arc<Vec<u8>>, u32),
    /// Nothing.
    None,
}

/// Everything a component needs to draw.
pub struct Ctx<'a> {
    /// Target.
    pub pm: &'a mut Pixmap,
    /// Fonts.
    pub text: &'a mut TextRenderer,
    /// Shadow / glow cache.
    pub fx: &'a mut Effects,
    /// Colours.
    pub pal: Palette,
    /// Monitor scale.
    pub s: f32,
    /// Cursor position (physical px) for hover states.
    pub cursor: Option<(f32, f32)>,
    /// Opacity applied to everything drawn (content fades).
    pub alpha: f32,
    /// Hit regions recorded so far.
    pub hits: Vec<Hit>,
}

impl<'a> Ctx<'a> {
    /// A context with no cursor, full opacity and no hits yet.
    pub fn new(
        pm: &'a mut Pixmap,
        text: &'a mut TextRenderer,
        fx: &'a mut Effects,
        pal: Palette,
        s: f32,
    ) -> Self {
        Self {
            pm,
            text,
            fx,
            pal,
            s: if s.is_finite() && s > 0.0 { s } else { 1.0 },
            cursor: None,
            alpha: 1.0,
            hits: Vec::new(),
        }
    }

    /// `c` with the context opacity (and `extra`) applied.
    pub fn fade(&self, c: Color, extra: f32) -> Color {
        let mut c = c;
        c.apply_opacity((self.alpha * extra).clamp(0.0, 1.0));
        c
    }

    /// Is the cursor inside this rect?
    pub fn hovered(&self, (x, y, w, h): (f32, f32, f32, f32)) -> bool {
        matches!(self.cursor, Some((cx, cy)) if cx >= x && cy >= y && cx < x + w && cy < y + h)
    }

    /// Record a hit region.
    pub fn hit(&mut self, (x, y, w, h): (f32, f32, f32, f32), kind: HitKind) {
        if w > 0.0 && h > 0.0 {
            self.hits.push(Hit { x, y, w, h, kind });
        }
    }

    /// Record a button.
    pub fn button(&mut self, rect: (f32, f32, f32, f32), action: Action) {
        self.hit(rect, HitKind::Button(action));
    }

    /// Fill a path with a solid colour (context opacity applied).
    pub fn fill(&mut self, path: Option<Path>, color: Color) {
        if let Some(p) = path {
            let paint = solid(self.fade(color, 1.0));
            self.pm
                .fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }

    /// Fill a path with a left-to-right gradient across `(x0, x1)`.
    pub fn fill_gradient(
        &mut self,
        path: Option<Path>,
        g: Gradient,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    ) {
        let Some(p) = path else { return };
        let (from, to) = (self.fade(g.from, 1.0), self.fade(g.to, 1.0));
        let shader = LinearGradient::new(
            Point::from_xy(x0, y0),
            Point::from_xy(x1, y1),
            vec![GradientStop::new(0.0, from), GradientStop::new(1.0, to)],
            SpreadMode::Pad,
            Transform::identity(),
        );
        let paint = match shader {
            Some(shader) => Paint {
                shader,
                anti_alias: true,
                ..Default::default()
            },
            None => solid(from),
        };
        self.pm
            .fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
    }

    /// Stroke a path.
    pub fn stroke(&mut self, path: Option<Path>, color: Color, width: f32) {
        if let Some(p) = path {
            let st = Stroke {
                width,
                line_cap: tiny_skia::LineCap::Round,
                ..Default::default()
            };
            let paint = solid(self.fade(color, 1.0));
            self.pm
                .stroke_path(&p, &paint, &st, Transform::identity(), None);
        }
    }

    /// Rounded rectangle fill.
    pub fn rrect(&mut self, (x, y, w, h): (f32, f32, f32, f32), r: f32, color: Color) {
        self.fill(crate::art::rounded_rect(x, y, w, h, r), color);
    }

    /// Draw an icon.
    pub fn icon(&mut self, icon: Icon, x: f32, y: f32, size: f32, color: Color) {
        let c = self.fade(color, 1.0);
        crate::icons::draw(self.pm, icon, x, y, size, c);
    }

    /// Text vertically centred in the band `[top, top + band)`, elided to
    /// `max_w`. Returns the width drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        s: &str,
        x: f32,
        top: f32,
        band: f32,
        (px, weight): (f32, Weight),
        color: Color,
        max_w: f32,
    ) -> f32 {
        if s.is_empty() || max_w <= 0.0 {
            return 0.0;
        }
        let px = px * self.s;
        let shown = self.text.elide_w(s, px, max_w, weight);
        let m = self.text.measure_w(&shown, px, weight);
        let baseline = top + (band + m.ascent - m.descent) / 2.0;
        let c = self.fade(color, 1.0);
        self.text
            .draw_w(self.pm, &shown, x, baseline, px, c, max_w + 1.0, weight)
    }

    /// Width a string would take (logical size × scale).
    pub fn measure(&mut self, s: &str, (px, weight): (f32, Weight)) -> f32 {
        self.text.measure_w(s, px * self.s, weight).width
    }
}

fn solid(color: Color) -> Paint<'static> {
    let mut p = Paint {
        anti_alias: true,
        ..Default::default()
    };
    p.set_color(color);
    p
}

/// Eight gradient pairs for avatars and app discs.
pub(crate) fn avatar_gradient(seed: u64) -> Gradient {
    const PAIRS: [(u32, u32); 8] = [
        (0x5B8CFF, 0x8A5BFF),
        (0x2BC48A, 0x1B8F9E),
        (0xF5A524, 0xE2553B),
        (0xFF7A59, 0xC4317A),
        (0x3BA7FF, 0x2E62D9),
        (0xB65BFF, 0x7B2FD1),
        (0x14B8A6, 0x0E7490),
        (0xF472B6, 0xDB2777),
    ];
    let (a, b) = PAIRS[(seed % 8) as usize];
    let c = |v: u32| Color::from_rgba8((v >> 16) as u8, (v >> 8) as u8, v as u8, 255);
    Gradient {
        from: c(a),
        to: c(b),
    }
}
