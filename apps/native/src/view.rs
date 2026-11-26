//! Turns readings into the strings, colours and sparkline paths the panel shows.
//! No toolkit types here, so it is plain code and unit tested on any machine.

use crate::marks::{self, Mark};
use pulse_platform::Sort;
use pulse_readings::{AppRow, Snapshot, Wsl, WslProc};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

const GB: f64 = 1_073_741_824.0;

/// The sparklines show this much time, newest at the right.
pub const WINDOW: Duration = Duration::from_secs(180);
/// Samples kept at most (one every 2 s fills the window).
pub const HISTORY: usize = 100;
/// App rows older than this no longer name "the app that uses the most".
pub const APPS_STALE: Duration = Duration::from_secs(10);
/// Sparkline box in px, as in the reference (`.fig` inside a 173 px tile).
const FIG_W: f32 = 149.0;
const FIG_H: f32 = 34.0;

/// Logical height of the panel when everything fits, and the vertical pitch of
/// one "Top apps" row (the rows start at 262.4 in the design).
pub const PANEL_H: f32 = 700.0;
const ROW_PITCH: f32 = 60.15;
/// Rows shown at full height, without and with the battery strip.
const ROWS_FULL: usize = 5;
const ROWS_FULL_BATTERY: usize = 4;
/// The list never gives up more than this: two rows always stay.
pub const MIN_ROWS: usize = 2;

/// How the panel fits a work area that may be shorter than [`PANEL_H`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// "Top apps" rows that fit at full size (the list scrolls beyond them).
    #[allow(dead_code)]
    pub rows: usize,
    /// Logical px removed from the panel height (the tiles and footer move up
    /// by the same amount).
    pub shrink: f32,
}

impl Layout {
    pub fn height(self) -> f32 {
        PANEL_H - self.shrink
    }
}

/// The most rows that fit in `avail_h` logical px, never fewer than
/// [`MIN_ROWS`]: header, tiles and footer always stay whole, the list gives
/// up rows instead.
pub fn fit(avail_h: f32, battery: bool) -> Layout {
    let full = if battery {
        ROWS_FULL_BATTERY
    } else {
        ROWS_FULL
    };
    let at = |rows: usize| Layout {
        rows,
        shrink: ((full - rows) as f32 * ROW_PITCH).round(),
    };
    (MIN_ROWS..=full)
        .rev()
        .map(at)
        .find(|l| l.height() <= avail_h)
        .unwrap_or(at(MIN_ROWS))
}

/// Recent percentages with the time they were read, for the two sparklines.
/// Readings come every 2 to 10 s, so the lines are drawn by time, not by count.
#[derive(Clone, Debug, Default)]
pub struct History {
    pub cpu: VecDeque<(Instant, f32)>,
    pub mem: VecDeque<(Instant, f32)>,
}

impl History {
    pub fn push(&mut self, snap: &Snapshot, at: Instant) {
        push_capped(&mut self.cpu, at, snap.cpu_pct);
        let pct = if snap.mem_total == 0 {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            {
                (snap.mem_used as f64 / snap.mem_total as f64 * 100.0) as f32
            }
        };
        push_capped(&mut self.mem, at, pct);
    }
}

fn push_capped(q: &mut VecDeque<(Instant, f32)>, at: Instant, v: f32) {
    // Drop what has left the window (one sample beyond it stays, so the line
    // starts at the left edge), and never keep more than HISTORY.
    while q.len() > 1 && at.saturating_duration_since(q[1].0) > WINDOW {
        q.pop_front();
    }
    if q.len() == HISTORY {
        q.pop_front();
    }
    q.push_back((at, v));
}

/// What a "Top apps" row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    App,
    /// The WSL virtual machine; its Linux programs follow as `Sub` rows.
    Wsl,
    Sub,
    /// "Reading apps..." before the first scan has finished.
    Reading,
}

/// One row of "Top apps", ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct AppView {
    pub kind: RowKind,
    pub letter: String,
    /// Red, green, blue of the icon tile.
    pub rgb: [u8; 3],
    pub name: String,
    pub detail: String,
    pub value: String,
    /// Bar length, 0 to 1, scaled to the top row.
    pub frac: f32,
    /// Executable path, for the real icon; `None` shows the coloured letter.
    pub path: Option<String>,
    /// Sub rows: a coding agent.
    pub agent: bool,
    /// Sub rows: "x3" text when more than one process, else empty.
    pub count: String,
    /// Last row of a WSL group: a hairline closes it.
    pub group_end: bool,
    /// Sub rows: the product mark that replaces the letter, when one is known.
    pub mark: Option<Mark>,
}

/// Everything the panel shows, as plain data.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub status: Status,
    pub cpu_big: String,
    pub cpu_sub: String,
    /// Uptime reached a week: " \u{b7} restart?" follows in the warning colour.
    pub cpu_restart: bool,
    pub cpu_line: String,
    pub cpu_fill: String,
    pub mem_big: String,
    pub mem_sub: String,
    pub mem_line: String,
    pub mem_fill: String,
    pub apps: Vec<AppView>,
    pub tiles: Tiles,
}

/// The line under the title and its dot colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub text: String,
    pub warn: bool,
}

/// The bottom row of tiles (and the battery tile when there is a battery).
#[derive(Clone, Debug, PartialEq)]
pub struct Tiles {
    pub gpu_big: String,
    /// No reading: smaller text, no bar fill.
    pub gpu_missing: bool,
    pub gpu_frac: f32,
    pub drive_big: String,
    pub drive_sub: String,
    pub drive_frac: f32,
    pub net_big: String,
    pub net_sub: String,
    pub battery: Option<BatteryView>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BatteryView {
    /// On battery at or under [`BATTERY_LOW`] percent: the bar takes the warning colour.
    pub low: bool,
    pub big: String,
    pub sub: String,
    pub frac: f32,
}

/// Readings beyond the first panel half. Stand-in until `Snapshot` carries them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extra {
    pub gpu_pct: Option<f32>,
    pub disk_free: u64,
    pub disk_total: u64,
    pub disk_read_bps: u64,
    pub disk_write_bps: u64,
    pub net_down_bps: u64,
    pub net_up_bps: u64,
    pub battery: Option<(u8, bool)>,
}

impl Extra {
    /// Longest plausible figures, for checking that no tile text is cut off
    /// (PULSE_STRESS=1, a development knob).
    pub fn stress() -> Self {
        Self {
            gpu_pct: None,
            disk_free: 999 * (1 << 30),
            disk_total: 1000 * (1 << 30),
            disk_read_bps: 600 * (1 << 20),
            disk_write_bps: 399 * (1 << 20),
            net_down_bps: 999 * 1024,
            net_up_bps: 99 * (1 << 20) + (1 << 20) * 9 / 10,
            battery: Some((100, false)),
        }
    }

    pub fn from_snapshot(s: &Snapshot) -> Self {
        Self {
            gpu_pct: s.gpu_pct,
            disk_free: s.disk_free,
            disk_total: s.disk_total,
            disk_read_bps: s.disk_read_bps,
            disk_write_bps: s.disk_write_bps,
            net_down_bps: s.net_down_bps,
            net_up_bps: s.net_up_bps,
            battery: s.battery.as_ref().map(|b| (b.pct, b.plugged)),
        }
    }
}

/// Very long names in every place a name can outgrow its space: app names and
/// exe lines, Linux program labels (with the agent badge and a count), and the
/// status line (busy, naming the app that uses the most). A development knob
/// (`PULSE_STRESS=1`), to check that every text ends in an ellipsis or wraps.
pub fn stress_snapshot(snap: &mut Snapshot) {
    let long = |name: &str, exe: &str, processes: u32, mem_gb: u64, cpu: f32| AppRow {
        name: name.into(),
        exe: exe.into(),
        processes,
        mem_bytes: mem_gb * (1 << 30),
        cpu_pct: cpu,
        path: None,
    };
    snap.cpu_pct = 92.0;
    snap.apps = vec![
        long(
            "Microsoft Visual Studio Code Insiders Extension Host (Preview)",
            "Code - Insiders - Extension Host Helper Renderer.exe",
            24,
            9,
            30.0,
        ),
        long("Chrome", "chrome.exe", 3, 6, 20.0),
        long(
            "Antimalware Service Executable with a very long name",
            "MsMpEng-long-name-for-the-stress-test-run.exe",
            1,
            3,
            10.0,
        ),
        long("Spotify", "Spotify.exe", 7, 2, 5.0),
    ];
    snap.apps_by_cpu = snap.apps.clone();
    snap.wsl = Some(Wsl {
        mem_bytes: 5 * (1 << 30),
        cpu_pct: 8.0,
        procs: vec![
            wsl_proc(&first_label(), true, 12, 900, 3.0),
            wsl_proc(
                "Claude Code with an extremely long label indeed",
                true,
                123,
                800,
                2.0,
            ),
            wsl_proc(
                "python3: /home/user/projects/some/very/long/path/server.py",
                false,
                2,
                400,
                1.0,
            ),
            wsl_proc("rust-analyzer", false, 1, 300, 0.5),
            wsl_proc("Claude Code", true, 3, 250, 0.4),
            wsl_proc("chrome-headless-shell", false, 4, 200, 0.3),
        ],
    });
}

/// The first Linux label of the stress rows. `PULSE_STRESS_LEN=<n>` cuts or
/// repeats it to n characters, to sweep lengths around the point where the
/// name starts to be cut.
fn first_label() -> String {
    let base = "node: pool_entry-[turbopack-node]_";
    match std::env::var("PULSE_STRESS_LEN")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        Some(n) => base.chars().cycle().take(n).collect(),
        None => base.into(),
    }
}

fn wsl_proc(label: &str, agent: bool, count: u32, mem_mb: u64, cpu: f32) -> WslProc {
    WslProc {
        label: label.into(),
        agent,
        count,
        mem_bytes: mem_mb * (1 << 20),
        cpu_pct: cpu,
    }
}

/// Processor use at or above this is "busy".
const CPU_BUSY: f32 = 85.0;
/// Memory use at or above this (percent) is "nearly full".
const MEM_FULL: u64 = 90;
/// Drive C: under this share free (percent) is "nearly full".
const DISK_LOW: u64 = 8;
/// On battery at or under this charge (percent) the battery bar warns.
const BATTERY_LOW: u8 = 15;
/// Uptime from which a restart is suggested.
const RESTART_AFTER_S: u64 = 7 * 86_400;

/// The header line, first match wins. `apps_fresh`: the app rows are recent
/// enough to say which app uses the most.
#[allow(clippy::cast_precision_loss)]
pub fn status(snap: &Snapshot, sample: bool, apps_fresh: bool) -> Status {
    let warn = |text: String| Status { text, warn: true };
    if snap.cpu_pct >= CPU_BUSY {
        return warn(match snap.apps_by_cpu.first().filter(|_| apps_fresh) {
            Some(a) => format!("Processor is busy \u{b7} {} uses the most", a.name),
            None => "Processor is busy".into(),
        });
    }
    if snap.mem_total > 0 && snap.mem_used * 100 >= snap.mem_total * MEM_FULL {
        return warn("Memory is nearly full".into());
    }
    if snap.disk_total > 0 && snap.disk_free * 100 < snap.disk_total * DISK_LOW {
        return warn("Drive C: is nearly full".into());
    }
    Status {
        text: if sample {
            "Sample data, not this PC"
        } else {
            "Running smoothly"
        }
        .into(),
        warn: false,
    }
}

/// The hover text of the tray icon, up to four lines.
#[allow(clippy::cast_precision_loss)]
pub fn tooltip(snap: &Snapshot) -> String {
    let mut lines = vec![format!(
        "CPU {}%  \u{b7}  RAM {:.1} / {} GB",
        round_half_up(f64::from(snap.cpu_pct)),
        snap.mem_used as f64 / GB,
        round_half_up(snap.mem_total as f64 / GB)
    )];
    if let Some(g) = snap.gpu_pct {
        lines.push(format!("GPU {}%", round_half_up(f64::from(g))));
    }
    lines.push(format!(
        "Net \u{2193} {}/s  \u{2191} {}/s",
        format_bytes(snap.net_down_bps),
        format_bytes(snap.net_up_bps)
    ));
    if let Some(b) = &snap.battery {
        lines.push(format!(
            "Battery {}% ({})",
            b.pct,
            if b.plugged {
                "plugged in"
            } else {
                "on battery"
            }
        ));
    }
    lines.join("\n")
}

/// Footer, left side: the key cap text (empty when the shortcut is off) and
/// the sentence after it.
pub fn footer_hint(hotkey: &str) -> (String, &'static str) {
    if hotkey.is_empty() {
        (String::new(), "Click the tray icon to open Pulse")
    } else {
        (hotkey.to_owned(), FOOT_LONG)
    }
}

pub const FOOT_LONG: &str = "opens and closes Pulse";
/// Used instead when a long key cap leaves no room for [`FOOT_LONG`].
pub const FOOT_SHORT: &str = "toggles Pulse";

/// Footer, right side.
pub fn updated_ago(secs: u64) -> String {
    format!("Updated {secs}s ago")
}

/// Share of the machine the processor bar is scaled against at least.
const CPU_BAR_FLOOR: f64 = 5.0;

/// The WSL machine is `vmmemWSL`; a plain `vmmem` is a Hyper-V or Sandbox
/// machine and stays an ordinary row.
fn is_wsl(a: &AppRow) -> bool {
    let exe = a.exe.strip_suffix(".exe").unwrap_or(&a.exe);
    exe.eq_ignore_ascii_case("vmmemwsl") || a.name == "WSL (Linux)"
}

/// Where the WSL row's penguin comes from.
fn wsl_icon() -> String {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    format!(r"{root}\System32\wsl.exe")
}

fn app_view(a: &AppRow, sort: Sort, wsl: Option<&Wsl>, max: f64) -> AppView {
    let base = a.exe.strip_suffix(".exe").unwrap_or(&a.exe);
    let wsl_row = wsl.is_some() && is_wsl(a);
    #[allow(clippy::cast_precision_loss)]
    let (value, amount) = match sort {
        Sort::Memory => (format_bytes(a.mem_bytes), a.mem_bytes as f64),
        Sort::Processor => (cpu_text(a.cpu_pct), f64::from(a.cpu_pct)),
    };
    AppView {
        kind: if wsl_row { RowKind::Wsl } else { RowKind::App },
        letter: a.name.chars().next().map(String::from).unwrap_or_default(),
        rgb: hsl_rgb(hue(base), 0.45, 0.45),
        name: a.name.clone(),
        detail: if wsl_row {
            wsl_detail(wsl)
        } else if a.processes > 1 {
            format!("{} \u{b7} {} processes", a.exe, a.processes)
        } else {
            a.exe.clone()
        },
        value,
        frac: (amount / max).clamp(0.0, 1.0) as f32,
        path: if wsl_row {
            Some(wsl_icon())
        } else {
            a.path.clone()
        },
        agent: false,
        count: String::new(),
        group_end: false,
        mark: None,
    }
}

fn wsl_detail(wsl: Option<&Wsl>) -> String {
    if wsl.is_some_and(|w| !w.procs.is_empty()) {
        "Linux virtual machine \u{b7} inside:".into()
    } else {
        "Linux virtual machine".into()
    }
}

fn cpu_text(pct: f32) -> String {
    format!("{pct:.1}%")
}

/// The row shown until the first scan of the apps has finished.
fn reading_row() -> AppView {
    AppView {
        kind: RowKind::Reading,
        letter: String::new(),
        rgb: [0, 0, 0],
        name: "Reading apps\u{2026}".into(),
        detail: String::new(),
        value: String::new(),
        frac: 0.0,
        path: None,
        agent: false,
        count: String::new(),
        group_end: false,
        mark: None,
    }
}

/// Linux program rows shown under the WSL row.
const SUB_ROWS: usize = 6;

/// Share of the machine the Linux processor bars are scaled against at least.
const SUB_CPU_BAR_FLOOR: f64 = 1.0;

/// The Linux rows for the chosen sort: the first [`SUB_ROWS`] programs by
/// memory, or by processor (memory breaks ties), with the same value as the
/// app rows above them.
#[allow(clippy::cast_precision_loss)]
fn sub_rows(w: &Wsl, sort: Sort) -> Vec<AppView> {
    let mut procs: Vec<&WslProc> = w.procs.iter().collect();
    match sort {
        Sort::Memory => procs.sort_by(|a, b| {
            b.mem_bytes
                .cmp(&a.mem_bytes)
                .then_with(|| a.label.cmp(&b.label))
        }),
        Sort::Processor => procs.sort_by(|a, b| {
            b.cpu_pct
                .total_cmp(&a.cpu_pct)
                .then_with(|| b.mem_bytes.cmp(&a.mem_bytes))
                .then_with(|| a.label.cmp(&b.label))
        }),
    }
    procs.truncate(SUB_ROWS);
    let amount = |p: &WslProc| match sort {
        Sort::Memory => p.mem_bytes as f64,
        Sort::Processor => f64::from(p.cpu_pct),
    };
    let top = procs.iter().map(|p| amount(p)).fold(
        if sort == Sort::Memory {
            1.0
        } else {
            SUB_CPU_BAR_FLOOR
        },
        f64::max,
    );
    procs
        .into_iter()
        .map(|p| {
            let mark = marks::for_label(&p.label);
            AppView {
                kind: RowKind::Sub,
                letter: p.label.chars().next().map(String::from).unwrap_or_default(),
                rgb: mark.map_or_else(|| hsl_rgb(hue(&p.label), 0.45, 0.45), |m| m.rgb),
                name: p.label.clone(),
                detail: String::new(),
                value: match sort {
                    Sort::Memory => format_bytes(p.mem_bytes),
                    Sort::Processor => cpu_text(p.cpu_pct),
                },
                frac: (amount(p) / top).clamp(0.0, 1.0) as f32,
                path: None,
                agent: p.agent,
                count: if p.count > 1 {
                    format!("\u{d7}{}", p.count)
                } else {
                    String::new()
                },
                group_end: false,
                mark,
            }
        })
        .collect()
}

/// The "Top apps" rows for the chosen sort: the apps, the WSL row when WSL
/// runs (in its sorted place, among the apps or between them) and its Linux
/// rows. Bars are scaled to the biggest row, WSL included.
#[allow(clippy::cast_precision_loss)]
pub fn app_rows(snap: &Snapshot, sort: Sort) -> Vec<AppView> {
    let apps = match sort {
        Sort::Memory => &snap.apps,
        Sort::Processor => &snap.apps_by_cpu,
    };
    if apps.is_empty() {
        return vec![reading_row()];
    }
    let wsl = snap.wsl.as_ref();
    let amount = |a: &AppRow| match sort {
        Sort::Memory => a.mem_bytes as f64,
        Sort::Processor => f64::from(a.cpu_pct),
    };
    let extra = match wsl {
        Some(_) if !apps.iter().any(is_wsl) => wsl.map(|w| AppRow {
            name: "WSL (Linux)".into(),
            exe: "vmmemWSL".into(),
            processes: 1,
            mem_bytes: w.mem_bytes,
            cpu_pct: w.cpu_pct,
            path: None,
        }),
        _ => None,
    };
    let mut list: Vec<&AppRow> = apps.iter().collect();
    if let Some(e) = &extra {
        let at = list
            .iter()
            .position(|a| amount(a) < amount(e))
            .unwrap_or(list.len());
        list.insert(at, e);
    }
    let max = list.iter().map(|a| amount(a)).fold(
        if sort == Sort::Memory {
            1.0
        } else {
            CPU_BAR_FLOOR
        },
        f64::max,
    );
    let mut rows = Vec::new();
    for a in list {
        let row = app_view(a, sort, wsl, max);
        let is_wsl_row = row.kind == RowKind::Wsl;
        rows.push(row);
        if let (true, Some(w)) = (is_wsl_row, wsl) {
            rows.extend(sub_rows(w, sort));
            if let Some(last) = rows.last_mut().filter(|r| r.kind == RowKind::Sub) {
                last.group_end = true;
            }
        }
    }
    rows
}

#[allow(clippy::cast_precision_loss)]
pub fn build(
    snap: &Snapshot,
    extra: &Extra,
    hist: &History,
    sample: bool,
    sort: Sort,
    now: Instant,
    apps_at: Instant,
) -> View {
    let used = snap.mem_used as f64;
    let total = snap.mem_total as f64;
    let (cpu_line, cpu_fill) = sparkline(&hist.cpu, now);
    let (mem_line, mem_fill) = sparkline(&hist.mem, now);
    let apps_fresh = now.saturating_duration_since(apps_at) <= APPS_STALE;
    View {
        status: status(snap, sample, apps_fresh),
        cpu_big: format!("{}%", round_half_up(f64::from(snap.cpu_pct))),
        // The tile is 149 px wide: with " \u{b7} restart?" after it there is only
        // room for the uptime, so the core count gives way.
        cpu_sub: if snap.uptime_s >= RESTART_AFTER_S {
            format!("up {}", uptime(snap.uptime_s))
        } else {
            format!("{} cores \u{b7} up {}", snap.cores, uptime(snap.uptime_s))
        },
        cpu_restart: snap.uptime_s >= RESTART_AFTER_S,
        cpu_line,
        cpu_fill,
        mem_big: format!("{:.1} GB", used / GB),
        mem_sub: format!(
            "of {} GB \u{b7} {}%",
            round_half_up(total / GB),
            round_half_up(if total > 0.0 {
                used / total * 100.0
            } else {
                0.0
            })
        ),
        mem_line,
        mem_fill,
        apps: app_rows(snap, sort),
        tiles: tiles(extra),
    }
}

#[allow(clippy::cast_precision_loss)]
fn tiles(x: &Extra) -> Tiles {
    let used_share = if x.disk_total == 0 {
        0.0
    } else {
        x.disk_total.saturating_sub(x.disk_free) as f64 / x.disk_total as f64
    };
    Tiles {
        gpu_big: x.gpu_pct.map_or_else(
            || "Not reported".into(),
            |g| format!("{}%", round_half_up(f64::from(g))),
        ),
        gpu_missing: x.gpu_pct.is_none(),
        gpu_frac: x.gpu_pct.map_or(0.0, |g| (g / 100.0).clamp(0.0, 1.0)),
        drive_big: format_bytes(x.disk_free),
        drive_sub: format!(
            "{}/s",
            format_bytes_at(x.disk_read_bps + x.disk_write_bps, 10.0)
        ),
        drive_frac: used_share as f32,
        net_big: format!("\u{2193} {}/s", format_bytes(x.net_down_bps)),
        net_sub: format!("\u{2191} {}/s", format_bytes(x.net_up_bps)),
        battery: x.battery.map(|(pct, plugged)| BatteryView {
            low: !plugged && pct <= BATTERY_LOW,
            big: format!("{pct}%"),
            sub: if plugged { "Plugged in" } else { "On battery" }.into(),
            frac: f32::from(pct.min(100)) / 100.0,
        }),
    }
}

fn round_half_up(v: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation)]
    {
        (v + 0.5).floor() as i64
    }
}

/// "3d 4h" like the reference; under a day it is "4h 12m".
pub fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs / 3_600 % 24, secs / 60 % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else {
        format!("{h}h {m}m")
    }
}

/// The reference page's `fb`: 1024 steps, one decimal under 100.
#[allow(clippy::cast_precision_loss)]
pub fn format_bytes(bytes: u64) -> String {
    format_bytes_at(bytes, 100.0)
}

/// Like `format_bytes`, with whole numbers from `whole_from` up (10 for rates).
/// The figure is rounded first and the unit chosen after, so 999.6 KB reads
/// "1.0 MB" and 99.96 MB reads "100 MB", never "1000 KB" or "100.0 MB".
#[allow(clippy::cast_precision_loss)]
fn format_bytes_at(bytes: u64, whole_from: f64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut b = bytes as f64;
    let mut i = 0;
    while b >= 1000.0 && i < 3 {
        b /= 1024.0;
        i += 1;
    }
    loop {
        let whole = round_half_up(b);
        if i > 0 && i < 3 && whole >= 1000 {
            b /= 1024.0;
            i += 1;
            continue;
        }
        let tenth = (b * 10.0 + 0.5).floor() / 10.0;
        return if i == 0 || tenth >= whole_from {
            format!("{whole} {}", UNITS[i])
        } else {
            format!("{tenth:.1} {}", UNITS[i])
        };
    }
}

/// The reference page's `hue`: a 31-multiplier string hash over UTF-16 units.
pub fn hue(name: &str) -> f32 {
    let mut h: u32 = 0;
    for c in name.encode_utf16() {
        h = (h * 31 + u32::from(c)) % 360;
    }
    #[allow(clippy::cast_precision_loss)]
    {
        h as f32
    }
}

pub fn hsl_rgb(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match h {
        h if h < 60.0 => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let q = |v: f32| ((v + m) * 255.0).round() as u8;
    [q(r), q(g), q(b)]
}

/// Sparkline as two SVG path strings (line, and the closed area under it).
/// Each sample sits at its age: the right edge is `now`, the left edge is
/// [`WINDOW`] ago, so samples taken every 10 s are spread, not bunched.
#[allow(clippy::cast_precision_loss)]
pub fn sparkline(values: &VecDeque<(Instant, f32)>, now: Instant) -> (String, String) {
    let x_of = |at: Instant| {
        let age = now.saturating_duration_since(at).as_secs_f32();
        (1.0 - age / WINDOW.as_secs_f32()) * FIG_W
    };
    let points: Vec<(f32, f32)> = values
        .iter()
        .filter(|(at, _)| x_of(*at) >= 0.0)
        .map(|&(at, v)| {
            let y = FIG_H - 2.0 - (v / 100.0).clamp(0.0, 1.0) * (FIG_H - 5.0);
            (x_of(at), y)
        })
        .collect();
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return (String::new(), String::new());
    };
    let mut line = String::new();
    for (i, (x, y)) in points.iter().enumerate() {
        let _ = write!(line, "{}{x:.2} {y:.2} ", if i == 0 { "M" } else { "L" });
    }
    let mut fill = line.clone();
    let _ = write!(fill, "L{:.2} {FIG_H} L{:.2} {FIG_H} Z", last.0, first.0);
    (line.trim_end().to_owned(), fill)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_readings::{Sample, Source};

    #[test]
    fn bytes_match_the_reference_page() {
        assert_eq!(format_bytes(2100 * (1 << 20)), "2.1 GB");
        assert_eq!(format_bytes(622 * (1 << 20)), "622 MB");
        assert_eq!(format_bytes(184 * (1 << 20)), "184 MB");
        assert_eq!(format_bytes(900), "900 B");
        assert_eq!(format_bytes(5 << 30), "5.0 GB");
    }

    #[test]
    fn hue_matches_the_reference_page() {
        // JS: for (const c of 'chrome') h = (h * 31 + c.charCodeAt(0)) % 360
        assert!((hue("chrome") - 98.0).abs() < f32::EPSILON);
        assert_eq!(hsl_rgb(0.0, 0.0, 0.5), [128, 128, 128]);
    }

    #[test]
    fn uptime_reads_like_the_design() {
        assert_eq!(uptime(3 * 86_400 + 4 * 3_600 + 5), "3d 4h");
        assert_eq!(uptime(3_600 * 2 + 60 * 7), "2h 7m");
    }

    fn at(t0: Instant, secs: u64) -> Instant {
        t0 + Duration::from_secs(secs)
    }

    #[test]
    fn sparkline_right_aligns_a_short_history() {
        let t0 = Instant::now();
        let v: VecDeque<(Instant, f32)> = [(t0, 0.0), (at(t0, 2), 100.0)].into();
        let (line, fill) = sparkline(&v, at(t0, 2));
        assert!(line.starts_with("M147.34 32.00 L149.00 3.00"), "{line}");
        assert!(fill.ends_with("L149.00 34 L147.34 34 Z"), "{fill}");
        assert_eq!(
            sparkline(&VecDeque::new(), t0),
            (String::new(), String::new())
        );
    }

    #[test]
    fn sparkline_is_drawn_by_time_not_by_count() {
        let t0 = Instant::now();
        // Samples 10 s apart (hidden refresh) are 10/180 of the width apart.
        let slow: VecDeque<(Instant, f32)> =
            [(t0, 50.0), (at(t0, 10), 50.0), (at(t0, 20), 50.0)].into();
        let (line, _) = sparkline(&slow, at(t0, 20));
        let xs: Vec<f32> = line
            .split(['M', 'L'])
            .filter(|p| !p.is_empty())
            .map(|p| p.split_whitespace().next().unwrap().parse().unwrap())
            .collect();
        assert_eq!(xs.len(), 3);
        let gap = FIG_W * 10.0 / 180.0;
        assert!((xs[2] - FIG_W).abs() < 0.01);
        assert!((xs[2] - xs[1] - gap).abs() < 0.02, "{xs:?}");
        assert!((xs[1] - xs[0] - gap).abs() < 0.02, "{xs:?}");
        // Samples older than three minutes are not drawn.
        let old: VecDeque<(Instant, f32)> = [(t0, 50.0), (at(t0, 200), 50.0)].into();
        let (line, _) = sparkline(&old, at(t0, 200));
        assert_eq!(line.matches('L').count(), 0, "{line}");
        assert!(line.starts_with("M149.00"), "{line}");
        // The left edge is exactly three minutes ago.
        let edge: VecDeque<(Instant, f32)> = [(t0, 50.0), (at(t0, 180), 50.0)].into();
        assert!(sparkline(&edge, at(t0, 180)).0.starts_with("M0.00"));
    }

    #[test]
    fn history_is_capped_and_forgets_what_left_the_window() {
        let mut h = History::default();
        let snap = Sample::default().read(true);
        let t0 = Instant::now();
        for i in 0..HISTORY as u64 + 10 {
            h.push(&snap, at(t0, i));
        }
        assert_eq!(h.cpu.len(), HISTORY);
        assert_eq!(h.mem.len(), HISTORY);
        // Slow samples: only the window (plus one beyond it) is kept.
        let mut h = History::default();
        for i in 0..40 {
            h.push(&snap, at(t0, i * 10));
        }
        assert!(h.cpu.len() <= 20, "{}", h.cpu.len());
        let newest = h.cpu.back().unwrap().0;
        let oldest = h.cpu.front().unwrap().0;
        assert!(newest - oldest >= WINDOW);
        assert!(newest - oldest <= WINDOW + Duration::from_secs(10));
    }

    fn sample() -> Snapshot {
        Sample::default().read(true)
    }

    #[test]
    fn view_of_the_sample_lists_apps_then_the_wsl_group() {
        let snap = sample();
        let mut h = History::default();
        let now = Instant::now();
        h.push(&snap, now);
        let v = build(
            &snap,
            &Extra::from_snapshot(&snap),
            &h,
            true,
            Sort::Memory,
            now,
            now,
        );
        // 6 apps, the WSL row (biggest, so first), then its Linux rows.
        let wsl = snap.wsl.as_ref().unwrap();
        assert_eq!(v.apps.len(), 6 + 1 + wsl.procs.len());
        assert_eq!(v.apps[0].kind, RowKind::Wsl);
        assert!((v.apps[0].frac - 1.0).abs() < f32::EPSILON);
        let chrome = v.apps.iter().find(|a| a.name == "Google Chrome").unwrap();
        assert_eq!(chrome.detail, "chrome.exe \u{b7} 24 processes");
        assert!(chrome.frac < 1.0);
        assert!(v.apps.iter().any(|a| a.detail == "obs64.exe"));
        assert_eq!(v.mem_sub, "of 32 GB \u{b7} 43%");
        assert_eq!(v.cpu_sub, "16 cores \u{b7} up 3d 4h");
        assert!(!v.cpu_restart);
        assert_eq!(v.tiles.drive_big, "296 GB");
        assert!(
            v.tiles.net_big.starts_with("\u{2193} ") && v.tiles.net_sub.starts_with("\u{2191} ")
        );
        assert!(!v.tiles.gpu_missing);
    }

    #[test]
    fn memory_sort_shows_sizes_and_processor_sort_shows_percent() {
        let snap = sample();
        let mem = app_rows(&snap, Sort::Memory);
        assert!(mem[0].value.ends_with("GB"), "{}", mem[0].value);
        let first_app = mem.iter().find(|r| r.kind == RowKind::App).unwrap();
        assert_eq!(first_app.name, snap.apps[0].name);
        let cpu = app_rows(&snap, Sort::Processor);
        assert_eq!(cpu[0].name, snap.apps_by_cpu[0].name);
        assert!(cpu[0].value.ends_with('%') && cpu[0].value.contains('.'));
        assert!((cpu[0].frac - 1.0).abs() < 1e-6 || cpu[0].frac > 0.0);
    }

    #[test]
    fn processor_bars_are_scaled_against_at_least_five_percent() {
        let mut snap = sample();
        for a in &mut snap.apps_by_cpu {
            a.cpu_pct *= 0.1;
        }
        snap.wsl = None;
        let top = snap.apps_by_cpu[0].cpu_pct;
        assert!(top < 5.0);
        let rows = app_rows(&snap, Sort::Processor);
        assert!((rows[0].frac - top / 5.0).abs() < 1e-5, "{}", rows[0].frac);
    }

    #[test]
    fn wsl_is_listed_once_with_its_linux_rows() {
        let snap = sample();
        let rows = app_rows(&snap, Sort::Memory);
        let wsl_rows: Vec<_> = rows.iter().filter(|r| r.kind == RowKind::Wsl).collect();
        assert_eq!(wsl_rows.len(), 1);
        let w = wsl_rows[0];
        assert_eq!(w.name, "WSL (Linux)");
        assert_eq!(w.detail, "Linux virtual machine \u{b7} inside:");
        assert!(w.path.as_deref().is_some_and(|p| p.ends_with("wsl.exe")));
        let subs: Vec<_> = rows.iter().filter(|r| r.kind == RowKind::Sub).collect();
        assert_eq!(subs.len(), snap.wsl.as_ref().unwrap().procs.len());
        assert!(subs[0].agent && subs[0].count == "\u{d7}3");
        assert!(!subs[1].agent && subs[1].count == "\u{d7}6");
        assert_eq!(rows.iter().filter(|r| r.group_end).count(), 1);
        // Sub rows come right after the WSL row, the last one closes the group.
        let at = rows.iter().position(|r| r.kind == RowKind::Wsl).unwrap();
        let n = subs.len();
        assert!(rows[at + 1..=at + n].iter().all(|r| r.kind == RowKind::Sub));
        assert!(rows[at + n].group_end);
    }

    #[test]
    fn wsl_already_among_the_apps_is_not_added_twice_and_stays_in_place() {
        let mut snap = sample();
        snap.apps[1] = AppRow {
            name: "WSL (Linux)".into(),
            exe: "vmmemWSL".into(),
            mem_bytes: snap.apps[1].mem_bytes,
            ..snap.apps[1].clone()
        };
        let rows = app_rows(&snap, Sort::Memory);
        assert_eq!(rows.iter().filter(|r| r.kind == RowKind::Wsl).count(), 1);
        assert_eq!(rows[1].kind, RowKind::Wsl);
        assert_eq!(rows[2].kind, RowKind::Sub);
        assert_eq!(rows.iter().filter(|r| r.name == "WSL (Linux)").count(), 1);
    }

    #[test]
    fn no_wsl_means_no_wsl_rows_and_a_bare_vm_has_no_inside_label() {
        let mut snap = sample();
        snap.wsl = None;
        assert!(
            app_rows(&snap, Sort::Memory)
                .iter()
                .all(|r| r.kind == RowKind::App)
        );
        snap.wsl = Some(Wsl {
            mem_bytes: 1 << 30,
            cpu_pct: 0.5,
            procs: Vec::new(),
        });
        let rows = app_rows(&snap, Sort::Memory);
        let w = rows.iter().find(|r| r.kind == RowKind::Wsl).unwrap();
        assert_eq!(w.detail, "Linux virtual machine");
        assert!(rows.iter().all(|r| !r.group_end));
    }

    fn subs(rows: &[AppView]) -> Vec<&AppView> {
        rows.iter().filter(|r| r.kind == RowKind::Sub).collect()
    }

    #[test]
    fn linux_rows_follow_the_sort_like_every_other_row() {
        let mut snap = sample();
        // The sample's figures move with its clock: pin them.
        for p in &mut snap.wsl.as_mut().unwrap().procs {
            p.cpu_pct = match p.label.as_str() {
                "next-server (v16.4.6)" => 0.9,
                "chrome-headless-shell" => 0.6,
                "Claude Code" => 0.4,
                _ => 0.05,
            };
        }
        let w = snap.wsl.as_ref().unwrap();
        // Memory sort: sizes, biggest first (unchanged).
        let mem = app_rows(&snap, Sort::Memory);
        let m = subs(&mem);
        assert!(m.iter().all(|r| r.value.ends_with("MB")), "{m:?}");
        let by_mem: Vec<&str> = {
            let mut p: Vec<_> = w.procs.iter().collect();
            p.sort_by_key(|p| std::cmp::Reverse(p.mem_bytes));
            p.iter().map(|p| p.label.as_str()).collect()
        };
        assert_eq!(
            m.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            by_mem
        );
        // Processor sort: percent with one decimal, busiest first.
        let cpu = app_rows(&snap, Sort::Processor);
        let c = subs(&cpu);
        assert_eq!(c.len(), w.procs.len());
        assert!(c.iter().all(|r| r.value.ends_with('%')), "{c:?}");
        let by_cpu: Vec<&str> = {
            let mut p: Vec<_> = w.procs.iter().collect();
            p.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
            p.iter().map(|p| p.label.as_str()).collect()
        };
        assert_eq!(
            c.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            by_cpu
        );
        assert_eq!(c[0].name, "next-server (v16.4.6)");
        assert_eq!(c[0].value, "0.9%");
        assert_eq!(c[1].value, "0.6%");
        // Bars are scaled to at least 1% of the machine.
        assert!((c[0].frac - 0.9).abs() < 1e-5, "{}", c[0].frac);
        // The group still closes with its hairline.
        assert!(c.last().unwrap().group_end);
    }

    #[test]
    fn linux_processor_ties_fall_back_to_memory_and_bars_have_a_floor() {
        let mut snap = sample();
        let w = snap.wsl.as_mut().unwrap();
        for p in &mut w.procs {
            p.cpu_pct = 0.0;
        }
        let rows = app_rows(&snap, Sort::Processor);
        let c = subs(&rows);
        assert_eq!(c[0].name, "Claude Code");
        assert!(c.iter().all(|r| r.value == "0.0%" && r.frac == 0.0));
        // A busy 2% program fills the bar by itself; 0.5% against it is a quarter.
        let w = snap.wsl.as_mut().unwrap();
        w.procs[0].cpu_pct = 2.0;
        w.procs[1].cpu_pct = 0.5;
        let rows = app_rows(&snap, Sort::Processor);
        let c = subs(&rows);
        assert!((c[0].frac - 1.0).abs() < 1e-6);
        assert!((c[1].frac - 0.25).abs() < 1e-6);
    }

    #[test]
    fn only_six_linux_rows_are_shown_in_either_sort() {
        let mut snap = sample();
        let w = snap.wsl.as_mut().unwrap();
        for i in 0..4u8 {
            w.procs.push(WslProc {
                label: format!("tool{i}"),
                mem_bytes: 1 << 20,
                // The smallest in memory but the busiest.
                cpu_pct: 5.0 + f32::from(i),
                ..WslProc::default()
            });
        }
        let mem = app_rows(&snap, Sort::Memory);
        assert_eq!(subs(&mem).len(), SUB_ROWS);
        assert!(subs(&mem).iter().all(|r| !r.name.starts_with("tool")));
        let cpu = app_rows(&snap, Sort::Processor);
        let c = subs(&cpu);
        assert_eq!(c.len(), SUB_ROWS);
        assert_eq!(
            c.iter()
                .take(4)
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            ["tool3", "tool2", "tool1", "tool0"]
        );
    }

    #[test]
    fn linux_rows_get_their_mark_or_keep_the_letter() {
        let mut snap = sample();
        snap.wsl.as_mut().unwrap().procs.push(WslProc {
            label: "tmux".into(),
            mem_bytes: 1,
            ..WslProc::default()
        });
        let rows = app_rows(&snap, Sort::Memory);
        let find = |n: &str| rows.iter().find(|r| r.name == n).unwrap().clone();
        let claude = find("Claude Code");
        assert_eq!(claude.mark.map(|m| m.slug), Some("claude"));
        assert_eq!(claude.rgb, [0xD9, 0x77, 0x57]);
        assert_eq!(
            find("next-server (v16.4.6)").mark.map(|m| m.slug),
            Some("nextdotjs")
        );
        assert_eq!(
            find("chrome-headless-shell").mark.map(|m| m.slug),
            Some("googlechrome")
        );
        // No mark in Simple Icons: the letter tile stays, in the hashed colour.
        for name in ["Codex CLI", "Editor server"] {
            let r = find(name);
            assert!(r.mark.is_none(), "{name}");
            assert_eq!(r.rgb, hsl_rgb(hue(name), 0.45, 0.45));
            assert_eq!(r.letter, name[..1]);
        }
        // App rows never carry one, and the sort does not change the marks.
        assert!(
            rows.iter()
                .filter(|r| r.kind != RowKind::Sub)
                .all(|r| r.mark.is_none())
        );
        let cpu = app_rows(&snap, Sort::Processor);
        assert_eq!(
            subs(&cpu).iter().filter(|r| r.mark.is_some()).count(),
            subs(&rows)
                .iter()
                .filter(|r| r.mark.is_some())
                .count()
                .min(SUB_ROWS)
        );
    }

    #[test]
    fn status_first_match_wins() {
        let mut s = sample();
        assert_eq!(
            status(&s, false, true),
            Status {
                text: "Running smoothly".into(),
                warn: false
            }
        );
        assert_eq!(status(&s, true, true).text, "Sample data, not this PC");
        s.disk_free = s.disk_total / 20;
        assert_eq!(status(&s, false, true).text, "Drive C: is nearly full");
        assert!(status(&s, false, true).warn);
        s.mem_used = s.mem_total * 91 / 100;
        assert_eq!(status(&s, false, true).text, "Memory is nearly full");
        s.cpu_pct = 85.0;
        let st = status(&s, false, true);
        assert_eq!(
            st.text,
            format!(
                "Processor is busy \u{b7} {} uses the most",
                s.apps_by_cpu[0].name
            )
        );
        assert!(st.warn);
        s.cpu_pct = 84.9;
        assert_eq!(status(&s, false, true).text, "Memory is nearly full");
        s.mem_used = s.mem_total * 89 / 100;
        s.disk_free = s.disk_total * 8 / 100 + 1;
        assert_eq!(status(&s, false, true).text, "Running smoothly");
    }

    #[test]
    fn a_week_of_uptime_asks_for_a_restart() {
        let mut snap = sample();
        snap.uptime_s = 7 * 86_400 - 1;
        assert!(
            !build(
                &snap,
                &Extra::default(),
                &History::default(),
                false,
                Sort::Memory,
                Instant::now(),
                Instant::now()
            )
            .cpu_restart
        );
        snap.uptime_s = 7 * 86_400;
        let v = build(
            &snap,
            &Extra::default(),
            &History::default(),
            false,
            Sort::Memory,
            Instant::now(),
            Instant::now(),
        );
        assert!(v.cpu_restart);
        assert_eq!(v.cpu_sub, "up 7d 0h");
    }

    #[test]
    fn tooltip_has_up_to_four_lines() {
        let mut s = sample();
        s.cpu_pct = 24.0;
        s.mem_used = 13_800_000_000;
        s.mem_total = 32 * (1 << 30);
        s.gpu_pct = Some(13.0);
        s.net_down_bps = 1_800_000;
        s.net_up_bps = 286 * 1024;
        s.battery = Some(pulse_readings::Battery {
            pct: 80,
            plugged: true,
        });
        let t = tooltip(&s);
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "CPU 24%  \u{b7}  RAM 12.9 / 32 GB");
        assert_eq!(lines[1], "GPU 13%");
        assert_eq!(lines[2], "Net \u{2193} 1.7 MB/s  \u{2191} 286 KB/s");
        assert_eq!(lines[3], "Battery 80% (plugged in)");
        s.gpu_pct = None;
        s.battery = Some(pulse_readings::Battery {
            pct: 40,
            plugged: false,
        });
        let t = tooltip(&s);
        assert_eq!(t.lines().count(), 3);
        assert!(t.ends_with("Battery 40% (on battery)"));
        s.battery = None;
        assert_eq!(tooltip(&s).lines().count(), 2);
        assert!(tooltip(&s).len() < 127);
    }

    #[test]
    fn footer_texts() {
        assert_eq!(
            footer_hint("Alt+Shift+P"),
            ("Alt+Shift+P".to_owned(), "opens and closes Pulse")
        );
        assert_eq!(
            footer_hint(""),
            (String::new(), "Click the tray icon to open Pulse")
        );
        assert_eq!(updated_ago(0), "Updated 0s ago");
        assert_eq!(updated_ago(7), "Updated 7s ago");
    }

    #[test]
    fn a_tall_work_area_shows_everything() {
        let l = fit(1000.0, false);
        assert_eq!((l.rows, l.shrink, l.height()), (5, 0.0, 700.0));
        assert_eq!(fit(700.0, false).rows, 5);
        assert_eq!(fit(1000.0, true).rows, 4);
        assert_eq!(fit(700.0, true).height(), 700.0);
    }

    #[test]
    fn a_short_work_area_gives_up_rows_and_keeps_the_footer() {
        // One row less frees 60 px.
        let l = fit(680.0, false);
        assert_eq!((l.rows, l.height()), (4, 640.0));
        let l = fit(600.0, false);
        assert_eq!((l.rows, l.height()), (3, 580.0));
        let l = fit(520.0, false);
        assert_eq!((l.rows, l.height()), (2, 520.0));
        for h in (300..900).map(|h| h as f32) {
            let l = fit(h, h as i32 % 2 == 0);
            assert!(l.rows >= MIN_ROWS && l.rows <= 5);
            // Either it fits, or it is at the minimum.
            assert!(l.height() <= h || l.rows == MIN_ROWS, "{h}");
        }
    }

    #[test]
    fn the_battery_strip_costs_a_row() {
        assert_eq!(fit(1000.0, true).rows, 4);
        assert_eq!(fit(640.0, true).rows, 3);
        assert_eq!(fit(639.0, true).rows, 2);
        let l = fit(300.0, true);
        assert_eq!(l.rows, MIN_ROWS);
    }

    #[test]
    fn rates_drop_the_decimal_from_ten_up() {
        assert_eq!(format_bytes_at(77 * (1 << 20), 10.0), "77 MB");
        assert_eq!(format_bytes_at(4_400_000, 10.0), "4.2 MB");
        assert_eq!(format_bytes_at(110 * (1 << 20), 10.0), "110 MB");
    }

    #[test]
    fn missing_gpu_says_so_and_battery_shows_when_present() {
        let x = Extra {
            battery: Some((86, true)),
            ..Extra::default()
        };
        let t = tiles(&x);
        assert_eq!(t.gpu_big, "Not reported");
        assert!(t.gpu_missing && t.gpu_frac == 0.0);
        let b = t.battery.unwrap();
        assert_eq!((b.big.as_str(), b.sub.as_str()), ("86%", "Plugged in"));
        assert_eq!(tiles(&Extra::default()).battery, None);
        let low = |pct, plugged| {
            tiles(&Extra {
                battery: Some((pct, plugged)),
                ..Extra::default()
            })
            .battery
            .unwrap()
            .low
        };
        assert!(low(15, false) && low(12, false) && low(0, false));
        assert!(!low(16, false), "above the line");
        assert!(!low(12, true), "plugged in is not a warning");
    }

    fn row(name: &str, exe: &str, mem_mb: u64, cpu: f32) -> AppRow {
        AppRow {
            name: name.into(),
            exe: exe.into(),
            processes: 1,
            mem_bytes: mem_mb * (1 << 20),
            cpu_pct: cpu,
            path: None,
        }
    }

    #[test]
    fn a_hyper_v_vmmem_row_is_not_the_wsl_row() {
        for sort in [Sort::Memory, Sort::Processor] {
            let mut snap = sample();
            let vm = row("Virtual machine", "vmmem", 900, 3.0);
            let wsl = row("WSL (Linux)", "vmmemWSL", 3400, 4.0);
            snap.apps = vec![wsl.clone(), vm.clone()];
            snap.apps_by_cpu = vec![wsl, vm];
            let rows = app_rows(&snap, sort);
            assert_eq!(
                rows.iter().filter(|r| r.kind == RowKind::Wsl).count(),
                1,
                "{sort:?}"
            );
            let vm_row = rows.iter().find(|r| r.name == "Virtual machine").unwrap();
            assert_eq!(vm_row.kind, RowKind::App);
            assert_eq!(vm_row.detail, "vmmem");
            assert!(vm_row.path.is_none());
            // Only the WSL row is followed by Linux rows.
            let at = rows.iter().position(|r| r.kind == RowKind::Wsl).unwrap();
            assert!(rows[at + 1].kind == RowKind::Sub);
            assert!(
                rows.iter().filter(|r| r.kind == RowKind::Sub).count()
                    == snap.wsl.as_ref().unwrap().procs.len()
            );
        }
        let mut snap = sample();
        snap.apps = vec![row("Virtual machine", "vmmem", 900, 3.0)];
        snap.apps_by_cpu = snap.apps.clone();
        // A vmmem row alone does not stop the WSL row from being added.
        let rows = app_rows(&snap, Sort::Memory);
        assert_eq!(rows.iter().filter(|r| r.kind == RowKind::Wsl).count(), 1);
        assert!(is_wsl(&row("x", "VMMEMWSL.exe", 1, 0.0)));
        assert!(!is_wsl(&row("x", "vmmemwsl2", 1, 0.0)));
    }

    #[test]
    fn the_added_wsl_row_sets_the_bars_and_sits_in_its_sorted_place() {
        let mut snap = sample();
        snap.apps = vec![
            row("Google Chrome", "chrome.exe", 2100, 6.0),
            row("Code", "code.exe", 800, 2.0),
            row("Notes", "notes.exe", 100, 0.1),
        ];
        snap.apps_by_cpu = vec![
            snap.apps[0].clone(),
            snap.apps[1].clone(),
            snap.apps[2].clone(),
        ];
        snap.wsl.as_mut().unwrap().mem_bytes = 3400 * (1 << 20);
        let rows = app_rows(&snap, Sort::Memory);
        assert_eq!(rows[0].kind, RowKind::Wsl, "3.4 GB beats Chrome 2.1 GB");
        assert!((rows[0].frac - 1.0).abs() < 1e-6);
        let chrome = rows.iter().find(|r| r.name == "Google Chrome").unwrap();
        assert!(
            (chrome.frac - 2100.0 / 3400.0).abs() < 1e-4,
            "{}",
            chrome.frac
        );
        // A smaller WSL lands between the apps.
        snap.wsl.as_mut().unwrap().mem_bytes = 500 * (1 << 20);
        let rows = app_rows(&snap, Sort::Memory);
        let order: Vec<&str> = rows
            .iter()
            .filter(|r| r.kind != RowKind::Sub)
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(order, ["Google Chrome", "Code", "WSL (Linux)", "Notes"]);
        // The biggest app is still full.
        assert!((rows[0].frac - 1.0).abs() < 1e-6);
        // Smallest of all goes last.
        snap.wsl.as_mut().unwrap().mem_bytes = 10 * (1 << 20);
        let rows = app_rows(&snap, Sort::Memory);
        let last_app = rows.iter().rev().find(|r| r.kind != RowKind::Sub).unwrap();
        assert_eq!(last_app.kind, RowKind::Wsl);
    }

    #[test]
    fn processor_sort_places_wsl_by_its_processor_use() {
        let mut snap = sample();
        snap.apps_by_cpu = vec![
            row("Google Chrome", "chrome.exe", 2100, 6.0),
            row("Code", "code.exe", 800, 2.0),
        ];
        snap.wsl.as_mut().unwrap().cpu_pct = 4.0;
        let rows = app_rows(&snap, Sort::Processor);
        let order: Vec<&str> = rows
            .iter()
            .filter(|r| r.kind != RowKind::Sub)
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(order, ["Google Chrome", "WSL (Linux)", "Code"]);
        snap.wsl.as_mut().unwrap().cpu_pct = 40.0;
        let rows = app_rows(&snap, Sort::Processor);
        assert_eq!(rows[0].kind, RowKind::Wsl);
        assert!((rows[0].frac - 1.0).abs() < 1e-6);
        assert!(rows.iter().find(|r| r.name == "Code").unwrap().frac < 0.1);
    }

    #[test]
    fn the_first_open_says_the_apps_are_being_read() {
        let mut snap = sample();
        snap.apps.clear();
        snap.apps_by_cpu.clear();
        snap.wsl = None;
        for sort in [Sort::Memory, Sort::Processor] {
            let rows = app_rows(&snap, sort);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].kind, RowKind::Reading);
            assert_eq!(rows[0].name, "Reading apps\u{2026}");
        }
    }

    #[test]
    fn old_app_rows_do_not_name_an_app_in_the_status() {
        let mut s = sample();
        s.cpu_pct = 90.0;
        let name = s.apps_by_cpu[0].name.clone();
        assert!(status(&s, false, true).text.contains(&name));
        assert_eq!(status(&s, false, false).text, "Processor is busy");
        assert!(status(&s, false, false).warn);
        let now = Instant::now();
        let v = |age: u64| {
            let sort = Sort::Memory;
            build(
                &s,
                &Extra::default(),
                &History::default(),
                false,
                sort,
                at(now, age),
                now,
            )
            .status
            .text
        };
        assert!(v(10).contains(&name));
        assert_eq!(v(11), "Processor is busy");
    }

    #[test]
    fn rounding_never_leaves_a_figure_that_needs_a_bigger_unit() {
        let mb = |x: f64| (x * 1_048_576.0) as u64;
        let kb = |x: f64| (x * 1024.0) as u64;
        assert_eq!(format_bytes(mb(99.96)), "100 MB");
        assert_eq!(format_bytes(mb(99.94)), "99.9 MB");
        assert_eq!(format_bytes(kb(999.6)), "1.0 MB");
        assert_eq!(format_bytes(kb(999.4)), "999 KB");
        assert_eq!(format_bytes(mb(999.7)), "1.0 GB");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1000), "1.0 KB");
        assert_eq!(format_bytes_at(mb(9.96), 10.0), "10 MB");
        assert_eq!(format_bytes_at(mb(9.94), 10.0), "9.9 MB");
        assert_eq!(format_bytes_at(kb(999.6), 10.0), "1.0 MB");
        assert_eq!(format_bytes_at(mb(999.7), 10.0), "1.0 GB");
        assert_eq!(format_bytes(100 * (1 << 30) - 1), "100 GB");
        assert_eq!(format_bytes(2100 * (1 << 20)), "2.1 GB");
    }

    #[test]
    fn the_footer_has_a_short_fallback_sentence() {
        assert_eq!(FOOT_SHORT, "toggles Pulse");
        assert!(FOOT_SHORT.len() < FOOT_LONG.len());
    }
}
