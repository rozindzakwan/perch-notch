//! Colours for the pill. Deliberately tiny: the notch is one surface.

use perch_core::config::Theme;
use tiny_skia::Color;

/// Resolved colour set for one theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// Pill background.
    pub bg: Color,
    /// Primary text / icon colour.
    pub fg: Color,
    /// Secondary text (artist, timestamps).
    pub muted: Color,
    /// Accent used for the played part of the seek bar and the mascot eye.
    pub accent: Color,
    /// Hairline border, one physical pixel.
    pub border: Color,
}

impl Palette {
    /// Dark pill, the default look.
    pub fn dark() -> Self {
        Self {
            bg: Color::from_rgba8(16, 17, 21, 248),
            fg: Color::from_rgba8(242, 243, 247, 255),
            muted: Color::from_rgba8(150, 154, 166, 255),
            accent: Color::from_rgba8(255, 176, 66, 255),
            border: Color::from_rgba8(255, 255, 255, 26),
        }
    }

    /// Light pill, for light desktops.
    pub fn light() -> Self {
        Self {
            bg: Color::from_rgba8(248, 248, 250, 240),
            fg: Color::from_rgba8(24, 25, 30, 255),
            muted: Color::from_rgba8(104, 108, 120, 255),
            accent: Color::from_rgba8(201, 120, 12, 255),
            border: Color::from_rgba8(0, 0, 0, 28),
        }
    }

    /// Resolve a configured theme. `System` follows `prefers_dark`, which the
    /// platform layer supplies (defaulting to dark when it cannot tell).
    pub fn resolve(theme: Theme, prefers_dark: bool) -> Self {
        match theme {
            Theme::Dark => Self::dark(),
            Theme::Light => Self::light(),
            Theme::System => {
                if prefers_dark {
                    Self::dark()
                } else {
                    Self::light()
                }
            }
        }
    }
}
