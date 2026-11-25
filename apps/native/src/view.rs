//! Turns readings into the strings, colours and sparkline paths the panel shows.
//! No toolkit types here, so it is plain code and unit tested on any machine.

use pulse_readings::Snapshot;
use std::collections::VecDeque;
use std::fmt::Write as _;

const GB: f64 = 1_073_741_824.0;

/// Samples kept for the sparklines (the web panel keeps 90 as well).
pub const HISTORY: usize = 90;
/// Sparkline box in px, as in the reference (`.fig` inside a 173 px tile).
const FIG_W: f32 = 149.0;
const FIG_H: f32 = 34.0;

/// Ring of recent percentages for the two sparklines.
#[derive(Clone, Debug, Default)]
pub struct History {
    pub cpu: VecDeque<f32>,
    pub mem: VecDeque<f32>,
}

impl History {
    pub fn push(&mut self, snap: &Snapshot) {
        push_capped(&mut self.cpu, snap.cpu_pct);
        let pct = if snap.mem_total == 0 {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            {
                (snap.mem_used as f64 / snap.mem_total as f64 * 100.0) as f32
            }
        };
        push_capped(&mut self.mem, pct);
    }
}

fn push_capped(q: &mut VecDeque<f32>, v: f32) {
    if q.len() == HISTORY {
        q.pop_front();
    }
    q.push_back(v);
}

/// One row of "Top apps", ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct AppView {
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
}

/// Everything the panel shows, as plain data.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub status: String,
    pub cpu_big: String,
    pub cpu_sub: String,
    pub cpu_line: String,
    pub cpu_fill: String,
    pub mem_big: String,
    pub mem_sub: String,
    pub mem_line: String,
    pub mem_fill: String,
    pub apps: Vec<AppView>,
    pub tiles: Tiles,
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

#[allow(clippy::cast_precision_loss)]
pub fn build(snap: &Snapshot, extra: &Extra, hist: &History, sample: bool) -> View {
    let used = snap.mem_used as f64;
    let total = snap.mem_total as f64;
    let (cpu_line, cpu_fill) = sparkline(&hist.cpu);
    let (mem_line, mem_fill) = sparkline(&hist.mem);
    let max = snap
        .apps
        .iter()
        .map(|a| a.mem_bytes)
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    View {
        status: if sample {
            "Sample data, not this PC"
        } else {
            "Running smoothly"
        }
        .into(),
        cpu_big: format!("{}%", round_half_up(f64::from(snap.cpu_pct))),
        cpu_sub: format!("{} cores \u{b7} up {}", snap.cores, uptime(snap.uptime_s)),
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
        apps: snap
            .apps
            .iter()
            .take(if extra.battery.is_some() { 4 } else { 5 })
            .map(|a| {
                let base = a.exe.strip_suffix(".exe").unwrap_or(&a.exe);
                AppView {
                    letter: a.name.chars().next().map(String::from).unwrap_or_default(),
                    rgb: hsl_rgb(hue(base), 0.45, 0.45),
                    name: a.name.clone(),
                    detail: if a.processes > 1 {
                        format!("{} \u{b7} {} processes", a.exe, a.processes)
                    } else {
                        a.exe.clone()
                    },
                    value: format_bytes(a.mem_bytes),
                    frac: (a.mem_bytes as f64 / max) as f32,
                    path: a.path.clone(),
                }
            })
            .collect(),
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
#[allow(clippy::cast_precision_loss)]
fn format_bytes_at(bytes: u64, whole_from: f64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut b = bytes as f64;
    let mut i = 0;
    while b >= 1000.0 && i < 3 {
        b /= 1024.0;
        i += 1;
    }
    if b >= whole_from || i == 0 {
        format!("{} {}", round_half_up(b), UNITS[i])
    } else {
        format!("{b:.1} {}", UNITS[i])
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

/// Sparkline as two SVG path strings (line, and the closed area under it),
/// built like the reference page's canvas code: 90 slots, newest at the right.
#[allow(clippy::cast_precision_loss)]
pub fn sparkline(values: &VecDeque<f32>) -> (String, String) {
    if values.is_empty() {
        return (String::new(), String::new());
    }
    let step = FIG_W / (HISTORY - 1) as f32;
    let first = HISTORY - values.len();
    let mut line = String::new();
    for (i, v) in values.iter().enumerate() {
        let x = (first + i) as f32 * step;
        let y = FIG_H - 2.0 - (v / 100.0).clamp(0.0, 1.0) * (FIG_H - 5.0);
        let _ = write!(line, "{}{x:.2} {y:.2} ", if i == 0 { "M" } else { "L" });
    }
    let mut fill = line.clone();
    let _ = write!(
        fill,
        "L{:.2} {FIG_H} L{:.2} {FIG_H} Z",
        (HISTORY - 1) as f32 * step,
        first as f32 * step
    );
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

    #[test]
    fn sparkline_right_aligns_a_short_history() {
        let v: VecDeque<f32> = [0.0, 100.0].into();
        let (line, fill) = sparkline(&v);
        assert!(line.starts_with("M147.33 32.00 L149.00 3.00"), "{line}");
        assert!(fill.ends_with("L149.00 34 L147.33 34 Z"), "{fill}");
        assert_eq!(sparkline(&VecDeque::new()), (String::new(), String::new()));
    }

    #[test]
    fn history_is_capped() {
        let mut h = History::default();
        let snap = Sample::default().read(true);
        for _ in 0..HISTORY + 10 {
            h.push(&snap);
        }
        assert_eq!(h.cpu.len(), HISTORY);
        assert_eq!(h.mem.len(), HISTORY);
    }

    #[test]
    fn view_of_the_sample_has_five_rows_and_the_top_bar_is_full() {
        let snap = Sample::default().read(true);
        let mut h = History::default();
        h.push(&snap);
        let v = build(&snap, &Extra::from_snapshot(&snap), &h, true);
        assert_eq!(v.apps.len(), 5);
        assert!((v.apps[0].frac - 1.0).abs() < f32::EPSILON);
        assert_eq!(v.apps[0].detail, "chrome.exe \u{b7} 24 processes");
        assert_eq!(v.apps[3].detail, "obs64.exe");
        assert_eq!(v.mem_sub, "of 32 GB \u{b7} 43%");
        assert_eq!(v.cpu_sub, "16 cores \u{b7} up 3d 4h");
        assert_eq!(v.tiles.drive_big, "296 GB");
        assert!(
            v.tiles.net_big.starts_with("\u{2193} ") && v.tiles.net_sub.starts_with("\u{2191} ")
        );
        assert!(!v.tiles.gpu_missing);
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
    }
}
