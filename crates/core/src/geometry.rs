//! Notch placement geometry: pure math, no OS calls.

use serde::{Deserialize, Serialize};

/// A screen rectangle in physical pixels, in the virtual-desktop coordinate space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width in physical pixels.
    pub w: u32,
    /// Height in physical pixels.
    pub h: u32,
}

impl Rect {
    /// Horizontal center.
    pub fn center_x(&self) -> i32 {
        self.x + self.w as i32 / 2
    }

    /// Vertical center.
    pub fn center_y(&self) -> i32 {
        self.y + self.h as i32 / 2
    }

    /// True when the point lies inside the rect (origin inclusive, far edge exclusive).
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w as i32 && py < self.y + self.h as i32
    }
}

/// A monitor as reported by the window-system backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Monitor {
    /// Backend-assigned stable-ish id.
    pub id: u32,
    /// Left edge in the virtual desktop.
    pub x: i32,
    /// Top edge in the virtual desktop.
    pub y: i32,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// HiDPI scale factor (1.0 = 96 dpi).
    pub scale: f64,
    /// Whether this is the primary monitor.
    pub primary: bool,
    /// The usable area after taskbars/panels (Windows `rcWork`, X11
    /// `_NET_WORKAREA`). `None` = the whole monitor. The notch and its hot zone
    /// hug this area, so a taskbar or a Cinnamon panel on the same edge pushes
    /// the pill below it instead of fighting it for the same pixels.
    pub work: Option<Rect>,
}

impl Monitor {
    /// The full monitor rectangle.
    pub fn full(&self) -> Rect {
        Rect {
            x: self.x,
            y: self.y,
            w: self.width,
            h: self.height,
        }
    }

    /// The rectangle the notch is placed in: the work area when it is known
    /// and sane (non-empty and inside the monitor), otherwise the full monitor.
    pub fn bounds(&self) -> Rect {
        let full = self.full();
        match self.work {
            Some(w)
                if w.w > 0
                    && w.h > 0
                    && w.x >= full.x
                    && w.y >= full.y
                    && w.x + w.w as i32 <= full.x + full.w as i32
                    && w.y + w.h as i32 <= full.y + full.h as i32 =>
            {
                w
            }
            _ => full,
        }
    }
}

/// Which screen edge the notch (and therefore its hot zone) lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// Top of the screen.
    Top,
    /// Bottom of the screen.
    Bottom,
    /// Left of the screen.
    Left,
    /// Right of the screen.
    Right,
}

/// One of eight anchor points for the pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotchPosition {
    /// Top edge, centered horizontally (the default, macOS-notch-like).
    #[default]
    TopCenter,
    /// Top edge, left aligned.
    TopLeft,
    /// Top edge, right aligned.
    TopRight,
    /// Bottom edge, centered.
    BottomCenter,
    /// Bottom edge, left aligned.
    BottomLeft,
    /// Bottom edge, right aligned.
    BottomRight,
    /// Left edge, vertically centered.
    LeftCenter,
    /// Right edge, vertically centered.
    RightCenter,
}

impl NotchPosition {
    /// Every supported position, in settings-menu order.
    pub const ALL: &'static [NotchPosition] = &[
        NotchPosition::TopCenter,
        NotchPosition::TopLeft,
        NotchPosition::TopRight,
        NotchPosition::BottomCenter,
        NotchPosition::BottomLeft,
        NotchPosition::BottomRight,
        NotchPosition::LeftCenter,
        NotchPosition::RightCenter,
    ];

    /// The screen edge this position is anchored to.
    pub fn edge(&self) -> Edge {
        match self {
            NotchPosition::TopCenter | NotchPosition::TopLeft | NotchPosition::TopRight => {
                Edge::Top
            }
            NotchPosition::BottomCenter
            | NotchPosition::BottomLeft
            | NotchPosition::BottomRight => Edge::Bottom,
            NotchPosition::LeftCenter => Edge::Left,
            NotchPosition::RightCenter => Edge::Right,
        }
    }

    /// Stable config-file spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            NotchPosition::TopCenter => "top-center",
            NotchPosition::TopLeft => "top-left",
            NotchPosition::TopRight => "top-right",
            NotchPosition::BottomCenter => "bottom-center",
            NotchPosition::BottomLeft => "bottom-left",
            NotchPosition::BottomRight => "bottom-right",
            NotchPosition::LeftCenter => "left-center",
            NotchPosition::RightCenter => "right-center",
        }
    }

    /// Parse a config-file spelling, returning `None` for unknown values so the
    /// caller can fall back to the default instead of failing.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.as_str() == s)
    }
}

/// Sizing knobs for the pill and its hot zone, in *logical* pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NotchGeometry {
    /// Collapsed `(width, height)`.
    pub collapsed: (u32, u32),
    /// Expanded `(width, height)`.
    pub expanded: (u32, u32),
    /// Gap from the screen edge for non-centered positions.
    pub margin: u32,
    /// Thickness of the invisible hover strip (1-2 px recommended).
    pub hot_zone_thickness: u32,
    /// Extra length added to each side of the hot zone beyond the collapsed pill.
    pub hot_zone_padding: u32,
}

impl Default for NotchGeometry {
    fn default() -> Self {
        Self {
            collapsed: (200, 32),
            expanded: (480, 200),
            margin: 12,
            hot_zone_thickness: 2,
            hot_zone_padding: 40,
        }
    }
}

/// The invisible hover strip that triggers expansion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotZone {
    /// Where to place the strip window.
    pub rect: Rect,
    /// Which edge it hugs.
    pub edge: Edge,
}

fn scaled(v: u32, scale: f64) -> u32 {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let out = (v as f64 * s).round();
    out.clamp(1.0, u32::MAX as f64) as u32
}

/// Clamp `(pos, len)` so the span stays inside `[origin, origin + extent)`.
fn clamp_span(pos: i32, len: u32, origin: i32, extent: u32) -> (i32, u32) {
    let len = len.min(extent.max(1));
    let max = origin + extent as i32 - len as i32;
    (pos.clamp(origin, max.max(origin)), len)
}

/// Compute where the pill window belongs for a monitor/position/state.
pub fn notch_rect(
    monitor: &Monitor,
    position: NotchPosition,
    geo: &NotchGeometry,
    expanded: bool,
) -> Rect {
    let size = if expanded {
        geo.expanded
    } else {
        geo.collapsed
    };
    notch_rect_sized(monitor, position, geo, size)
}

/// Place a pill of an arbitrary logical `size` at the anchor. Used while
/// animating, when the size is interpolated between collapsed and expanded.
pub fn notch_rect_sized(
    monitor: &Monitor,
    position: NotchPosition,
    geo: &NotchGeometry,
    size: (u32, u32),
) -> Rect {
    let (lw, lh) = size;
    let w = scaled(lw, monitor.scale);
    let h = scaled(lh, monitor.scale);
    let margin = scaled(geo.margin, monitor.scale) as i32;

    let b = monitor.bounds();
    let left = b.x;
    let top = b.y;
    let right = b.x + b.w as i32;
    let bottom = b.y + b.h as i32;

    let centered_x = left + (b.w as i32 - w as i32) / 2;
    let centered_y = top + (b.h as i32 - h as i32) / 2;

    let (x, y) = match position {
        NotchPosition::TopCenter => (centered_x, top),
        NotchPosition::TopLeft => (left + margin, top),
        NotchPosition::TopRight => (right - margin - w as i32, top),
        NotchPosition::BottomCenter => (centered_x, bottom - h as i32),
        NotchPosition::BottomLeft => (left + margin, bottom - h as i32),
        NotchPosition::BottomRight => (right - margin - w as i32, bottom - h as i32),
        NotchPosition::LeftCenter => (left, centered_y),
        NotchPosition::RightCenter => (right - w as i32, centered_y),
    };

    let (x, w) = clamp_span(x, w, left, b.w);
    let (y, h) = clamp_span(y, h, top, b.h);
    Rect { x, y, w, h }
}

/// Compute the invisible hover strip for a monitor/position.
pub fn hot_zone_rect(monitor: &Monitor, position: NotchPosition, geo: &NotchGeometry) -> HotZone {
    let edge = position.edge();
    let collapsed = notch_rect(monitor, position, geo, false);
    let thickness = scaled(geo.hot_zone_thickness.max(1), monitor.scale);
    let pad = scaled(geo.hot_zone_padding, monitor.scale);

    let b = monitor.bounds();
    let rect = match edge {
        Edge::Top | Edge::Bottom => {
            let len = collapsed.w.saturating_add(pad.saturating_mul(2));
            let x = collapsed.center_x() - len as i32 / 2;
            let (x, len) = clamp_span(x, len, b.x, b.w);
            let y = if edge == Edge::Top {
                b.y
            } else {
                b.y + b.h as i32 - thickness as i32
            };
            let (y, thickness) = clamp_span(y, thickness, b.y, b.h);
            Rect {
                x,
                y,
                w: len,
                h: thickness,
            }
        }
        Edge::Left | Edge::Right => {
            let len = collapsed.h.saturating_add(pad.saturating_mul(2));
            let y = collapsed.center_y() - len as i32 / 2;
            let (y, len) = clamp_span(y, len, b.y, b.h);
            let x = if edge == Edge::Left {
                b.x
            } else {
                b.x + b.w as i32 - thickness as i32
            };
            let (x, thickness) = clamp_span(x, thickness, b.x, b.w);
            Rect {
                x,
                y,
                w: thickness,
                h: len,
            }
        }
    };

    HotZone { rect, edge }
}

/// Pick the monitor a point falls on, falling back to the primary, then the first.
pub fn monitor_at(monitors: &[Monitor], px: i32, py: i32) -> Option<&Monitor> {
    monitors
        .iter()
        .find(|m| {
            Rect {
                x: m.x,
                y: m.y,
                w: m.width,
                h: m.height,
            }
            .contains(px, py)
        })
        .or_else(|| monitors.iter().find(|m| m.primary))
        .or_else(|| monitors.first())
}
