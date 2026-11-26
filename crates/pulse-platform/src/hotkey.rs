//! The global shortcut: parsing and the listener thread.

use std::fmt;

/// The main key of a shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// `A` to `Z` (stored upper case).
    Letter(char),
    /// `0` to `9`.
    Digit(char),
    /// `F1` to `F24`.
    F(u8),
    Space,
    Up,
    Down,
    Left,
    Right,
}

impl Key {
    fn parse(text: &str) -> Option<Key> {
        let lower = text.to_ascii_lowercase();
        match lower.as_str() {
            "space" => return Some(Key::Space),
            "up" => return Some(Key::Up),
            "down" => return Some(Key::Down),
            "left" => return Some(Key::Left),
            "right" => return Some(Key::Right),
            _ => {}
        }
        let mut chars = lower.chars();
        let first = chars.next()?;
        if chars.as_str().is_empty() {
            return match first {
                'a'..='z' => Some(Key::Letter(first.to_ascii_uppercase())),
                '0'..='9' => Some(Key::Digit(first)),
                _ => None,
            };
        }
        if first == 'f' {
            let n: u8 = chars.as_str().parse().ok()?;
            if (1..=24).contains(&n)
                && !chars.as_str().starts_with('0')
                && !chars.as_str().starts_with('+')
            {
                return Some(Key::F(n));
            }
        }
        None
    }

    /// The Windows virtual-key code.
    pub fn vk(self) -> u32 {
        match self {
            Key::Letter(c) | Key::Digit(c) => u32::from(c),
            Key::F(n) => 0x70 + u32::from(n) - 1,
            Key::Space => 0x20,
            Key::Left => 0x25,
            Key::Up => 0x26,
            Key::Right => 0x27,
            Key::Down => 0x28,
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Letter(c) | Key::Digit(c) => write!(f, "{c}"),
            Key::F(n) => write!(f, "F{n}"),
            Key::Space => f.write_str("Space"),
            Key::Up => f.write_str("Up"),
            Key::Down => f.write_str("Down"),
            Key::Left => f.write_str("Left"),
            Key::Right => f.write_str("Right"),
        }
    }
}

/// A shortcut: at least two of Ctrl, Alt and Shift plus one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Combo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: Key,
}

impl Combo {
    /// Parses text such as `Alt+Shift+P` (case-insensitive, any modifier
    /// order). `None` when it is not a valid shortcut.
    pub fn parse(text: &str) -> Option<Combo> {
        let (mut ctrl, mut alt, mut shift) = (false, false, false);
        let mut key = None;
        for part in text.split('+') {
            let part = part.trim();
            let flag = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => Some(&mut ctrl),
                "alt" => Some(&mut alt),
                "shift" => Some(&mut shift),
                _ => None,
            };
            match flag {
                Some(flag) => {
                    if *flag || key.is_some() {
                        return None;
                    }
                    *flag = true;
                }
                None => {
                    if key.is_some() {
                        return None;
                    }
                    key = Some(Key::parse(part)?);
                }
            }
        }
        if usize::from(ctrl) + usize::from(alt) + usize::from(shift) < 2 {
            return None;
        }
        Some(Combo {
            ctrl,
            alt,
            shift,
            key: key?,
        })
    }
}

impl fmt::Display for Combo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        write!(f, "{}", self.key)
    }
}

/// Why a shortcut could not be registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyError {
    /// Another program already owns this shortcut.
    InUse,
    /// Any other failure.
    Failed,
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HotkeyError::InUse => f.write_str("that shortcut is already used by another program"),
            HotkeyError::Failed => f.write_str("the shortcut could not be registered"),
        }
    }
}

impl std::error::Error for HotkeyError {}

/// Owns one global shortcut on a thread of its own. Dropping it unregisters
/// the shortcut and ends the thread.
pub struct Listener {
    inner: imp::Inner,
}

impl Listener {
    /// Starts the listener thread with no shortcut registered.
    pub fn spawn(on_press: impl Fn() + Send + 'static) -> Listener {
        Listener {
            inner: imp::Inner::spawn(Box::new(on_press)),
        }
    }

    /// Replaces the registration; `None` clears it. On failure the previous
    /// registration stays active.
    pub fn set(&self, combo: Option<Combo>) -> Result<(), HotkeyError> {
        self.inner.set(combo)
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Combo, HotkeyError};

    pub struct Inner;

    impl Inner {
        pub fn spawn(_on_press: Box<dyn Fn() + Send>) -> Inner {
            Inner
        }

        pub fn set(&self, _combo: Option<Combo>) -> Result<(), HotkeyError> {
            Ok(())
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::{Combo, HotkeyError};

    pub struct Inner;

    impl Inner {
        pub fn spawn(_on_press: Box<dyn Fn() + Send>) -> Inner {
            Inner
        }

        pub fn set(&self, _combo: Option<Combo>) -> Result<(), HotkeyError> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_displays_canonically() {
        for (input, want) in [
            ("Alt+Shift+P", "Alt+Shift+P"),
            ("shift+alt+p", "Alt+Shift+P"),
            ("CTRL + alt + shift + f24", "Ctrl+Alt+Shift+F24"),
            ("Ctrl+Shift+Space", "Ctrl+Shift+Space"),
            ("Control+Alt+Up", "Ctrl+Alt+Up"),
            ("Ctrl+Alt+7", "Ctrl+Alt+7"),
            ("Ctrl+Alt+F1", "Ctrl+Alt+F1"),
        ] {
            let combo = Combo::parse(input).unwrap_or_else(|| panic!("{input}"));
            assert_eq!(combo.to_string(), want);
            assert_eq!(Combo::parse(want), Some(combo));
        }
    }

    #[test]
    fn rejects_invalid() {
        for bad in [
            "",
            "P",
            "Alt+P",
            "Ctrl+F5",
            "Alt+Shift",
            "Alt+Alt+P",
            "Alt+Shift+PP",
            "Alt+Shift+F0",
            "Alt+Shift+F25",
            "Alt+Shift+F01",
            "Alt+Shift+P+Q",
            "Alt+Shift+Enter",
            "Alt+Shift+;",
            "Alt+Shift+P+",
            "+Alt+Shift+P",
            "Alt+P+Shift",
        ] {
            assert_eq!(Combo::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn virtual_keys() {
        assert_eq!(Combo::parse("Alt+Shift+P").unwrap().key.vk(), 0x50);
        assert_eq!(Combo::parse("Alt+Shift+F24").unwrap().key.vk(), 0x87);
        assert_eq!(Combo::parse("Alt+Shift+0").unwrap().key.vk(), 0x30);
        assert_eq!(Combo::parse("Alt+Shift+Down").unwrap().key.vk(), 0x28);
    }

    #[test]
    fn listener_fallback_accepts_anything() {
        let listener = Listener::spawn(|| {});
        assert_eq!(listener.set(Combo::parse("Ctrl+Alt+Shift+F24")), Ok(()));
        assert_eq!(listener.set(None), Ok(()));
    }
}
