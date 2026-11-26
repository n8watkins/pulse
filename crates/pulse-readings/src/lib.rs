//! System readings for Pulse.
//!
//! Both panels (the webview one and the native one) draw the same [`Snapshot`],
//! so a side-by-side comparison shows the same figures.
//! [`Sample`] is deterministic and works everywhere; [`system`] returns the real
//! reader on Windows and the sample elsewhere.

use serde::Serialize;

// The pure logic builds everywhere so its tests run on Linux too.
#[cfg_attr(not(windows), allow(dead_code))]
mod gpu;
#[cfg_attr(not(windows), allow(dead_code))]
mod group;
#[cfg(windows)]
mod icon;
#[cfg_attr(not(windows), allow(dead_code))]
mod known;
#[cfg_attr(not(windows), allow(dead_code))]
mod net;
#[cfg(windows)]
mod perf;
#[cfg_attr(not(windows), allow(dead_code))]
mod scale;
#[cfg(windows)]
mod win;
#[cfg_attr(not(windows), allow(dead_code))]
mod wsl;

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
    /// Top apps by processor use, largest first (the same rows as `apps`, a
    /// different ranking). Empty when read with `with_apps` false.
    pub apps_by_cpu: Vec<AppRow>,
    /// The WSL 2 virtual machine and the Linux programs in it. `Some` whenever
    /// a `vmmemWSL` (or `vmmem`) process exists and `with_apps` is true.
    pub wsl: Option<Wsl>,
    /// 3D engine use, 0 to 100. `None` when the PC does not report it (never 0).
    pub gpu_pct: Option<f32>,
    /// Drive C:, bytes.
    pub disk_free: u64,
    pub disk_total: u64,
    /// Drive activity, bytes per second.
    pub disk_read_bps: u64,
    pub disk_write_bps: u64,
    /// Network over real adapters (Ethernet and Wi-Fi), bytes per second.
    pub net_down_bps: u64,
    pub net_up_bps: u64,
    /// `None` when the PC has no battery.
    pub battery: Option<Battery>,
}

/// The battery of a laptop.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Battery {
    /// Charge, 0 to 100.
    pub pct: u8,
    /// Whether the charger is connected.
    pub plugged: bool,
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
    /// The group's share of the whole machine, 0 to 100 (0 on the first read
    /// of a process).
    pub cpu_pct: f32,
    /// Where to get the row's icon: the full path of one process in the group,
    /// or for system rows a stand-in file (the `System32` file of the same
    /// name for protected processes, or a built-in pick such as `wsl.exe` for
    /// WSL). `None` only when there is nothing to use (sample data).
    pub path: Option<String>,
}

/// The WSL 2 virtual machine as the Windows side sees it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Wsl {
    /// Memory of the `vmmemWSL` (or `vmmem`) process.
    pub mem_bytes: u64,
    /// That process's share of the whole machine, 0 to 100.
    pub cpu_pct: f32,
    /// The Linux programs in the default distro, grouped by label, largest in
    /// memory first: the 6 using the most memory plus the 6 using the most
    /// processor (at most 12, the panel shows 6 of them in its chosen sort).
    /// Empty when the distro is not running, the query failed or it has not
    /// returned yet.
    pub procs: Vec<WslProc>,
}

/// One labelled group of Linux processes.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct WslProc {
    /// For example "Claude Code" or "node: next-server".
    pub label: String,
    /// Whether this is a coding agent.
    pub agent: bool,
    pub count: u32,
    pub mem_bytes: u64,
    /// The group's share of the whole machine, 0 to 100, on the scale of
    /// `AppRow::cpu_pct` (processor time used between two queries, over the
    /// logical processors of Windows). 0 on the first query.
    pub cpu_pct: f32,
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

/// Keeps COM initialised on the calling thread while it is alive.
///
/// A thread that calls [`icon_rgba`] repeatedly (a worker) creates one at
/// start, so the per-call initialisation inside `icon_rgba` is a cheap no-op.
/// It does nothing off Windows.
#[must_use = "COM is uninitialised again when this is dropped"]
pub struct IconThread {
    #[cfg(windows)]
    com: bool,
}

impl IconThread {
    /// Initialises COM (single-threaded apartment) for this thread.
    pub fn init() -> Self {
        #[cfg(windows)]
        {
            Self {
                com: icon::com_init(),
            }
        }
        #[cfg(not(windows))]
        {
            Self {}
        }
    }
}

impl Drop for IconThread {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.com {
            icon::com_uninit();
        }
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
        // The reference page's activity factor, scaled to 0..1.
        let busy = ((1.0 + 0.7 * wave).max(0.35) - 0.35) / 1.35;
        // Processor figures follow the same activity factor as the rest.
        let factor = 0.4 + 1.2 * busy;
        let mut apps: Vec<AppRow> = if with_apps {
            [
                ("Google Chrome", "chrome.exe", 24, 2100 * MB, 6.0),
                ("Visual Studio Code", "Code.exe", 11, 1300 * MB, 3.4),
                ("Discord", "Discord.exe", 6, 622 * MB, 1.8),
                ("OBS Studio", "obs64.exe", 1, 410 * MB, 1.5),
                ("Spotify", "Spotify.exe", 5, 329 * MB, 0.8),
                ("Windows Explorer", "explorer.exe", 1, 184 * MB, 0.4),
            ]
            .into_iter()
            .map(|(name, exe, processes, mem_bytes, cpu)| AppRow {
                name: name.into(),
                exe: exe.into(),
                processes,
                mem_bytes,
                #[allow(clippy::cast_possible_truncation)]
                cpu_pct: (cpu * factor) as f32,
                path: None,
            })
            .collect()
        } else {
            Vec::new()
        };
        let mut apps_by_cpu = apps.clone();
        apps_by_cpu.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
        apps.sort_by_key(|a| std::cmp::Reverse(a.mem_bytes));
        let wsl = with_apps.then(|| Wsl {
            mem_bytes: 3 * GB + 400 * MB,
            #[allow(clippy::cast_possible_truncation)]
            cpu_pct: (1.2 * factor) as f32,
            procs: [
                ("Claude Code", true, 3, 640 * MB, 0.4),
                ("VS Code server", false, 6, 512 * MB, 0.2),
                ("next-server (v16.4.6)", false, 1, 318 * MB, 0.9),
                ("Codex CLI", true, 2, 236 * MB, 0.1),
                ("Docker", false, 2, 174 * MB, 0.05),
                ("chrome-headless-shell", false, 4, 120 * MB, 0.6),
            ]
            .into_iter()
            .map(|(label, agent, count, mem_bytes, cpu)| WslProc {
                label: label.into(),
                agent,
                count,
                mem_bytes,
                #[allow(clippy::cast_possible_truncation)]
                cpu_pct: (cpu * factor) as f32,
            })
            .collect(),
        });
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Snapshot {
            cpu_pct: (24.0 + 14.0 * wave) as f32,
            cores: 16,
            uptime_s: 3 * 86_400 + 4 * 3_600 + u64::from(self.tick) * 2,
            mem_used: (13.8 * GB as f64 + 0.4 * GB as f64 * (t / 15.0).sin()) as u64,
            mem_total: 32 * GB,
            apps,
            apps_by_cpu,
            wsl,
            gpu_pct: Some((4.0 + 18.0 * busy) as f32),
            disk_free: 296 * GB,
            disk_total: 953 * GB,
            disk_read_bps: (0.2 * MB as f64 + 3.0 * MB as f64 * busy) as u64,
            disk_write_bps: (0.1 * MB as f64 + 1.2 * MB as f64 * busy) as u64,
            net_down_bps: (3.0e4 + (2.5e6 - 3.0e4) * busy) as u64,
            net_up_bps: (1.0e4 + (3.0e5 - 1.0e4) * busy) as u64,
            battery: None,
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

    #[test]
    fn sample_has_cpu_ranking_and_wsl() {
        let mut s = Sample::default();
        let x = s.read(true);
        assert_eq!(x.apps_by_cpu.len(), 6);
        assert!(
            x.apps_by_cpu
                .windows(2)
                .all(|w| w[0].cpu_pct >= w[1].cpu_pct)
        );
        assert_eq!(x.apps_by_cpu[0].exe, "chrome.exe");
        let w = x.wsl.expect("wsl in sample");
        assert_eq!(w.procs.len(), 6);
        assert!(w.procs.windows(2).all(|p| p[0].mem_bytes >= p[1].mem_bytes));
        // The processor order differs from the memory order, as in real use.
        assert!(w.procs.iter().all(|p| p.cpu_pct > 0.0));
        assert!(w.procs.windows(2).any(|p| p[0].cpu_pct < p[1].cpu_pct));
        let off = s.read(false);
        assert!(off.apps_by_cpu.is_empty() && off.wsl.is_none());
    }
}
