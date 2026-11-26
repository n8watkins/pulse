//! The real reader for Windows: no process handles, no sysinfo.
//!
//! Processor from `GetSystemTimes` deltas, memory from `GlobalMemoryStatusEx`,
//! uptime from `GetTickCount64`, and the apps from one
//! `NtQuerySystemInformation(SystemProcessInformation)` call per read.

use crate::known::{Known, Probe};
use crate::{AppRow, Snapshot, Source, group, icon, perf, wsl};
use std::collections::HashMap;
use std::time::Instant;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::SystemInformation::{
    GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX,
};
use windows_sys::Win32::System::Threading::{GetActiveProcessorCount, GetSystemTimes};

// The offsets below are those of 64-bit Windows.
#[cfg(not(target_pointer_width = "64"))]
compile_error!(
    "pulse-readings reads SYSTEM_PROCESS_INFORMATION at 64-bit offsets: build for a 64-bit Windows target"
);

/// `ALL_PROCESSOR_GROUPS`: count the cores of every processor group.
const ALL_PROCESSOR_GROUPS: u16 = 0xFFFF;

const SYSTEM_PROCESS_INFORMATION: u32 = 5;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(
        class: u32,
        info: *mut core::ffi::c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

// Offsets inside SYSTEM_PROCESS_INFORMATION on 64-bit Windows.
const OFF_NEXT: usize = 0;
const OFF_PRIVATE_WS: usize = 8;
const OFF_CREATE_TIME: usize = 32;
const OFF_USER_TIME: usize = 40;
const OFF_KERNEL_TIME: usize = 48;
const OFF_NAME_LEN: usize = 56;
const OFF_NAME_PTR: usize = 64;
const OFF_PID: usize = 80;
const MIN_ENTRY: usize = 88;

/// Shortest span (100 ns units, per core) a processor figure is taken over: 250 ms.
const MIN_SPAN: u64 = 2_500_000;

/// A per-process baseline older than this is dropped: the figure would be an
/// average over a stretch when nobody was looking (the panel was hidden).
const BASELINE_MAX_AGE_NS: u128 = 10_000_000_000;
/// Shortest span (ns) a per-process figure is taken over.
const PROC_MIN_SPAN_NS: u128 = 250_000_000;

/// Kernel plus user time per (pid, create time).
type CpuTimes = HashMap<(u32, u64), u64>;

/// One process of the single `SystemProcessInformation` buffer.
struct Proc {
    name: String,
    private_ws: u64,
    pid: u32,
    create_time: u64,
    /// Kernel plus user time, 100 ns units.
    ///
    /// Checked against the `\Process(*)\% Processor Time` counter over the
    /// same window (`compare_with_pdh_counter`, 2026-11-25, 16 cores): the
    /// figures agree to 0.01 points for `System` (pid 4) and every other
    /// image, tick by tick (for example 4.83 against 4.84, 0.39 against 0.39).
    /// `System` is bursty (0.4 to 4.8 percent from one second to the next), so
    /// a one-second sample of the counter and a tick of the panel taken at
    /// other moments differ a lot. Interrupt and DPC time is not in any
    /// process of this call: it is a separate pseudo-row ("System interrupts",
    /// Task Manager's Details tab, about 1.5 percent of this PC) and is left
    /// out here, as in the `Process` counter. The Idle process (pid 0) has an
    /// empty name and is skipped when grouping on purpose.
    cpu_time: u64,
}

fn system32(file: &str) -> Option<String> {
    let root = std::env::var("SystemRoot").ok()?;
    let p = std::path::Path::new(&root).join("System32").join(file);
    p.is_file().then(|| p.to_string_lossy().into_owned())
}

/// The real machine, for the cache of icon paths.
struct Machine;

impl Probe for Machine {
    fn process_path(&self, pid: u32) -> Option<String> {
        icon::process_path(pid)
    }
    fn stand_in(&self, exe: &str) -> Option<String> {
        system32(group::stand_in_file(exe)?)
    }
    fn system32(&self, exe: &str) -> Option<String> {
        system32(exe)
    }
    fn exists(&self, path: &str) -> bool {
        std::path::Path::new(path).is_file()
    }
    fn description(&self, path: &str) -> Option<String> {
        icon::file_description(path)
    }
}

pub struct Windows {
    last: Option<(u64, u64)>,
    pct: f32,
    cores: u32,
    /// u64 storage so the buffer is 8-byte aligned.
    buf: Vec<u64>,
    /// Per lowercase exe name: the path found and the friendly name from its version info.
    known: Known,
    extras: perf::Extras,
    /// Kernel plus user time per (pid, create time), and when it was taken.
    cpu_base: Option<(Instant, CpuTimes)>,
    /// The last per-process figures, repeated by a read that follows too soon.
    last_pct: HashMap<(u32, u64), f32>,
    wsl: wsl::Query,
}

impl Windows {
    pub fn new() -> Self {
        // SAFETY: no pointers.
        let cores = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) }.max(1);
        let mut w = Self {
            last: None,
            pct: 0.0,
            cores,
            buf: vec![0; (1 << 20) / 8],
            known: Known::default(),
            extras: perf::Extras::new(),
            cpu_base: None,
            last_pct: HashMap::new(),
            wsl: wsl::Query::default(),
        };
        w.cpu_pct();
        w
    }

    /// Whole-machine use since the previous call; 0 on the first call.
    fn cpu_pct(&mut self) -> f32 {
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut idle, mut kernel, mut user) = (zero, zero, zero);
        // SAFETY: three valid out pointers.
        let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
        if ok == 0 {
            return 0.0;
        }
        let t = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
        // Kernel time includes idle time.
        let (idle, total) = (t(idle), t(kernel) + t(user));
        let pct = match self.last {
            // Too short a window (a read right after another) is noise: keep
            // the earlier baseline and figure.
            Some((_, pt)) if (total.saturating_sub(pt)) / u64::from(self.cores) < MIN_SPAN => {
                return self.pct;
            }
            Some((pi, pt)) => {
                let dt = (total - pt) as f64;
                let di = idle.saturating_sub(pi) as f64;
                ((1.0 - di / dt) * 100.0).clamp(0.0, 100.0) as f32
            }
            None => 0.0,
        };
        self.last = Some((idle, total));
        self.pct = pct;
        pct
    }

    /// Adds the path of one process and the file's own description as the name
    /// (the built-in names win), cached per exe name; see [`Known`].
    fn resolve(&mut self, mut row: AppRow, pids: &[u32]) -> AppRow {
        if let Some((path, desc)) = self.known.resolve(&Machine, &row.exe, pids, Instant::now()) {
            row.path = Some(path);
            if let Some(d) = desc
                && !group::is_builtin(&row.exe)
            {
                row.name = d;
            }
        }
        row
    }

    /// Each process's share of the whole machine in percent, in the order of
    /// `procs`, from the change in kernel plus user time since the last read.
    /// A process not seen before (or a recycled pid: the key includes the
    /// create time) has no baseline and reads 0.
    fn cpu_shares(&mut self, procs: &[Proc]) -> Vec<f32> {
        let now = Instant::now();
        let key = |p: &Proc| (p.pid, p.create_time);
        let Some((at, base)) = &self.cpu_base else {
            return self.start_baseline(now, procs);
        };
        let span_ns = now.duration_since(*at).as_nanos();
        if span_ns > BASELINE_MAX_AGE_NS {
            return self.start_baseline(now, procs);
        }
        if span_ns < PROC_MIN_SPAN_NS {
            // A read right after another: repeat the last figures and keep the
            // older baseline, so the span grows instead of the figure turning
            // to noise.
            return procs
                .iter()
                .map(|p| self.last_pct.get(&key(p)).copied().unwrap_or(0.0))
                .collect();
        }
        // Times are in 100 ns units, per core.
        let denom = span_ns as f64 / 100.0 * f64::from(self.cores);
        let pcts: HashMap<(u32, u64), f32> = procs
            .iter()
            .map(|p| {
                let pct = base.get(&key(p)).map_or(0.0, |prev| {
                    (p.cpu_time.saturating_sub(*prev) as f64 * 100.0 / denom).clamp(0.0, 100.0)
                        as f32
                });
                (key(p), pct)
            })
            .collect();
        let shares = procs.iter().map(|p| pcts[&key(p)]).collect();
        self.cpu_base = Some((now, procs.iter().map(|p| (key(p), p.cpu_time)).collect()));
        self.last_pct = pcts;
        shares
    }

    /// The first read, or one after a long pause: no figures yet, new baseline.
    fn start_baseline(&mut self, now: Instant, procs: &[Proc]) -> Vec<f32> {
        self.cpu_base = Some((
            now,
            procs
                .iter()
                .map(|p| ((p.pid, p.create_time), p.cpu_time))
                .collect(),
        ));
        self.last_pct.clear();
        vec![0.0; procs.len()]
    }

    /// One call for every process; returns (image name, private working set).
    fn processes(&mut self) -> Vec<Proc> {
        let mut returned = 0u32;
        for _ in 0..4 {
            let bytes = self.buf.len() * 8;
            // SAFETY: the buffer is `bytes` long and `returned` is a valid out pointer.
            let status = unsafe {
                NtQuerySystemInformation(
                    SYSTEM_PROCESS_INFORMATION,
                    self.buf.as_mut_ptr().cast(),
                    u32::try_from(bytes).unwrap_or(u32::MAX),
                    &mut returned,
                )
            };
            if status >= 0 {
                let bytes_ptr = self.buf.as_ptr().cast::<u8>();
                // SAFETY: the Vec<u64> owns `bytes` initialised bytes.
                let slice = unsafe { core::slice::from_raw_parts(bytes_ptr, bytes) };
                return parse(slice, bytes_ptr as usize);
            }
            if status != STATUS_INFO_LENGTH_MISMATCH {
                return Vec::new();
            }
            let want = (returned as usize + (256 << 10)).div_ceil(8);
            self.buf.resize(want.max(self.buf.len() * 2), 0);
        }
        Vec::new()
    }
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// Walks the linked entries in `buf`; `base` is the address `buf` starts at,
/// which the name pointers are relative to.
fn parse(buf: &[u8], base: usize) -> Vec<Proc> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + MIN_ENTRY <= buf.len() {
        let next = u32_at(buf, at + OFF_NEXT).unwrap_or(0) as usize;
        let private = u64_at(buf, at + OFF_PRIVATE_WS).unwrap_or(0);
        let len = buf
            .get(at + OFF_NAME_LEN..at + OFF_NAME_LEN + 2)
            .map_or(0, |b| usize::from(u16::from_le_bytes([b[0], b[1]])));
        let ptr = u64_at(buf, at + OFF_NAME_PTR).unwrap_or(0) as usize;
        let name = ptr
            .checked_sub(base)
            .and_then(|o| buf.get(o..o.checked_add(len)?))
            .map(|b| {
                let units: Vec<u16> = b
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16_lossy(&units)
            })
            .unwrap_or_default();
        let pid = u64_at(buf, at + OFF_PID).unwrap_or(0) as u32;
        let create_time = u64_at(buf, at + OFF_CREATE_TIME).unwrap_or(0);
        let cpu_time = u64_at(buf, at + OFF_USER_TIME)
            .unwrap_or(0)
            .saturating_add(u64_at(buf, at + OFF_KERNEL_TIME).unwrap_or(0));
        out.push(Proc {
            name,
            private_ws: private,
            pid,
            create_time,
            cpu_time,
        });
        if next == 0 {
            break;
        }
        at += next;
    }
    out
}

impl Source for Windows {
    fn read(&mut self, with_apps: bool) -> Snapshot {
        let cpu_pct = self.cpu_pct();
        // SAFETY: zeroed plain-data structs with their size fields set.
        let (mem_used, mem_total) = unsafe {
            let mut m: MEMORYSTATUSEX = core::mem::zeroed();
            m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
            if GlobalMemoryStatusEx(&mut m) == 0 {
                (0, 0)
            } else {
                (
                    m.ullTotalPhys.saturating_sub(m.ullAvailPhys),
                    m.ullTotalPhys,
                )
            }
        };
        let cores = self.cores;
        // SAFETY: no arguments.
        let uptime_s = unsafe { GetTickCount64() } / 1000;
        let (apps, apps_by_cpu, wsl) = if with_apps {
            self.known.begin_read();
            let procs = self.processes();
            let shares = self.cpu_shares(&procs);
            let groups = group::group(
                procs
                    .into_iter()
                    .zip(shares)
                    .map(|(p, share)| (p.name, p.private_ws, p.pid, share)),
            );
            let by_mem = groups
                .by_mem
                .into_iter()
                .map(|(row, pids)| self.resolve(row, &pids))
                .collect();
            let by_cpu = groups
                .by_cpu
                .into_iter()
                .map(|(row, pids)| self.resolve(row, &pids))
                .collect();
            let wsl = groups.vm.map(|(mem_bytes, cpu_pct)| crate::Wsl {
                mem_bytes,
                cpu_pct,
                procs: self.wsl.poll(true),
            });
            if wsl.is_none() {
                self.wsl.poll(false);
            }
            (by_mem, by_cpu, wsl)
        } else {
            (Vec::new(), Vec::new(), None)
        };
        let x = self.extras.read();
        Snapshot {
            cpu_pct,
            cores,
            uptime_s,
            mem_used,
            mem_total,
            apps,
            apps_by_cpu,
            wsl,
            gpu_pct: x.gpu_pct,
            disk_free: x.disk_free,
            disk_total: x.disk_total,
            disk_read_bps: x.disk_read_bps,
            disk_write_bps: x.disk_write_bps,
            net_down_bps: x.net_down_bps,
            net_up_bps: x.net_up_bps,
            battery: x.battery,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that need this particular PC (WSL running with a "Claude Code"
    /// process, the shell icon cache, `explorer.exe` with its English
    /// description) run only with `PULSE_LIVE_TESTS=1`; a clean CI runner
    /// has none of that.
    fn live(test: &str) -> bool {
        let on = std::env::var_os("PULSE_LIVE_TESTS").is_some_and(|v| v != "0" && !v.is_empty());
        if !on {
            println!("skipped {test}: set PULSE_LIVE_TESTS=1 to run it on a real PC");
        }
        on
    }

    #[test]
    fn icons_and_names_come_from_the_exe() {
        if !live("icons_and_names_come_from_the_exe") {
            return;
        }
        let path = r"C:\Windows\explorer.exe";
        let img = crate::icon_rgba(path, 32).expect("explorer has an icon");
        assert_eq!((img.width, img.height), (32, 32));
        assert_eq!(img.rgba.len(), 32 * 32 * 4);
        assert!(img.rgba.chunks_exact(4).any(|p| p[3] > 0));
        assert_eq!(
            icon::file_description(path).as_deref(),
            Some("Windows Explorer")
        );
    }

    #[test]
    fn every_stand_in_has_an_icon() {
        if !live("every_stand_in_has_an_icon") {
            return;
        }
        let mut checked = 0;
        for (key, file) in group::STAND_INS {
            let Some(path) = system32(file) else { continue };
            let img =
                crate::icon_rgba(&path, 32).unwrap_or_else(|| panic!("{key}: no icon from {path}"));
            assert_eq!(img.rgba.len(), 32 * 32 * 4, "{key}");
            assert!(img.rgba.chunks_exact(4).any(|p| p[3] > 0), "{key}: blank");
            checked += 1;
        }
        assert!(checked > 0);
    }

    #[test]
    fn reads_plausible_figures() {
        let mut w = Windows::new();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let s = w.read(true);
        assert!(s.cores >= 1);
        assert!(s.mem_total > 0 && s.mem_used <= s.mem_total);
        assert!((0.0..=100.0).contains(&s.cpu_pct));
        assert!(!s.apps.is_empty() && s.apps.len() <= group::TOP);
        assert!(s.apps.windows(2).all(|p| p[0].mem_bytes >= p[1].mem_bytes));
        assert!(w.read(false).apps.is_empty());
        // Drive, graphics and network are read on every call.
        std::thread::sleep(std::time::Duration::from_millis(400));
        let s = w.read(false);
        assert!(s.disk_total > 0 && s.disk_free <= s.disk_total);
        assert!(s.gpu_pct.is_none_or(|g| (0.0..=100.0).contains(&g)));
        assert!(s.battery.is_none_or(|b| b.pct <= 100));
    }

    #[test]
    fn live_cpu_ranking_and_wsl() {
        if !live("live_cpu_ranking_and_wsl") {
            return;
        }
        let mut w = Windows::new();
        let first = w.read(true);
        assert!(
            first.apps_by_cpu.iter().all(|a| a.cpu_pct == 0.0),
            "first read has no baseline"
        );
        std::thread::sleep(std::time::Duration::from_secs(1));
        let s = w.read(true);
        assert!(!s.apps_by_cpu.is_empty() && s.apps_by_cpu.len() <= group::TOP);
        assert!(
            s.apps_by_cpu
                .windows(2)
                .all(|p| p[0].cpu_pct >= p[1].cpu_pct)
        );
        let sum: f32 = s.apps_by_cpu.iter().map(|a| a.cpu_pct).sum();
        assert!((0.0..=105.0).contains(&sum), "sum {sum}");
        println!("top by cpu (machine {:.1}%):", s.cpu_pct);
        for a in &s.apps_by_cpu {
            println!("  {:<28} {:>5.1}%  x{}", a.exe, a.cpu_pct, a.processes);
        }
        // WSL is running on this PC; the query is asynchronous.
        let start = std::time::Instant::now();
        let mut snap = s;
        while start.elapsed() < std::time::Duration::from_secs(10) {
            if snap.wsl.as_ref().is_some_and(|w| !w.procs.is_empty()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
            snap = w.read(true);
        }
        let wsl = snap.wsl.expect("vmmemWSL exists while WSL runs");
        println!("wsl: {} MiB, {:.1}%", wsl.mem_bytes >> 20, wsl.cpu_pct);
        for p in &wsl.procs {
            println!(
                "  {:<30} agent={} x{} {} MiB",
                p.label,
                p.agent,
                p.count,
                p.mem_bytes >> 20
            );
        }
        assert!(wsl.mem_bytes > 0 && wsl.procs.len() <= 12);
        assert!(
            wsl.procs
                .iter()
                .any(|p| p.label == "Claude Code" && p.agent),
            "no Claude Code entry in {:?}",
            wsl.procs
        );
        assert!(w.read(false).wsl.is_none());
    }

    /// The reader against the PDH counter over the same window (a few seconds,
    /// `PULSE_COMPARE_SECS`, default 10): both are started together, both are
    /// read together. Prints per image name the reader's share and the
    /// counter's `% Processor Time` divided by the core count, for `System`,
    /// the busiest apps, `Idle` and `_Total`.
    #[test]
    fn compare_with_pdh_counter() {
        if !live("compare_with_pdh_counter") {
            return;
        }
        let secs: u64 = std::env::var("PULSE_COMPARE_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        let mut w = Windows::new();
        let cores = f64::from(w.cores);
        let q = perf::Query::new(&[r"\Process(*)\% Processor Time"]).expect("PDH query");
        let first = w.processes();
        w.cpu_shares(&first);
        q.collect();
        // The reader is polled once a second like the app does (a baseline
        // older than ten seconds is dropped on purpose); its figures are
        // averaged over the window with the length of each tick as weight.
        // One-second pairs for System, and the interrupt and DPC time the
        // counters keep apart from every process (Task Manager's Details tab
        // shows it as "System interrupts").
        let tick = perf::Query::new(&[
            r"\Process(System)\% Processor Time",
            r"\Processor Information(_Total)\% Interrupt Time",
            r"\Processor Information(_Total)\% DPC Time",
        ])
        .expect("PDH tick query");
        let (mut irq, mut dpc, mut n) = (0.0, 0.0, 0.0);
        println!("tick: System reader / System pdh / interrupt / dpc (percent of the machine)");
        let started = Instant::now();
        let mut ours: HashMap<String, f64> = HashMap::new();
        let mut last = started;
        while started.elapsed() < std::time::Duration::from_secs(secs) {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let procs = w.processes();
            let shares = w.cpu_shares(&procs);
            let now = Instant::now();
            let dt = now.duration_since(last).as_secs_f64();
            last = now;
            for (p, s) in procs.iter().zip(&shares) {
                let name = if p.pid == 0 { "idle" } else { &p.name };
                let name = name.trim_end_matches(".exe").to_ascii_lowercase();
                *ours.entry(name).or_default() += f64::from(*s) * dt;
            }
            tick.collect();
            let sys_reader = procs
                .iter()
                .zip(&shares)
                .find(|(p, _)| p.pid == 4)
                .map_or(0.0, |(_, s)| *s);
            let v = |i| tick.value(i).unwrap_or(0.0);
            println!(
                "  {:>6.2} {:>6.2} {:>6.2} {:>6.2}",
                sys_reader,
                v(0) / cores,
                v(1),
                v(2)
            );
            irq += v(1);
            dpc += v(2);
            n += 1.0;
        }
        println!("interrupts {:.2}%, dpc {:.2}% (mean)", irq / n, dpc / n);
        q.collect();
        let total = started.elapsed().as_secs_f64();
        for v in ours.values_mut() {
            *v /= total;
        }
        let pdh = q.instances(0).expect("PDH instances");
        let mut theirs: HashMap<String, f64> = HashMap::new();
        for (name, v, ok) in &pdh {
            if !ok {
                continue;
            }
            // "chrome#12" is the 13th chrome: fold it into "chrome".
            let base = name.split('#').next().unwrap_or(name).to_ascii_lowercase();
            *theirs.entry(base).or_default() += v / cores;
        }
        let mut names: Vec<&String> = theirs.keys().collect();
        names.sort_by(|a, b| theirs[*b].total_cmp(&theirs[*a]));
        println!("window {secs} s, {cores} cores; percent of the whole machine");
        println!("{:<28} {:>8} {:>8}", "image", "reader", "pdh");
        let mut shown = 0;
        for n in names {
            if matches!(n.as_str(), "_total" | "idle" | "system") || shown < 3 {
                if !matches!(n.as_str(), "_total" | "idle" | "system") {
                    shown += 1;
                }
                println!(
                    "{:<28} {:>8.2} {:>8.2}",
                    n,
                    ours.get(n).copied().unwrap_or(0.0),
                    theirs[n]
                );
            }
        }
        println!(
            "{:<28} {:>8.2} {:>8}",
            "(reader sum incl. idle)",
            ours.values().sum::<f64>(),
            ""
        );
        // PDH caps one instance at 100 (one core) unless asked not to, so only
        // instances below that are comparable.
        let mut compared = 0;
        for (name, theirs) in &theirs {
            let comparable = *theirs < 100.0 / cores - 0.5 && !matches!(name.as_str(), "_total");
            if !comparable || *theirs < 0.3 {
                continue;
            }
            let mine = ours.get(name).copied().unwrap_or(0.0);
            assert!(
                (mine - theirs).abs() <= 0.3 + 0.1 * theirs,
                "{name}: reader {mine:.2} against counter {theirs:.2}"
            );
            compared += 1;
        }
        assert!(compared > 0, "nothing to compare");
    }
}
