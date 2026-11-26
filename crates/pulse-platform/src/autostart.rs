//! Start with Windows: the `Pulse` value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//!
//! The value name can be overridden with `PULSE_AUTOSTART_NAME` (tests).

use std::io;

/// Adds or removes the Run value holding the quoted path of the current exe.
pub fn set(enabled: bool) -> io::Result<()> {
    imp::set(&value_name(), enabled)
}

/// True only when the value exists and points at the current exe.
pub fn is_enabled() -> bool {
    imp::is_enabled(&value_name())
}

fn value_name() -> String {
    match std::env::var("PULSE_AUTOSTART_NAME") {
        Ok(name) if !name.is_empty() => name,
        _ => "Pulse".to_owned(),
    }
}

#[cfg(not(windows))]
mod imp {
    use std::io;

    pub fn set(_name: &str, _enabled: bool) -> io::Result<()> {
        Ok(())
    }

    pub fn is_enabled(_name: &str) -> bool {
        false
    }
}

#[cfg(windows)]
mod imp {
    use std::io;

    pub fn set(_name: &str, _enabled: bool) -> io::Result<()> {
        Ok(())
    }

    pub fn is_enabled(_name: &str) -> bool {
        false
    }
}
