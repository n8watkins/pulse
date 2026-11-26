use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    Memory,
    Processor,
}

/// Light or dark: follow the system (the default), or always one of them.
/// The stored words are `system`, `light` and `dark` on every platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    System,
    Light,
    Dark,
}

impl Appearance {
    /// Whether the panel is dark, given whether the system is.
    pub fn is_dark(self, system_dark: bool) -> bool {
        match self {
            Self::System => system_dark,
            Self::Light => false,
            Self::Dark => true,
        }
    }
}

/// Which size the panel has: the full view (default) or the compact one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelView {
    Expanded,
    Compact,
}

impl PanelView {
    /// The other view (what the header button switches to).
    pub fn other(self) -> Self {
        match self {
            Self::Expanded => Self::Compact,
            Self::Compact => Self::Expanded,
        }
    }

    /// `expanded` or `compact` (any case); used by `PULSE_VIEW`.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "expanded" => Some(Self::Expanded),
            "compact" => Some(Self::Compact),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The active shortcut, canonical text such as `Alt+Shift+P`; empty means
    /// off. This is the original field, so an older Pulse reads a new file
    /// the same way.
    pub hotkey: String,
    /// The combination to bring back when the shortcut is switched on again.
    /// Always equal to `hotkey` while the shortcut is on; kept while it is off.
    pub hotkey_last: String,
    pub keep_open: bool,
    pub autostart: bool,
    /// Seconds between refreshes while the panel is hidden: 2, 5 or 10.
    pub hidden_refresh_s: u8,
    pub sort: Sort,
    /// Light, dark, or follow the system.
    pub appearance: Appearance,
    /// The panel view, expanded by default.
    pub view: PanelView,
    /// The compact view's Linux (WSL) list is open. Closed by default; the
    /// expanded view always shows it.
    pub wsl_open: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: DEFAULT_HOTKEY.to_owned(),
            hotkey_last: DEFAULT_HOTKEY.to_owned(),
            keep_open: false,
            autostart: true,
            hidden_refresh_s: 2,
            sort: Sort::Memory,
            appearance: Appearance::System,
            view: PanelView::Expanded,
            wsl_open: false,
        }
    }
}

pub const DEFAULT_HOTKEY: &str = "Alt+Shift+P";

/// `%APPDATA%\Pulse\settings.json`, or `$PULSE_SETTINGS_DIR/settings.json`.
pub fn settings_path() -> PathBuf {
    let dir = match std::env::var_os("PULSE_SETTINGS_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => base_dir().join("Pulse"),
    };
    dir.join("settings.json")
}

fn base_dir() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA").filter(|v| !v.is_empty()) {
        return PathBuf::from(appdata);
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg);
    }
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".config"),
        None => std::env::temp_dir(),
    }
}

impl Settings {
    /// Whether the open/close shortcut is switched on.
    pub fn shortcut_on(&self) -> bool {
        !self.hotkey.is_empty()
    }

    /// The combination the shortcut has, or would have when switched on.
    pub fn remembered_hotkey(&self) -> &str {
        if self.hotkey.is_empty() {
            &self.hotkey_last
        } else {
            &self.hotkey
        }
    }

    /// Sets a new combination and switches the shortcut on.
    pub fn set_shortcut(&mut self, combo: &str) {
        combo.clone_into(&mut self.hotkey);
        combo.clone_into(&mut self.hotkey_last);
    }

    /// Switches the shortcut off and keeps the combination for later.
    pub fn switch_shortcut_off(&mut self) {
        if self.shortcut_on() {
            self.hotkey_last = std::mem::take(&mut self.hotkey);
        }
    }

    /// Reads the settings file; defaults when it is missing or corrupt.
    pub fn load() -> Settings {
        Self::load_from(&settings_path())
    }

    /// Writes the settings file atomically (temp file, then rename).
    pub fn save(&self) -> io::Result<()> {
        self.save_to(&settings_path())
    }

    pub fn load_from(path: &Path) -> Settings {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_json(&text),
            Err(_) => Settings::default(),
        }
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    /// Lenient parse: each field falls back to its default on its own.
    pub fn from_json(text: &str) -> Settings {
        let mut out = Settings::default();
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
            return out;
        };
        if let Some(s) = map.get("hotkey").and_then(Value::as_str) {
            s.trim().clone_into(&mut out.hotkey);
        }
        // A file from before the switch has only `hotkey`: a combination is
        // on, an empty text is off (and brings back the default when it is
        // switched on again).
        match map
            .get("hotkey_last")
            .and_then(Value::as_str)
            .map(str::trim)
        {
            Some(s) if !s.is_empty() => s.clone_into(&mut out.hotkey_last),
            _ => DEFAULT_HOTKEY.clone_into(&mut out.hotkey_last),
        }
        if !out.hotkey.is_empty() {
            out.hotkey_last.clone_from(&out.hotkey);
        }
        if let Some(b) = map.get("keep_open").and_then(Value::as_bool) {
            out.keep_open = b;
        }
        if let Some(b) = map.get("autostart").and_then(Value::as_bool) {
            out.autostart = b;
        }
        if let Some(n) = map.get("hidden_refresh_s").and_then(Value::as_u64) {
            out.hidden_refresh_s = match n {
                2 | 5 | 10 => n as u8,
                _ => 2,
            };
        }
        if let Some(v) = map
            .get("sort")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            out.sort = v;
        }
        if let Some(v) = map
            .get("appearance")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            out.appearance = v;
        }
        if let Some(v) = map
            .get("view")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            out.view = v;
        }
        if let Some(b) = map.get("wsl_open").and_then(Value::as_bool) {
            out.wsl_open = b;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pulse-platform-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn defaults() {
        let s = Settings::default();
        assert_eq!(s.hotkey, "Alt+Shift+P");
        assert!(!s.keep_open);
        assert!(s.autostart);
        assert_eq!(s.hidden_refresh_s, 2);
        assert_eq!(s.sort, Sort::Memory);
        assert_eq!(s.view, PanelView::Expanded);
        assert!(!s.wsl_open);
    }

    #[test]
    fn round_trip() {
        let dir = temp("roundtrip");
        let path = dir.join("settings.json");
        let s = Settings {
            hotkey: String::new(),
            hotkey_last: "Ctrl+Shift+K".to_owned(),
            keep_open: true,
            autostart: false,
            hidden_refresh_s: 10,
            sort: Sort::Processor,
            appearance: Appearance::Dark,
            view: PanelView::Compact,
            wsl_open: true,
        };
        s.save_to(&path).unwrap();
        assert!(!dir.join("settings.json.tmp").exists());
        assert_eq!(Settings::load_from(&path), s);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"dark\"") && text.contains("\"compact\""));
        assert!(
            !text.contains("theme"),
            "the removed color theme is not written"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_removed_color_theme_in_an_old_file_is_ignored_and_dropped_on_save() {
        for old in [
            r#"{"hotkey":"Alt+Shift+P","theme":"mint","sort":"processor"}"#,
            r#"{"hotkey":"Alt+Shift+P","theme":"neutral","sort":"processor"}"#,
            r#"{"hotkey":"Alt+Shift+P","theme":42,"sort":"processor"}"#,
        ] {
            let s = Settings::from_json(old);
            assert_eq!(s.sort, Sort::Processor, "{old}");
            assert_eq!(s.hotkey, "Alt+Shift+P");
            let dir = temp("oldtheme");
            let path = dir.join("settings.json");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, old).unwrap();
            let loaded = Settings::load_from(&path);
            assert_eq!(loaded, s);
            loaded.save_to(&path).unwrap();
            assert!(!std::fs::read_to_string(&path).unwrap().contains("theme"));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn missing_file_gives_defaults() {
        let path = temp("missing").join("settings.json");
        assert_eq!(Settings::load_from(&path), Settings::default());
    }

    #[test]
    fn corrupt_file_gives_defaults() {
        let dir = temp("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        std::fs::write(&path, "[1,2]").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_ignored_missing_defaulted() {
        let s = Settings::from_json(r#"{"theme":"neutral","future":{"a":1}}"#);
        assert_eq!(s.hotkey, "Alt+Shift+P");
        assert_eq!(s.hidden_refresh_s, 2);
    }

    #[test]
    fn bad_field_values_fall_back_individually() {
        let s = Settings::from_json(r#"{"theme":"pink","sort":"processor","keep_open":"yes"}"#);
        assert_eq!(s.sort, Sort::Processor);
        assert!(!s.keep_open);
    }

    #[test]
    fn a_file_from_before_the_views_loads_expanded_with_wsl_closed() {
        // The exact shape older Pulse versions wrote.
        let old = r#"{"hotkey":"Alt+Shift+P","hotkey_last":"Alt+Shift+P","theme":"neutral","keep_open":true,"autostart":false,"hidden_refresh_s":5,"sort":"processor"}"#;
        let s = Settings::from_json(old);
        assert_eq!(s.view, PanelView::Expanded);
        assert!(!s.wsl_open);
        assert_eq!(s.sort, Sort::Processor);
        assert_eq!(s.hidden_refresh_s, 5);
        assert!(s.keep_open && !s.autostart);
    }

    #[test]
    fn the_view_and_the_wsl_state_are_read_and_bad_values_fall_back() {
        let s = Settings::from_json(r#"{"view":"compact","wsl_open":true}"#);
        assert_eq!(s.view, PanelView::Compact);
        assert!(s.wsl_open);
        let s = Settings::from_json(r#"{"view":"huge","wsl_open":"yes","sort":"processor"}"#);
        assert_eq!(s.view, PanelView::Expanded);
        assert!(!s.wsl_open);
        assert_eq!(s.sort, Sort::Processor);
        let v: Value =
            serde_json::from_str(&serde_json::to_string(&Settings::default()).unwrap()).unwrap();
        assert_eq!(v["view"], "expanded");
        assert_eq!(v["wsl_open"], false);
    }

    #[test]
    fn an_old_file_follows_the_system_and_the_words_are_platform_neutral() {
        assert_eq!(Settings::from_json("{}").appearance, Appearance::System);
        let old = r#"{"hotkey":"Alt+Shift+P","theme":"neutral","sort":"memory"}"#;
        assert_eq!(Settings::from_json(old).appearance, Appearance::System);
        for (word, want) in [
            ("system", Appearance::System),
            ("light", Appearance::Light),
            ("dark", Appearance::Dark),
        ] {
            let s = Settings::from_json(&format!(r#"{{"appearance":"{word}"}}"#));
            assert_eq!(s.appearance, want);
            let v: Value = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
            assert_eq!(v["appearance"], word);
        }
        // A bad value falls back on its own, the rest of the file still loads.
        let s = Settings::from_json(r#"{"appearance":"sepia","theme":"neutral"}"#);
        assert_eq!(s.appearance, Appearance::System);
    }

    #[test]
    fn which_theme_follows_from_the_setting_and_the_system() {
        for system_dark in [false, true] {
            assert_eq!(Appearance::System.is_dark(system_dark), system_dark);
            assert!(!Appearance::Light.is_dark(system_dark));
            assert!(Appearance::Dark.is_dark(system_dark));
        }
    }

    #[test]
    fn views_switch_and_parse() {
        assert_eq!(PanelView::Expanded.other(), PanelView::Compact);
        assert_eq!(PanelView::Compact.other(), PanelView::Expanded);
        assert_eq!(PanelView::parse(" Compact "), Some(PanelView::Compact));
        assert_eq!(PanelView::parse("expanded"), Some(PanelView::Expanded));
        assert_eq!(PanelView::parse("tiny"), None);
    }

    #[test]
    fn refresh_forced_to_allowed_values() {
        for (given, want) in [(2, 2), (5, 5), (10, 10), (0, 2), (3, 2), (11, 2), (300, 2)] {
            let s = Settings::from_json(&format!(r#"{{"hidden_refresh_s":{given}}}"#));
            assert_eq!(s.hidden_refresh_s, want, "given {given}");
        }
    }

    #[test]
    fn empty_hotkey_means_off() {
        assert_eq!(Settings::from_json(r#"{"hotkey":""}"#).hotkey, "");
    }

    #[test]
    fn old_file_with_a_combination_loads_as_on() {
        let s = Settings::from_json(r#"{"hotkey":"Ctrl+Shift+K","theme":"neutral"}"#);
        assert!(s.shortcut_on());
        assert_eq!(s.hotkey, "Ctrl+Shift+K");
        assert_eq!(s.hotkey_last, "Ctrl+Shift+K");
        assert_eq!(s.remembered_hotkey(), "Ctrl+Shift+K");
    }

    #[test]
    fn old_file_with_an_empty_hotkey_loads_as_off_with_the_default_remembered() {
        let s = Settings::from_json(r#"{"hotkey":""}"#);
        assert!(!s.shortcut_on());
        assert_eq!(s.hotkey, "");
        assert_eq!(s.remembered_hotkey(), DEFAULT_HOTKEY);
    }

    #[test]
    fn a_file_without_any_hotkey_is_the_default_on() {
        let s = Settings::from_json("{}");
        assert!(s.shortcut_on());
        assert_eq!(s.hotkey, DEFAULT_HOTKEY);
    }

    #[test]
    fn off_keeps_the_combination_and_on_restores_it() {
        let mut s = Settings::from_json(r#"{"hotkey":"Ctrl+Alt+Shift+F12"}"#);
        s.switch_shortcut_off();
        assert!(!s.shortcut_on());
        assert_eq!(s.hotkey, "");
        assert_eq!(s.remembered_hotkey(), "Ctrl+Alt+Shift+F12");
        // Through the file and back: still off, still remembered.
        let again = Settings::from_json(&serde_json::to_string(&s).unwrap());
        assert_eq!(again, s);
        assert!(!again.shortcut_on());
        assert_eq!(again.remembered_hotkey(), "Ctrl+Alt+Shift+F12");
        // Switching on is setting the remembered combination.
        let mut on = again;
        let combo = on.remembered_hotkey().to_owned();
        on.set_shortcut(&combo);
        assert!(on.shortcut_on());
        assert_eq!(on.hotkey, "Ctrl+Alt+Shift+F12");
    }

    #[test]
    fn off_twice_does_not_lose_the_combination() {
        let mut s = Settings::default();
        s.switch_shortcut_off();
        s.switch_shortcut_off();
        assert_eq!(s.remembered_hotkey(), DEFAULT_HOTKEY);
    }

    #[test]
    fn a_new_combination_replaces_the_remembered_one() {
        let mut s = Settings::default();
        s.switch_shortcut_off();
        s.set_shortcut("Alt+Shift+Q");
        assert!(s.shortcut_on());
        assert_eq!(s.hotkey_last, "Alt+Shift+Q");
    }

    #[test]
    fn the_active_combination_wins_over_a_stale_remembered_one() {
        let s = Settings::from_json(r#"{"hotkey":"Alt+Shift+Q","hotkey_last":"Ctrl+Shift+K"}"#);
        assert_eq!(s.hotkey_last, "Alt+Shift+Q");
    }

    #[test]
    fn the_file_keeps_the_original_hotkey_field_for_an_older_pulse() {
        let mut s = Settings::default();
        s.switch_shortcut_off();
        let v: Value = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(v["hotkey"], "");
        assert_eq!(v["hotkey_last"], DEFAULT_HOTKEY);
    }

    #[test]
    fn env_overrides_path() {
        let dir = temp("env");
        // SAFETY: the only test that touches this variable.
        unsafe { std::env::set_var("PULSE_SETTINGS_DIR", &dir) };
        assert_eq!(settings_path(), dir.join("settings.json"));
        let s = Settings {
            sort: Sort::Processor,
            ..Settings::default()
        };
        s.save().unwrap();
        assert_eq!(Settings::load(), s);
        // SAFETY: as above.
        unsafe { std::env::remove_var("PULSE_SETTINGS_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
