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
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn command() -> io::Result<String> {
        let exe = std::env::current_exe()?;
        Ok(format!("\"{}\"", exe.display()))
    }

    fn check(code: u32) -> io::Result<()> {
        if code == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(code as i32))
        }
    }

    pub fn set(name: &str, enabled: bool) -> io::Result<()> {
        let key_name = wide(RUN_KEY);
        let value_name = wide(name);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: all pointers are valid for the call; the key is closed below.
        unsafe {
            check(RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key_name.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            ))?;
        }
        let result = if enabled {
            let data: Vec<u16> = std::ffi::OsStr::new(&command()?)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            // SAFETY: data is a NUL-terminated UTF-16 buffer of the given byte length.
            check(unsafe {
                RegSetValueExW(
                    key,
                    value_name.as_ptr(),
                    0,
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            })
        } else {
            // SAFETY: valid key and NUL-terminated name.
            let code = unsafe { RegDeleteValueW(key, value_name.as_ptr()) };
            if code == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                check(code)
            }
        };
        // SAFETY: key was opened above.
        unsafe { RegCloseKey(key) };
        result
    }

    fn read(name: &str) -> Option<String> {
        let key_name = wide(RUN_KEY);
        let value_name = wide(name);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: valid pointers; key closed below; buffer length passed in bytes.
        unsafe {
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                key_name.as_ptr(),
                0,
                KEY_QUERY_VALUE,
                &mut key,
            ) != ERROR_SUCCESS
            {
                return None;
            }
            let mut buf = vec![0u16; 2048];
            let mut kind = 0u32;
            let mut len = (buf.len() * 2) as u32;
            let code = RegQueryValueExW(
                key,
                value_name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                buf.as_mut_ptr().cast(),
                &mut len,
            );
            RegCloseKey(key);
            if code != ERROR_SUCCESS || kind != REG_SZ {
                return None;
            }
            let units = (len as usize / 2).min(buf.len());
            let text = &buf[..units];
            let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
            Some(String::from_utf16_lossy(&text[..end]))
        }
    }

    pub fn is_enabled(name: &str) -> bool {
        match (read(name), command()) {
            (Some(have), Ok(want)) => have.eq_ignore_ascii_case(&want),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_value_name_is_pulse() {
        // Reads the env only; never writes the registry.
        if std::env::var_os("PULSE_AUTOSTART_NAME").is_none() {
            assert_eq!(value_name(), "Pulse");
        }
    }

    #[cfg(windows)]
    #[test]
    fn set_is_enabled_unset() {
        let name = format!("Pulse-test-{}", std::process::id());
        assert!(!imp::is_enabled(&name));
        imp::set(&name, true).unwrap();
        let enabled = imp::is_enabled(&name);
        imp::set(&name, false).unwrap();
        assert!(enabled);
        assert!(!imp::is_enabled(&name));
        imp::set(&name, false).unwrap();
    }
}
