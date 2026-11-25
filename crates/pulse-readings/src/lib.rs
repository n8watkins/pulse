//! System readings for Pulse.
//!
//! Both panels (the webview one and the native one) draw the same [`Snapshot`],
//! so a side-by-side comparison shows the same figures.
//! [`Sample`] is deterministic and works everywhere; [`system`] returns the real
//! reader on Windows and the sample elsewhere.

use serde::Serialize;

#[cfg_attr(not(windows), allow(dead_code))]
mod group;
#[cfg(windows)]
mod icon;
#[cfg(windows)]
mod win;

/// One reading of the machine. Sizes are bytes.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Snapshot {
    /// Whole-machine processor use, 0 to 100.
    pub cpu_pct: f32,
    /// Logical cores.
    pub cores: u32,
    /// Seconds since boot.
    pub uptime_s: u64,
    pub mem_used: u64,
    pub mem_total: u64,
    /// Top apps by memory, largest first. Empty when read with `with_apps` false.
    pub apps: Vec<AppRow>,
}

/// Processes grouped by executable name.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct AppRow {
    /// Friendly name, for example "Google Chrome".
    pub name: String,
    /// Executable name without a path, for example "chrome.exe".
    pub exe: String,
    pub processes: u32,
    /// Private working set summed over the group, as Task Manager shows it.
    pub mem_bytes: u64,
    /// Where to get the row's icon: the full path of one process in the group,
    /// or for system rows a stand-in file (the `System32` file of the same
    /// name for protected processes, or a built-in pick such as `wsl.exe` for
    /// WSL). `None` only when there is nothing to use (sample data).
    pub path: Option<String>,
}

/// An icon as straight (non-premultiplied) RGBA, top-down.
#[derive(Clone, Debug, PartialEq)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The executable's own icon at `size` px (32 is what both panels ask for).
/// `None` off Windows or on failure. Not cached: callers cache per path.
#[must_use]
pub fn icon_rgba(path: &str, size: u32) -> Option<IconImage> {
    #[cfg(windows)]
    {
        icon::rgba(path, size)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, size);
        None
    }
}

/// A source of snapshots. `with_apps` is false while the panel is hidden, so
/// the per-process scan is skipped.
pub trait Source: Send {
    fn read(&mut self, with_apps: bool) -> Snapshot;
}

/// The reader for this machine: real figures on Windows, [`Sample`] elsewhere.
#[must_use]
pub fn system() -> Box<dyn Source> {
    #[cfg(windows)]
    {
        Box::new(win::Windows::new())
    }
    #[cfg(not(windows))]
    {
        Box::new(Sample::default())
    }
}

const GB: u64 = 1 << 30;
const MB: u64 = 1 << 20;

/// Deterministic sample data: the same sequence on every run, so screenshots
/// of two panels can be compared pixel for pixel.
#[derive(Clone, Debug, Default)]
pub struct Sample {
    tick: u32,
}

impl Source for Sample {
    fn read(&mut self, with_apps: bool) -> Snapshot {
        self.tick += 1;
        let t = f64::from(self.tick);
        let wave = (t / 7.0).sin();
        let apps = if with_apps {
            [
                ("Google Chrome", "chrome.exe", 24, 2100 * MB),
                ("Visual Studio Code", "Code.exe", 11, 1300 * MB),
                ("Discord", "Discord.exe", 6, 622 * MB),
                ("OBS Studio", "obs64.exe", 1, 410 * MB),
                ("Spotify", "Spotify.exe", 5, 329 * MB),
                ("Windows Explorer", "explorer.exe", 1, 184 * MB),
            ]
            .into_iter()
            .map(|(name, exe, processes, mem_bytes)| AppRow {
                name: name.into(),
                exe: exe.into(),
                processes,
                mem_bytes,
                path: None,
            })
            .collect()
        } else {
            Vec::new()
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Snapshot {
            cpu_pct: (24.0 + 14.0 * wave) as f32,
            cores: 16,
            uptime_s: 3 * 86_400 + 4 * 3_600 + u64::from(self.tick) * 2,
            mem_used: (13.8 * GB as f64 + 0.4 * GB as f64 * (t / 15.0).sin()) as u64,
            mem_total: 32 * GB,
            apps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_is_deterministic_and_sorted() {
        let (mut a, mut b) = (Sample::default(), Sample::default());
        let (x, y) = (a.read(true), b.read(true));
        assert_eq!(x, y);
        assert!(x.apps.windows(2).all(|w| w[0].mem_bytes >= w[1].mem_bytes));
        assert!(a.read(false).apps.is_empty());
    }
}
