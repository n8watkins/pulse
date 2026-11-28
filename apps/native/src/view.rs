//! Turns readings into the strings, colors and sparkline paths the panel shows.
//! No toolkit types here, so it is plain code and unit tested on any machine.

use crate::marks::{self, Mark};
use crate::sessions::{self, Line};
use pulse_platform::{GroupsOpen, PanelView, Sort};
use pulse_readings::{AppRow, Snapshot, Wsl, WslProc, WslSession};
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

/// Logical height of the expanded view when everything fits (380 x 580, the
/// "B Tighter" layout), and the vertical pitch of one two-line "Top apps" row.
/// The sizes here and in `ui/panel.slint` (`Metrics`) are the same numbers.
pub const EXPANDED_H: f32 = 580.0;
const ROW_PITCH: f32 = 53.0;
/// Rows shown at full height, without and with the battery strip.
const ROWS_FULL: usize = 4;
const ROWS_FULL_BATTERY: usize = 3;
/// The list never gives up more than this: two rows always stay.
pub const MIN_ROWS: usize = 2;

/// The compact view ("A Mini tiles"): the part above the list (header 43,
/// Processor and Memory tiles 49, a gap of 8, the "Top apps" line 34), the
/// 4 px under the list, the footer row (one line 32, with the battery line
/// 51) and the panel's own bottom border.
const COMPACT_ABOVE_LIST: f32 = 134.0;
const COMPACT_BELOW_LIST: f32 = 4.0;
const COMPACT_FOOT_1: f32 = 32.0;
const COMPACT_FOOT_2: f32 = 51.0;
const COMPACT_BORDER: f32 = 1.0;
/// The compact panel never grows beyond this, then the list scrolls.
pub const COMPACT_MAX_H: f32 = 620.0;
/// The compact view lists this many apps before the Linux row.
pub const COMPACT_APPS: usize = 3;

/// Height of one list row in the given view.
pub fn row_height(kind: RowKind, compact: bool) -> f32 {
    match (kind, compact) {
        (RowKind::App | RowKind::Wsl, false) => ROW_PITCH,
        (RowKind::Sub, false) => 24.0,
        (RowKind::Reading, false) => 40.0,
        (RowKind::App | RowKind::Wsl, true) => 29.0,
        (RowKind::Sub, true) => 22.0,
        (RowKind::Reading, true) => 36.0,
    }
}

/// How the expanded panel fits a work area that may be shorter than [`EXPANDED_H`].
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
        EXPANDED_H - self.shrink
    }
}

/// Height of the compact panel: what the rows need, never more than
/// [`COMPACT_MAX_H`] or the room (`avail_h`), never less than two rows.
/// The battery adds a second line to the footer row.
pub fn compact_height(rows: &[AppView], battery: bool, avail_h: f32) -> f32 {
    let list: f32 = rows.iter().map(|r| row_height(r.kind, true)).sum();
    let foot = if battery {
        COMPACT_FOOT_2
    } else {
        COMPACT_FOOT_1
    };
    let chrome = COMPACT_ABOVE_LIST + COMPACT_BELOW_LIST + foot + COMPACT_BORDER;
    let least = chrome + MIN_ROWS as f32 * row_height(RowKind::App, true);
    (chrome + list).min(COMPACT_MAX_H).min(avail_h).max(least)
}

/// The height of the view itself (no Settings sheet).
pub fn view_height(view: PanelView, rows: &[AppView], battery: bool, avail_h: f32) -> f32 {
    match view {
        PanelView::Expanded => fit(avail_h, battery).height(),
        PanelView::Compact => compact_height(rows, battery, avail_h),
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
    /// The head of a group: the WSL virtual machine, whose Linux programs
    /// follow as `Sub` rows. A head with sub items has a [`Fold`].
    Wsl,
    Sub,
    /// "Reading apps..." before the first scan has finished.
    Reading,
}

/// Whether a row has sub items to open and close, and which way it is now.
/// A row with sub items has a chevron, the toggle and, closed, a summary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fold {
    /// No sub items: no chevron.
    #[default]
    None,
    Closed,
    Open,
}

/// The id of the Linux machine's group, the key of its open state in the
/// settings file.
pub const GROUP_WSL: &str = "wsl";

/// The dot before a Linux row: a coding-agent session is busy or idle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dot {
    #[default]
    None,
    Idle,
    Busy,
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
    /// Executable path, for the real icon; `None` shows the colored letter.
    pub path: Option<String>,
    /// Sub rows: a coding agent.
    pub agent: bool,
    /// Sub rows: "x3" text when more than one process, else empty.
    pub count: String,
    /// Last row of a WSL group: a hairline closes it.
    pub group_end: bool,
    /// Sub rows: the product mark that replaces the letter, when one is known.
    pub mark: Option<Mark>,
    /// Session rows: busy or idle (a folded line is busy when one of them is).
    pub dot: Dot,
    /// Session rows that look like another one (same agent, same folder): how
    /// long the session has run, small, to tell them apart. Else empty.
    pub age: String,
    /// Sub rows that are dev servers: their port, small and muted, for example
    /// ":3000" or ":3000 +1" (the lowest and how many more); else empty.
    pub port: String,
    /// Head rows: the id of the group the following `Sub` rows belong to (the
    /// key of its saved open state); empty on every other row.
    pub group: String,
    /// Head rows: what the closed row says about its sub items, for example
    /// "14 programs \u{b7} 5 agents"; empty when there are none.
    pub summary: String,
    /// Head rows with sub items: closed or open (see [`fold_groups`]).
    pub fold: Fold,
}

/// Everything the panel shows, as plain data.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub status: Status,
    pub cpu_big: String,
    pub cpu_sub: String,
    /// Uptime reached a week: " \u{b7} restart?" follows in the warning color.
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

/// The line under the title and its dot color.
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
    /// The compact footer's figures: no space after the arrow.
    pub net_down: String,
    pub net_up: String,
    pub battery: Option<BatteryView>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BatteryView {
    /// On battery at or under [`BATTERY_LOW`] percent: the bar takes the warning color.
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
            wsl_proc(&first_label(), false, 12, 900, 3.0),
            wsl_proc(
                "Program with an extremely long label indeed",
                false,
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
            wsl_proc("chrome-headless-shell", false, 4, 200, 0.3),
        ],
        sessions: stress_sessions(),
        // Three digits, as on a PC with many agents.
        dev_servers: Some(128),
    });
    // Ports: five digits, and several ports on one row.
    for (p, ports) in snap.wsl.iter_mut().flat_map(|w| w.procs.iter_mut()).zip([
        vec![65535, 65534, 3000],
        vec![65535],
        vec![8000, 8001],
        vec![],
    ]) {
        p.ports = ports;
    }
}

/// Fourteen sessions: two busy, two in the same folder, one with a very long
/// folder name, one in the home folder, one without a readable folder, and
/// plenty of idle ones to fold.
fn stress_sessions() -> Vec<WslSession> {
    let make = |label: &str, folder: Option<&str>, mem_mb: u64, cpu: f32, busy: bool, run: u64| {
        WslSession {
            label: label.into(),
            folder: folder.map(Into::into),
            mem_bytes: mem_mb * (1 << 20),
            cpu_pct: cpu,
            busy,
            run_secs: Some(run),
            pid: 0,
        }
    };
    let mut v = vec![
        make("Claude Code", Some("pulse"), 640, 2.4, true, 2 * 3_600),
        make("Claude Code", Some("pulse"), 410, 0.0, false, 14 * 60),
        make(
            "Claude Code",
            Some("a-project-folder-with-an-extremely-long-name-for-the-stress-run"),
            380,
            1.1,
            true,
            50 * 3_600,
        ),
        make("Codex CLI", Some("~"), 300, 0.0, false, 90),
        make("Gemini CLI", None, 250, 0.0, false, 5 * 60),
    ];
    for i in 0..9 {
        v.push(make(
            "Claude Code",
            Some("shop-site"),
            200 - i * 5,
            0.0,
            false,
            (i + 1) * 600,
        ));
    }
    for (i, x) in v.iter_mut().enumerate() {
        x.pid = 100 + u32::try_from(i).unwrap_or(0);
    }
    v
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
        ports: Vec::new(),
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
        dot: Dot::None,
        age: String::new(),
        port: String::new(),
        group: String::new(),
        summary: String::new(),
        fold: Fold::None,
    }
}

/// The head row's detail line when it is open: the dev server count (it is not
/// in the Linux rows, which show some of the programs) and "inside:" when
/// there are rows below. With a count the machine is "Linux VM": the bar
/// takes the right of the line, and the count must stay readable.
fn wsl_detail(wsl: Option<&Wsl>) -> String {
    let mut text = String::from("Linux virtual machine");
    if let Some(w) = wsl {
        if let Some(servers) = servers_text(w.server_count()) {
            text = String::from("Linux VM \u{b7} ");
            text.push_str(&servers);
        }
        if !w.procs.is_empty() {
            text.push_str(" \u{b7} inside:");
        }
    }
    text
}

/// "3 dev servers", or `None` for none (zero is not shown).
fn servers_text(n: u32) -> Option<String> {
    (n > 0).then(|| format!("{n} dev server{}", if n == 1 { "" } else { "s" }))
}

/// The port text of a Linux row: ":3000", or ":3000 +2" for several (the
/// lowest, then how many more). Empty without ports.
fn port_text(ports: &[u16]) -> String {
    match ports.iter().min() {
        None => String::new(),
        Some(low) if ports.len() == 1 => format!(":{low}"),
        Some(low) => format!(":{low} +{}", ports.len() - 1),
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
        dot: Dot::None,
        age: String::new(),
        port: String::new(),
        group: String::new(),
        summary: String::new(),
        fold: Fold::None,
    }
}

/// Linux program rows shown under the WSL row.
const SUB_ROWS: usize = 6;

/// Share of the machine the Linux processor bars are scaled against at least.
const SUB_CPU_BAR_FLOOR: f64 = 1.0;

/// The Linux rows for the chosen sort, [`SUB_ROWS`] at most: first the
/// coding-agent sessions (see [`sessions`]: one row each, busy ones first, the
/// rest of many folded into one line), then the programs by memory, or by
/// processor (memory breaks ties), with the same value as the app rows above
/// them.
#[allow(clippy::cast_precision_loss)]
fn sub_rows(w: &Wsl, sort: Sort) -> Vec<AppView> {
    let lines = sessions::plan(&w.sessions, sort);
    let lookalikes = sessions::needs_age(&w.sessions);
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
    procs.truncate(SUB_ROWS.saturating_sub(lines.len()));
    let amount = |bytes: u64, cpu: f32| match sort {
        Sort::Memory => bytes as f64,
        Sort::Processor => f64::from(cpu),
    };
    let text = |bytes: u64, cpu: f32| match sort {
        Sort::Memory => format_bytes(bytes),
        Sort::Processor => cpu_text(cpu),
    };
    let line_amount = |l: &Line<'_>| match l {
        Line::One(x) => amount(x.mem_bytes, x.cpu_pct),
        Line::Fold(f) => amount(f.mem_bytes, f.cpu_pct),
    };
    let top = lines
        .iter()
        .map(line_amount)
        .chain(procs.iter().map(|p| amount(p.mem_bytes, p.cpu_pct)))
        .fold(
            if sort == Sort::Memory {
                1.0
            } else {
                SUB_CPU_BAR_FLOOR
            },
            f64::max,
        );
    let tile = |label: &str| {
        let mark = marks::for_label(label);
        let rgb = mark.map_or_else(|| hsl_rgb(hue(label), 0.45, 0.45), |m| m.rgb);
        (
            mark,
            rgb,
            label.chars().next().map(String::from).unwrap_or_default(),
        )
    };
    let mut rows: Vec<AppView> = lines
        .iter()
        .map(|l| {
            let (name, label, dot, age, value, a) = match l {
                Line::One(x) => (
                    match &x.folder {
                        Some(f) => format!("{} \u{b7} {f}", x.label),
                        None => x.label.clone(),
                    },
                    x.label.as_str(),
                    if x.busy { Dot::Busy } else { Dot::Idle },
                    x.run_secs
                        .filter(|_| lookalikes.contains(&x.pid))
                        .map(sessions::age)
                        .unwrap_or_default(),
                    text(x.mem_bytes, x.cpu_pct),
                    line_amount(l),
                ),
                Line::Fold(f) => (
                    f.title(),
                    f.lead.as_str(),
                    if f.any_busy { Dot::Busy } else { Dot::Idle },
                    String::new(),
                    text(f.mem_bytes, f.cpu_pct),
                    line_amount(l),
                ),
            };
            let (mark, rgb, letter) = tile(label);
            AppView {
                kind: RowKind::Sub,
                letter,
                rgb,
                name,
                detail: String::new(),
                value,
                frac: (a / top).clamp(0.0, 1.0) as f32,
                path: None,
                agent: false,
                count: String::new(),
                group_end: false,
                mark,
                dot,
                age,
                port: String::new(),
                group: String::new(),
                summary: String::new(),
                fold: Fold::None,
            }
        })
        .collect();
    rows.extend(procs.into_iter().map(|p| {
        let (mark, rgb, letter) = tile(&p.label);
        AppView {
            kind: RowKind::Sub,
            letter,
            rgb,
            name: p.label.clone(),
            detail: String::new(),
            value: text(p.mem_bytes, p.cpu_pct),
            frac: (amount(p.mem_bytes, p.cpu_pct) / top).clamp(0.0, 1.0) as f32,
            path: None,
            agent: p.agent,
            count: if p.count > 1 {
                format!("\u{d7}{}", p.count)
            } else {
                String::new()
            },
            group_end: false,
            mark,
            dot: Dot::None,
            age: String::new(),
            port: port_text(&p.ports),
            group: String::new(),
            summary: String::new(),
            fold: Fold::None,
        }
    }));
    rows
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
        let mut row = app_view(a, sort, wsl, max);
        let is_wsl_row = row.kind == RowKind::Wsl;
        if let (true, Some(w)) = (is_wsl_row, wsl) {
            GROUP_WSL.clone_into(&mut row.group);
            row.summary = wsl_summary(w);
        }
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

/// What the panel is asked to show: the sort, the view, and which groups of
/// rows are open in that view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mode {
    pub sort: Sort,
    pub view: PanelView,
    pub open: GroupsOpen,
}

impl Mode {
    /// Whether group `id` is open in this mode's view.
    pub fn is_open(&self, id: &str) -> bool {
        self.open.is_open(self.view, id)
    }
}

#[cfg(test)]
impl Mode {
    /// The expanded view sorted by `sort`, every group open.
    pub fn expanded(sort: Sort) -> Self {
        Self {
            sort,
            view: PanelView::Expanded,
            open: GroupsOpen::default(),
        }
    }
}

/// "14 programs \u{b7} 5 agents \u{b7} 3 dev servers": the Linux programs that are not coding agents,
/// counted by their process count, and the coding-agent sessions (each agent
/// is counted once however many helper processes it has, so the two numbers
/// never overlap), and the dev servers (counted from all Linux processes; left
/// out when none or unknown). Parts that are zero are left out; nothing
/// running inside: empty.
pub fn wsl_summary(w: &Wsl) -> String {
    let [programs, agents, servers] = summary_parts(w);
    join_parts([programs, agents, servers])
}

/// The compact row's summary: the same parts, the newest and most useful
/// first, because the row has room for about 30 characters and the end is
/// what ends in an ellipsis ("3 dev servers \u{b7} 5 agents \u{b7} 18 progr...").
fn wsl_summary_compact(w: &Wsl) -> String {
    let [programs, agents, servers] = summary_parts(w);
    join_parts([servers, agents, programs])
}

/// Programs, agents and dev servers as text, each `None` at zero.
fn summary_parts(w: &Wsl) -> [Option<String>; 3] {
    let programs: u32 = w.procs.iter().map(|p| p.count.max(1)).sum();
    let agents = u32::try_from(w.agent_count()).unwrap_or(u32::MAX);
    let noun = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    [
        (programs > 0).then(|| noun(programs, "program", "programs")),
        (agents > 0).then(|| noun(agents, "agent", "agents")),
        servers_text(w.server_count()),
    ]
}

fn join_parts(parts: [Option<String>; 3]) -> String {
    parts
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
}

/// Applies the open or closed state of every group: a head row with sub items
/// gets its [`Fold`] and, closed, its summary in place of the detail line
/// and no sub rows below it. `summary_when_open` keeps the summary on an open
/// head too (the compact view has no room for the long detail).
pub fn fold_groups(
    rows: Vec<AppView>,
    is_open: &dyn Fn(&str) -> bool,
    summary_when_open: bool,
) -> Vec<AppView> {
    let mut out = Vec::with_capacity(rows.len());
    let mut hidden = false;
    for mut r in rows {
        if r.kind == RowKind::Sub {
            if !hidden {
                out.push(r);
            }
            continue;
        }
        hidden = false;
        if !r.group.is_empty() && !r.summary.is_empty() {
            let open = is_open(&r.group);
            r.fold = if open { Fold::Open } else { Fold::Closed };
            hidden = !open;
            if hidden || summary_when_open {
                r.detail.clone_from(&r.summary);
            }
        }
        out.push(r);
    }
    out
}

/// The expanded view's rows: every app, the Linux row where its sort puts
/// it and, when its group is open, the Linux programs under it.
pub fn expanded_rows(snap: &Snapshot, sort: Sort, is_open: &dyn Fn(&str) -> bool) -> Vec<AppView> {
    fold_groups(app_rows(snap, sort), is_open, false)
}

/// The compact view's rows: the three biggest apps for the sort, then one
/// Linux row (always last, with its counts and a chevron) and, when the list
/// is open, the Linux programs under it. Built from [`app_rows`], so the
/// values, bars and the Linux rows are the same as in the expanded view.
pub fn compact_rows(snap: &Snapshot, sort: Sort, is_open: &dyn Fn(&str) -> bool) -> Vec<AppView> {
    let mut apps = Vec::new();
    let mut head = None;
    let mut subs = Vec::new();
    for r in app_rows(snap, sort) {
        match r.kind {
            RowKind::Wsl => head = Some(r),
            RowKind::Sub => subs.push(r),
            _ => apps.push(r),
        }
    }
    apps.truncate(COMPACT_APPS.max(1));
    if let Some(mut h) = head {
        if let Some(w) = &snap.wsl {
            h.summary = wsl_summary_compact(w);
        }
        apps.push(h);
        // No closing hairline: the Linux list is the end of the compact list.
        for s in &mut subs {
            s.group_end = false;
        }
        apps.extend(subs);
    }
    fold_groups(apps, is_open, true)
}

#[allow(clippy::cast_precision_loss)]
pub fn build(
    snap: &Snapshot,
    extra: &Extra,
    hist: &History,
    sample: bool,
    mode: &Mode,
    now: Instant,
    apps_at: Instant,
) -> View {
    let sort = mode.sort;
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
        apps: match mode.view {
            PanelView::Expanded => expanded_rows(snap, sort, &|id| mode.is_open(id)),
            PanelView::Compact => compact_rows(snap, sort, &|id| mode.is_open(id)),
        },
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
        net_down: format!("\u{2193}{}/s", format_bytes(x.net_down_bps)),
        net_up: format!("\u{2191}{}/s", format_bytes(x.net_up_bps)),
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
            &Mode::expanded(Sort::Memory),
            now,
            now,
        );
        // 6 apps, the WSL row (biggest, so first), then its Linux rows.
        assert_eq!(v.apps.len(), 6 + 1 + SUB_ROWS);
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
        assert_eq!(w.detail, "Linux VM \u{b7} 3 dev servers \u{b7} inside:");
        assert!(w.path.as_deref().is_some_and(|p| p.ends_with("wsl.exe")));
        let subs: Vec<_> = rows.iter().filter(|r| r.kind == RowKind::Sub).collect();
        // Sessions first (3 rows and a fold), then the biggest programs.
        assert_eq!(subs.len(), SUB_ROWS);
        assert!(subs[..4].iter().all(|r| r.dot != Dot::None && !r.agent));
        assert!(subs[4..].iter().all(|r| r.dot == Dot::None && !r.agent));
        assert_eq!(subs[4].name, "VS Code server");
        assert_eq!(subs[4].count, "\u{d7}6");
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
            ..Wsl::default()
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
        snap.wsl.as_mut().unwrap().sessions.clear();
        // The sample's figures move with its clock: pin them.
        for p in &mut snap.wsl.as_mut().unwrap().procs {
            p.cpu_pct = match p.label.as_str() {
                "next-server (v16.4.6)" => 0.9,
                "chrome-headless-shell" => 0.6,
                "VS Code server" => 0.4,
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
        w.sessions.clear();
        for p in &mut w.procs {
            p.cpu_pct = 0.0;
        }
        let rows = app_rows(&snap, Sort::Processor);
        let c = subs(&rows);
        assert_eq!(c[0].name, "VS Code server");
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
        w.sessions.clear();
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
        snap.wsl.as_mut().unwrap().sessions.clear();
        snap.wsl.as_mut().unwrap().procs = [
            "Claude Code",
            "VS Code server",
            "Codex CLI",
            "next-server (v16.4.6)",
            "chrome-headless-shell",
            "rust-analyzer",
            "Editor server",
        ]
        .iter()
        .enumerate()
        .map(|(i, l)| WslProc {
            label: (*l).into(),
            mem_bytes: (100 - i as u64) << 20,
            ..WslProc::default()
        })
        .collect();
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
        for (name, slug) in [
            ("VS Code server", "visualstudiocode"),
            ("Codex CLI", "openai"),
            ("rust-analyzer", "rust"),
        ] {
            assert_eq!(find(name).mark.map(|m| m.slug), Some(slug), "{name}");
        }
        // Cursor's and Windsurf's server has no mark: the letter tile stays,
        // in the hashed color. (Only the first six rows by memory are shown.)
        snap.wsl.as_mut().unwrap().procs[0].mem_bytes = 1;
        let rows2 = app_rows(&snap, Sort::Memory);
        let r = rows2.iter().find(|r| r.name == "Editor server").unwrap();
        assert!(r.mark.is_none());
        assert_eq!(r.rgb, hsl_rgb(hue("Editor server"), 0.45, 0.45));
        assert_eq!(r.letter, "E");
        // App rows never carry one, and the sort does not change the marks.
        assert!(
            rows.iter()
                .filter(|r| r.kind != RowKind::Sub)
                .all(|r| r.mark.is_none())
        );
        let cpu = app_rows(&snap, Sort::Processor);
        assert_eq!(
            subs(&cpu).iter().filter(|r| r.mark.is_some()).count(),
            subs(&rows2).iter().filter(|r| r.mark.is_some()).count()
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
                &Mode::expanded(Sort::Memory),
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
            &Mode::expanded(Sort::Memory),
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
        assert_eq!((l.rows, l.shrink, l.height()), (4, 0.0, 580.0));
        assert_eq!(fit(580.0, false).rows, 4);
        assert_eq!(fit(1000.0, true).rows, 3);
        assert_eq!(fit(580.0, true).height(), 580.0);
    }

    #[test]
    fn a_short_work_area_gives_up_rows_and_keeps_the_footer() {
        // One row less frees 53 px.
        let l = fit(560.0, false);
        assert_eq!((l.rows, l.height()), (3, 527.0));
        let l = fit(500.0, false);
        assert_eq!((l.rows, l.height()), (2, 474.0));
        let l = fit(400.0, false);
        assert_eq!((l.rows, l.height()), (2, 474.0), "two rows always stay");
        for h in (300..900).map(|h| h as f32) {
            let l = fit(h, h as i32 % 2 == 0);
            assert!(l.rows >= MIN_ROWS && l.rows <= ROWS_FULL);
            // Either it fits, or it is at the minimum.
            assert!(l.height() <= h || l.rows == MIN_ROWS, "{h}");
        }
    }

    #[test]
    fn the_battery_strip_costs_a_row() {
        assert_eq!(fit(1000.0, true).rows, 3);
        assert_eq!(fit(580.0, true).rows, 3);
        assert_eq!(fit(579.0, true).rows, 2);
        let l = fit(300.0, true);
        assert_eq!(l.rows, MIN_ROWS);
    }

    fn compact_of(snap: &Snapshot, sort: Sort, open: bool) -> Vec<AppView> {
        compact_rows(snap, sort, &|_| open)
    }

    #[test]
    fn compact_lists_three_apps_then_one_closed_linux_row() {
        let snap = sample();
        for sort in [Sort::Memory, Sort::Processor] {
            let rows = compact_of(&snap, sort, false);
            assert_eq!(rows.len(), COMPACT_APPS + 1, "{sort:?}");
            assert!(rows[..COMPACT_APPS].iter().all(|r| r.kind == RowKind::App));
            let head = rows.last().unwrap();
            assert_eq!(head.kind, RowKind::Wsl);
            assert_eq!(head.fold, Fold::Closed);
            assert_eq!(head.group, GROUP_WSL);
            assert_eq!(head.name, "WSL (Linux)");
            assert!(head.path.as_deref().is_some_and(|p| p.ends_with("wsl.exe")));
        }
        // The apps are the biggest ones for the sort, in the sort's order.
        let by_mem = compact_of(&snap, Sort::Memory, false);
        let names: Vec<&str> = snap
            .apps
            .iter()
            .take(COMPACT_APPS)
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(
            by_mem[..COMPACT_APPS]
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            names
        );
        let by_cpu = compact_of(&snap, Sort::Processor, false);
        assert_eq!(by_cpu[0].name, snap.apps_by_cpu[0].name);
        assert!(by_cpu[0].value.ends_with('%') && by_mem[0].value.ends_with('B'));
        // The Linux row's value follows the sort too.
        assert!(by_cpu.last().unwrap().value.ends_with('%'));
        assert!(by_mem.last().unwrap().value.ends_with('B'));
    }

    #[test]
    fn the_closed_linux_row_counts_programs_and_agents() {
        let snap = sample();
        let w = snap.wsl.as_ref().unwrap();
        let programs: u32 = w.procs.iter().map(|p| p.count.max(1)).sum();
        let agents = u32::try_from(w.agent_count()).unwrap();
        let rows = compact_of(&snap, Sort::Memory, false);
        let head = rows.last().unwrap();
        // The compact row has little room: the servers come first.
        assert_eq!(
            head.detail,
            format!("3 dev servers \u{b7} {agents} agents \u{b7} {programs} programs")
        );
        // The sample: five agent sessions, and 18 processes of other programs.
        assert_eq!((programs, agents), (18, 5));
        // Open or closed, the compact row keeps its summary.
        let open = compact_of(&snap, Sort::Memory, true);
        let open_head = open.iter().find(|r| r.kind == RowKind::Wsl).unwrap();
        assert_eq!(open_head.detail, head.detail);
        // The expanded row keeps the long order.
        assert_eq!(
            wsl_summary(w),
            "18 programs \u{b7} 5 agents \u{b7} 3 dev servers"
        );
    }

    #[test]
    fn the_summary_handles_one_none_and_no_agents() {
        // Programs (by process count) and agent sessions: (counts, sessions).
        let w = |procs: Vec<u32>, agents: u32| Wsl {
            mem_bytes: 1 << 30,
            cpu_pct: 1.0,
            procs: procs
                .into_iter()
                .map(|count| wsl_proc("x", false, count, 10, 0.0))
                .collect(),
            sessions: (0..agents)
                .map(|pid| session("Claude Code", Some("p"), 10, 0.0, false, pid))
                .collect(),
            dev_servers: None,
        };
        assert_eq!(wsl_summary(&w(vec![], 0)), "");
        assert_eq!(wsl_summary(&w(vec![1], 0)), "1 program");
        assert_eq!(wsl_summary(&w(vec![1], 1)), "1 program \u{b7} 1 agent");
        assert_eq!(wsl_summary(&w(vec![4, 2], 0)), "6 programs");
        assert_eq!(wsl_summary(&w(vec![9], 5)), "9 programs \u{b7} 5 agents");
        // Only agents running: no "0 programs".
        assert_eq!(wsl_summary(&w(vec![], 3)), "3 agents");
    }

    #[test]
    fn the_summary_and_detail_count_dev_servers_and_omit_zero() {
        let mut w = Wsl {
            procs: vec![wsl_proc("x", false, 9, 10, 0.0)],
            sessions: (0..5)
                .map(|pid| session("Claude Code", Some("p"), 10, 0.0, false, pid))
                .collect(),
            dev_servers: Some(3),
            ..Wsl::default()
        };
        assert_eq!(
            wsl_summary(&w),
            "9 programs \u{b7} 5 agents \u{b7} 3 dev servers"
        );
        assert_eq!(
            wsl_detail(Some(&w)),
            "Linux VM \u{b7} 3 dev servers \u{b7} inside:"
        );
        w.dev_servers = Some(1);
        assert_eq!(
            wsl_summary(&w),
            "9 programs \u{b7} 5 agents \u{b7} 1 dev server"
        );
        // Zero and unknown leave the count out.
        for none in [Some(0), None] {
            w.dev_servers = none;
            assert_eq!(wsl_summary(&w), "9 programs \u{b7} 5 agents");
            assert_eq!(wsl_detail(Some(&w)), "Linux virtual machine \u{b7} inside:");
        }
        // Servers alone, nothing else running in Linux.
        let only = Wsl {
            dev_servers: Some(2),
            ..Wsl::default()
        };
        assert_eq!(wsl_summary(&only), "2 dev servers");
        assert_eq!(wsl_detail(Some(&only)), "Linux VM \u{b7} 2 dev servers");
        assert_eq!(wsl_detail(None), "Linux virtual machine");
    }

    #[test]
    fn a_dev_server_row_shows_its_lowest_port_and_how_many_more() {
        assert_eq!(port_text(&[]), "");
        assert_eq!(port_text(&[3000]), ":3000");
        assert_eq!(port_text(&[8001, 8000]), ":8000 +1");
        assert_eq!(port_text(&[5173, 9229, 24678]), ":5173 +2");
        let rows = app_rows(&sample(), Sort::Memory);
        let next = rows.iter().find(|r| r.name.starts_with("next-server"));
        assert_eq!(next.map(|r| r.port.as_str()), Some(":3000"));
        let others = rows
            .iter()
            .filter(|r| r.kind == RowKind::Sub && !r.name.starts_with("next-server"));
        assert!(others.into_iter().all(|r| r.port.is_empty()));
        // A worker with two ports, where the rows have room for it.
        let mut snap = sample();
        snap.wsl.as_mut().unwrap().sessions.clear();
        let rows = app_rows(&snap, Sort::Processor);
        assert!(rows.iter().any(|r| r.port == ":8000 +1"));
    }

    #[test]
    fn opening_the_linux_row_adds_its_programs_below_it() {
        let snap = sample();
        // The same Linux rows as the expanded view: sessions, then programs.
        let n = SUB_ROWS;
        let closed = compact_of(&snap, Sort::Memory, false);
        let open = compact_of(&snap, Sort::Memory, true);
        assert_eq!(open.len(), closed.len() + n);
        assert_eq!(open[COMPACT_APPS].kind, RowKind::Wsl);
        assert_eq!(open[COMPACT_APPS].fold, Fold::Open);
        assert!(
            open[COMPACT_APPS + 1..]
                .iter()
                .all(|r| r.kind == RowKind::Sub)
        );
        assert_eq!(open[..COMPACT_APPS], closed[..COMPACT_APPS]);
        // No closing hairline: the Linux list is the end of the compact list.
        assert!(open.iter().all(|r| !r.group_end));
        // The expanded rows are not changed by any of this: its own state rules there.
        let full = expanded_rows(&snap, Sort::Memory, &|_| true);
        assert!(full.iter().any(|r| r.kind == RowKind::Sub));
    }

    #[test]
    fn compact_keeps_the_linux_row_last_even_when_it_is_the_biggest() {
        let mut snap = sample();
        snap.wsl.as_mut().unwrap().mem_bytes = 20 << 30;
        let rows = compact_of(&snap, Sort::Memory, false);
        assert_eq!(rows.len(), COMPACT_APPS + 1);
        assert_eq!(rows.last().unwrap().fold, Fold::Closed);
        assert!(rows[..COMPACT_APPS].iter().all(|r| r.kind == RowKind::App));
    }

    #[test]
    fn compact_without_linux_or_apps() {
        let mut snap = sample();
        snap.wsl = None;
        let rows = compact_of(&snap, Sort::Memory, true);
        assert_eq!(rows.len(), COMPACT_APPS);
        assert!(rows.iter().all(|r| r.kind == RowKind::App));
        // A machine with nothing running inside has no chevron and no list.
        snap.wsl = Some(Wsl {
            mem_bytes: 1 << 30,
            cpu_pct: 0.5,
            ..Wsl::default()
        });
        let rows = compact_of(&snap, Sort::Memory, true);
        assert_eq!(rows.last().unwrap().kind, RowKind::Wsl);
        assert_eq!(rows.last().unwrap().fold, Fold::None);
        assert_eq!(rows.len(), COMPACT_APPS + 1);
        // Reading apps.
        snap.apps.clear();
        snap.apps_by_cpu.clear();
        snap.wsl = None;
        let rows = compact_of(&snap, Sort::Memory, false);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, RowKind::Reading);
    }

    #[test]
    fn the_compact_panel_is_about_287_tall_with_the_linux_list_closed() {
        let snap = sample();
        let rows = compact_of(&snap, Sort::Memory, false);
        let h = compact_height(&rows, false, 1000.0);
        assert!((h - 287.0).abs() < 0.5, "{h}");
        // The battery adds a line to the footer row.
        assert!((compact_height(&rows, true, 1000.0) - h - 19.0).abs() < 0.01);
    }

    #[test]
    fn the_compact_panel_grows_with_the_linux_list_up_to_its_cap_and_the_room() {
        let snap = sample();
        let closed = compact_height(&compact_of(&snap, Sort::Memory, false), false, 1000.0);
        let open_rows = compact_of(&snap, Sort::Memory, true);
        let open = compact_height(&open_rows, false, 1000.0);
        let n = snap.wsl.as_ref().unwrap().procs.len() as f32;
        assert!((open - closed - n * row_height(RowKind::Sub, true)).abs() < 0.01);
        // A long list stops at the cap, then scrolls.
        let mut long = open_rows.clone();
        long.extend(open_rows.iter().cloned().cycle().take(40));
        assert_eq!(compact_height(&long, false, 1000.0), COMPACT_MAX_H);
        // A short screen limits it as well, but never below two rows.
        assert_eq!(compact_height(&long, false, 400.0), 400.0);
        let least = compact_height(&long, false, 10.0);
        assert!(least > 150.0 && least < 250.0, "{least}");
        assert_eq!(compact_height(&[], false, 10.0), least);
    }

    #[test]
    fn each_view_has_its_own_height_and_settings_never_changes_it() {
        let snap = sample();
        let rows = compact_of(&snap, Sort::Memory, false);
        assert_eq!(view_height(PanelView::Expanded, &[], false, 1000.0), 580.0);
        let c = view_height(PanelView::Compact, &rows, false, 1000.0);
        assert!((c - 287.0).abs() < 0.5);
        // The Settings sheet has a compact form for the compact panel, so there is
        // no separate height for it: the panel keeps the height of its view.
        // A short screen limits both.
        assert_eq!(view_height(PanelView::Expanded, &[], false, 540.0), 527.0);
        assert_eq!(view_height(PanelView::Compact, &rows, false, 250.0), 250.0);
    }

    #[test]
    fn expanded_closed_hides_the_programs_and_says_how_many() {
        let snap = sample();
        let open = expanded_rows(&snap, Sort::Memory, &|_| true);
        let closed = expanded_rows(&snap, Sort::Memory, &|_| false);
        assert_eq!(open.len(), closed.len() + SUB_ROWS);
        let head = |rows: &[AppView]| rows.iter().find(|r| r.kind == RowKind::Wsl).cloned();
        let (o, c) = (head(&open).unwrap(), head(&closed).unwrap());
        assert_eq!((o.fold, c.fold), (Fold::Open, Fold::Closed));
        assert_eq!(o.detail, "Linux VM \u{b7} 3 dev servers \u{b7} inside:");
        assert_eq!(c.detail, "18 programs \u{b7} 5 agents \u{b7} 3 dev servers");
        assert!(closed.iter().all(|r| r.kind != RowKind::Sub));
        // Every app row is still there, in the same order.
        let apps = |rows: &[AppView]| {
            rows.iter()
                .filter(|r| r.kind != RowKind::Sub)
                .map(|r| r.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(apps(&open), apps(&closed));
        // The list height shrinks by exactly the Linux rows.
        let h = |rows: &[AppView]| rows.iter().map(|r| row_height(r.kind, false)).sum::<f32>();
        assert_eq!(h(&open) - h(&closed), SUB_ROWS as f32 * 24.0);
    }

    #[test]
    fn a_machine_with_nothing_inside_has_no_fold_in_either_view() {
        let mut snap = sample();
        snap.wsl = Some(Wsl {
            mem_bytes: 1 << 30,
            cpu_pct: 0.5,
            ..Wsl::default()
        });
        for open in [true, false] {
            let rows = expanded_rows(&snap, Sort::Memory, &|_| open);
            assert!(rows.iter().all(|r| r.fold == Fold::None));
            assert!(rows.iter().all(|r| r.kind != RowKind::Sub));
        }
    }

    #[test]
    fn only_the_asked_group_folds() {
        let snap = sample();
        let rows = expanded_rows(&snap, Sort::Memory, &|id| id != "other");
        assert!(rows.iter().any(|r| r.kind == RowKind::Sub));
        let rows = expanded_rows(&snap, Sort::Memory, &|id| id != GROUP_WSL);
        assert!(rows.iter().all(|r| r.kind != RowKind::Sub));
    }

    #[test]
    fn each_view_reads_its_own_saved_state() {
        let snap = sample();
        let mut open = GroupsOpen::default();
        open.set(PanelView::Compact, GROUP_WSL, true);
        open.set(PanelView::Expanded, GROUP_WSL, false);
        let subs = |view| {
            let mode = Mode {
                sort: Sort::Memory,
                view,
                open: open.clone(),
            };
            let (hist, extra) = (History::default(), Extra::default());
            let now = Instant::now();
            build(&snap, &extra, &hist, true, &mode, now, now)
                .apps
                .iter()
                .filter(|r| r.kind == RowKind::Sub)
                .count()
        };
        assert_eq!(subs(PanelView::Expanded), 0);
        assert_eq!(subs(PanelView::Compact), SUB_ROWS);
    }

    #[test]
    fn row_heights_are_the_ones_the_panel_draws() {
        // These are the Metrics of ui/panel.slint.
        assert_eq!(row_height(RowKind::App, false), 53.0);
        assert_eq!(row_height(RowKind::Wsl, false), 53.0);
        assert_eq!(row_height(RowKind::Sub, false), 24.0);
        assert_eq!(row_height(RowKind::App, true), 29.0);
        assert_eq!(row_height(RowKind::Wsl, true), 29.0);
        assert_eq!(row_height(RowKind::Sub, true), 22.0);
    }

    #[test]
    fn the_footer_shows_arrows_without_a_space_and_the_battery_only_when_there_is_one() {
        let x = Extra::stress();
        let t = tiles(&x);
        assert_eq!(t.net_down, "\u{2193}999 KB/s");
        assert_eq!(t.net_up, "\u{2191}99.9 MB/s");
        assert!(t.battery.is_some());
        assert!(tiles(&Extra::default()).battery.is_none());
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
                &Mode::expanded(sort),
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

    fn session(
        label: &str,
        folder: Option<&str>,
        mem_mb: u64,
        cpu: f32,
        busy: bool,
        pid: u32,
    ) -> WslSession {
        WslSession {
            label: label.into(),
            folder: folder.map(Into::into),
            mem_bytes: mem_mb << 20,
            cpu_pct: cpu,
            busy,
            run_secs: Some(u64::from(pid) * 60),
            pid,
        }
    }

    fn with_sessions(sessions: Vec<WslSession>) -> Snapshot {
        let mut snap = sample();
        snap.wsl.as_mut().unwrap().sessions = sessions;
        snap
    }

    #[test]
    fn a_session_row_is_the_agent_its_folder_a_dot_and_the_value_of_the_sort() {
        let snap = with_sessions(vec![
            session("Claude Code", Some("pulse"), 640, 2.6, true, 1),
            session("Codex CLI", None, 236, 0.0, false, 2),
        ]);
        let mem = app_rows(&snap, Sort::Memory);
        let m = subs(&mem);
        assert_eq!(m[0].name, "Claude Code \u{b7} pulse");
        assert_eq!((m[0].dot, m[0].value.as_str()), (Dot::Busy, "640 MB"));
        assert_eq!(m[0].mark.map(|x| x.slug), Some("claude"));
        assert!(!m[0].agent && m[0].count.is_empty());
        // No readable folder: the agent's name alone.
        assert_eq!(m[1].name, "Codex CLI");
        assert_eq!((m[1].dot, m[1].value.as_str()), (Dot::Idle, "236 MB"));
        // Programs follow the sessions, with no dot.
        assert!(m[2..].iter().all(|r| r.dot == Dot::None));
        assert_eq!(m.len(), SUB_ROWS);
        let cpu = app_rows(&snap, Sort::Processor);
        let c = subs(&cpu);
        assert_eq!(c[0].value, "2.6%");
        assert_eq!(c[1].value, "0.0%");
        assert_eq!(c[0].dot, Dot::Busy);
    }

    #[test]
    fn sessions_that_look_alike_show_how_long_they_run() {
        let snap = with_sessions(vec![
            session("Claude Code", Some("pulse"), 640, 0.0, false, 130),
            session("Claude Code", Some("pulse"), 400, 0.0, false, 14),
            session("Claude Code", Some("shop-site"), 300, 0.0, false, 5),
        ]);
        let rows = app_rows(&snap, Sort::Memory);
        let m = subs(&rows);
        assert_eq!(
            m.iter().take(3).map(|r| r.age.as_str()).collect::<Vec<_>>(),
            ["2h 10m", "14m", ""]
        );
    }

    #[test]
    fn many_sessions_fold_and_leave_room_for_the_programs() {
        let many: Vec<WslSession> = (0..14)
            .map(|i| {
                session(
                    "Claude Code",
                    Some("shop-site"),
                    100 + u64::from(i),
                    0.0,
                    false,
                    10 + i,
                )
            })
            .collect();
        let snap = with_sessions(many);
        for sort in [Sort::Memory, Sort::Processor] {
            let rows = app_rows(&snap, sort);
            let m = subs(&rows);
            assert_eq!(m.len(), SUB_ROWS, "{sort:?}");
            // Three sessions, the fold, then two programs.
            assert!(
                m[..3]
                    .iter()
                    .all(|r| r.name == "Claude Code \u{b7} shop-site")
            );
            assert_eq!(m[3].name, "Claude Code \u{b7} 11 idle");
            assert_eq!(m[3].dot, Dot::Idle);
            assert!(m[3].age.is_empty());
            assert!(m[4..].iter().all(|r| r.dot == Dot::None));
        }
        let mem = app_rows(&snap, Sort::Memory);
        // 100..=113 MB: the fold holds the 11 smallest, 100 to 110 MB.
        assert_eq!(
            subs(&mem)[3].value,
            format_bytes((100..=110u64).sum::<u64>() << 20)
        );
        // The group still closes after the last row.
        assert!(
            mem.iter()
                .rev()
                .find(|r| r.kind == RowKind::Sub)
                .unwrap()
                .group_end
        );
    }

    #[test]
    fn a_busy_fold_has_a_busy_dot_and_the_stress_data_fits() {
        let mut snap = sample();
        stress_snapshot(&mut snap);
        let w = snap.wsl.as_ref().unwrap();
        assert_eq!(w.agent_count(), 14);
        for sort in [Sort::Memory, Sort::Processor] {
            let rows = app_rows(&snap, sort);
            let m = subs(&rows);
            assert_eq!(m.len(), SUB_ROWS);
            assert_eq!(m.iter().filter(|r| r.dot != Dot::None).count(), 4);
            // Busy sessions lead.
            assert_eq!(m[0].dot, Dot::Busy);
            assert_eq!(m[1].dot, Dot::Busy);
            assert_eq!(m[2].dot, Dot::Idle);
        }
    }

    #[test]
    fn no_sessions_leave_the_programs_alone() {
        let snap = with_sessions(Vec::new());
        let rows = app_rows(&snap, Sort::Memory);
        assert!(subs(&rows).iter().all(|r| r.dot == Dot::None));
        assert_eq!(subs(&rows).len(), SUB_ROWS);
    }
}
