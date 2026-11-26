//! The Linux side of WSL: which programs use the memory of the WSL 2 VM.
//!
//! The parsing and labelling are pure, so their tests run on Linux. The query
//! itself (Windows only) never starts WSL: it runs only when the VM process
//! exists and `wsl.exe -l --running` lists the default distro.

use crate::WslProc;
use std::collections::HashMap;

/// How many labelled groups are returned.
pub(crate) const TOP: usize = 6;

/// The longest "<runtime>: <script>" script name, in characters.
const SCRIPT_MAX: usize = 28;

/// Characters the kernel keeps of a command name.
const COMM_MAX: usize = 15;

/// The line that separates the `ps` rows from the `/proc/<pid>/stat` dump.
const STAT_MARK: &str = "@@PULSE_STAT";

/// The command run inside the default distro (through `sh -c`): the biggest
/// processes by memory, then the processor times of all processes, in one
/// call. `ps` can only report a lifetime average, so the live processor use
/// is the difference between two dumps of the times.
pub(crate) const PS_COMMAND: &str = "ps -eo pid=,rss=,comm=,args= --sort=-rss | head -n 60; echo @@PULSE_STAT; cat /proc/[0-9]*/stat 2>/dev/null";

/// Clock ticks per second of the times in `/proc/<pid>/stat` (`USER_HZ`, 100
/// on every Linux kernel WSL ships).
const TICKS_PER_SEC: f64 = 100.0;

/// Splits the output of [`PS_COMMAND`] into the `ps` rows and the stat dump.
pub(crate) fn split_output(out: &str) -> (&str, &str) {
    match out.find(STAT_MARK) {
        Some(i) => (&out[..i], &out[i + STAT_MARK.len()..]),
        None => (out, ""),
    }
}

/// Processor ticks (user plus system time) per process id, from a dump of
/// `/proc/<pid>/stat` files, one line each. The command name sits in brackets
/// and may hold spaces and brackets, so the fields are counted from the last
/// closing bracket. Lines that do not parse are ignored.
pub(crate) fn parse_stat(dump: &str) -> HashMap<u32, u64> {
    let mut out = HashMap::new();
    for line in dump.lines() {
        let Some((pid, rest)) = line.split_once(' ') else {
            continue;
        };
        let (Ok(pid), Some(close)) = (pid.trim().parse::<u32>(), rest.rfind(')')) else {
            continue;
        };
        // After the bracket: state, ppid, ..., utime is field 14 and stime 15
        // of the file (index 11 and 12 here).
        let fields: Vec<&str> = rest[close + 1..].split_whitespace().collect();
        let tick = |i: usize| fields.get(i).and_then(|f| f.parse::<u64>().ok());
        if let (Some(u), Some(s)) = (tick(11), tick(12)) {
            out.insert(pid, u + s);
        }
    }
    out
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

/// Directory names that mean an editor's remote server.
const EDITOR_DIRS: &[&str] = &[
    ".vscode-server",
    "vscode-server",
    ".vscode-server-insiders",
    ".cursor-server",
    "cursor-server",
    ".windsurf-server",
    "windsurf-server",
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

fn in_editor_dir(path: &str) -> bool {
    path.split('/').any(|p| EDITOR_DIRS.contains(&p))
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
            if in_editor_dir(script) || in_editor_dir(first) {
                return plain("Editor server");
            }
            let name = truncate_chars(file_name(script), SCRIPT_MAX);
            return Some((format!("{exe}: {name}"), false));
        }
        if in_editor_dir(first) {
            return plain("Editor server");
        }
        return Some((exe.to_string(), false));
    }
    if in_editor_dir(first) {
        return plain("Editor server");
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

/// Groups the `ps` rows by label. The result holds the biggest [`TOP`] groups
/// by memory and the busiest [`TOP`] by processor (so either sort has its
/// full list), largest in memory first. `cpu` is each process's share of the
/// machine (see [`cpu_shares`]); a process without an entry counts as idle.
/// Lines that do not parse are ignored.
pub(crate) fn top_procs(ps_output: &str, cpu: &HashMap<u32, f32>) -> Vec<WslProc> {
    let mut groups: HashMap<String, WslProc> = HashMap::new();
    for line in ps_output.lines() {
        let Some(row) = parse_line(line) else {
            continue;
        };
        let Some((name, agent)) = label(&row.comm, row.args) else {
            continue;
        };
        let g = groups.entry(name.clone()).or_insert_with(|| WslProc {
            label: name,
            agent,
            count: 0,
            mem_bytes: 0,
            cpu_pct: 0.0,
        });
        g.count += 1;
        g.mem_bytes += row.rss_kib * 1024;
        g.cpu_pct += cpu.get(&row.pid).copied().unwrap_or(0.0);
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
    by_mem
}

#[cfg(windows)]
pub(crate) use runner::Query;

/// The Windows side: spawn `wsl.exe` without a window, off the reading thread.
#[cfg(windows)]
mod runner {
    use super::{
        PS_COMMAND, cpu_shares, decode_utf16le, default_is_running, parse_stat, split_output,
        top_procs,
    };
    use crate::WslProc;
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

    #[derive(Default)]
    struct Shared {
        procs: Vec<WslProc>,
        running: bool,
        /// When the last stat dump was read, and what it held.
        prev: Option<Dump>,
    }

    type Dump = (Instant, HashMap<u32, u64>);

    /// Runs the query on its own thread and keeps the last finished result.
    #[derive(Default)]
    pub(crate) struct Query {
        shared: Arc<Mutex<Shared>>,
        last_start: Option<Instant>,
    }

    impl Query {
        /// Call on every read with apps. `vm_present`: a `vmmemWSL` or `vmmem`
        /// process exists. Never blocks.
        pub(crate) fn poll(&mut self, vm_present: bool) -> Vec<WslProc> {
            if !vm_present {
                // Nothing runs: forget the old result and start nothing.
                if let Ok(mut s) = self.shared.lock() {
                    s.procs.clear();
                    s.prev = None;
                }
                return Vec::new();
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
                            s.procs = result;
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
                .map(|s| s.procs.clone())
                .unwrap_or_default()
        }
    }

    /// The labelled programs, or empty when the default distro is not running
    /// or anything failed or timed out.
    fn query(prev: Option<Dump>) -> (Vec<WslProc>, Option<Dump>) {
        let deadline = Instant::now() + TIMEOUT;
        let Some(listing) = run(&["-l", "--running"], deadline) else {
            return (Vec::new(), None);
        };
        if !default_is_running(&decode_utf16le(&listing)) {
            return (Vec::new(), None);
        }
        let Some(out) = run(&["-e", "sh", "-c", PS_COMMAND], deadline) else {
            return (Vec::new(), None);
        };
        let at = Instant::now();
        let text = String::from_utf8_lossy(&out);
        let (ps, stat) = split_output(&text);
        let ticks = parse_stat(stat);
        // The first query has nothing to compare with: every program is idle.
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        let shares = prev.map_or_else(HashMap::new, |(was, before)| {
            cpu_shares(&before, &ticks, at.duration_since(was).as_secs_f64(), cores)
        });
        (top_procs(ps, &shares), Some((at, ticks)))
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

    /// A `/proc/<pid>/stat` line: fields 14 and 15 are user and system time.
    fn stat_line(pid: u32, comm: &str, utime: u64, stime: u64) -> String {
        format!(
            "{pid} ({comm}) S 1 1 1 0 -1 4194560 100 0 0 0 {utime} {stime} 0 0 20 0 1 0 5 1000 50 18446744073709551615\n"
        )
    }

    #[test]
    fn parses_stat_dumps() {
        let dump = format!(
            "{}{}{}garbage\n12 (x) S\n",
            stat_line(10, "claude", 300, 120),
            // The name holds spaces and brackets.
            stat_line(11, "a) b (c", 7, 3),
            stat_line(12, "next-server (v1", 0, 0),
        );
        let t = parse_stat(&dump);
        assert_eq!(t.get(&10), Some(&420));
        assert_eq!(t.get(&11), Some(&10));
        assert_eq!(t.get(&12), Some(&0));
        assert_eq!(t.len(), 3);
        assert!(parse_stat("").is_empty());
    }

    #[test]
    fn splits_ps_from_the_stat_dump() {
        let (ps, stat) = split_output("1 2 a b\n@@PULSE_STAT\n10 (a) S\n");
        assert_eq!(ps, "1 2 a b\n");
        assert_eq!(stat, "\n10 (a) S\n");
        assert_eq!(split_output("1 2 a b\n"), ("1 2 a b\n", ""));
    }

    #[test]
    fn shares_are_a_part_of_the_whole_machine() {
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
            plain("Editor server")
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

    #[test]
    fn groups_and_sorts() {
        let out = "\
 11 440764 claude         claude --resume a
 12 429320 claude         claude --resume b
 13 100000 sh             sh -c ps
 14 1000 ps             ps -eo
 15 600000 MainThread     node /home/u/.vscode-server/bin/x/out/server-main.js
 16 100 SessionLeader   /init
 junk line
 17 2048 Relay(5)       /init
 18 1 a b c
";
        let cpu: HashMap<u32, f32> = [(11, 0.5), (12, 0.25), (15, 1.5), (13, 40.0)].into();
        let rows = top_procs(out, &cpu);
        assert_eq!(rows[1].label, "Editor server");
        assert!((rows[1].cpu_pct - 1.5).abs() < 1e-6);
        // Both processes of a group add up; plumbing (pid 13) adds nothing.
        assert!((rows[0].cpu_pct - 0.75).abs() < 1e-6);
        assert_eq!(rows[2].cpu_pct, 0.0);
        assert_eq!(rows[1].mem_bytes, 600_000 * 1024);
        assert_eq!(rows[0].label, "Claude Code");
        assert!(rows[0].agent);
        assert_eq!(
            (rows[0].count, rows[0].mem_bytes),
            (2, (440_764 + 429_320) * 1024)
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].label, "a");
    }

    #[test]
    fn keeps_six() {
        let out: String = (0..10)
            .map(|i| format!("{} {} prog{i} /bin/prog{i}\n", 500 + i, 100 + i))
            .collect();
        let rows = top_procs(&out, &HashMap::new());
        assert_eq!(rows.len(), TOP);
        assert_eq!(rows[0].label, "prog9");
    }

    #[test]
    fn a_busy_small_program_is_kept_for_the_processor_sort() {
        let out: String = (0..10)
            .map(|i| format!("{} {} prog{i} /bin/prog{i}\n", 500 + i, 100 + i))
            .collect();
        // prog0 is the smallest in memory and the busiest; prog1 is second.
        let cpu: HashMap<u32, f32> = [(500, 9.0), (501, 4.0)].into();
        let rows = top_procs(&out, &cpu);
        assert_eq!(rows.len(), TOP + 2);
        // Still largest in memory first.
        assert_eq!(rows[0].label, "prog9");
        let mut by_cpu: Vec<&WslProc> = rows.iter().collect();
        by_cpu.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
        assert_eq!(by_cpu[0].label, "prog0");
        assert_eq!(by_cpu[1].label, "prog1");
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
