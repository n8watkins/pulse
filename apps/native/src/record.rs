//! The shortcut recorder as plain logic: what one key press means while the
//! Settings screen is waiting for a new shortcut. No toolkit types, so it is
//! unit tested on any machine.

use pulse_platform::hotkey::{Combo, HotkeyError, Key};

pub const RECORDING: &str = "Press the new keys\u{2026} (Esc cancels)";
pub const HOLD_TWO: &str = "Hold two of Ctrl, Alt and Shift, then press a key. Esc cancels.";
pub const BAD_KEY: &str = "Use a letter, digit, F-key, Space or an arrow key. Esc cancels.";

pub const CTRL_ALT: &str =
    "Ctrl+Alt is AltGr on some keyboards. Add Shift, or use Ctrl+Shift or Alt+Shift.";

/// The note for a shortcut that did not register.
pub fn error_note(e: HotkeyError, combo: Combo) -> String {
    match e {
        HotkeyError::InUse => in_use(combo),
        HotkeyError::Failed => FAILED.to_owned(),
    }
}

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
    /// Ctrl and Alt without Shift: that is AltGr on many keyboards.
    CtrlAlt,
    /// Try to register this one.
    Try(Combo),
}

pub fn step(mods: Mods, key: Pressed) -> Step {
    match key {
        Pressed::Escape => Step::Cancel,
        Pressed::Modifier => Step::Ignore,
        _ if mods.count() < 2 => Step::NeedTwo,
        _ if mods.ctrl && mods.alt && !mods.shift => Step::CtrlAlt,
        Pressed::Other => Step::BadKey,
        Pressed::Main(key) => Step::Try(Combo {
            ctrl: mods.ctrl,
            alt: mods.alt,
            shift: mods.shift,
            key,
        }),
    }
}

/// What a key press means, from the physical key and the letter the layout
/// gives it. `code` is winit's `KeyCode` name (`KeyA`, `Digit1`, `F5`,
/// `ArrowUp`, `Space`, `NumpadAdd`...); `latin` is the key's character without
/// modifiers when it is an ASCII letter.
///
/// Digits, F keys, arrows and Space come from the physical key, so a French
/// layout's digit row (which types `&` and `\u{e9}` unshifted) and non-Latin
/// layouts work. A letter follows the layout when it types a Latin one (the
/// shortcut is registered by virtual key, which is the typed letter: on AZERTY
/// the key marked A is `VK_A`), and otherwise the physical key's letter (the
/// virtual keys of Cyrillic and Greek layouts are the Latin keys in the same
/// places). The number pad is not usable.
pub fn pressed_from_code(code: &str, latin: Option<char>) -> Pressed {
    match code {
        "Escape" => return Pressed::Escape,
        "ShiftLeft" | "ShiftRight" | "ControlLeft" | "ControlRight" | "AltLeft" | "AltRight"
        | "SuperLeft" | "SuperRight" | "Meta" | "Hyper" | "Fn" | "FnLock" => {
            return Pressed::Modifier;
        }
        _ => {}
    }
    if let Some(c) = latin.filter(char::is_ascii_alphabetic) {
        return Pressed::Main(Key::Letter(c.to_ascii_uppercase()));
    }
    if let Some(letter) = code.strip_prefix("Key").and_then(single_char) {
        return if letter.is_ascii_alphabetic() {
            Pressed::Main(Key::Letter(letter.to_ascii_uppercase()))
        } else {
            Pressed::Other
        };
    }
    if let Some(d) = code.strip_prefix("Digit").and_then(single_char) {
        return if d.is_ascii_digit() {
            Pressed::Main(Key::Digit(d))
        } else {
            Pressed::Other
        };
    }
    match code {
        "Space" => Pressed::Main(Key::Space),
        "ArrowUp" => Pressed::Main(Key::Up),
        "ArrowDown" => Pressed::Main(Key::Down),
        "ArrowLeft" => Pressed::Main(Key::Left),
        "ArrowRight" => Pressed::Main(Key::Right),
        _ => code
            .strip_prefix('F')
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|n| n.parse::<u8>().ok())
            .filter(|n| (1..=24).contains(n))
            .map_or(Pressed::Other, |n| Pressed::Main(Key::F(n))),
    }
}

fn single_char(s: &str) -> Option<char> {
    let mut it = s.chars();
    let c = it.next()?;
    it.next().is_none().then_some(c)
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
        let Step::Try(c) = step else {
            panic!("{step:?}")
        };
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
    fn ctrl_alt_without_shift_is_refused() {
        let ctrl_alt = Mods {
            ctrl: true,
            alt: true,
            shift: false,
        };
        assert_eq!(
            step(ctrl_alt, Pressed::Main(Key::Letter('P'))),
            Step::CtrlAlt
        );
        assert_eq!(step(ctrl_alt, Pressed::Other), Step::CtrlAlt);
        // Modifier presses and Esc still behave.
        assert_eq!(step(ctrl_alt, Pressed::Modifier), Step::Ignore);
        assert_eq!(step(ctrl_alt, Pressed::Escape), Step::Cancel);
        // Adding Shift, or using another pair, is fine.
        let all = Mods {
            shift: true,
            ..ctrl_alt
        };
        assert!(matches!(
            step(all, Pressed::Main(Key::Letter('P'))),
            Step::Try(_)
        ));
        let ctrl_shift = Mods {
            ctrl: true,
            alt: false,
            shift: true,
        };
        assert!(matches!(
            step(ctrl_shift, Pressed::Main(Key::Digit('1'))),
            Step::Try(_)
        ));
        assert_eq!(
            CTRL_ALT,
            "Ctrl+Alt is AltGr on some keyboards. Add Shift, or use Ctrl+Shift or Alt+Shift."
        );
    }

    #[test]
    fn physical_keys_map_to_shortcut_keys() {
        let main = |code, latin| pressed_from_code(code, latin);
        assert_eq!(main("KeyP", Some('p')), Pressed::Main(Key::Letter('P')));
        // AZERTY digit row: the unshifted character is not a digit.
        assert_eq!(main("Digit1", None), Pressed::Main(Key::Digit('1')));
        assert_eq!(main("Digit0", None), Pressed::Main(Key::Digit('0')));
        // AZERTY: the key in the Q position types A, which is VK_A.
        assert_eq!(main("KeyQ", Some('a')), Pressed::Main(Key::Letter('A')));
        // Russian layout: the letter is Cyrillic, so the physical key counts.
        assert_eq!(main("KeyF", None), Pressed::Main(Key::Letter('F')));
        assert_eq!(main("F1", None), Pressed::Main(Key::F(1)));
        assert_eq!(main("F24", None), Pressed::Main(Key::F(24)));
        assert_eq!(main("F25", None), Pressed::Other);
        assert_eq!(main("F0", None), Pressed::Other);
        assert_eq!(main("Space", None), Pressed::Main(Key::Space));
        assert_eq!(main("ArrowLeft", None), Pressed::Main(Key::Left));
        assert_eq!(main("ArrowDown", None), Pressed::Main(Key::Down));
        assert_eq!(main("ArrowUp", None), Pressed::Main(Key::Up));
        assert_eq!(main("ArrowRight", None), Pressed::Main(Key::Right));
    }

    #[test]
    fn the_number_pad_and_other_keys_are_not_usable() {
        for code in [
            "Numpad0",
            "Numpad5",
            "NumpadAdd",
            "NumpadEnter",
            "Enter",
            "Tab",
            "Semicolon",
            "Minus",
            "Backspace",
            "Unidentified",
            "Home",
        ] {
            assert_eq!(pressed_from_code(code, None), Pressed::Other, "{code}");
        }
        // The pad's digits type digits, never letters.
        assert_eq!(pressed_from_code("Numpad1", None), Pressed::Other);
    }

    #[test]
    fn escape_and_modifier_keys_are_recognised_by_code() {
        assert_eq!(pressed_from_code("Escape", None), Pressed::Escape);
        for code in [
            "ShiftLeft",
            "ShiftRight",
            "ControlLeft",
            "ControlRight",
            "AltLeft",
            "AltRight",
            "SuperLeft",
            "SuperRight",
        ] {
            assert_eq!(pressed_from_code(code, None), Pressed::Modifier, "{code}");
        }
    }

    #[test]
    fn error_notes_name_the_combo() {
        let c = Combo::parse("Alt+Shift+P").unwrap();
        assert_eq!(
            error_note(HotkeyError::InUse, c),
            "Another app already uses Alt+Shift+P. Try another."
        );
        assert_eq!(error_note(HotkeyError::Failed, c), FAILED);
    }

    #[test]
    fn messages_match_the_spec() {
        let c = Combo::parse("Alt+Shift+P").unwrap();
        assert_eq!(
            in_use(c),
            "Another app already uses Alt+Shift+P. Try another."
        );
        assert!(HOLD_TWO.starts_with("Hold two of Ctrl, Alt and Shift, then press a key."));
        assert!(RECORDING.contains("Esc cancels"));
    }
}
