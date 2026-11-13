//! Global hotkey accelerators: parsing `"Ctrl+Alt+N"` into a platform-neutral
//! description that the Windows (`RegisterHotKey`) and X11 (`XGrabKey`)
//! backends translate into their own key codes. Pure, so it is unit-tested.

use std::fmt;

/// The key part of an accelerator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// `A`..`Z` (stored upper-case).
    Letter(char),
    /// `0`..`9`.
    Digit(u8),
    /// `F1`..`F24`.
    F(u8),
    /// Space bar.
    Space,
    /// Print Screen.
    PrintScreen,
    /// Pause/Break.
    Pause,
}

/// A parsed accelerator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Accel {
    /// Control.
    pub ctrl: bool,
    /// Alt.
    pub alt: bool,
    /// Shift.
    pub shift: bool,
    /// Windows / Super.
    pub super_key: bool,
    /// The key.
    pub key: Key,
}

/// Why an accelerator string was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccelError {
    /// Nothing given.
    Empty,
    /// A part that is neither a modifier nor a key.
    Unknown(String),
    /// Two non-modifier keys.
    TwoKeys,
    /// Only modifiers.
    NoKey,
    /// A plain letter/digit without a modifier would steal normal typing.
    NeedsModifier,
}

impl fmt::Display for AccelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AccelError::Empty => write!(f, "empty shortcut"),
            AccelError::Unknown(p) => write!(f, "unknown key \"{p}\""),
            AccelError::TwoKeys => write!(f, "a shortcut has exactly one non-modifier key"),
            AccelError::NoKey => write!(f, "a shortcut needs a key besides the modifiers"),
            AccelError::NeedsModifier => {
                write!(
                    f,
                    "letters and digits need Ctrl, Alt or Super, or they would steal typing"
                )
            }
        }
    }
}

impl Accel {
    /// Parse `"Ctrl+Alt+N"`, `"super+shift+s"`, `"Print"`, `"F9"` (case and
    /// spacing insensitive; `+` or `-` separators).
    pub fn parse(s: &str) -> Result<Self, AccelError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(AccelError::Empty);
        }
        let (mut ctrl, mut alt, mut shift, mut super_key) = (false, false, false, false);
        let mut key = None;
        for raw in s.split(['+', '-']).map(str::trim).filter(|p| !p.is_empty()) {
            let p = raw.to_ascii_lowercase();
            match p.as_str() {
                "ctrl" | "control" | "ctl" => ctrl = true,
                "alt" | "option" => alt = true,
                "shift" => shift = true,
                "super" | "win" | "windows" | "meta" | "cmd" => super_key = true,
                _ => {
                    let k = parse_key(&p).ok_or_else(|| AccelError::Unknown(raw.to_string()))?;
                    if key.replace(k).is_some() {
                        return Err(AccelError::TwoKeys);
                    }
                }
            }
        }
        let key = key.ok_or(AccelError::NoKey)?;
        if matches!(key, Key::Letter(_) | Key::Digit(_) | Key::Space) && !(ctrl || alt || super_key)
        {
            return Err(AccelError::NeedsModifier);
        }
        Ok(Self {
            ctrl,
            alt,
            shift,
            super_key,
            key,
        })
    }
}

fn parse_key(p: &str) -> Option<Key> {
    let mut chars = p.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => {
            return Some(Key::Letter(c.to_ascii_uppercase()))
        }
        (Some(c), None) if c.is_ascii_digit() => return Some(Key::Digit(c as u8 - b'0')),
        _ => {}
    }
    match p {
        "space" => Some(Key::Space),
        "print" | "printscreen" | "prtsc" | "prtscr" => Some(Key::PrintScreen),
        "pause" | "break" => Some(Key::Pause),
        _ => {
            let n: u8 = p.strip_prefix('f')?.parse().ok()?;
            (1..=24).contains(&n).then_some(Key::F(n))
        }
    }
}

impl fmt::Display for Accel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".into());
        }
        if self.alt {
            parts.push("Alt".into());
        }
        if self.shift {
            parts.push("Shift".into());
        }
        if self.super_key {
            parts.push("Super".into());
        }
        parts.push(match self.key {
            Key::Letter(c) => c.to_string(),
            Key::Digit(d) => d.to_string(),
            Key::F(n) => format!("F{n}"),
            Key::Space => "Space".into(),
            Key::PrintScreen => "Print".into(),
            Key::Pause => "Pause".into(),
        });
        write!(f, "{}", parts.join("+"))
    }
}

/// A hotkey to register: id (the command verb it triggers) + accelerator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// Command verb, e.g. `"toggle-notch"`, `"shot"`, `"dictate"`.
    pub id: String,
    /// Accelerator.
    pub accel: Accel,
}
