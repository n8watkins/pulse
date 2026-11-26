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
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_APP, WM_HOTKEY, WM_QUIT,
    };

    const ERROR_HOTKEY_ALREADY_REGISTERED: u32 = 1409;
    const WM_COMMAND_READY: u32 = WM_APP + 1;

    type Reply = Sender<Result<(), HotkeyError>>;

    pub struct Inner {
        thread_id: u32,
        commands: Mutex<Sender<(Option<Combo>, Reply)>>,
        /// Id of the active registration, 0 when none (tests post WM_HOTKEY with it).
        #[cfg(test)]
        active_id: Arc<AtomicI32>,
        join: Option<JoinHandle<()>>,
    }

    impl Inner {
        pub fn spawn(on_press: Box<dyn Fn() + Send>) -> Inner {
            let (cmd_tx, cmd_rx) = channel();
            let (id_tx, id_rx) = channel();
            let active_id = Arc::new(AtomicI32::new(0));
            let active = Arc::clone(&active_id);
            #[cfg(not(test))]
            let _ = &active_id;
            let join = std::thread::Builder::new()
                .name("pulse-hotkey".into())
                .spawn(move || run(on_press, cmd_rx, id_tx, active))
                .expect("spawn hotkey thread");
            let thread_id = id_rx.recv().unwrap_or(0);
            Inner {
                thread_id,
                commands: Mutex::new(cmd_tx),
                #[cfg(test)]
                active_id,
                join: Some(join),
            }
        }

        pub fn set(&self, combo: Option<Combo>) -> Result<(), HotkeyError> {
            let (reply_tx, reply_rx) = channel();
            {
                let tx = self.commands.lock().map_err(|_| HotkeyError::Failed)?;
                tx.send((combo, reply_tx))
                    .map_err(|_| HotkeyError::Failed)?;
            }
            // SAFETY: plain thread message with no pointers.
            let posted = unsafe { PostThreadMessageW(self.thread_id, WM_COMMAND_READY, 0, 0) };
            if posted == 0 {
                return Err(HotkeyError::Failed);
            }
            reply_rx.recv().unwrap_or(Err(HotkeyError::Failed))
        }

        /// Test hook: what Windows posts when the shortcut is pressed.
        #[cfg(test)]
        pub fn post_hotkey(&self) -> bool {
            let id = self.active_id.load(Ordering::SeqCst);
            // SAFETY: plain thread message with no pointers.
            id != 0 && unsafe { PostThreadMessageW(self.thread_id, WM_HOTKEY, id as usize, 0) } != 0
        }
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            // SAFETY: plain thread message with no pointers.
            unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    fn modifiers(combo: Combo) -> u32 {
        let mut m = MOD_NOREPEAT;
        if combo.ctrl {
            m |= MOD_CONTROL;
        }
        if combo.alt {
            m |= MOD_ALT;
        }
        if combo.shift {
            m |= MOD_SHIFT;
        }
        m
    }

    fn run(
        on_press: Box<dyn Fn() + Send>,
        commands: Receiver<(Option<Combo>, Reply)>,
        ready: Sender<u32>,
        active: Arc<AtomicI32>,
    ) {
        // SAFETY: a zeroed MSG is valid; PeekMessageW creates this thread's queue
        // so posted thread messages are kept from here on.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            PeekMessageW(&mut msg, std::ptr::null_mut(), WM_APP, WM_APP, PM_NOREMOVE);
            let _ = ready.send(GetCurrentThreadId());
        }
        let mut current: Option<Combo> = None;
        loop {
            // SAFETY: msg is a valid out pointer; a null hwnd reads thread messages.
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            let got = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
            if got <= 0 {
                break;
            }
            match msg.message {
                WM_HOTKEY if msg.wParam as i32 == active.load(Ordering::SeqCst) => on_press(),
                WM_COMMAND_READY => {
                    while let Ok((combo, reply)) = commands.try_recv() {
                        let result = apply(&mut current, combo, &active);
                        let _ = reply.send(result);
                    }
                }
                _ => {}
            }
        }
        if current.is_some() {
            let id = active.swap(0, Ordering::SeqCst);
            // SAFETY: unregisters this thread's own registration.
            unsafe { UnregisterHotKey(std::ptr::null_mut(), id) };
        }
    }

    fn apply(
        current: &mut Option<Combo>,
        wanted: Option<Combo>,
        active: &AtomicI32,
    ) -> Result<(), HotkeyError> {
        if *current == wanted {
            return Ok(());
        }
        let old_id = active.load(Ordering::SeqCst);
        if let Some(combo) = wanted {
            let new_id = if old_id == 1 { 2 } else { 1 };
            // SAFETY: registers for this thread (null hwnd).
            let ok = unsafe {
                RegisterHotKey(
                    std::ptr::null_mut(),
                    new_id,
                    modifiers(combo),
                    combo.key.vk(),
                )
            };
            if ok == 0 {
                // SAFETY: reads this thread's last error.
                return Err(
                    if unsafe { GetLastError() } == ERROR_HOTKEY_ALREADY_REGISTERED {
                        HotkeyError::InUse
                    } else {
                        HotkeyError::Failed
                    },
                );
            }
            active.store(new_id, Ordering::SeqCst);
        } else {
            active.store(0, Ordering::SeqCst);
        }
        if current.is_some() {
            // SAFETY: unregisters this thread's previous registration.
            unsafe { UnregisterHotKey(std::ptr::null_mut(), old_id) };
        }
        *current = wanted;
        Ok(())
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

    #[cfg(not(windows))]
    #[test]
    fn listener_fallback_accepts_anything() {
        let listener = Listener::spawn(|| {});
        assert_eq!(listener.set(Combo::parse("Ctrl+Alt+Shift+F24")), Ok(()));
        assert_eq!(listener.set(None), Ok(()));
    }

    #[cfg(windows)]
    mod windows {
        use super::*;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};

        fn f21() -> Option<Combo> {
            Combo::parse("Ctrl+Alt+Shift+F21")
        }

        fn f24() -> Option<Combo> {
            Combo::parse("Ctrl+Alt+Shift+F24")
        }

        #[test]
        fn set_clear_and_in_use() {
            let first = Listener::spawn(|| {});
            assert_eq!(first.set(f24()), Ok(()));
            assert_eq!(first.set(f24()), Ok(()), "same combo again is a no-op");
            let second = Listener::spawn(|| {});
            assert_eq!(second.set(f24()), Err(HotkeyError::InUse));
            drop(first);
            assert_eq!(second.set(f24()), Ok(()));
            assert_eq!(second.set(None), Ok(()));
        }

        #[test]
        fn failed_set_keeps_previous_registration() {
            let holder = Listener::spawn(|| {});
            let other = Combo::parse("Ctrl+Alt+Shift+F20");
            assert_eq!(holder.set(other), Ok(()));
            let l = Listener::spawn(|| {});
            assert_eq!(l.set(f21()), Ok(()));
            assert_eq!(l.set(other), Err(HotkeyError::InUse));
            // The old registration is still held: another listener cannot take it.
            let third = Listener::spawn(|| {});
            assert_eq!(third.set(f21()), Err(HotkeyError::InUse));
        }

        #[test]
        fn hotkey_message_calls_on_press() {
            let count = Arc::new(AtomicUsize::new(0));
            let c = Arc::clone(&count);
            let l = Listener::spawn(move || {
                c.fetch_add(1, Ordering::SeqCst);
            });
            assert!(!l.inner.post_hotkey(), "nothing registered yet");
            assert_eq!(l.set(Combo::parse("Ctrl+Alt+Shift+F22")), Ok(()));
            assert!(l.inner.post_hotkey());
            let end = Instant::now() + Duration::from_secs(5);
            while count.load(Ordering::SeqCst) == 0 && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(count.load(Ordering::SeqCst), 1);
            assert_eq!(l.set(None), Ok(()));
            assert!(!l.inner.post_hotkey());
        }
    }
}
