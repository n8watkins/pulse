//! Coding-agent sessions as rows of the Linux list: which come first, which
//! fold into one line, and how a run time is written. Plain code, no toolkit.
//!
//! The rule (also in the README):
//! 1. Order: busy sessions first, then by the value of the active sort
//!    (memory, or processor), then by name, folder and process id.
//! 2. Up to [`SESSION_LINES`] sessions are one row each.
//! 3. With more, the first `SESSION_LINES - 1` are rows and all the others fold
//!    into one line: "Claude Code · 9 idle" (or "9 idle agents" when they are
//!    not all one agent; "more" instead of "idle" when one of them is busy),
//!    with their memory or processor summed.

use pulse_platform::Sort;
use pulse_readings::WslSession;
use std::collections::{HashMap, HashSet};

/// The most lines the sessions take of the Linux list.
pub const SESSION_LINES: usize = 4;

/// One line of the session list.
#[derive(Debug, PartialEq)]
pub enum Line<'a> {
    One(&'a WslSession),
    Fold(Fold),
}

/// Sessions folded into one line.
#[derive(Debug, PartialEq)]
pub struct Fold {
    /// The agent when all folded sessions are the same one.
    pub common: Option<String>,
    /// The agent with the most folded sessions (for the mark).
    pub lead: String,
    pub count: usize,
    pub any_busy: bool,
    pub mem_bytes: u64,
    pub cpu_pct: f32,
}

impl Fold {
    /// "Claude Code · 9 idle", "9 idle agents", "Claude Code · 3 more".
    pub fn title(&self) -> String {
        let word = if self.any_busy { "more" } else { "idle" };
        match &self.common {
            Some(label) => format!("{label} \u{b7} {} {word}", self.count),
            None => format!("{} {word} agents", self.count),
        }
    }
}

/// The value the sort ranks a session by.
#[allow(clippy::cast_precision_loss)]
fn amount(s: &WslSession, sort: Sort) -> f64 {
    match sort {
        Sort::Memory => s.mem_bytes as f64,
        Sort::Processor => f64::from(s.cpu_pct),
    }
}

/// Busy sessions first, then the biggest in the active sort.
pub fn order(sessions: &[WslSession], sort: Sort) -> Vec<&WslSession> {
    let mut v: Vec<&WslSession> = sessions.iter().collect();
    v.sort_by(|a, b| {
        b.busy
            .cmp(&a.busy)
            .then_with(|| amount(b, sort).total_cmp(&amount(a, sort)))
            .then_with(|| b.mem_bytes.cmp(&a.mem_bytes))
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.folder.cmp(&b.folder))
            .then_with(|| a.pid.cmp(&b.pid))
    });
    v
}

/// The lines to show for the sessions (see the module note).
pub fn plan(sessions: &[WslSession], sort: Sort) -> Vec<Line<'_>> {
    let ordered = order(sessions, sort);
    if ordered.len() <= SESSION_LINES {
        return ordered.into_iter().map(Line::One).collect();
    }
    let (shown, rest) = ordered.split_at(SESSION_LINES - 1);
    let mut per_label: HashMap<&str, usize> = HashMap::new();
    for s in rest {
        *per_label.entry(&s.label).or_default() += 1;
    }
    let lead = per_label
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(l, _)| (*l).to_string())
        .unwrap_or_default();
    let fold = Fold {
        common: (per_label.len() == 1).then(|| lead.clone()),
        lead,
        count: rest.len(),
        any_busy: rest.iter().any(|s| s.busy),
        mem_bytes: rest.iter().map(|s| s.mem_bytes).sum(),
        cpu_pct: rest.iter().map(|s| s.cpu_pct).sum(),
    };
    shown
        .iter()
        .map(|s| Line::One(s))
        .chain(std::iter::once(Line::Fold(fold)))
        .collect()
}

/// The sessions that share their agent and folder with another one: only
/// these show a run time, to tell them apart.
pub fn needs_age(sessions: &[WslSession]) -> HashSet<u32> {
    let mut by_key: HashMap<(&str, Option<&str>), Vec<u32>> = HashMap::new();
    for s in sessions {
        by_key
            .entry((s.label.as_str(), s.folder.as_deref()))
            .or_default()
            .push(s.pid);
    }
    by_key
        .into_values()
        .filter(|pids| pids.len() > 1)
        .flatten()
        .collect()
}

/// A run time, short: "now" under a minute, "14m", "2h 10m", "3d 4h".
pub fn age(secs: u64) -> String {
    let (m, h, d) = (secs / 60, secs / 3_600, secs / 86_400);
    if m == 0 {
        "now".into()
    } else if h == 0 {
        format!("{m}m")
    } else if d == 0 {
        match m % 60 {
            0 => format!("{h}h"),
            rest => format!("{h}h {rest}m"),
        }
    } else {
        match h % 24 {
            0 => format!("{d}d"),
            rest => format!("{d}d {rest}h"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(label: &str, folder: &str, mem_mb: u64, cpu: f32, busy: bool, pid: u32) -> WslSession {
        WslSession {
            label: label.into(),
            folder: Some(folder.into()),
            mem_bytes: mem_mb << 20,
            cpu_pct: cpu,
            busy,
            run_secs: Some(60),
            pid,
        }
    }

    fn names(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| match l {
                Line::One(s) => format!("{}:{}", s.label, s.pid),
                Line::Fold(f) => format!("fold:{}", f.title()),
            })
            .collect()
    }

    #[test]
    fn busy_first_then_the_sort_value() {
        let v = [
            s("Claude Code", "a", 900, 0.0, false, 1),
            s("Claude Code", "b", 100, 0.1, true, 2),
            s("Claude Code", "c", 500, 0.0, false, 3),
            s("Codex CLI", "d", 200, 3.0, true, 4),
        ];
        // Memory: busy first (the bigger of the two first), then idle by size.
        assert_eq!(
            names(&plan(&v, Sort::Memory)),
            [
                "Codex CLI:4",
                "Claude Code:2",
                "Claude Code:1",
                "Claude Code:3"
            ]
        );
        // Processor: busy first by processor, then idle (equal) by memory.
        assert_eq!(
            names(&plan(&v, Sort::Processor)),
            [
                "Codex CLI:4",
                "Claude Code:2",
                "Claude Code:1",
                "Claude Code:3"
            ]
        );
        // A big busy-by-flag session still ranks under a smaller busier one.
        let w = [
            s("Claude Code", "a", 100, 0.1, true, 1),
            s("Claude Code", "b", 900, 2.0, true, 2),
        ];
        assert_eq!(
            names(&plan(&w, Sort::Processor)),
            ["Claude Code:2", "Claude Code:1"]
        );
        assert_eq!(
            names(&plan(&w, Sort::Memory)),
            ["Claude Code:2", "Claude Code:1"]
        );
    }

    #[test]
    fn ties_are_settled_the_same_way_every_time() {
        let v = [
            s("Claude Code", "a", 100, 0.0, false, 9),
            s("Claude Code", "a", 100, 0.0, false, 3),
            s("Aider", "a", 100, 0.0, false, 5),
        ];
        assert_eq!(
            names(&plan(&v, Sort::Memory)),
            ["Aider:5", "Claude Code:3", "Claude Code:9"]
        );
    }

    #[test]
    fn up_to_four_are_rows_and_more_fold_into_one_line() {
        let mut v: Vec<WslSession> = (0..4)
            .map(|i| s("Claude Code", "p", 100 + u64::from(i), 0.0, false, 10 + i))
            .collect();
        assert_eq!(plan(&v, Sort::Memory).len(), 4);
        assert!(
            plan(&v, Sort::Memory)
                .iter()
                .all(|l| matches!(l, Line::One(_)))
        );
        v.push(s("Claude Code", "p", 50, 0.0, false, 20));
        let lines = plan(&v, Sort::Memory);
        // Three rows and one fold of the two smallest.
        assert_eq!(lines.len(), SESSION_LINES);
        let Line::Fold(f) = &lines[3] else {
            panic!("{lines:?}")
        };
        assert_eq!(f.title(), "Claude Code \u{b7} 2 idle");
        assert_eq!(f.mem_bytes, (100 + 50) << 20);
        assert!(!f.any_busy);
    }

    #[test]
    fn fourteen_sessions_take_four_lines_busy_ones_first() {
        let v: Vec<WslSession> = (0..14)
            .map(|i| {
                // Two busy ones among them, both small.
                let busy = i == 5 || i == 9;
                s(
                    "Claude Code",
                    "p",
                    100 + u64::from(i) * 10,
                    if busy { 1.0 } else { 0.0 },
                    busy,
                    100 + i,
                )
            })
            .collect();
        for sort in [Sort::Memory, Sort::Processor] {
            let lines = plan(&v, sort);
            assert_eq!(lines.len(), SESSION_LINES);
            let Line::One(a) = &lines[0] else { panic!() };
            let Line::One(b) = &lines[1] else { panic!() };
            assert!(a.busy && b.busy, "busy sessions lead in {sort:?}");
            let Line::Fold(f) = &lines[3] else { panic!() };
            assert_eq!(f.count, 11);
            assert_eq!(f.title(), "Claude Code \u{b7} 11 idle");
            // The fold holds everything that is not shown, nothing twice.
            let shown: u64 = lines[..3]
                .iter()
                .map(|l| match l {
                    Line::One(s) => s.mem_bytes,
                    Line::Fold(_) => 0,
                })
                .sum();
            let all: u64 = v.iter().map(|s| s.mem_bytes).sum();
            assert_eq!(shown + f.mem_bytes, all);
        }
    }

    #[test]
    fn a_mixed_or_busy_fold_says_so() {
        let mut v = vec![
            s("Claude Code", "a", 900, 0.0, false, 1),
            s("Claude Code", "b", 800, 0.0, false, 2),
            s("Claude Code", "c", 700, 0.0, false, 3),
            s("Codex CLI", "d", 100, 0.0, false, 4),
            s("Gemini CLI", "e", 90, 0.0, false, 5),
        ];
        let lines = plan(&v, Sort::Memory);
        let Line::Fold(f) = &lines[3] else { panic!() };
        assert_eq!(f.title(), "2 idle agents");
        assert_eq!(f.lead, "Codex CLI");
        // Busy overflow: more busy sessions than rows.
        v.iter_mut().for_each(|s| s.busy = true);
        let lines = plan(&v, Sort::Memory);
        let Line::Fold(f) = &lines[3] else { panic!() };
        assert_eq!(f.title(), "2 more agents");
        assert!(f.any_busy);
        let same: Vec<WslSession> = (0..6).map(|i| s("Aider", "x", 10, 0.0, i < 5, i)).collect();
        let lines = plan(&same, Sort::Memory);
        let Line::Fold(f) = &lines[3] else { panic!() };
        assert_eq!(f.title(), "Aider \u{b7} 3 more");
    }

    #[test]
    fn nothing_to_plan_is_nothing() {
        assert!(plan(&[], Sort::Memory).is_empty());
    }

    #[test]
    fn only_sessions_that_look_alike_show_a_run_time() {
        let v = [
            s("Claude Code", "pulse", 1, 0.0, false, 1),
            s("Claude Code", "pulse", 1, 0.0, false, 2),
            s("Claude Code", "shop-site", 1, 0.0, false, 3),
            s("Codex CLI", "pulse", 1, 0.0, false, 4),
        ];
        let n = needs_age(&v);
        assert_eq!(n.len(), 2);
        assert!(n.contains(&1) && n.contains(&2));
        // Two without a folder look alike too.
        let mut w = [
            s("Aider", "x", 1, 0.0, false, 7),
            s("Aider", "x", 1, 0.0, false, 8),
        ];
        w.iter_mut().for_each(|s| s.folder = None);
        assert_eq!(needs_age(&w).len(), 2);
    }

    #[test]
    fn run_times_are_short() {
        assert_eq!(age(0), "now");
        assert_eq!(age(59), "now");
        assert_eq!(age(60), "1m");
        assert_eq!(age(14 * 60 + 59), "14m");
        assert_eq!(age(3_599), "59m");
        assert_eq!(age(3_600), "1h");
        assert_eq!(age(2 * 3_600 + 10 * 60), "2h 10m");
        assert_eq!(age(23 * 3_600 + 59 * 60), "23h 59m");
        assert_eq!(age(86_400), "1d");
        assert_eq!(age(3 * 86_400 + 4 * 3_600 + 59), "3d 4h");
    }
}
