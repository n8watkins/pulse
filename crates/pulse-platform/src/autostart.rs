//! Start with Windows: the `Pulse` value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//!
//! The value name can be overridden with `PULSE_AUTOSTART_NAME` (tests).
//!
//! The value points at one exe. The settings switch and the tray item write it
//! for the running exe on purpose; the automatic check at start never takes it
//! over from another existing copy (see [`sync_action`]).

use std::io;
use std::path::{Path, PathBuf};

/// Adds or removes the Run value holding the quoted path of the current exe.
pub fn set(enabled: bool) -> io::Result<()> {
    imp::set(&value_name(), enabled)
}

/// True only when the value exists and points at the current exe.
pub fn is_enabled() -> bool {
    registered() == Registered::ThisCopy
}

/// The exe the Run value starts (unquoted), when there is a value.
pub fn target() -> Option<PathBuf> {
    imp::target(&value_name())
}

/// Where the Run value points, compared with the running exe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Registered {
    /// No value.
    Missing,
    /// The value starts this exe.
    ThisCopy,
    /// The value starts another exe that exists.
    OtherCopy,
    /// The value starts a file that no longer exists.
    Gone,
}

/// What the automatic check at start does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncAction {
    Nothing,
    Write,
    Remove,
}

pub fn registered() -> Registered {
    match std::env::current_exe() {
        Ok(me) => classify(target().as_deref(), &me, Path::exists),
        Err(_) => Registered::Missing,
    }
}

/// Pure: `exists` says whether a file is there.
pub fn classify(
    target: Option<&Path>,
    current: &Path,
    exists: impl Fn(&Path) -> bool,
) -> Registered {
    match target {
        None => Registered::Missing,
        Some(t) if same_path(t, current) => Registered::ThisCopy,
        Some(t) if !exists(t) => Registered::Gone,
        Some(_) => Registered::OtherCopy,
    }
}

/// What to do at start so the Run value follows the setting without taking
/// it over from another copy of Pulse:
/// - on, and no value or a value for a file that is gone: write it;
/// - on, and the value starts this or another existing copy: leave it;
/// - off, and the value starts this exe: remove it; anything else: leave it.
pub fn sync_action(want: bool, now: Registered) -> SyncAction {
    match (want, now) {
        (true, Registered::Missing | Registered::Gone) => SyncAction::Write,
        (false, Registered::ThisCopy) => SyncAction::Remove,
        _ => SyncAction::Nothing,
    }
}

/// Whether the automatic check at start may run at all. Only the installed
/// exe name is `Pulse.exe`; development builds are `pulse-native.exe`, and
/// neither those nor test runs may touch the user's Run value.
pub fn auto_sync_allowed(exe: &Path, test_run: bool) -> bool {
    let env_test = ["PULSE_SETTINGS_DIR", "PULSE_INSTANCE_NAME"]
        .iter()
        .any(|k| std::env::var_os(k).is_some());
    allowed(exe, test_run || env_test)
}

fn allowed(exe: &Path, test_run: bool) -> bool {
    // Split by hand so a Windows path is read the same way on any host.
    let text = exe.to_string_lossy();
    let name = text.rsplit(['\\', '/']).next().unwrap_or_default();
    !test_run && name.eq_ignore_ascii_case("Pulse.exe")
}

/// Brings the Run value in line with the setting `want`, as far as
/// [`sync_action`] allows. Returns what it did.
pub fn sync(want: bool) -> io::Result<SyncAction> {
    let action = sync_action(want, registered());
    match action {
        SyncAction::Write => set(true)?,
        SyncAction::Remove => set(false)?,
        SyncAction::Nothing => {}
    }
    Ok(action)
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

/// The exe path of a Run command: `"C:\a b\Pulse.exe" --arg` or `C:\a\Pulse.exe`.
fn parse_command(text: &str) -> Option<PathBuf> {
    let t = text.trim();
    let path = if let Some(rest) = t.strip_prefix('"') {
        rest.split('"').next()?
    } else if let Some(i) = t.to_ascii_lowercase().find(".exe") {
        &t[..i + 4]
    } else {
        t
    };
    (!path.is_empty()).then(|| PathBuf::from(path))
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

    pub fn target(_name: &str) -> Option<std::path::PathBuf> {
        None
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

    pub fn target(name: &str) -> Option<std::path::PathBuf> {
        super::parse_command(&read(name)?)
    }

    #[cfg(test)]
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

    fn p(s: &str) -> &Path {
        Path::new(s)
    }

    const ME: &str = r"C:\Users\a\AppData\Local\Programs\Pulse\Pulse.exe";
    const DEV: &str = r"C:\work\target\release\pulse-native.exe";

    #[test]
    fn classify_covers_every_case() {
        let exists = |q: &Path| q == p(ME) || q == p(DEV);
        assert_eq!(classify(None, p(ME), exists), Registered::Missing);
        assert_eq!(classify(Some(p(ME)), p(ME), exists), Registered::ThisCopy);
        // Case does not matter for the same file.
        assert_eq!(
            classify(Some(p(&ME.to_uppercase())), p(ME), exists),
            Registered::ThisCopy
        );
        assert_eq!(classify(Some(p(DEV)), p(ME), exists), Registered::OtherCopy);
        assert_eq!(
            classify(Some(p(r"C:\gone\Pulse.exe")), p(ME), exists),
            Registered::Gone
        );
    }

    #[test]
    fn setting_on_writes_only_when_missing_or_gone() {
        use Registered::*;
        assert_eq!(sync_action(true, Missing), SyncAction::Write);
        assert_eq!(sync_action(true, Gone), SyncAction::Write);
        assert_eq!(sync_action(true, ThisCopy), SyncAction::Nothing);
        // Another existing copy keeps its value.
        assert_eq!(sync_action(true, OtherCopy), SyncAction::Nothing);
    }

    #[test]
    fn setting_off_removes_only_this_copys_value() {
        use Registered::*;
        assert_eq!(sync_action(false, ThisCopy), SyncAction::Remove);
        assert_eq!(sync_action(false, OtherCopy), SyncAction::Nothing);
        assert_eq!(sync_action(false, Gone), SyncAction::Nothing);
        assert_eq!(sync_action(false, Missing), SyncAction::Nothing);
    }

    #[test]
    fn only_the_installed_name_syncs_and_never_a_test_run() {
        assert!(allowed(p(ME), false));
        assert!(allowed(p(r"C:\x\pulse.EXE"), false));
        assert!(!allowed(p(DEV), false));
        assert!(!allowed(p(ME), true));
        assert!(!allowed(p(""), false));
    }

    #[test]
    fn run_commands_are_unquoted() {
        assert_eq!(parse_command(&format!("\"{ME}\"")), Some(PathBuf::from(ME)));
        assert_eq!(
            parse_command(&format!("\"{ME}\" --tray")),
            Some(PathBuf::from(ME))
        );
        assert_eq!(
            parse_command(r"C:\Program Files\P\Pulse.exe /x"),
            Some(PathBuf::from(r"C:\Program Files\P\Pulse.exe"))
        );
        assert_eq!(parse_command(""), None);
        assert_eq!(parse_command("\"\""), None);
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
