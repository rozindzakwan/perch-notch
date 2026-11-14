//! Sizes, spacing and type for the Glass design (logical pixels; multiply by
//! the monitor scale). Colours live in [`crate::theme`].

/// Font weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    /// Regular (400).
    Regular,
    /// Medium (500).
    Medium,
    /// Bold (700).
    Bold,
}

/// Corner radii.
pub mod radius {
    /// Expanded panel.
    pub const PANEL: f32 = 26.0;
    /// Notification banner.
    pub const BANNER: f32 = 22.0;
    /// List rows (hover background).
    pub const ROW: f32 = 14.0;
    /// Album cover.
    pub const COVER: f32 = 16.0;
}

/// Spacing (4-pt grid).
pub mod space {
    /// Panel padding.
    pub const PAD: f32 = 12.0;
    /// One-line row height.
    pub const ROW1: f32 = 34.0;
    /// Two-line row height.
    pub const ROW2: f32 = 42.0;
}

/// Type scale: (size, weight).
pub mod text {
    use super::Weight;
    /// Titles (track name, section titles).
    pub const TITLE: (f32, Weight) = (15.0, Weight::Bold);
    /// Body text.
    pub const BODY: (f32, Weight) = (12.5, Weight::Medium);
    /// Secondary text (drawn at [`super::SECONDARY_ALPHA`]).
    pub const SECONDARY: (f32, Weight) = (12.0, Weight::Regular);
    /// Captions, times, badges.
    pub const CAPTION: (f32, Weight) = (10.5, Weight::Medium);
}

/// Opacity of secondary text.
pub const SECONDARY_ALPHA: f32 = 0.62;
