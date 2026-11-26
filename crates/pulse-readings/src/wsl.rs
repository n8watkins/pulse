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

/// The command run inside the default distro (through `sh -c`).
pub(crate) const PS_COMMAND: &str = "ps -eo pcpu=,rss=,comm=,args= --sort=-rss | head -n 40";

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

/// One line of `ps -eo pcpu=,rss=,comm=,args=`.
#[derive(Debug, PartialEq)]
struct PsRow<'a> {
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
    let (cpu, rest) = split_word(rest)?;
    cpu.parse::<f32>().ok()?;
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

/// Groups the output of the `ps` command by label and returns the biggest
/// [`TOP`] groups, largest first. Lines that do not parse are ignored.
pub(crate) fn top_procs(ps_output: &str) -> Vec<WslProc> {
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
        });
        g.count += 1;
        g.mem_bytes += row.rss_kib * 1024;
    }
    let mut rows: Vec<WslProc> = groups.into_values().collect();
    rows.sort_by(|a, b| {
        b.mem_bytes
            .cmp(&a.mem_bytes)
            .then_with(|| a.label.cmp(&b.label))
    });
    rows.truncate(TOP);
    rows
}

#[cfg(windows)]
pub(crate) use runner::Query;

/// The Windows side: spawn `wsl.exe` without a window, off the reading thread.
#[cfg(windows)]
mod runner {
    use super::{PS_COMMAND, decode_utf16le, default_is_running, top_procs};
    use crate::WslProc;
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
    }

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
                        let result = query();
                        if let Ok(mut s) = shared.lock() {
                            s.procs = result;
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
    fn query() -> Vec<WslProc> {
        let deadline = Instant::now() + TIMEOUT;
        let Some(listing) = run(&["-l", "--running"], deadline) else {
            return Vec::new();
        };
        if !default_is_running(&decode_utf16le(&listing)) {
            return Vec::new();
        }
        let Some(out) = run(&["-e", "sh", "-c", PS_COMMAND], deadline) else {
            return Vec::new();
        };
        top_procs(&String::from_utf8_lossy(&out))
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
        let r = parse_line("  3.5 440764 claude         claude --resume 29e1").unwrap();
        assert_eq!(
            (r.rss_kib, r.comm.as_str(), r.args),
            (440_764, "claude", "claude --resume 29e1")
        );
        // Spaces in comm, cut at 15 characters.
        let r = parse_line("67.5 889456 next-server (v1 next-server (v16.4.6)").unwrap();
        assert_eq!(r.comm, "next-server (v1");
        assert_eq!(r.args, "next-server (v16.4.6)");
        let r = parse_line(" 0.0   64 npm ci        npm ci").unwrap();
        assert_eq!((r.comm.as_str(), r.args), ("npm ci", "npm ci"));
        // A renamed thread: comm differs from the command line.
        let r = parse_line("9.1 184404 MainThread     /usr/bin/node /srv/app.js --x").unwrap();
        assert_eq!(
            (r.comm.as_str(), r.args),
            ("MainThread", "/usr/bin/node /srv/app.js --x")
        );
        // A kernel-style line with no arguments of its own.
        let r = parse_line("0.0 10 kthreadd [kthreadd]").unwrap();
        assert_eq!(r.comm, "kthreadd");
        // Garbage and short lines.
        assert!(parse_line("").is_none());
        assert!(parse_line("abc 12 x y").is_none());
        assert!(parse_line("1.0").is_none());
        assert!(parse_line("1.0 12").is_none());
        // Comm only (empty args).
        let r = parse_line("0.0 12 foo").unwrap();
        assert_eq!((r.comm.as_str(), r.args), ("foo", ""));
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
 3.5 440764 claude         claude --resume a
 4.9 429320 claude         claude --resume b
 0.1 100000 sh             sh -c ps
 0.1 1000 ps             ps -eo
 2.0 600000 MainThread     node /home/u/.vscode-server/bin/x/out/server-main.js
 2.0 100 SessionLeader   /init
 junk line
 1.0 2048 Relay(5)       /init
 9.9 1 a b c
";
        let rows = top_procs(out);
        assert_eq!(rows[1].label, "Editor server");
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
            .map(|i| format!("0.0 {} prog{i} /bin/prog{i}\n", 100 + i))
            .collect();
        let rows = top_procs(&out);
        assert_eq!(rows.len(), TOP);
        assert_eq!(rows[0].label, "prog9");
    }
}
