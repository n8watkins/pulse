//! The shortcut recorder as plain logic: what one key press means while the
//! Settings screen is waiting for a new shortcut. No toolkit types, so it is
//! unit tested on any machine.

use pulse_platform::hotkey::{Combo, HotkeyError, Key};

/// The field's text while recording and no modifier is held.
pub const PRESS_KEYS: &str = "Press keys\u{2026}";
/// The help line while recording (the field itself says what is happening).
pub const RECORDING: &str = "Hold two of Ctrl, Alt, Shift and press a key.";
pub const IDLE_NOTE: &str = "Press it anywhere to open or close Pulse.";
pub const OFF_NOTE: &str = "Off. Switch on to use the shortcut.";
pub const HOLD_TWO: &str = "Hold two of Ctrl, Alt and Shift, then a key.";
pub const BAD_KEY: &str = "Use a letter, digit, F-key, Space or an arrow.";

pub const CTRL_ALT: &str = "Ctrl+Alt is AltGr on some keyboards. Add Shift.";

/// The hint inside the field: what clicking it does now.
pub const HINT_IDLE: &str = "Click to change";
pub const HINT_RECORDING: &str = "Esc to cancel";

/// The field's text while recording: the modifiers held so far, then an
/// ellipsis, or a prompt when none is held.
pub fn preview(mods: Mods) -> String {
    let mut parts = Vec::new();
    if mods.ctrl {
        parts.push("Ctrl");
    }
    if mods.alt {
        parts.push("Alt");
    }
    if mods.shift {
        parts.push("Shift");
    }
    if parts.is_empty() {
        PRESS_KEYS.to_owned()
    } else {
        format!("{}+\u{2026}", parts.join("+"))
    }
}

/// How the shortcut field looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// A shortcut that works.
    Idle,
    /// The shortcut is switched off: the remembered keys are dimmed.
    Off,
    /// Waiting for keys.
    Recording,
    /// The last attempt, or the saved shortcut, did not register.
    Error,
}

/// `error` is a note that is showing because a shortcut did not register or a
/// key did not fit; recording wins over everything, then the error.
pub fn field(on: bool, recording: bool, error: bool) -> Field {
    if recording {
        Field::Recording
    } else if error {
        Field::Error
    } else if on {
        Field::Idle
    } else {
        Field::Off
    }
}

/// What a click or Space on the field does: start recording, or (while
/// recording) cancel it, the same toggle as Scribe's.
pub fn click(recording: bool) -> Click {
    if recording {
        Click::Cancel
    } else {
        Click::Start
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    Start,
    Cancel,
}

/// The help line under the field.
pub fn note(on: bool, recording: bool, message: &str) -> &str {
    if !message.is_empty() {
        message
    } else if recording {
        RECORDING
    } else if on {
        IDLE_NOTE
    } else {
        OFF_NOTE
    }
}

/// The note for a shortcut that did not register.
pub fn error_note(e: HotkeyError, combo: Combo) -> String {
    match e {
        HotkeyError::InUse => in_use(combo),
        HotkeyError::Failed => FAILED.to_owned(),
    }
}

pub fn in_use(combo: Combo) -> String {
    format!("{combo} is taken by another app. Try another.")
}

pub const NOT_VALID: &str = "The saved shortcut is not valid. Click it to change.";

pub const FAILED: &str = "Pulse could not register that. Try another.";

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
        assert_eq!(CTRL_ALT, "Ctrl+Alt is AltGr on some keyboards. Add Shift.");
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
            "Alt+Shift+P is taken by another app. Try another."
        );
        assert_eq!(error_note(HotkeyError::Failed, c), FAILED);
    }

    #[test]
    fn every_note_fits_one_short_line() {
        let c = Combo::parse("Ctrl+Alt+Shift+F12").unwrap();
        for text in [
            IDLE_NOTE,
            OFF_NOTE,
            RECORDING,
            HOLD_TWO,
            BAD_KEY,
            CTRL_ALT,
            FAILED,
            &in_use(c),
        ] {
            assert!(text.chars().count() <= 58, "{text}");
        }
    }

    #[test]
    fn preview_lists_the_held_modifiers_in_combo_order() {
        assert_eq!(preview(Mods::default()), PRESS_KEYS);
        assert_eq!(preview(ALT_SHIFT), "Alt+Shift+\u{2026}");
        let all = Mods {
            ctrl: true,
            ..ALT_SHIFT
        };
        assert_eq!(preview(all), "Ctrl+Alt+Shift+\u{2026}");
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        assert_eq!(preview(ctrl), "Ctrl+\u{2026}");
    }

    #[test]
    fn the_field_state_follows_recording_then_error_then_switch() {
        assert_eq!(field(true, false, false), Field::Idle);
        assert_eq!(field(false, false, false), Field::Off);
        assert_eq!(field(true, true, false), Field::Recording);
        // Recording wins over an old error and over the switch being off.
        assert_eq!(field(true, true, true), Field::Recording);
        assert_eq!(field(false, true, false), Field::Recording);
        assert_eq!(field(true, false, true), Field::Error);
        assert_eq!(field(false, false, true), Field::Error);
    }

    #[test]
    fn a_click_starts_recording_and_a_second_click_cancels() {
        assert_eq!(click(false), Click::Start);
        assert_eq!(click(true), Click::Cancel);
    }

    #[test]
    fn the_note_prefers_a_message_then_the_state() {
        assert_eq!(note(true, false, ""), IDLE_NOTE);
        assert_eq!(note(false, false, ""), OFF_NOTE);
        assert_eq!(note(true, true, ""), RECORDING);
        assert_eq!(note(true, true, HOLD_TWO), HOLD_TWO);
        assert_eq!(note(false, false, FAILED), FAILED);
    }

    /// A whole recording, as the app drives it: a click, a lone modifier, a
    /// key too soon, a refused combination, then one that is tried.
    #[test]
    fn a_recording_walks_through_every_outcome() {
        let mut recording = click(false) == Click::Start;
        assert!(recording);
        assert_eq!(step(Mods::default(), Pressed::Modifier), Step::Ignore);
        assert_eq!(preview(ALT_SHIFT), "Alt+Shift+\u{2026}");
        assert_eq!(
            step(Mods::default(), Pressed::Main(Key::Letter('A'))),
            Step::NeedTwo
        );
        let Step::Try(c) = step(ALT_SHIFT, Pressed::Main(Key::F(20))) else {
            panic!()
        };
        assert_eq!(c.to_string(), "Alt+Shift+F20");
        // A refused try ends the recording and shows the error.
        recording = false;
        assert_eq!(field(true, recording, true), Field::Error);
        // Clicking again starts over and clears the error look.
        assert_eq!(click(recording), Click::Start);
        assert_eq!(field(true, true, false), Field::Recording);
    }
}
