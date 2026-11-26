//! The shortcut recorder as plain logic: what one key press means while the
//! Settings screen is waiting for a new shortcut. No toolkit types, so it is
//! unit tested on any machine.

use pulse_platform::hotkey::{Combo, Key};

pub const RECORDING: &str = "Press the new keys\u{2026} (Esc cancels)";
pub const HOLD_TWO: &str = "Hold two of Ctrl, Alt and Shift, then press a key. Esc cancels.";
pub const BAD_KEY: &str = "Use a letter, digit, F-key, Space or an arrow key. Esc cancels.";

pub fn in_use(combo: Combo) -> String {
    format!("Another app already uses {combo}. Try another.")
}

pub const FAILED: &str = "Pulse could not register that shortcut. Try another.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Mods {
    fn count(self) -> usize {
        usize::from(self.ctrl) + usize::from(self.alt) + usize::from(self.shift)
    }
}

/// One key press, reduced to what the recorder cares about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pressed {
    Escape,
    /// Ctrl, Alt, Shift or the Windows key on their own.
    Modifier,
    /// A key a shortcut can use.
    Main(Key),
    /// Any other key (Enter, Tab, punctuation...).
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Stop recording, change nothing.
    Cancel,
    /// Keep waiting.
    Ignore,
    /// Fewer than two of Ctrl, Alt and Shift.
    NeedTwo,
    /// Enough modifiers, but a key a shortcut cannot use.
    BadKey,
    /// Try to register this one.
    Try(Combo),
}

pub fn step(mods: Mods, key: Pressed) -> Step {
    match key {
        Pressed::Escape => Step::Cancel,
        Pressed::Modifier => Step::Ignore,
        _ if mods.count() < 2 => Step::NeedTwo,
        Pressed::Other => Step::BadKey,
        Pressed::Main(key) => Step::Try(Combo {
            ctrl: mods.ctrl,
            alt: mods.alt,
            shift: mods.shift,
            key,
        }),
    }
}

/// The key a typed character stands for: a letter or digit.
pub fn key_from_char(c: char) -> Option<Key> {
    match c {
        'a'..='z' | 'A'..='Z' => Some(Key::Letter(c.to_ascii_uppercase())),
        '0'..='9' => Some(Key::Digit(c)),
        ' ' => Some(Key::Space),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALT_SHIFT: Mods = Mods {
        ctrl: false,
        alt: true,
        shift: true,
    };

    #[test]
    fn two_modifiers_and_a_letter_are_tried() {
        let step = step(ALT_SHIFT, Pressed::Main(Key::Letter('Q')));
        let Step::Try(c) = step else { panic!("{step:?}") };
        assert_eq!(c.to_string(), "Alt+Shift+Q");
        let all = Mods {
            ctrl: true,
            ..ALT_SHIFT
        };
        let Step::Try(c) = super::step(all, Pressed::Main(Key::F(12))) else {
            panic!()
        };
        assert_eq!(c.to_string(), "Ctrl+Alt+Shift+F12");
    }

    #[test]
    fn digits_space_and_arrows_work() {
        for (k, want) in [
            (Key::Digit('7'), "Alt+Shift+7"),
            (Key::Space, "Alt+Shift+Space"),
            (Key::Left, "Alt+Shift+Left"),
        ] {
            let Step::Try(c) = step(ALT_SHIFT, Pressed::Main(k)) else {
                panic!()
            };
            assert_eq!(c.to_string(), want);
        }
    }

    #[test]
    fn fewer_than_two_modifiers_asks_for_more() {
        for mods in [
            Mods::default(),
            Mods {
                ctrl: true,
                ..Mods::default()
            },
            Mods {
                shift: true,
                ..Mods::default()
            },
        ] {
            assert_eq!(step(mods, Pressed::Main(Key::Letter('P'))), Step::NeedTwo);
            assert_eq!(step(mods, Pressed::Other), Step::NeedTwo);
        }
    }

    #[test]
    fn escape_cancels_and_a_lone_modifier_keeps_waiting() {
        assert_eq!(step(ALT_SHIFT, Pressed::Escape), Step::Cancel);
        assert_eq!(step(Mods::default(), Pressed::Escape), Step::Cancel);
        assert_eq!(step(Mods::default(), Pressed::Modifier), Step::Ignore);
        assert_eq!(step(ALT_SHIFT, Pressed::Modifier), Step::Ignore);
    }

    #[test]
    fn an_unusable_key_with_enough_modifiers_says_so() {
        assert_eq!(step(ALT_SHIFT, Pressed::Other), Step::BadKey);
    }

    #[test]
    fn characters_map_to_keys() {
        assert_eq!(key_from_char('p'), Some(Key::Letter('P')));
        assert_eq!(key_from_char('P'), Some(Key::Letter('P')));
        assert_eq!(key_from_char('5'), Some(Key::Digit('5')));
        assert_eq!(key_from_char(';'), None);
        assert_eq!(key_from_char('\u{e9}'), None);
    }

    #[test]
    fn messages_match_the_spec() {
        let c = Combo::parse("Alt+Shift+P").unwrap();
        assert_eq!(in_use(c), "Another app already uses Alt+Shift+P. Try another.");
        assert!(HOLD_TWO.starts_with("Hold two of Ctrl, Alt and Shift, then press a key."));
        assert!(RECORDING.contains("Esc cancels"));
    }
}
