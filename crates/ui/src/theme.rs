//! Colours for every Perch surface (Glass design, see
//! docs/superpowers/specs/2026-11-14-glass-ui-design.md). Sizes and type live
//! in [`crate::tokens`].

use perch_core::config::Theme;
use tiny_skia::Color;

/// A two-stop gradient (left/top to right/bottom).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gradient {
    /// Start colour.
    pub from: Color,
    /// End colour.
    pub to: Color,
}

impl Gradient {
    fn hex(from: u32, to: u32) -> Self {
        Self {
            from: hex(from),
            to: hex(to),
        }
    }
}

fn hex(rgb: u32) -> Color {
    Color::from_rgba8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255)
}

fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::from_rgba8(r, g, b, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Resolved colour set for one theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// Glass surface fill (translucent).
    pub surface: Color,
    /// Same as `surface`; kept for surfaces not yet restyled.
    pub bg: Color,
    /// Primary text / icon colour.
    pub fg: Color,
    /// Secondary text (artist, timestamps): `fg` at 62 %.
    pub muted: Color,
    /// Raised elements at three strengths: hover, buttons, active tab.
    pub raised: [Color; 3],
    /// Hairline border.
    pub border: Color,
    /// One-pixel highlight along the top inside edge.
    pub highlight: Color,
    /// Status: fine.
    pub ok: Color,
    /// Status: problem.
    pub error: Color,
    /// Music / generic accent (replaced by album colours when known).
    pub accent: Gradient,
    /// Mail accent.
    pub mail: Gradient,
    /// Done / tasks accent.
    pub tasks: Gradient,
    /// Whether this palette is the dark variant.
    pub dark: bool,
}

fn accent() -> Gradient {
    Gradient::hex(0xFF6F91, 0xFFB199)
}
fn mail() -> Gradient {
    Gradient::hex(0x5B8CFF, 0x8A5BFF)
}
fn tasks() -> Gradient {
    Gradient::hex(0x2BB673, 0x7EE0A1)
}

impl Palette {
    /// Dark glass, the default. `blur` = the OS blurs what is behind the
    /// window, so the fill can be more transparent.
    pub fn dark(blur: bool) -> Self {
        let surface = rgba(18, 20, 28, if blur { 0.58 } else { 0.88 });
        Self {
            surface,
            bg: surface,
            fg: rgba(255, 255, 255, 1.0),
            muted: rgba(255, 255, 255, crate::tokens::SECONDARY_ALPHA),
            raised: [
                rgba(255, 255, 255, 0.08),
                rgba(255, 255, 255, 0.12),
                rgba(255, 255, 255, 0.20),
            ],
            border: rgba(255, 255, 255, 0.16),
            highlight: rgba(255, 255, 255, 0.16),
            ok: hex(0x5BD38A),
            error: hex(0xFF6B6B),
            accent: accent(),
            mail: mail(),
            tasks: tasks(),
            dark: true,
        }
    }

    /// Light glass, for light desktops.
    pub fn light(blur: bool) -> Self {
        let surface = rgba(246, 247, 250, if blur { 0.72 } else { 0.92 });
        Self {
            surface,
            bg: surface,
            fg: rgba(22, 24, 30, 1.0),
            muted: rgba(22, 24, 30, crate::tokens::SECONDARY_ALPHA),
            raised: [
                rgba(0, 0, 0, 0.05),
                rgba(0, 0, 0, 0.08),
                rgba(0, 0, 0, 0.12),
            ],
            border: rgba(0, 0, 0, 0.10),
            highlight: rgba(255, 255, 255, 0.70),
            ok: hex(0x1F9D57),
            error: hex(0xD93838),
            accent: accent(),
            mail: mail(),
            tasks: tasks(),
            dark: false,
        }
    }

    /// Resolve a configured theme. `System` follows `prefers_dark`, which the
    /// platform layer supplies (defaulting to dark when it cannot tell).
    pub fn resolve(theme: Theme, prefers_dark: bool, blur: bool) -> Self {
        match theme {
            Theme::Dark => Self::dark(blur),
            Theme::Light => Self::light(blur),
            Theme::System => {
                if prefers_dark {
                    Self::dark(blur)
                } else {
                    Self::light(blur)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(c: Color) -> (u8, u8, u8) {
        let u = c.to_color_u8();
        (u.red(), u.green(), u.blue())
    }

    #[test]
    fn glass_fill_depends_on_blur() {
        let close = |a: f32, b: f32| (a - b).abs() < 0.01;
        assert!(close(Palette::dark(true).surface.alpha(), 0.58));
        assert!(close(Palette::dark(false).surface.alpha(), 0.88));
        assert!(close(Palette::light(true).surface.alpha(), 0.72));
        assert!(close(Palette::light(false).surface.alpha(), 0.92));
    }

    #[test]
    fn accents_match_the_spec() {
        let p = Palette::dark(false);
        assert_eq!(rgb(p.mail.from), (0x5B, 0x8C, 0xFF));
        assert_eq!(rgb(p.tasks.to), (0x7E, 0xE0, 0xA1));
        assert_eq!(rgb(p.accent.from), (0xFF, 0x6F, 0x91));
    }
}
