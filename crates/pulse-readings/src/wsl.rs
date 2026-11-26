//! The Linux side of WSL: which programs use the memory of the WSL 2 VM.
//!
//! The parsing and labelling are pure, so their tests run on Linux. The query
//! itself (Windows only) never starts WSL: it runs only when the VM process
//! exists and `wsl.exe -l --running` lists the default distro.

use crate::{WslProc, WslSession};
use std::collections::{HashMap, HashSet};

/// How many labelled groups are returned.
pub(crate) const TOP: usize = 6;

/// The longest "<runtime>: <script>" script name, in characters.
const SCRIPT_MAX: usize = 28;

/// Characters the kernel keeps of a command name.
const COMM_MAX: usize = 15;

/// The longest session folder name kept, in characters (the panel cuts
/// shorter ones to its width).
const FOLDER_MAX: usize = 40;

/// Lines that separate the parts of the output of [`PS_COMMAND`].
const STAT_MARK: &str = "@@PULSE_STAT";
const META_MARK: &str = "@@PULSE_META";
const CWD_MARK: &str = "@@PULSE_CWD";

/// The command run inside the default distro (through `sh -c`), one call:
/// - the 120 biggest processes by memory (`ps`), each line cut to 240
///   characters: the output stays bounded however many processes run, and a
///   coding agent, being big, is not cut off;
/// - the processor times, parent and start of all processes
///   (`/proc/<pid>/stat`). `ps` can only report a lifetime average, so the
///   live processor use is the difference between two dumps of the times;
/// - the home folder and the uptime (to turn a start into a run time);
/// - the working folder of every process (`ls -l` of `/proc/<pid>/cwd`, at
///   most 400 lines; only the user's own processes are readable, which is
///   every session).
///
/// There is no quote character in it: `wsl.exe` would mangle it.
pub(crate) const PS_COMMAND: &str = "ps -eo pid=,rss=,comm=,args= --sort=-rss | head -n 120 | cut -c1-240; echo @@PULSE_STAT; cat /proc/[0-9]*/stat 2>/dev/null; echo @@PULSE_META; echo H=$HOME; cat /proc/uptime; echo @@PULSE_CWD; ls -l /proc/[0-9]*/cwd 2>/dev/null | head -n 400";

/// Clock ticks per second of the times in `/proc/<pid>/stat` (`USER_HZ`, 100
/// on every Linux kernel WSL ships).
const TICKS_PER_SEC: f64 = 100.0;

/// The parts of the output of [`PS_COMMAND`].
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Sections<'a> {
    pub ps: &'a str,
    pub stat: &'a str,
    pub meta: &'a str,
    pub cwd: &'a str,
}

/// Splits the output of [`PS_COMMAND`] at its marker lines. A marker is a line
/// of its own: the command line of the query itself shows in `ps` with the
/// marker words in it, and must not split anything. A missing marker leaves
/// the parts after it empty.
pub(crate) fn split_output(out: &str) -> Sections<'_> {
    fn cut<'a>(text: &'a str, mark: &str) -> (&'a str, &'a str) {
        let mut at = 0;
        for line in text.split_inclusive('\n') {
            if line.trim_end_matches(['\r', '\n']) == mark {
                return (&text[..at], &text[at + line.len()..]);
            }
            at += line.len();
        }
        (text, "")
    }
    let (ps, rest) = cut(out, STAT_MARK);
    let (stat, rest) = cut(rest, META_MARK);
    let (meta, cwd) = cut(rest, CWD_MARK);
    Sections {
        ps,
        stat,
        meta,
        cwd,
    }
}

/// What `/proc/<pid>/stat` tells about one process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stat {
    /// User plus system time, in clock ticks.
    pub ticks: u64,
    pub ppid: u32,
    /// Start time, in clock ticks after boot.
    pub start: u64,
}

/// The stat of every process in a dump of `/proc/<pid>/stat` files, one line
/// each. The command name sits in brackets and may hold spaces and brackets,
/// so the fields are counted from the last closing bracket. Lines that do not
/// parse are ignored.
pub(crate) fn parse_stat(dump: &str) -> HashMap<u32, Stat> {
    let mut out = HashMap::new();
    for line in dump.lines() {
        let Some((pid, rest)) = line.split_once(' ') else {
            continue;
        };
        let (Ok(pid), Some(close)) = (pid.trim().parse::<u32>(), rest.rfind(')')) else {
            continue;
        };
        // After the bracket: state, ppid, ..., utime is field 14, stime 15 and
        // starttime 22 of the file (index 11, 12 and 19 here).
        let fields: Vec<&str> = rest[close + 1..].split_whitespace().collect();
        let num = |i: usize| fields.get(i).and_then(|f| f.parse::<u64>().ok());
        if let (Some(u), Some(s), Some(ppid), Some(start)) = (num(11), num(12), num(1), num(19))
            && let Ok(ppid) = u32::try_from(ppid)
        {
            out.insert(
                pid,
                Stat {
                    ticks: u + s,
                    ppid,
                    start,
                },
            );
        }
    }
    out
}

/// The processor ticks alone, to compare two dumps.
pub(crate) fn ticks(stats: &HashMap<u32, Stat>) -> HashMap<u32, u64> {
    stats.iter().map(|(p, s)| (*p, s.ticks)).collect()
}

/// The home folder and the uptime in seconds, from the part after
/// [`META_MARK`]: a line `H=<home>` and the content of `/proc/uptime`.
pub(crate) fn parse_meta(meta: &str) -> (Option<String>, Option<f64>) {
    let mut home = None;
    let mut uptime = None;
    for line in meta.lines() {
        if let Some(h) = line.strip_prefix("H=") {
            home = Some(h.trim().to_string()).filter(|h| !h.is_empty());
        } else if uptime.is_none()
            && let Some(first) = line.split_whitespace().next()
        {
            uptime = first.parse::<f64>().ok().filter(|u| u.is_finite());
        }
    }
    (home, uptime)
}

/// The working folder of each process from `ls -l /proc/<pid>/cwd` lines
/// (`... /proc/411/cwd -> /home/u/pulse`). A folder that was deleted keeps
/// its path (the " (deleted)" note is dropped).
pub(crate) fn parse_cwds(text: &str) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let Some(at) = line.find(" /proc/") else {
            continue;
        };
        let rest = &line[at + " /proc/".len()..];
        let Some((pid, target)) = rest.split_once("/cwd -> ") else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        let target = target.trim_end();
        let target = target.strip_suffix(" (deleted)").unwrap_or(target);
        if target.starts_with('/') {
            out.insert(pid, target.to_string());
        }
    }
    out
}

/// The name of the last folder of a working directory: never a path. The
/// home folder is `~` (its name would be the user name) and `/` has no name.
pub(crate) fn folder_name(cwd: &str, home: Option<&str>) -> Option<String> {
    let cwd = cwd.trim_end_matches('/');
    if cwd.is_empty() {
        return None;
    }
    if home.is_some_and(|h| h.trim_end_matches('/') == cwd) {
        return Some("~".into());
    }
    let name = cwd.rsplit('/').next().unwrap_or(cwd);
    (!name.is_empty()).then(|| truncate_chars(name, FOLDER_MAX))
}

/// Each process's share of the whole machine, 0 to 100 (the scale of
/// `AppRow::cpu_pct`), from two dumps `secs` apart on a machine with `cores`
/// logical processors. A process missing from the first dump (new) or whose
/// time went down (the id was reused) counts as idle.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub(crate) fn cpu_shares(
    before: &HashMap<u32, u64>,
    after: &HashMap<u32, u64>,
    secs: f64,
    cores: usize,
) -> HashMap<u32, f32> {
    if secs <= 0.0 || cores == 0 {
        return HashMap::new();
    }
    let per_tick = 100.0 / (TICKS_PER_SEC * secs * cores as f64);
    after
        .iter()
        .filter_map(|(pid, now)| {
            let was = before.get(pid)?;
            Some((*pid, (now.saturating_sub(*was) as f64 * per_tick) as f32))
        })
        .collect()
}

/// Decodes the UTF-16LE output of `wsl.exe` (a byte-order mark is dropped, a
/// dangling odd byte is ignored).
pub(crate) fn decode_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let text = String::from_utf16_lossy(&units);
    text.strip_prefix('\u{feff}').unwrap_or(&text).to_string()
}

/// Whether the output of `wsl.exe -l --running` lists the default distro.
///
/// The output is a header line ending in a colon, then one distro per line;
/// the default carries a parenthesised marker, `Ubuntu-24.04 (Default)` in
/// English (any language is accepted, the marker is the brackets). A distro
/// name has no spaces, so any other line (such as "There are no running
/// distributions.") is a message, not a distro. A list with no marked line
/// means the default distro is stopped, and running `wsl -e` would start it,
/// so that counts as "no".
pub(crate) fn default_is_running(listing: &str) -> bool {
    listing.lines().any(|line| {
        let line = line.trim();
        let mut words = line.split_whitespace();
        let (Some(name), Some(marker), None) = (words.next(), words.next(), words.next()) else {
            return false;
        };
        !name.ends_with(':') && marker.starts_with('(') && marker.ends_with(')')
    })
}

/// One line of `ps -eo pid=,rss=,comm=,args=`.
#[derive(Debug, PartialEq)]
struct PsRow<'a> {
    pid: u32,
    rss_kib: u64,
    comm: String,
    args: &'a str,
}

/// Parses one line. The two numbers come first; `comm` may hold spaces (and
/// is cut to 15 characters by the kernel), so the rest is split by looking for
/// the longest run of leading words that the command line starts with, and
/// failing that the first word alone (a renamed thread such as `MainThread`).
fn parse_line(line: &str) -> Option<PsRow<'_>> {
    let rest = line.trim_start();
    let (pid, rest) = split_word(rest)?;
    let pid: u32 = pid.parse().ok()?;
    let (rss, rest) = split_word(rest)?;
    let rss_kib: u64 = rss.parse().ok()?;
    let words: Vec<(usize, &str)> = word_spans(rest);
    if words.is_empty() {
        return None;
    }
    let max_k = words.len().min(5);
    let mut split_at = 1;
    for k in (1..=max_k).rev() {
        let (comm_end, args_start) = if k < words.len() {
            let (s, w) = words[k - 1];
            (s + w.len(), words[k].0)
        } else {
            (rest.len(), rest.len())
        };
        let comm_text = collapse(&rest[..comm_end]);
        let args = &rest[args_start..];
        if args_start > 0 && command_starts_with(args, &comm_text) {
            split_at = k;
            break;
        }
    }
    let (comm_end, args_start) = if split_at < words.len() {
        let (s, w) = words[split_at - 1];
        (s + w.len(), words[split_at].0)
    } else {
        (rest.len(), rest.len())
    };
    Some(PsRow {
        pid,
        rss_kib,
        comm: collapse(&rest[..comm_end]),
        args: rest[args_start..].trim_end(),
    })
}

fn split_word(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    Some((&s[..end], &s[end..]))
}

/// Start offset and text of every whitespace-separated word.
fn word_spans(s: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices() {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(i),
            (true, Some(b)) => {
                out.push((b, &s[b..i]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(b) = start {
        out.push((b, &s[b..]));
    }
    out
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `args` begins with `comm` (the command name, compared after the
/// directory part of the first word is removed).
fn command_starts_with(args: &str, comm: &str) -> bool {
    let args = collapse(args);
    let first_end = args.find(' ').unwrap_or(args.len());
    let first = &args[..first_end];
    let short = file_name(first);
    let tail = &args[first_end..];
    let shortened = format!("{short}{tail}");
    !comm.is_empty() && (shortened.starts_with(comm) || args.starts_with(comm))
}

/// The part after the last `/`.
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Names `ps` shows for the plumbing of the query and of WSL itself.
fn is_skipped(comm: &str) -> bool {
    matches!(comm, "ps" | "sh" | "head" | "init" | "SessionLeader")
        || comm.starts_with("Relay(")
        || comm.starts_with("init-systemd")
}

const RUNTIMES: &[&str] = &[
    "node", "nodejs", "python", "python3", "bun", "deno", "ruby", "java",
];

fn is_runtime(exe: &str) -> bool {
    RUNTIMES.contains(&exe)
        || exe
            .strip_prefix("python3.")
            .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
}

/// Coding agents by the name of their executable or script (lowercase, no
/// extension).
const AGENTS: &[(&str, &str)] = &[
    ("claude", "Claude Code"),
    ("codex", "Codex CLI"),
    ("gemini", "Gemini CLI"),
    ("aider", "Aider"),
    ("opencode", "opencode"),
    ("copilot", "Copilot CLI"),
    ("cursor-agent", "Cursor Agent"),
    ("goose", "Goose"),
    ("qwen", "Qwen Code"),
];

/// Packages (a directory under `node_modules`) whose entry file is not named
/// after the program: `node_modules/@anthropic-ai/claude-code/cli.js`.
const AGENT_PACKAGES: &[(&str, &str)] = &[
    ("claude-code", "Claude Code"),
    ("codex", "Codex CLI"),
    ("gemini-cli", "Gemini CLI"),
    ("aider", "Aider"),
    ("opencode-ai", "opencode"),
    ("opencode", "opencode"),
    ("copilot", "Copilot CLI"),
    ("cursor-agent", "Cursor Agent"),
    ("qwen-code", "Qwen Code"),
];

/// Directory (or file) names that mean an editor's remote server, and the
/// label of that editor. Visual Studio Code's own server is told apart so the
/// panel can show its mark; the other editors share one label.
const EDITOR_DIRS: &[(&str, &str)] = &[
    (".vscode-server", "VS Code server"),
    ("vscode-server", "VS Code server"),
    (".vscode-server-insiders", "VS Code server"),
    (".vscode-remote", "VS Code server"),
    ("vscode-remote", "VS Code server"),
    ("code-server", "VS Code server"),
    (".cursor-server", "Editor server"),
    ("cursor-server", "Editor server"),
    (".windsurf-server", "Editor server"),
    ("windsurf-server", "Editor server"),
];

const ENTRY_FILES: &[&str] = &["cli", "index", "main", "bin"];

/// Lowercase file name without a script or program extension.
fn stem(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    for ext in [".js", ".mjs", ".cjs", ".py", ".exe", ".ts"] {
        if let Some(s) = lower.strip_suffix(ext)
            && !s.is_empty()
        {
            return s.to_string();
        }
    }
    lower
}

fn agent_named(name: &str) -> Option<&'static str> {
    let s = stem(name);
    AGENTS.iter().find(|(k, _)| *k == s).map(|(_, l)| *l)
}

/// An agent whose entry file (`cli.js`) sits in a known package under
/// `node_modules`. Only directory names are compared, never substrings.
fn agent_by_package(path: &str) -> Option<&'static str> {
    let parts: Vec<&str> = path.split('/').collect();
    let nm = parts.iter().position(|p| *p == "node_modules")?;
    let file = stem(parts.last()?);
    if !ENTRY_FILES.contains(&file.as_str()) {
        return None;
    }
    parts[nm + 1..parts.len() - 1].iter().find_map(|dir| {
        AGENT_PACKAGES
            .iter()
            .find(|(k, _)| k == dir)
            .map(|(_, l)| *l)
    })
}

/// The editor-server label when a path has an editor's server directory.
fn editor_label(path: &str) -> Option<&'static str> {
    path.split('/')
        .find_map(|p| EDITOR_DIRS.iter().find(|(d, _)| *d == p).map(|(_, l)| *l))
}

fn is_docker(name: &str) -> bool {
    name == "dockerd" || name.starts_with("containerd")
}

/// At most `max` characters, the last one an ellipsis when something was cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// The kernel keeps 15 characters of a command name (`comm`), so a long one
/// arrives cut ("next-server (v1"). When the command line starts with the cut
/// name, the word it ends in is completed from the command line.
fn complete_comm(comm: &str, args: &str) -> String {
    if comm.chars().count() < COMM_MAX {
        return comm.to_string();
    }
    let args = args.split_whitespace().collect::<Vec<_>>();
    let Some(first) = args.first() else {
        return comm.to_string();
    };
    let short = std::iter::once(file_name(first))
        .chain(args[1..].iter().copied())
        .collect::<Vec<_>>()
        .join(" ");
    if !short.starts_with(comm) {
        return comm.to_string();
    }
    // Up to the end of the word the cut name stops in.
    let end = short[comm.len()..]
        .find(char::is_whitespace)
        .map_or(short.len(), |i| comm.len() + i);
    truncate_chars(&short[..end], SCRIPT_MAX)
}

/// The label of one process and whether it is a coding agent; `None` for the
/// plumbing that is skipped.
fn label(comm: &str, args: &str) -> Option<(String, bool)> {
    if is_skipped(comm) {
        return None;
    }
    let mut words = args.split_whitespace();
    let first = words.next().unwrap_or("");
    let exe = file_name(first);
    let agent = |l: &str| Some((l.to_string(), true));
    let plain = |l: &str| Some((l.to_string(), false));

    if let Some(l) = agent_named(exe) {
        return agent(l);
    }
    // The native Claude Code binary is installed as `.../claude/versions/<n>`.
    if first.contains("/claude/versions/") {
        return agent("Claude Code");
    }
    if is_runtime(exe) {
        let script = words.find(|w| !w.starts_with('-'));
        if let Some(script) = script {
            if let Some(l) = agent_named(file_name(script)).or_else(|| agent_by_package(script)) {
                return agent(l);
            }
            if let Some(l) = editor_label(script).or_else(|| editor_label(first)) {
                return plain(l);
            }
            let name = truncate_chars(file_name(script), SCRIPT_MAX);
            return Some((format!("{exe}: {name}"), false));
        }
        if let Some(l) = editor_label(first) {
            return plain(l);
        }
        return Some((exe.to_string(), false));
    }
    if let Some(l) = editor_label(first) {
        return plain(l);
    }
    if is_docker(exe) || is_docker(comm) {
        return plain("Docker");
    }
    if exe == "ollama" || comm == "ollama" {
        return plain("Ollama");
    }
    if let Some(l) = agent_named(comm) {
        return agent(l);
    }
    // A thread name stands in for the command when a program renames itself.
    let shown = if comm == "MainThread" && !exe.is_empty() {
        exe.to_string()
    } else {
        complete_comm(comm, args)
    };
    Some((shown, false))
}

/// A session turns busy when it and everything it started use at least this
/// share of one processor core (0.10 = 10 percent of a core). Measured on a
/// PC with many waiting Claude Code sessions: a waiting one, with its helper
/// servers, uses 0 to 6 percent of a core, a working one far more...
const BUSY_ON: f32 = 0.10;
/// ...and stays busy until that falls under this share. The gap between the
/// two keeps the dot from flickering on a session near the line.
const BUSY_OFF: f32 = 0.04;

/// Whether a session is busy now: `was` is the previous answer, `cores_used`
/// the processor it and everything it started used since the last query, in
/// cores (1.0 is one whole core).
pub(crate) fn busy_next(was: bool, cores_used: f32) -> bool {
    cores_used >= if was { BUSY_OFF } else { BUSY_ON }
}

/// What one query read, ready to be turned into rows.
pub(crate) struct Reading<'a> {
    /// The `ps` rows.
    pub ps: &'a str,
    /// Parent, start and times of all processes.
    pub stats: &'a HashMap<u32, Stat>,
    /// Each process's share of the machine (empty on the first query).
    pub cpu: &'a HashMap<u32, f32>,
    /// Working folder of each process.
    pub cwds: &'a HashMap<u32, String>,
    pub home: Option<&'a str>,
    /// Seconds since Linux booted.
    pub uptime_s: Option<f64>,
    /// Logical processors (the scale of `cpu`).
    pub cores: usize,
    /// Which sessions (by root process id) were busy at the last query.
    pub was_busy: &'a HashMap<u32, bool>,
}

/// One `ps` row that is kept.
struct Row {
    pid: u32,
    rss_kib: u64,
    label: String,
    agent: bool,
}

/// Groups the `ps` rows: the coding agents into sessions, everything else by
/// label.
///
/// A session is the root process of an agent (an agent process whose parent
/// is not an agent of the same kind) plus its helpers, the agent processes
/// whose parent is, directly, one of the session's own (Claude Code keeps a
/// spare process, Codex runs a native binary under a script). A program an
/// agent started is no part of it: it keeps its own group, and its processor
/// use counts only towards whether the session is busy.
///
/// The groups are the biggest [`TOP`] by memory and the busiest [`TOP`] by
/// processor (so either sort has its full list), largest in memory first.
/// `cpu` is each process's share of the machine (see [`cpu_shares`]); a
/// process without an entry counts as idle. Lines that do not parse are
/// ignored.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn build(r: &Reading<'_>) -> (Vec<WslProc>, Vec<WslSession>) {
    let rows: Vec<Row> =
        r.ps.lines()
            .filter_map(|line| {
                let row = parse_line(line)?;
                let (label, agent) = label(&row.comm, row.args)?;
                Some(Row {
                    pid: row.pid,
                    rss_kib: row.rss_kib,
                    label,
                    agent,
                })
            })
            .collect();
    let by_pid: HashMap<u32, usize> = rows.iter().enumerate().map(|(i, x)| (x.pid, i)).collect();
    let cpu_of = |pid: u32| r.cpu.get(&pid).copied().unwrap_or(0.0);

    // Agent processes, each under the root of its session.
    let mut members: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut agent_pids: HashSet<u32> = HashSet::new();
    for (i, row) in rows.iter().enumerate().filter(|(_, x)| x.agent) {
        agent_pids.insert(row.pid);
        let mut root = i;
        // A short walk: a chain of helpers is never long.
        for _ in 0..8 {
            let parent = r
                .stats
                .get(&rows[root].pid)
                .and_then(|s| by_pid.get(&s.ppid));
            match parent {
                Some(&p) if rows[p].agent && rows[p].label == row.label => root = p,
                _ => break,
            }
        }
        members.entry(rows[root].pid).or_default().push(i);
    }

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, s) in r.stats {
        children.entry(s.ppid).or_default().push(*pid);
    }

    let mut sessions: Vec<WslSession> = members
        .iter()
        .map(|(&root, idx)| {
            let own: HashSet<u32> = idx.iter().map(|&i| rows[i].pid).collect();
            let mem_bytes = idx.iter().map(|&i| rows[i].rss_kib * 1024).sum();
            let cpu_pct: f32 = own.iter().map(|&p| cpu_of(p)).sum();
            // What the session started: every descendant, except the sessions
            // of other agents (which count for themselves).
            let mut tree = cpu_pct;
            let mut seen: HashSet<u32> = own.clone();
            let mut todo: Vec<u32> = own.iter().copied().collect();
            while let Some(p) = todo.pop() {
                for &c in children.get(&p).into_iter().flatten() {
                    if agent_pids.contains(&c) && !own.contains(&c) {
                        continue;
                    }
                    if seen.insert(c) {
                        tree += cpu_of(c);
                        todo.push(c);
                    }
                }
            }
            let cores_used = tree * r.cores as f32 / 100.0;
            let was = r.was_busy.get(&root).copied().unwrap_or(false);
            let root_row = &rows[by_pid[&root]];
            let run_secs = r.uptime_s.zip(r.stats.get(&root)).map(|(up, s)| {
                #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
                let secs = (up - s.start as f64 / TICKS_PER_SEC).max(0.0);
                secs as u64
            });
            WslSession {
                label: root_row.label.clone(),
                folder: r.cwds.get(&root).and_then(|cwd| folder_name(cwd, r.home)),
                mem_bytes,
                cpu_pct,
                busy: busy_next(was, cores_used),
                run_secs,
                pid: root,
            }
        })
        .collect();
    sessions.sort_by(|a, b| a.label.cmp(&b.label).then(a.pid.cmp(&b.pid)));

    let mut groups: HashMap<String, WslProc> = HashMap::new();
    for row in rows.iter().filter(|x| !x.agent) {
        let g = groups.entry(row.label.clone()).or_insert_with(|| WslProc {
            label: row.label.clone(),
            agent: false,
            count: 0,
            mem_bytes: 0,
            cpu_pct: 0.0,
        });
        g.count += 1;
        g.mem_bytes += row.rss_kib * 1024;
        g.cpu_pct += cpu_of(row.pid);
    }
    let mut by_mem: Vec<WslProc> = groups.into_values().collect();
    by_mem.sort_by(|a, b| {
        b.mem_bytes
            .cmp(&a.mem_bytes)
            .then_with(|| a.label.cmp(&b.label))
    });
    let mut by_cpu: Vec<&WslProc> = by_mem.iter().collect();
    by_cpu.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
    let busy: Vec<String> = by_cpu
        .iter()
        .take(TOP)
        .filter(|p| p.cpu_pct > 0.0)
        .map(|p| p.label.clone())
        .collect();
    let mut kept = 0;
    by_mem.retain(|p| {
        kept += 1;
        kept <= TOP || busy.contains(&p.label)
    });
    (by_mem, sessions)
}

#[cfg(windows)]
pub(crate) use runner::Query;

/// The Windows side: spawn `wsl.exe` without a window, off the reading thread.
#[cfg(windows)]
mod runner {
    use super::{
        PS_COMMAND, Reading, build, cpu_shares, decode_utf16le, default_is_running, parse_cwds,
        parse_meta, parse_stat, split_output, ticks,
    };
    use crate::{WslProc, WslSession};
    use std::collections::HashMap;
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    /// Least time between two queries.
    const EVERY: Duration = Duration::from_secs(6);
    /// Longest a query (both commands together) may take.
    const TIMEOUT: Duration = Duration::from_secs(5);

    /// What a query found.
    #[derive(Clone, Default)]
    pub(crate) struct Found {
        pub procs: Vec<WslProc>,
        pub sessions: Vec<WslSession>,
    }

    #[derive(Default)]
    struct Shared {
        found: Found,
        running: bool,
        /// When the last stat dump was read, and what it held.
        prev: Option<Dump>,
    }

    /// The time of a stat dump, its processor ticks, and which sessions were
    /// busy then (by root process id).
    type Dump = (Instant, HashMap<u32, u64>, HashMap<u32, bool>);

    /// Runs the query on its own thread and keeps the last finished result.
    #[derive(Default)]
    pub(crate) struct Query {
        shared: Arc<Mutex<Shared>>,
        last_start: Option<Instant>,
    }

    impl Query {
        /// Call on every read with apps. `vm_present`: a `vmmemWSL` or `vmmem`
        /// process exists. Never blocks.
        pub(crate) fn poll(&mut self, vm_present: bool) -> Found {
            if !vm_present {
                // Nothing runs: forget the old result and start nothing.
                if let Ok(mut s) = self.shared.lock() {
                    s.found = Found::default();
                    s.prev = None;
                }
                return Found::default();
            }
            let due = self.last_start.is_none_or(|t| t.elapsed() >= EVERY);
            let idle = self.shared.lock().is_ok_and(|s| !s.running);
            if due && idle {
                self.last_start = Some(Instant::now());
                if let Ok(mut s) = self.shared.lock() {
                    s.running = true;
                }
                let shared = Arc::clone(&self.shared);
                let spawned = std::thread::Builder::new().name("pulse-wsl".into()).spawn({
                    let shared = Arc::clone(&shared);
                    move || {
                        let prev = shared.lock().ok().and_then(|mut s| s.prev.take());
                        let (result, dump) = query(prev);
                        if let Ok(mut s) = shared.lock() {
                            s.found = result;
                            s.prev = dump;
                            s.running = false;
                        }
                    }
                });
                if spawned.is_err()
                    && let Ok(mut s) = shared.lock()
                {
                    s.running = false;
                }
            }
            self.shared
                .lock()
                .map(|s| s.found.clone())
                .unwrap_or_default()
        }
    }

    /// The labelled programs and sessions, or nothing when the default distro
    /// is not running or anything failed or timed out.
    fn query(prev: Option<Dump>) -> (Found, Option<Dump>) {
        let deadline = Instant::now() + TIMEOUT;
        let Some(listing) = run(&["-l", "--running"], deadline) else {
            return (Found::default(), None);
        };
        if !default_is_running(&decode_utf16le(&listing)) {
            return (Found::default(), None);
        }
        let Some(out) = run(&["-e", "sh", "-c", PS_COMMAND], deadline) else {
            return (Found::default(), None);
        };
        let at = Instant::now();
        let text = String::from_utf8_lossy(&out);
        let parts = split_output(&text);
        let stats = parse_stat(parts.stat);
        let now_ticks = ticks(&stats);
        let (home, uptime_s) = parse_meta(parts.meta);
        let cwds = parse_cwds(parts.cwd);
        // The first query has nothing to compare with: every program is idle.
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        let (shares, was_busy) = match prev {
            Some((was, before, busy)) => (
                cpu_shares(
                    &before,
                    &now_ticks,
                    at.duration_since(was).as_secs_f64(),
                    cores,
                ),
                busy,
            ),
            None => (HashMap::new(), HashMap::new()),
        };
        let (procs, sessions) = build(&Reading {
            ps: parts.ps,
            stats: &stats,
            cpu: &shares,
            cwds: &cwds,
            home: home.as_deref(),
            uptime_s,
            cores,
            was_busy: &was_busy,
        });
        let busy = sessions.iter().map(|x| (x.pid, x.busy)).collect();
        (Found { procs, sessions }, Some((at, now_ticks, busy)))
    }

    /// Runs `wsl.exe` with no window and returns stdout, or `None` on failure
    /// or when `deadline` passes (the child is killed then; it is our own).
    fn run(args: &[&str], deadline: Instant) -> Option<Vec<u8>> {
        let mut child = Command::new("wsl.exe")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stdout.read_to_end(&mut buf);
            buf
        });
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let out = reader.join().ok()?;
                    return status.success().then_some(out);
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(40));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(comm: &str, args: &str) -> Option<(String, bool)> {
        label(comm, args)
    }

    #[test]
    fn decodes_utf16() {
        let text = "Ubuntu (Default)\r\n";
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        bytes.push(0x41);
        assert_eq!(decode_utf16le(&bytes), text);
    }

    #[test]
    fn default_distro_detection() {
        assert!(default_is_running(
            "Windows Subsystem for Linux Distributions:\r\nUbuntu-24.04 (Default)\r\n"
        ));
        // Localised marker.
        assert!(default_is_running(
            "Distributionen:\r\nDebian\r\nUbuntu (Standard)\r\n"
        ));
        // Another distro runs, the default does not: starting it is not allowed.
        assert!(!default_is_running(
            "Windows Subsystem for Linux Distributions:\r\nDebian\r\n"
        ));
        assert!(!default_is_running(
            "There are no running distributions.\r\n"
        ));
        assert!(!default_is_running(""));
        // The header alone, even with brackets in it, is not a distro.
        assert!(!default_is_running("Distributions (all):\r\n"));
    }

    #[test]
    fn parses_ps_lines() {
        let r = parse_line("  351 440764 claude         claude --resume 29e1").unwrap();
        assert_eq!(
            (r.pid, r.rss_kib, r.comm.as_str(), r.args),
            (351, 440_764, "claude", "claude --resume 29e1")
        );
        // Spaces in comm, cut at 15 characters.
        let r = parse_line("675 889456 next-server (v1 next-server (v16.4.6)").unwrap();
        assert_eq!(r.comm, "next-server (v1");
        assert_eq!(r.args, "next-server (v16.4.6)");
        let r = parse_line(" 12   64 npm ci        npm ci").unwrap();
        assert_eq!((r.comm.as_str(), r.args), ("npm ci", "npm ci"));
        // A renamed thread: comm differs from the command line.
        let r = parse_line("91 184404 MainThread     /usr/bin/node /srv/app.js --x").unwrap();
        assert_eq!(
            (r.comm.as_str(), r.args),
            ("MainThread", "/usr/bin/node /srv/app.js --x")
        );
        // A kernel-style line with no arguments of its own.
        let r = parse_line("2 10 kthreadd [kthreadd]").unwrap();
        assert_eq!(r.comm, "kthreadd");
        // Garbage and short lines.
        assert!(parse_line("").is_none());
        assert!(parse_line("abc 12 x y").is_none());
        assert!(parse_line("1.5 12 x y").is_none());
        assert!(parse_line("10").is_none());
        assert!(parse_line("10 12").is_none());
        // Comm only (empty args).
        let r = parse_line("5 12 foo").unwrap();
        assert_eq!((r.comm.as_str(), r.args), ("foo", ""));
    }

    /// A `/proc/<pid>/stat` line: fields 4, 14, 15 and 22 are the parent, the
    /// user and system time and the start.
    fn stat_line(pid: u32, ppid: u32, comm: &str, utime: u64, stime: u64, start: u64) -> String {
        format!(
            "{pid} ({comm}) S {ppid} 1 1 0 -1 4194560 100 0 0 0 {utime} {stime} 0 0 20 0 1 0 {start} 1000 50 18446744073709551615\n"
        )
    }

    #[test]
    fn parses_stat_dumps() {
        let dump = format!(
            "{}{}{}garbage\n12 (x) S\n",
            stat_line(10, 1, "claude", 300, 120, 700),
            // The name holds spaces and brackets.
            stat_line(11, 10, "a) b (c", 7, 3, 800),
            stat_line(12, 10, "next-server (v1", 0, 0, 900),
        );
        let t = parse_stat(&dump);
        assert_eq!(
            t.get(&10),
            Some(&Stat {
                ticks: 420,
                ppid: 1,
                start: 700
            })
        );
        assert_eq!(t[&11].ticks, 10);
        assert_eq!((t[&11].ppid, t[&11].start), (10, 800));
        assert_eq!(t[&12].ticks, 0);
        assert_eq!(t.len(), 3);
        assert!(parse_stat("").is_empty());
        assert_eq!(ticks(&t)[&10], 420);
    }

    #[test]
    fn splits_the_output_in_four() {
        let p = split_output(
            "1 2 a b\n@@PULSE_STAT\n10 (a) S\n@@PULSE_META\nH=/home/u\n12.5 3.0\n@@PULSE_CWD\nl /proc/1/cwd -> /\n",
        );
        assert_eq!(p.ps, "1 2 a b\n");
        assert_eq!(p.stat, "10 (a) S\n");
        assert_eq!(p.meta, "H=/home/u\n12.5 3.0\n");
        assert_eq!(p.cwd, "l /proc/1/cwd -> /\n");
        // Parts that are missing are empty.
        let p = split_output("1 2 a b\n");
        assert_eq!((p.ps, p.stat, p.meta, p.cwd), ("1 2 a b\n", "", "", ""));
        let p = split_output("x\n@@PULSE_STAT\ny\n");
        assert_eq!((p.ps, p.stat, p.meta, p.cwd), ("x\n", "y\n", "", ""));
        // The query's own command line shows in `ps`, markers and all.
        let text = format!(
            "1 2 sh sh -c ps; echo @@PULSE_STAT; echo @@PULSE_CWD\n3 4 b b\n{STAT_MARK}\n10 (a) S\n{META_MARK}\nH=/h\n1.0 1.0\n{CWD_MARK}\nx\n"
        );
        let p = split_output(&text);
        assert!(p.ps.ends_with("3 4 b b\n") && p.ps.starts_with("1 2 sh"));
        assert_eq!(
            (p.stat, p.meta, p.cwd),
            ("10 (a) S\n", "H=/h\n1.0 1.0\n", "x\n")
        );
    }

    #[test]
    fn the_command_is_one_bounded_call_without_quotes() {
        assert!(!PS_COMMAND.contains(['"', '\'']));
        assert!(PS_COMMAND.contains("head -n 120"));
        assert!(PS_COMMAND.contains("cut -c1-240"));
        assert!(PS_COMMAND.contains("head -n 400"));
        for mark in [STAT_MARK, META_MARK, CWD_MARK] {
            assert!(PS_COMMAND.contains(mark));
        }
    }

    #[test]
    fn reads_home_and_uptime() {
        assert_eq!(
            parse_meta("\nH=/home/u\n12345.67 98765.43\n"),
            (Some("/home/u".into()), Some(12345.67))
        );
        assert_eq!(parse_meta("\nH=\n5.0 1.0\n"), (None, Some(5.0)));
        assert_eq!(parse_meta(""), (None, None));
        assert_eq!(parse_meta("H=/x\nnan 1\n").1, None);
    }

    #[test]
    fn reads_working_folders() {
        let text = "\
lrwxrwxrwx 1 u u 0 Nov 25 10:00 /proc/411/cwd -> /home/u/projects/pulse
lrwxrwxrwx 1 u u 0 Nov 25 10:00 /proc/412/cwd -> /home/u/my project
lrwxrwxrwx 1 u u 0 Nov 25 10:00 /proc/413/cwd -> /home/u/gone (deleted)
lrwxrwxrwx 1 u u 0 Nov 25 10:00 /proc/414/cwd -> /
junk
lrwxrwxrwx 1 u u 0 Nov 25 10:00 /proc/self/cwd -> /x
";
        let m = parse_cwds(text);
        assert_eq!(m[&411], "/home/u/projects/pulse");
        assert_eq!(m[&412], "/home/u/my project");
        assert_eq!(m[&413], "/home/u/gone");
        assert_eq!(m[&414], "/");
        assert_eq!(m.len(), 4);
    }

    #[test]
    fn a_folder_is_a_name_never_a_path() {
        let home = Some("/home/u");
        assert_eq!(
            folder_name("/home/u/projects/pulse", home).as_deref(),
            Some("pulse")
        );
        assert_eq!(
            folder_name("/home/u/projects/pulse/", home).as_deref(),
            Some("pulse")
        );
        assert_eq!(folder_name("/home/u", home).as_deref(), Some("~"));
        assert_eq!(folder_name("/home/u/", home).as_deref(), Some("~"));
        assert_eq!(folder_name("/", home), None);
        assert_eq!(folder_name("", home), None);
        assert_eq!(folder_name("/srv/site", None).as_deref(), Some("site"));
        let long = format!("/w/{}", "x".repeat(100));
        let f = folder_name(&long, home).unwrap();
        assert_eq!(f.chars().count(), FOLDER_MAX);
        assert!(f.ends_with('\u{2026}'));
    }

    #[test]
    fn splits_are_gone_shares_stay() {
        let before: HashMap<u32, u64> = [(1, 1000), (2, 50), (3, 500)].into();
        // Over 5 s on 4 cores: pid 1 used 100 ticks = 1 s = 20% of one core =
        // 5% of the machine; pid 3 went backwards (id reused); pid 4 is new.
        let after: HashMap<u32, u64> = [(1, 1100), (2, 900), (3, 10), (4, 7)].into();
        let s = cpu_shares(&before, &after, 5.0, 4);
        assert!((s[&1] - 5.0).abs() < 1e-4, "{s:?}");
        assert_eq!(s[&3], 0.0);
        assert!(!s.contains_key(&4));
        assert!(s.contains_key(&2));
        // Everything busy on every core is 100.
        let all = cpu_shares(&[(1, 0)].into(), &[(1, 4 * 500)].into(), 5.0, 4);
        assert!((all[&1] - 100.0).abs() < 1e-3);
        assert!(cpu_shares(&before, &after, 0.0, 4).is_empty());
        assert!(cpu_shares(&before, &after, 5.0, 0).is_empty());
    }

    #[test]
    fn agents_are_named() {
        let want = |c, a, l: &str| assert_eq!(one(c, a), Some((l.to_string(), true)), "{a}");
        want("claude", "claude --resume x", "Claude Code");
        want("claude", "/home/u/.local/bin/claude", "Claude Code");
        want(
            "MainThread",
            "node /usr/lib/node_modules/@anthropic-ai/claude-code/cli.js",
            "Claude Code",
        );
        want("codex", "codex exec", "Codex CLI");
        want(
            "MainThread",
            "node /x/node_modules/@openai/codex/bin/codex.js",
            "Codex CLI",
        );
        want(
            "MainThread",
            "node /x/node_modules/@google/gemini-cli/dist/index.js",
            "Gemini CLI",
        );
        want(
            "aider",
            "/usr/bin/python3 /home/u/.local/bin/aider --model x",
            "Aider",
        );
        want("opencode", "opencode", "opencode");
        want("copilot", "copilot", "Copilot CLI");
        want("cursor-agent", "cursor-agent", "Cursor Agent");
        want("goose", "goose session", "Goose");
        want("qwen", "qwen", "Qwen Code");
    }

    #[test]
    fn matches_names_not_paths() {
        // A project folder named after an agent is not the agent.
        assert_eq!(
            one("MainThread", "node /home/x/claude-notes/server.js"),
            Some(("node: server.js".into(), false))
        );
        assert_eq!(
            one("MainThread", "node /home/x/claude/server.js"),
            Some(("node: server.js".into(), false))
        );
        assert_eq!(
            one("server", "/home/x/claude-notes/server --port 1"),
            Some(("server".into(), false))
        );
        // cli.js outside node_modules is just a script.
        assert_eq!(
            one("MainThread", "node /home/x/claude-code/cli.js"),
            Some(("node: cli.js".into(), false))
        );
    }

    #[test]
    fn other_programs() {
        let plain = |l: &str| Some((l.to_string(), false));
        assert_eq!(
            one(
                "MainThread",
                "/home/u/.vscode-server/bin/abc/node /home/u/.vscode-server/bin/abc/out/server-main.js"
            ),
            plain("VS Code server")
        );
        assert_eq!(
            one(
                "node",
                "node /home/u/.vscode-server-insiders/bin/1/out/x.js"
            ),
            plain("VS Code server")
        );
        assert_eq!(
            one(
                "code-server",
                "/usr/lib/code-server/lib/node /usr/lib/code-server --port 8080"
            ),
            plain("VS Code server")
        );
        assert_eq!(
            one("node", "node /home/u/.vscode-remote/x.js"),
            plain("VS Code server")
        );
        assert_eq!(
            one("node", "node /home/u/.cursor-server/bin/1/out/x.js"),
            plain("Editor server")
        );
        assert_eq!(
            one("MainThread", "/home/u/.windsurf-server/bin/node x.js"),
            plain("Editor server")
        );
        assert_eq!(one("dockerd", "/usr/bin/dockerd -H fd://"), plain("Docker"));
        assert_eq!(one("containerd", "/usr/bin/containerd"), plain("Docker"));
        assert_eq!(
            one("ollama", "/usr/local/bin/ollama serve"),
            plain("Ollama")
        );
        assert_eq!(
            one(
                "python3",
                "python3 -u /srv/app/worker_with_a_really_long_name_indeed.py"
            ),
            plain("python3: worker_with_a_really_long_n\u{2026}")
        );
        // A name that just fits is not marked.
        assert_eq!(
            one("node", "node /srv/pool_entry-[turbopack-node]_"),
            plain("node: pool_entry-[turbopack-node]_")
        );
        // The kernel cuts a command name at 15 characters ("next-server (v1"):
        // the whole last word comes from the command line.
        assert_eq!(
            one("next-server (v1", "next-server (v16.4.6)"),
            plain("next-server (v16.4.6)")
        );
        assert_eq!(
            one("chrome-headless", "/opt/chrome-headless-shell --headless"),
            plain("chrome-headless-shell")
        );
        // Not cut, or the command line does not start with it: unchanged.
        assert_eq!(one("short name", "short name --x"), plain("short name"));
        assert_eq!(
            one("fifteen-chars-x", "/bin/other --x"),
            plain("fifteen-chars-x")
        );
        assert_eq!(one("bun", "bun --hot run dev.ts"), plain("bun: run"));
        assert_eq!(
            one("java", "java -jar /opt/x/app.jar"),
            plain("java: app.jar")
        );
        assert_eq!(one("python3", "python3"), plain("python3"));
        assert_eq!(
            one("rust-analyzer", "/home/u/.rustup/bin/rust-analyzer"),
            plain("rust-analyzer")
        );
        assert_eq!(one("MainThread", "node"), plain("node"));
    }

    #[test]
    fn plumbing_is_skipped() {
        for c in [
            "ps",
            "sh",
            "head",
            "init",
            "SessionLeader",
            "Relay(270)",
            "init-systemd(Ub",
        ] {
            assert_eq!(one(c, "/init"), None, "{c}");
        }
    }

    /// A small world for the grouping tests: `ps` rows and their stats.
    struct World {
        ps: String,
        stats: HashMap<u32, Stat>,
        cpu: HashMap<u32, f32>,
        cwds: HashMap<u32, String>,
        was_busy: HashMap<u32, bool>,
    }

    impl World {
        fn new() -> Self {
            Self {
                ps: String::new(),
                stats: HashMap::new(),
                cpu: HashMap::new(),
                cwds: HashMap::new(),
                was_busy: HashMap::new(),
            }
        }

        /// A process: id, parent, `comm`, command line, memory in KiB.
        fn add(&mut self, pid: u32, ppid: u32, comm: &str, args: &str, rss: u64) -> &mut Self {
            self.ps += &format!("{pid} {rss} {comm} {args}\n");
            self.stats.insert(
                pid,
                Stat {
                    ticks: 0,
                    ppid,
                    start: 0,
                },
            );
            self
        }

        fn cpu(&mut self, pid: u32, pct: f32) -> &mut Self {
            self.cpu.insert(pid, pct);
            self
        }

        fn cwd(&mut self, pid: u32, path: &str) -> &mut Self {
            self.cwds.insert(pid, path.into());
            self
        }

        /// Ten cores: 10% of the machine is one whole core.
        fn build(&self) -> (Vec<WslProc>, Vec<WslSession>) {
            build(&Reading {
                ps: &self.ps,
                stats: &self.stats,
                cpu: &self.cpu,
                cwds: &self.cwds,
                home: Some("/home/u"),
                uptime_s: Some(10_000.0),
                cores: 10,
                was_busy: &self.was_busy,
            })
        }
    }

    const CLAUDE: &str = "/home/u/.local/share/claude/versions/2.1.332 --resume";

    #[test]
    fn helpers_join_their_session_and_started_programs_do_not() {
        let mut w = World::new();
        w.add(1, 0, "init", "/init", 10);
        // Session A: a root and its spare helper.
        w.add(11, 1, "2.1.332", CLAUDE, 400_000)
            .add(
                12,
                11,
                "2.1.332",
                "claude bg-spare --bg-spare /tmp/x.sock",
                50_000,
            )
            .cwd(11, "/home/u/projects/pulse");
        // A dev server the agent started: its own row, not in the session.
        w.add(20, 11, "MainThread", "node /srv/app/dev-server.js", 300_000);
        // Session B in another folder.
        w.add(13, 1, "2.1.332", CLAUDE, 200_000)
            .cwd(13, "/home/u/shop-site");
        let (procs, sessions) = w.build();
        assert_eq!(sessions.len(), 2);
        let a = sessions.iter().find(|s| s.pid == 11).unwrap();
        assert_eq!(a.label, "Claude Code");
        assert_eq!(a.folder.as_deref(), Some("pulse"));
        assert_eq!(a.mem_bytes, 450_000 * 1024, "root plus helper only");
        let b = sessions.iter().find(|s| s.pid == 13).unwrap();
        assert_eq!(b.folder.as_deref(), Some("shop-site"));
        assert_eq!(b.mem_bytes, 200_000 * 1024);
        // The dev server keeps its own group; agents are not in the groups.
        assert!(procs.iter().any(|p| p.label == "node: dev-server.js"));
        assert!(procs.iter().all(|p| !p.agent && p.label != "Claude Code"));
    }

    #[test]
    fn the_session_count_is_the_number_of_agents() {
        let mut w = World::new();
        w.add(11, 1, "2.1.332", CLAUDE, 1)
            .add(12, 11, "2.1.332", CLAUDE, 1)
            .add(13, 1, "2.1.332", CLAUDE, 1)
            .add(14, 1, "codex", "codex exec", 1);
        let (_, sessions) = w.build();
        let wsl = crate::Wsl {
            sessions,
            ..crate::Wsl::default()
        };
        // Four agent processes, three sessions.
        assert_eq!(wsl.agent_count(), 3);
    }

    #[test]
    fn another_agent_or_a_nested_session_is_its_own() {
        let mut w = World::new();
        w.add(11, 1, "2.1.332", CLAUDE, 100)
            // Codex started by Claude Code: a different agent, a session of its own.
            .add(21, 11, "codex", "codex exec", 100)
            // Claude Code run by a shell the agent started: its own session.
            .add(31, 11, "bash", "bash -c claude", 10)
            .add(32, 31, "2.1.332", CLAUDE, 100)
            // A script wrapper with its native binary under it: one session.
            .add(
                41,
                1,
                "MainThread",
                "node /x/node_modules/@openai/codex/bin/codex.js",
                80,
            )
            .add(42, 41, "codex", "/x/codex/vendor/codex exec", 120);
        let (_, sessions) = w.build();
        let by = |pid| sessions.iter().find(|s| s.pid == pid);
        assert_eq!(sessions.len(), 4);
        assert!(by(11).is_some() && by(21).is_some() && by(32).is_some());
        assert!(by(12).is_none() && by(31).is_none() && by(42).is_none());
        let wrapped = by(41).unwrap();
        assert_eq!(wrapped.label, "Codex CLI");
        assert_eq!(wrapped.mem_bytes, 200 * 1024);
    }

    #[test]
    fn programs_the_agent_started_make_it_busy_but_add_nothing_to_it() {
        let mut w = World::new();
        w.add(11, 1, "2.1.332", CLAUDE, 1000)
            .add(20, 11, "bash", "bash -c build", 10)
            .add(21, 20, "cargo", "cargo test", 10)
            .add(13, 1, "2.1.332", CLAUDE, 1000);
        // The test run uses a whole core (10% of ten cores); the agent itself
        // nothing. The other session is not busy.
        w.cpu(21, 10.0).cpu(11, 0.0);
        let (procs, sessions) = w.build();
        let a = sessions.iter().find(|s| s.pid == 11).unwrap();
        assert!(a.busy);
        assert_eq!(a.cpu_pct, 0.0, "the figure is the session's own");
        assert!(!sessions.iter().find(|s| s.pid == 13).unwrap().busy);
        // The test run still has its row, with its processor use.
        let cargo = procs.iter().find(|p| p.label == "cargo").unwrap();
        assert!((cargo.cpu_pct - 10.0).abs() < 1e-6);
    }

    #[test]
    fn another_session_is_not_counted_for_this_one() {
        let mut w = World::new();
        w.add(11, 1, "2.1.332", CLAUDE, 10)
            .add(21, 11, "codex", "codex exec", 10);
        w.cpu(21, 50.0);
        let (_, sessions) = w.build();
        let outer = sessions.iter().find(|s| s.pid == 11).unwrap();
        let inner = sessions.iter().find(|s| s.pid == 21).unwrap();
        assert!(!outer.busy, "the nested agent's work is its own");
        assert!(inner.busy);
        assert!((inner.cpu_pct - 50.0).abs() < 1e-6);
    }

    #[test]
    fn the_first_query_has_no_processor_figures_so_all_are_idle() {
        let mut w = World::new();
        w.add(11, 1, "2.1.332", CLAUDE, 10);
        let (_, sessions) = w.build();
        assert!(!sessions[0].busy);
        assert_eq!(sessions[0].cpu_pct, 0.0);
    }

    #[test]
    fn busy_has_a_threshold_and_a_hysteresis() {
        // Idle turns busy at 10 percent of a core, not before.
        assert!(!busy_next(false, 0.0));
        assert!(!busy_next(false, 0.099));
        assert!(busy_next(false, 0.10));
        assert!(busy_next(false, 1.5));
        // Busy stays busy down to 4 percent of a core.
        assert!(busy_next(true, 0.10));
        assert!(busy_next(true, 0.041));
        assert!(busy_next(true, 0.04));
        assert!(!busy_next(true, 0.039));
        assert!(!busy_next(true, 0.0));
        // Between the two, the answer is the previous one: no flicker.
        for u in [0.042_f32, 0.07, 0.099] {
            assert!(busy_next(true, u));
            assert!(!busy_next(false, u));
        }
        // A noisy series around the line changes the state once, not each tick.
        let series = [0.15, 0.07, 0.12, 0.05, 0.08, 0.045, 0.1, 0.02, 0.07];
        let mut state = false;
        let mut flips = 0;
        for u in series {
            let next = busy_next(state, u);
            flips += i32::from(next != state);
            state = next;
        }
        assert_eq!(flips, 2, "on at 0.15, off at 0.02, nothing in between");
        assert!(!state);
    }

    #[test]
    fn busy_follows_the_last_query_through_the_session_id() {
        let mut w = World::new();
        w.add(11, 1, "2.1.332", CLAUDE, 10);
        // 6 percent of a core: busy only because it was busy before.
        w.cpu(11, 0.6);
        assert!(!w.build().1[0].busy);
        w.was_busy.insert(11, true);
        assert!(w.build().1[0].busy);
        // Another session's state does not carry over.
        w.was_busy = [(99, true)].into();
        assert!(!w.build().1[0].busy);
    }

    #[test]
    fn run_time_comes_from_the_start_and_the_uptime() {
        let mut w = World::new();
        w.add(11, 1, "2.1.332", CLAUDE, 10);
        // Started 8 minutes after boot (48000 ticks), 10000 s up.
        w.stats.get_mut(&11).unwrap().start = 48_000;
        let (_, sessions) = w.build();
        assert_eq!(sessions[0].run_secs, Some(10_000 - 480));
        let none = build(&Reading {
            ps: &w.ps,
            stats: &w.stats,
            cpu: &w.cpu,
            cwds: &w.cwds,
            home: None,
            uptime_s: None,
            cores: 10,
            was_busy: &w.was_busy,
        });
        assert_eq!(none.1[0].run_secs, None);
        assert_eq!(none.1[0].folder, None);
    }

    #[test]
    fn a_session_without_a_readable_folder_has_none() {
        let mut w = World::new();
        w.add(11, 1, "2.1.332", CLAUDE, 10)
            .add(13, 1, "2.1.332", CLAUDE, 10);
        w.cwd(13, "/home/u");
        let (_, sessions) = w.build();
        assert_eq!(sessions.iter().find(|s| s.pid == 11).unwrap().folder, None);
        assert_eq!(
            sessions
                .iter()
                .find(|s| s.pid == 13)
                .unwrap()
                .folder
                .as_deref(),
            Some("~")
        );
    }

    #[test]
    fn groups_and_sorts() {
        let out = "\
 11 440764 claude         claude --resume a
 13 100000 sh             sh -c ps
 14 1000 ps             ps -eo
 15 600000 MainThread     node /home/u/.vscode-server/bin/x/out/server-main.js
 16 100 SessionLeader   /init
 junk line
 17 2048 Relay(5)       /init
 18 1 a b c
";
        let cpu: HashMap<u32, f32> = [(11, 0.5), (15, 1.5), (13, 40.0)].into();
        let stats = HashMap::new();
        let cwds = HashMap::new();
        let busy = HashMap::new();
        let (rows, sessions) = build(&Reading {
            ps: out,
            stats: &stats,
            cpu: &cpu,
            cwds: &cwds,
            home: None,
            uptime_s: None,
            cores: 1,
            was_busy: &busy,
        });
        assert_eq!(rows[0].label, "VS Code server");
        assert!((rows[0].cpu_pct - 1.5).abs() < 1e-6);
        assert_eq!(rows[0].mem_bytes, 600_000 * 1024);
        // Plumbing (pid 13) adds nothing; the agent is a session, not a group.
        assert_eq!(rows[1].label, "a");
        assert_eq!(rows[1].cpu_pct, 0.0);
        assert_eq!(rows.len(), 2);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].label, "Claude Code");
        assert!((sessions[0].cpu_pct - 0.5).abs() < 1e-6);
    }

    fn many_programs() -> String {
        (0..10)
            .map(|i| format!("{} {} prog{i} /bin/prog{i}\n", 500 + i, 100 + i))
            .collect()
    }

    fn groups(out: &str, cpu: &HashMap<u32, f32>) -> Vec<WslProc> {
        let (stats, cwds, busy) = (HashMap::new(), HashMap::new(), HashMap::new());
        build(&Reading {
            ps: out,
            stats: &stats,
            cpu,
            cwds: &cwds,
            home: None,
            uptime_s: None,
            cores: 1,
            was_busy: &busy,
        })
        .0
    }

    #[test]
    fn keeps_six() {
        let rows = groups(&many_programs(), &HashMap::new());
        assert_eq!(rows.len(), TOP);
        assert_eq!(rows[0].label, "prog9");
    }

    #[test]
    fn a_busy_small_program_is_kept_for_the_processor_sort() {
        // prog0 is the smallest in memory and the busiest; prog1 is second.
        let cpu: HashMap<u32, f32> = [(500, 9.0), (501, 4.0)].into();
        let rows = groups(&many_programs(), &cpu);
        assert_eq!(rows.len(), TOP + 2);
        // Still largest in memory first.
        assert_eq!(rows[0].label, "prog9");
        let mut by_cpu: Vec<&WslProc> = rows.iter().collect();
        by_cpu.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
        assert_eq!(by_cpu[0].label, "prog0");
        assert_eq!(by_cpu[1].label, "prog1");
    }

    #[test]
    fn many_sessions_all_come_through() {
        let mut w = World::new();
        for i in 0..14 {
            w.add(100 + i, 1, "2.1.332", CLAUDE, 1000 + u64::from(i));
        }
        assert_eq!(w.build().1.len(), 14);
    }

    #[test]
    fn the_native_claude_binary_is_claude_code() {
        assert_eq!(
            one(
                "2.1.332",
                "/home/u/.local/share/claude/versions/2.1.332 --session-id 4f87 --resume"
            ),
            Some(("Claude Code".into(), true))
        );
        assert_eq!(
            one("2.1.332", "claude bg-spare --bg-spare /tmp/x.sock"),
            Some(("Claude Code".into(), true))
        );
    }
}
