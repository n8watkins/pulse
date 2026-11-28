//! System readings for Pulse.
//!
//! Both panels (the webview one and the native one) draw the same [`Snapshot`],
//! so a side-by-side comparison shows the same figures.
//! [`Sample`] is deterministic and works everywhere; [`system`] returns the real
//! reader on Windows and the sample elsewhere.

use serde::Serialize;

#[cfg(windows)]
mod cards;
// The pure logic builds everywhere so its tests run on Linux too.
#[cfg_attr(not(windows), allow(dead_code))]
mod drives;
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
    /// The busiest graphics card's use, 0 to 100 (see [`Gpu::busy_pct`]):
    /// `gpus`' busiest, kept for readers of one figure. `None` when no card
    /// reports it (never 0).
    pub gpu_pct: Option<f32>,
    /// The graphics cards, software adapters (such as "Microsoft Basic Render
    /// Driver") left out, in the order Windows lists them. Empty when none is
    /// found.
    pub gpus: Vec<Gpu>,
    /// Every drive the reader counts, in the system's order (letter order on
    /// Windows: fixed local volumes with a letter, see the README). Empty when
    /// none could be read.
    pub drives: Vec<Drive>,
    /// Drive C: (or the first drive when there is no C:), bytes. Kept for
    /// older readers of the snapshot; set with [`Snapshot::set_drives`].
    pub disk_free: u64,
    pub disk_total: u64,
    /// Activity of all of `drives` together, bytes per second.
    pub disk_read_bps: u64,
    pub disk_write_bps: u64,
    /// Network over real adapters (Ethernet and Wi-Fi), bytes per second.
    pub net_down_bps: u64,
    pub net_up_bps: u64,
    /// `None` when the PC has no battery.
    pub battery: Option<Battery>,
}

impl Snapshot {
    /// Sets `drives` and the single-drive fields that follow from them.
    pub fn set_drives(&mut self, drives: Vec<Drive>) {
        let main = drives
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case("C:"))
            .or(drives.first());
        (self.disk_free, self.disk_total) = main.map_or((0, 0), |d| (d.free, d.total));
        self.disk_read_bps = drives.iter().map(|d| d.read_bps).sum();
        self.disk_write_bps = drives.iter().map(|d| d.write_bps).sum();
        self.drives = drives;
    }
}

/// One drive as the user knows it. Platform neutral: a Windows reader names
/// it by its letter ("C:"), another system may use a volume name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Drive {
    /// The name to show, for example "C:". Never assumed to be a letter.
    pub name: String,
    /// The volume label, for example "Data". `None` when it has none.
    pub label: Option<String>,
    /// Bytes free for the user (what Explorer shows).
    pub free: u64,
    /// Size in bytes, never 0 for a listed drive.
    pub total: u64,
    /// Activity, bytes per second.
    pub read_bps: u64,
    pub write_bps: u64,
}

impl Drive {
    /// Whether this drive has a smaller share free than `other`.
    fn fuller_than(&self, other: &Drive) -> bool {
        // free / total < other.free / other.total, exact in integers.
        u128::from(self.free) * u128::from(other.total)
            < u128::from(other.free) * u128::from(self.total)
    }

    /// Whether under `pct` percent of the drive is free.
    #[must_use]
    pub fn free_under(&self, pct: u8) -> bool {
        self.total > 0 && u128::from(self.free) * 100 < u128::from(self.total) * u128::from(pct)
    }

    /// The share used, 0 to 1.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn used_share(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.total.saturating_sub(self.free) as f64 / self.total as f64) as f32
    }
}

/// The fullest drive: the smallest share free. On a tie, the first in the
/// list. Drives of no size are passed over.
#[must_use]
pub fn fullest(drives: &[Drive]) -> Option<&Drive> {
    drives
        .iter()
        .filter(|d| d.total > 0)
        .fold(None, |best: Option<&Drive>, d| match best {
            Some(b) if !d.fuller_than(b) => Some(b),
            _ => Some(d),
        })
}

/// Extra made-up drives after the real ones, up to `count` drives in all
/// (`PULSE_DRIVES=<count>`, a development knob for a PC with one drive). The
/// first one added is nearly full; they take the next free letters.
pub fn simulate_drives(drives: &mut Vec<Drive>, count: usize) {
    const MADE_UP: [(&str, u64, u64, u64, u64); 4] = [
        // label, total GB, free GB, read MB/s, write MB/s
        ("Games", 931, 38, 42, 3),
        ("Data", 1863, 1120, 0, 1),
        ("Backup", 3726, 2900, 0, 0),
        ("Spare", 466, 300, 0, 0),
    ];
    let mut letters = (b'D'..=b'Z')
        .map(|b| format!("{}:", char::from(b)))
        .filter(|n| !drives.iter().any(|d| d.name.eq_ignore_ascii_case(n)))
        .collect::<Vec<_>>()
        .into_iter();
    let mut made = MADE_UP.iter().cycle();
    while drives.len() < count.min(26) {
        let (Some(name), Some(&(label, total, free, r, w))) = (letters.next(), made.next()) else {
            break;
        };
        drives.push(Drive {
            name,
            label: Some(label.into()),
            free: free * GB,
            total: total * GB,
            read_bps: r * MB,
            write_bps: w * MB,
        });
    }
}

impl Snapshot {
    /// Replaces the cards and keeps `gpu_pct` in step with them.
    pub fn set_gpus(&mut self, gpus: Vec<Gpu>) {
        self.gpu_pct = busiest_gpu(&gpus).and_then(|g| g.busy_pct);
        self.gpus = gpus;
    }
}

/// One graphics card. Filled on Windows from the engine and adapter memory
/// counters and the adapter list; another platform's reader fills the same
/// fields.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Gpu {
    /// The adapter's LUID (high word above the low one), the key that ties
    /// the counters to the card. Stable until the driver restarts.
    pub luid: u64,
    /// As the driver names it, for example "AMD Radeon RX 7800 XT". Empty
    /// when the adapter list could not be read.
    pub name: String,
    /// The busiest engine's use, 0 to 100, as Task Manager's GPU page shows
    /// it: summed over processes per engine, the largest engine of any kind.
    /// `None` when the PC does not report it (never 0 for "unknown").
    pub busy_pct: Option<f32>,
    /// The kind of work of that busiest engine. `None` with `busy_pct`.
    pub lead: Option<GpuEngine>,
    /// Dedicated video memory in use, bytes. `None` when not reported.
    pub mem_used: Option<u64>,
    /// Dedicated video memory of the card, bytes. `None` when not known.
    pub mem_total: Option<u64>,
}

/// The kind of work an engine of a graphics card does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum GpuEngine {
    /// Drawing: games, the desktop, browsers.
    #[default]
    ThreeD,
    /// General computing, for example local AI models.
    Compute,
    /// Moving data in and out of video memory.
    Copy,
    VideoDecode,
    VideoEncode,
    /// A video engine that does both (AMD's "Video Codec") or other video work.
    Video,
    /// Anything else (security, timers, audio).
    Other,
}

/// The card the panel shows: the busiest (no reading counts as least busy),
/// a tie going to the one with more video memory, then to the first listed.
#[must_use]
pub fn busiest_gpu(gpus: &[Gpu]) -> Option<&Gpu> {
    let key = |g: &Gpu| (g.busy_pct.unwrap_or(-1.0), g.mem_total.unwrap_or(0));
    gpus.iter().reduce(|best, g| {
        let (a, b) = (key(best), key(g));
        if b.0 > a.0 || (b.0 == a.0 && b.1 > a.1) {
            g
        } else {
            best
        }
    })
}

/// Made-up cards for the sample data and for `PULSE_GPUS` (0 to 2 of them).
/// `busy` is the activity factor, 0 to 1. The first is a 16 GB card at
/// moderate use, drawing or (when busier) running a model on its compute
/// engines; the second a busier 24 GB card running a model.
#[must_use]
pub fn sample_gpus(cards: usize, busy: f64) -> Vec<Gpu> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let first = Gpu {
        luid: 0x0001_8B3A,
        name: "AMD Radeon RX 7800 XT".into(),
        busy_pct: Some((4.0 + 18.0 * busy) as f32),
        lead: Some(if busy > 0.55 {
            GpuEngine::Compute
        } else {
            GpuEngine::ThreeD
        }),
        mem_used: Some((6.2 * GB as f64 + 3.0 * GB as f64 * busy) as u64),
        mem_total: Some(16_368 * MB),
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let second = Gpu {
        luid: 0x0002_1C40,
        name: "NVIDIA GeForce RTX 4090".into(),
        busy_pct: Some((55.0 + 30.0 * busy) as f32),
        lead: Some(GpuEngine::Compute),
        mem_used: Some((18.4 * GB as f64 + 2.0 * GB as f64 * busy) as u64),
        mem_total: Some(24 * GB - 400 * MB),
    };
    [first, second].into_iter().take(cards).collect()
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
    /// The Linux programs in the default distro that are not coding agents,
    /// grouped by label, largest in memory first: the 6 using the most memory
    /// plus the 6 using the most processor (at most 12, the panel shows some
    /// of them in its chosen sort). Programs a coding agent started are here
    /// too, under their own label. Empty when the distro is not running, the
    /// query failed or it has not returned yet.
    pub procs: Vec<WslProc>,
    /// The coding-agent sessions, one each (see [`WslSession`]), in no
    /// particular order. They replace the agent groups that used to sit in
    /// `procs`: the number of agents is [`Wsl::agent_count`].
    pub sessions: Vec<WslSession>,
    /// How many dev servers run in Linux (Linux processes listening on a TCP
    /// port that are development programs, see the README), counted from all
    /// processes, not only those among `procs`. `None` when it could not be
    /// read: then no count is shown, never a wrong one.
    pub dev_servers: Option<u32>,
}

impl Wsl {
    /// How many coding-agent sessions run in Linux.
    #[must_use]
    pub fn agent_count(&self) -> usize {
        self.sessions.len()
    }

    /// How many dev servers run in Linux, 0 when unknown.
    #[must_use]
    pub fn server_count(&self) -> u32 {
        self.dev_servers.unwrap_or(0)
    }
}

/// One coding-agent session: an agent's root process plus the helper
/// processes that are part of the agent itself (for example its spare
/// background process). Programs the agent started are not part of it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct WslSession {
    /// The agent, for example "Claude Code".
    pub label: String,
    /// The name of the folder the session works in (never a full path), `~`
    /// for the home folder. `None` when it cannot be read.
    pub folder: Option<String>,
    /// Memory of the session's own processes.
    pub mem_bytes: u64,
    /// Their share of the whole machine, same scale as `WslProc::cpu_pct`.
    pub cpu_pct: f32,
    /// Whether the session is working now: its processes and everything it
    /// started use processor (with hysteresis).
    pub busy: bool,
    /// Seconds since the session started, `None` when unknown.
    pub run_secs: Option<u64>,
    /// The root process id, stable while the session runs.
    pub pid: u32,
}

/// One labelled group of Linux processes.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct WslProc {
    /// For example "node: next-server".
    pub label: String,
    /// Whether this is a coding agent. Always false in `Wsl::procs` now (the
    /// agents are in `Wsl::sessions`); the field stays for readers of it.
    pub agent: bool,
    pub count: u32,
    pub mem_bytes: u64,
    /// The group's share of the whole machine, 0 to 100, on the scale of
    /// `AppRow::cpu_pct` (processor time used between two queries, over the
    /// logical processors of Windows). 0 on the first query.
    pub cpu_pct: f32,
    /// The TCP ports this group's dev servers listen on, ascending; empty when
    /// it has none.
    pub ports: Vec<u16>,
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
#[derive(Clone, Debug)]
pub struct Sample {
    tick: u32,
    cards: usize,
}

impl Default for Sample {
    /// One graphics card.
    fn default() -> Self {
        Self::with_cards(1)
    }
}

impl Sample {
    /// Sample data with that many graphics cards (see [`sample_gpus`]).
    #[must_use]
    pub fn with_cards(cards: usize) -> Self {
        Self { tick: 0, cards }
    }
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
                ("VS Code server", false, 6, 512 * MB, 0.2),
                ("next-server (v16.4.6)", false, 1, 318 * MB, 0.9),
                ("rust-analyzer", false, 2, 174 * MB, 0.05),
                ("Docker", false, 3, 150 * MB, 0.15),
                ("chrome-headless-shell", false, 4, 120 * MB, 0.6),
                ("python3: worker.py", false, 2, 90 * MB, 0.1),
            ]
            .into_iter()
            .map(|(label, agent, count, mem_bytes, cpu)| WslProc {
                label: label.into(),
                agent,
                count,
                mem_bytes,
                #[allow(clippy::cast_possible_truncation)]
                cpu_pct: (cpu * factor) as f32,
                ports: match label {
                    "next-server (v16.4.6)" => vec![3000],
                    "python3: worker.py" => vec![8000, 8001],
                    _ => Vec::new(),
                },
            })
            .collect(),
            // Two sessions share a folder (told apart by run time), one
            // agent is not Claude Code, and the idle ones fold into one line.
            sessions: [
                (
                    "Claude Code",
                    "pulse",
                    420,
                    0.9,
                    true,
                    2 * 3_600 + 10 * 60,
                    411,
                ),
                ("Claude Code", "pulse", 260, 0.0, false, 14 * 60, 412),
                ("Claude Code", "shop-site", 310, 0.0, false, 5 * 3_600, 413),
                ("Codex CLI", "api-server", 236, 0.5, true, 36 * 60, 414),
                ("Gemini CLI", "docs", 180, 0.0, false, 3_600 + 5 * 60, 415),
            ]
            .into_iter()
            .map(|(label, folder, mem, cpu, busy, run, pid)| WslSession {
                label: label.into(),
                folder: Some(folder.into()),
                mem_bytes: mem * MB,
                #[allow(clippy::cast_possible_truncation)]
                cpu_pct: (cpu * factor) as f32,
                busy,
                run_secs: Some(run),
                pid,
            })
            .collect(),
            // The two with ports above, and one small server not among the rows.
            dev_servers: Some(3),
        });
        let gpus = sample_gpus(self.cards, busy);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let c = Drive {
            name: "C:".into(),
            label: Some("Windows".into()),
            free: 296 * GB,
            total: 953 * GB,
            read_bps: (0.2 * MB as f64 + 3.0 * MB as f64 * busy) as u64,
            write_bps: (0.1 * MB as f64 + 1.2 * MB as f64 * busy) as u64,
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let mut snap = Snapshot {
            cpu_pct: (24.0 + 14.0 * wave) as f32,
            cores: 16,
            uptime_s: 3 * 86_400 + 4 * 3_600 + u64::from(self.tick) * 2,
            mem_used: (13.8 * GB as f64 + 0.4 * GB as f64 * (t / 15.0).sin()) as u64,
            mem_total: 32 * GB,
            apps,
            apps_by_cpu,
            wsl,
            gpu_pct: busiest_gpu(&gpus).and_then(|g| g.busy_pct),
            gpus,
            net_down_bps: (3.0e4 + (2.5e6 - 3.0e4) * busy) as u64,
            net_up_bps: (1.0e4 + (3.0e5 - 1.0e4) * busy) as u64,
            battery: None,
            ..Snapshot::default()
        };
        snap.set_drives(vec![c]);
        snap
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
        // Sessions: more than the panel shows one by one, two in one folder.
        assert_eq!(w.agent_count(), 5);
        assert!(w.sessions.iter().any(|x| x.busy) && w.sessions.iter().any(|x| !x.busy));
        assert!(w.procs.iter().all(|p| !p.agent));
        let pulse = w
            .sessions
            .iter()
            .filter(|x| x.folder.as_deref() == Some("pulse"))
            .count();
        assert_eq!(pulse, 2);
        // Dev servers: counted, two of them with ports on a row, one not on a row.
        assert_eq!(w.dev_servers, Some(3));
        assert_eq!(w.server_count(), 3);
        assert_eq!(w.procs.iter().filter(|p| !p.ports.is_empty()).count(), 2);
        assert_eq!(Wsl::default().server_count(), 0);
        let off = s.read(false);
        assert!(off.apps_by_cpu.is_empty() && off.wsl.is_none());
    }

    fn drive(name: &str, free: u64, total: u64) -> Drive {
        Drive {
            name: name.into(),
            free: free * GB,
            total: total * GB,
            ..Drive::default()
        }
    }

    #[test]
    fn the_sample_has_one_drive_and_the_old_fields_follow_it() {
        let x = Sample::default().read(false);
        assert_eq!(x.drives.len(), 1);
        let c = &x.drives[0];
        assert_eq!(c.name, "C:");
        assert_eq!((x.disk_free, x.disk_total), (296 * GB, 953 * GB));
        assert_eq!(x.disk_read_bps, c.read_bps);
        assert!(c.read_bps > 0 && c.write_bps > 0);
    }

    #[test]
    fn the_old_fields_take_c_and_add_up_the_speeds() {
        let mut s = Snapshot::default();
        let mut d = drive("D:", 10, 100);
        d.read_bps = 5;
        let mut c = drive("C:", 50, 200);
        c.read_bps = 2;
        c.write_bps = 1;
        s.set_drives(vec![d.clone(), c]);
        assert_eq!((s.disk_free, s.disk_total), (50 * GB, 200 * GB));
        assert_eq!((s.disk_read_bps, s.disk_write_bps), (7, 1));
        // No C: (another system, or a PC that boots from D:): the first one.
        s.set_drives(vec![d]);
        assert_eq!(s.disk_total, 100 * GB);
        s.set_drives(Vec::new());
        assert_eq!((s.disk_free, s.disk_total, s.disk_read_bps), (0, 0, 0));
    }

    #[test]
    fn the_fullest_drive_has_the_smallest_share_free() {
        assert_eq!(fullest(&[]), None);
        let v = [
            drive("C:", 300, 1000),
            drive("D:", 100, 2000),
            drive("E:", 50, 500),
        ];
        // D: has the most used bytes share (5% free), not E: with the fewest bytes free (10%).
        assert_eq!(fullest(&v).unwrap().name, "D:");
        // A tie goes to the first one.
        let tie = [drive("C:", 10, 100), drive("D:", 20, 200)];
        assert_eq!(fullest(&tie).unwrap().name, "C:");
        // A drive of no size never wins.
        let odd = [drive("C:", 500, 1000), drive("Z:", 0, 0)];
        assert_eq!(fullest(&odd).unwrap().name, "C:");
        // Huge sizes do not overflow.
        let big = [
            Drive {
                name: "A".into(),
                free: u64::MAX / 2,
                total: u64::MAX,
                ..Drive::default()
            },
            Drive {
                name: "B".into(),
                free: u64::MAX / 3,
                total: u64::MAX,
                ..Drive::default()
            },
        ];
        assert_eq!(fullest(&big).unwrap().name, "B");
    }

    #[test]
    fn free_under_is_strict_and_used_share_is_a_fraction() {
        let d = drive("C:", 8, 100);
        assert!(!d.free_under(8), "8% free is not under 8%");
        assert!(drive("C:", 7, 100).free_under(8));
        assert!(!Drive::default().free_under(8));
        assert!((d.used_share() - 0.92).abs() < 1e-6);
        assert_eq!(Drive::default().used_share(), 0.0);
    }

    #[test]
    fn simulated_drives_add_up_to_the_count_with_one_nearly_full() {
        let mut v = Sample::default().read(false).drives;
        simulate_drives(&mut v, 3);
        let names: Vec<&str> = v.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["C:", "D:", "E:"]);
        assert!(v[1].free_under(8), "D: is nearly full");
        assert!(!v[0].free_under(8) && !v[2].free_under(8));
        assert_eq!(fullest(&v).unwrap().name, "D:");
        // Never fewer than there are, never a letter twice, at most 26.
        simulate_drives(&mut v, 1);
        assert_eq!(v.len(), 3);
        let mut w = vec![drive("C:", 1, 2), drive("D:", 1, 2)];
        simulate_drives(&mut w, 3);
        assert_eq!(w[2].name, "E:");
        simulate_drives(&mut w, 99);
        assert_eq!(w.len(), 24, "C: plus D: to Z:");
        let mut seen: Vec<&str> = w.iter().map(|d| d.name.as_str()).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 24);
    }

    #[test]
    fn sample_has_one_card_or_two_and_gpu_pct_is_the_busiest() {
        let x = Sample::default().read(false);
        assert_eq!(x.gpus.len(), 1);
        assert_eq!(x.gpu_pct, x.gpus[0].busy_pct);
        let g = &x.gpus[0];
        assert!(g.mem_used.unwrap() < g.mem_total.unwrap());
        let two = Sample::with_cards(2).read(false);
        assert_eq!(two.gpus.len(), 2);
        let top = busiest_gpu(&two.gpus).unwrap();
        assert_eq!(top.name, "NVIDIA GeForce RTX 4090");
        assert_eq!(two.gpu_pct, top.busy_pct);
        let none = Sample::with_cards(0).read(false);
        assert!(none.gpus.is_empty() && none.gpu_pct.is_none());
    }

    #[test]
    fn the_busiest_card_wins_and_a_tie_goes_to_more_memory() {
        let card = |busy: Option<f32>, total: u64| Gpu {
            busy_pct: busy,
            mem_total: Some(total),
            ..Gpu::default()
        };
        assert!(busiest_gpu(&[]).is_none());
        let g = [card(Some(5.0), 1), card(Some(40.0), 1), card(None, 9)];
        assert_eq!(busiest_gpu(&g), Some(&g[1]));
        // Idle integrated graphics and an idle discrete card: the discrete one.
        let g = [card(Some(0.0), 512 * MB), card(Some(0.0), 16 * GB)];
        assert_eq!(busiest_gpu(&g), Some(&g[1]));
        // An exact tie of everything: the first listed.
        let g = [card(Some(3.0), GB), card(Some(3.0), GB)];
        assert!(std::ptr::eq(busiest_gpu(&g).unwrap(), &g[0]));
        // No reading anywhere: still a card to name, and gpu_pct stays None.
        let mut s = Snapshot::default();
        s.set_gpus(vec![card(None, GB)]);
        assert_eq!((s.gpus.len(), s.gpu_pct), (1, None));
        s.set_gpus(vec![card(Some(2.0), GB), card(Some(7.5), GB)]);
        assert_eq!(s.gpu_pct, Some(7.5));
    }
}
