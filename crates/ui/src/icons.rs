//! Vector icons for the Glass UI, drawn with tiny-skia so they stay sharp at
//! any scale. Each icon is designed on a 24-unit grid and kept inside
//! [2, 22] so its 2-unit stroke never leaves the box.

use perch_core::view::Tab;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform,
};

/// Every icon Perch draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Icon {
    Music,
    Check,
    Mail,
    Clipboard,
    Shelf,
    Speaker,
    Mute,
    Waves,
    Clock,
    Tools,
    Gear,
    Pin,
    Play,
    Pause,
    Next,
    Prev,
    Plus,
    Close,
    Link,
    Image,
    Text,
    Warning,
    Camera,
    Mic,
    Screenshot,
}

impl Icon {
    /// All icons (for tests and previews).
    pub const ALL: &'static [Icon] = &[
        Icon::Music,
        Icon::Check,
        Icon::Mail,
        Icon::Clipboard,
        Icon::Shelf,
        Icon::Speaker,
        Icon::Mute,
        Icon::Waves,
        Icon::Clock,
        Icon::Tools,
        Icon::Gear,
        Icon::Pin,
        Icon::Play,
        Icon::Pause,
        Icon::Next,
        Icon::Prev,
        Icon::Plus,
        Icon::Close,
        Icon::Link,
        Icon::Image,
        Icon::Text,
        Icon::Warning,
        Icon::Camera,
        Icon::Mic,
        Icon::Screenshot,
    ];

    /// The icon shown for a panel tab.
    pub fn for_tab(tab: Tab) -> Icon {
        match tab {
            Tab::Media => Icon::Music,
            Tab::Tasks => Icon::Check,
            Tab::Clipboard => Icon::Clipboard,
            Tab::Shelf => Icon::Shelf,
            Tab::Mixer => Icon::Speaker,
            Tab::Sounds => Icon::Waves,
            Tab::Time => Icon::Clock,
            Tab::Tools => Icon::Tools,
            Tab::Mail => Icon::Mail,
        }
    }
}

/// Grid-to-pixel helper: collects stroked and filled shapes for one icon.
struct G {
    x: f32,
    y: f32,
    u: f32,
    stroke: PathBuilder,
    fill: PathBuilder,
}

impl G {
    fn p(&self, gx: f32, gy: f32) -> (f32, f32) {
        (self.x + gx * self.u, self.y + gy * self.u)
    }
    /// Stroked polyline through grid points.
    fn line(&mut self, pts: &[(f32, f32)]) {
        let mut it = pts.iter();
        if let Some(&(x, y)) = it.next() {
            let (px, py) = self.p(x, y);
            self.stroke.move_to(px, py);
            for &(x, y) in it {
                let (px, py) = self.p(x, y);
                self.stroke.line_to(px, py);
            }
        }
    }
    /// Stroked closed polygon.
    fn poly(&mut self, pts: &[(f32, f32)]) {
        self.line(pts);
        self.stroke.close();
    }
    /// Filled closed polygon.
    fn solid(&mut self, pts: &[(f32, f32)]) {
        let mut it = pts.iter();
        if let Some(&(x, y)) = it.next() {
            let (px, py) = self.p(x, y);
            self.fill.move_to(px, py);
            for &(x, y) in it {
                let (px, py) = self.p(x, y);
                self.fill.line_to(px, py);
            }
            self.fill.close();
        }
    }
    /// Stroked quadratic curve.
    fn quad(&mut self, a: (f32, f32), c: (f32, f32), b: (f32, f32)) {
        let (ax, ay) = self.p(a.0, a.1);
        let (cx, cy) = self.p(c.0, c.1);
        let (bx, by) = self.p(b.0, b.1);
        self.stroke.move_to(ax, ay);
        self.stroke.quad_to(cx, cy, bx, by);
    }
    fn circle(&mut self, cx: f32, cy: f32, r: f32) {
        let (px, py) = self.p(cx, cy);
        self.stroke.push_circle(px, py, r * self.u);
    }
    fn dot(&mut self, cx: f32, cy: f32, r: f32) {
        let (px, py) = self.p(cx, cy);
        self.fill.push_circle(px, py, r * self.u);
    }
    /// Stroked rounded rectangle.
    fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let at = self.p(x, y);
        rounded(&mut self.stroke, at, w * self.u, h * self.u, r * self.u);
    }
    /// Filled rounded rectangle.
    fn rsolid(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let at = self.p(x, y);
        rounded(&mut self.fill, at, w * self.u, h * self.u, r * self.u);
    }
}

fn rounded(pb: &mut PathBuilder, (x, y): (f32, f32), w: f32, h: f32, r: f32) {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = 0.552_284_8 * r;
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
}

fn shape(g: &mut G, icon: Icon) {
    let speaker = [
        (4.0, 9.5),
        (8.0, 9.5),
        (13.0, 5.0),
        (13.0, 19.0),
        (8.0, 14.5),
        (4.0, 14.5),
    ];
    match icon {
        Icon::Music => {
            g.line(&[(8.5, 17.0), (8.5, 6.0), (19.0, 4.0), (19.0, 15.0)]);
            g.dot(6.0, 17.5, 3.0);
            g.dot(16.5, 15.5, 3.0);
        }
        Icon::Check => g.line(&[(5.0, 12.5), (10.0, 17.5), (19.0, 7.0)]),
        Icon::Mail => {
            g.rrect(3.0, 5.5, 18.0, 13.0, 2.5);
            g.line(&[(4.0, 7.0), (12.0, 13.0), (20.0, 7.0)]);
        }
        Icon::Clipboard => {
            g.rrect(5.0, 5.0, 14.0, 16.0, 2.5);
            g.rrect(9.0, 3.0, 6.0, 4.0, 1.5);
        }
        Icon::Shelf => {
            g.rrect(3.0, 4.0, 18.0, 16.0, 3.0);
            g.line(&[
                (3.5, 13.0),
                (8.0, 13.0),
                (9.5, 16.0),
                (14.5, 16.0),
                (16.0, 13.0),
                (20.5, 13.0),
            ]);
        }
        Icon::Speaker => {
            g.solid(&speaker);
            g.quad((16.0, 8.5), (19.0, 12.0), (16.0, 15.5));
        }
        Icon::Mute => {
            g.solid(&speaker);
            g.line(&[(16.0, 9.5), (21.0, 14.5)]);
            g.line(&[(21.0, 9.5), (16.0, 14.5)]);
        }
        Icon::Waves => {
            for y in [7.0_f32, 12.0, 17.0] {
                g.quad((3.0, y), (7.5, y - 3.0), (12.0, y));
                g.quad((12.0, y), (16.5, y + 3.0), (21.0, y));
            }
        }
        Icon::Clock => {
            g.circle(12.0, 12.0, 9.0);
            g.line(&[(12.0, 7.0), (12.0, 12.0), (15.5, 14.0)]);
        }
        Icon::Tools => {
            g.rrect(4.0, 4.0, 7.0, 7.0, 2.0);
            g.rrect(13.0, 4.0, 7.0, 7.0, 2.0);
            g.rrect(4.0, 13.0, 7.0, 7.0, 2.0);
            g.rrect(13.0, 13.0, 7.0, 7.0, 2.0);
        }
        Icon::Gear => {
            g.circle(12.0, 12.0, 6.0);
            g.circle(12.0, 12.0, 2.5);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let (s, c) = a.sin_cos();
                g.line(&[
                    (12.0 + 6.5 * c, 12.0 + 6.5 * s),
                    (12.0 + 9.0 * c, 12.0 + 9.0 * s),
                ]);
            }
        }
        Icon::Pin => {
            g.circle(12.0, 8.0, 4.5);
            g.line(&[(12.0, 12.5), (12.0, 21.0)]);
        }
        Icon::Play => g.solid(&[(8.0, 5.0), (19.0, 12.0), (8.0, 19.0)]),
        Icon::Pause => {
            g.rsolid(6.5, 5.0, 4.0, 14.0, 1.2);
            g.rsolid(13.5, 5.0, 4.0, 14.0, 1.2);
        }
        Icon::Next => {
            g.solid(&[(5.0, 5.0), (15.0, 12.0), (5.0, 19.0)]);
            g.rsolid(16.0, 5.0, 3.0, 14.0, 1.0);
        }
        Icon::Prev => {
            g.solid(&[(19.0, 5.0), (9.0, 12.0), (19.0, 19.0)]);
            g.rsolid(5.0, 5.0, 3.0, 14.0, 1.0);
        }
        Icon::Plus => {
            g.line(&[(12.0, 5.0), (12.0, 19.0)]);
            g.line(&[(5.0, 12.0), (19.0, 12.0)]);
        }
        Icon::Close => {
            g.line(&[(6.0, 6.0), (18.0, 18.0)]);
            g.line(&[(18.0, 6.0), (6.0, 18.0)]);
        }
        Icon::Link => {
            g.rrect(3.0, 8.5, 11.0, 7.0, 3.5);
            g.rrect(10.0, 8.5, 11.0, 7.0, 3.5);
        }
        Icon::Image => {
            g.rrect(3.0, 5.0, 18.0, 14.0, 2.5);
            g.line(&[
                (5.0, 17.0),
                (10.0, 11.5),
                (13.5, 15.0),
                (15.5, 13.0),
                (19.0, 17.0),
            ]);
            g.dot(16.0, 9.0, 1.8);
        }
        Icon::Text => {
            g.line(&[(5.0, 7.0), (19.0, 7.0)]);
            g.line(&[(5.0, 12.0), (19.0, 12.0)]);
            g.line(&[(5.0, 17.0), (14.0, 17.0)]);
        }
        Icon::Warning => {
            g.poly(&[(12.0, 3.5), (21.0, 19.5), (3.0, 19.5)]);
            g.line(&[(12.0, 9.0), (12.0, 13.5)]);
            g.dot(12.0, 16.7, 1.2);
        }
        Icon::Camera => {
            g.rrect(3.0, 7.0, 18.0, 13.0, 3.0);
            g.circle(12.0, 13.5, 3.5);
            g.rsolid(8.0, 4.0, 8.0, 3.0, 1.0);
        }
        Icon::Mic => {
            g.rrect(9.0, 3.0, 6.0, 11.0, 3.0);
            g.quad((6.0, 11.0), (6.0, 17.0), (12.0, 17.0));
            g.quad((12.0, 17.0), (18.0, 17.0), (18.0, 11.0));
            g.line(&[(12.0, 17.0), (12.0, 21.0)]);
        }
        Icon::Screenshot => {
            g.line(&[(4.0, 9.0), (4.0, 4.0), (9.0, 4.0)]);
            g.line(&[(15.0, 4.0), (20.0, 4.0), (20.0, 9.0)]);
            g.line(&[(20.0, 15.0), (20.0, 20.0), (15.0, 20.0)]);
            g.line(&[(9.0, 20.0), (4.0, 20.0), (4.0, 15.0)]);
        }
    }
}

/// Draw `icon` in the square `(x, y, size, size)`.
pub fn draw(pm: &mut Pixmap, icon: Icon, x: f32, y: f32, size: f32, color: Color) {
    if size <= 0.0 || !size.is_finite() {
        return;
    }
    let mut g = G {
        x,
        y,
        u: size / 24.0,
        stroke: PathBuilder::new(),
        fill: PathBuilder::new(),
    };
    shape(&mut g, icon);
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color(color);
    if let Some(p) = g.fill.finish() {
        pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
    }
    if let Some(p) = g.stroke.finish() {
        let st = Stroke {
            width: (size / 12.0).max(1.0),
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        };
        pm.stroke_path(&p, &paint, &st, Transform::identity(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::{Color, Pixmap};

    #[test]
    fn every_icon_draws_inside_its_box() {
        for icon in Icon::ALL {
            for size in [12.0_f32, 16.0, 24.0] {
                for scale in [1.0_f32, 2.0] {
                    let s = size * scale;
                    let side = (s + 40.0) as u32;
                    let mut pm = Pixmap::new(side, side).unwrap_or_else(|| panic!("pixmap"));
                    draw(&mut pm, *icon, 20.0, 20.0, s, Color::WHITE);
                    let mut inside = 0;
                    for y in 0..side {
                        for x in 0..side {
                            let a = pm.pixel(x, y).map(|p| p.alpha()).unwrap_or(0);
                            if a == 0 {
                                continue;
                            }
                            let (fx, fy) = (x as f32, y as f32);
                            let within =
                                fx >= 19.0 && fy >= 19.0 && fx <= 21.0 + s && fy <= 21.0 + s;
                            assert!(
                                within,
                                "{icon:?} at size {s}: pixel ({x},{y}) outside its box"
                            );
                            inside += 1;
                        }
                    }
                    assert!(inside > 0, "{icon:?} at size {s} drew nothing");
                }
            }
        }
    }
}
