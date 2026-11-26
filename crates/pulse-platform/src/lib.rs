//! Platform pieces for Pulse: the settings file, the global shortcut, start
//! with Windows and single instance. Everything has a harmless fallback off
//! Windows so the crate builds and its pure parts test on Linux.

pub mod autostart;
pub mod hotkey;
pub mod single;

mod settings;

pub use settings::{Settings, Sort, Theme, settings_path};

/// Whether the user wants animations (Windows "Animation effects" setting).
/// True off Windows.
pub fn animations_enabled() -> bool {
    imp::animations_enabled()
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::BOOL;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };

    pub fn animations_enabled() -> bool {
        let mut on: BOOL = 1;
        // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL through the pointer.
        let ok = unsafe {
            SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut on).cast(), 0)
        };
        ok == 0 || on != 0
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn animations_enabled() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn animations_query_does_not_panic() {
        let _ = super::animations_enabled();
    }
}
