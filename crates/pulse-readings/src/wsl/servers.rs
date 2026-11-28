//! Dev servers in Linux: the programs a developer or a coding agent started
//! that listen on a TCP port.
//!
//! A dev server here is a process that
//! - listens on a TCP port (`ss -ltnpH` run by the same single query as
//!   everything else) and belongs to the user running the query (`ss` shows the
//!   process of a socket only for its own user: the system's services are
//!   anonymous rows and are never counted), and
//! - is a development program: Node, Bun, Deno, Python, Ruby or PHP running
//!   code from a project folder, a well-known dev tool by name (`next-server`,
//!   `vite`, `webpack`, `wrangler`, `workerd` and so on), or a compiled binary
//!   (Go, Rust) run from a project folder, and
//! - is not a coding agent, an editor's server, Docker, Ollama, nor a globally
//!   installed tool (an agent's helper bridge lives under `lib/node_modules`,
//!   an `npx` download under `.npm/_npx`), and does not run from `/`, the home
//!   folder itself or a system folder (a daemon).
//!
//! Databases (`postgres`, `redis-server`, `mysqld`, `mongod`) are not counted:
//! they are services that stay up on purpose, not what agents leave behind.
//! Java is not counted either (its language servers and build daemons listen).
//!
//! Processes that belong to one server count once: the same port, or a parent
//! and its child that both listen (a wrapper and its worker).

use super::{
    PORTS_MISSING, PsRow, Stat, editor_label, file_name, is_python, label, parse_line, stem,
};
use std::collections::{BTreeSet, HashMap};

/// What the listening sockets say.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Servers {
    /// The dev servers (one per group of processes, see the module notes).
    pub count: u32,
    /// The ports of the counted servers by the label of the process that
    /// holds them (the label of the Linux rows), each list ascending.
    pub ports: HashMap<String, Vec<u16>>,
}

/// One listening socket: its port and the processes holding it. A socket the
/// user cannot see the process of has no processes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Listener {
    pub port: u16,
    pub pids: Vec<u32>,
}

/// The listening sockets of the `ss -ltnpH` lines, or `None` when `ss` was
/// missing or failed.
///
/// A line reads `LISTEN <recv-q> <send-q> <local address:port> <peer> [users:((
/// "name",pid=1,fd=2),...)]`. The address may be `*`, `[::]` or carry an
/// interface (`127.0.0.53%lo`); the port is what follows the last colon.
pub(crate) fn parse_listeners(text: &str) -> Option<Vec<Listener>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line == PORTS_MISSING {
            return None;
        }
        let mut fields = line.split_whitespace();
        if fields.next() != Some("LISTEN") {
            continue;
        }
        let Some(port) = fields
            .nth(2)
            .and_then(|local| local.rsplit_once(':'))
            .and_then(|(_, port)| port.parse::<u16>().ok())
        else {
            continue;
        };
        let mut pids = Vec::new();
        for (at, _) in line.match_indices("pid=") {
            let digits: String = line[at + 4..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(pid) = digits.parse::<u32>()
                && !pids.contains(&pid)
            {
                pids.push(pid);
            }
        }
        out.push(Listener { port, pids });
    }
    Some(out)
}

/// The working folder of each process from `pwdx` lines (`588: /home/u/app`).
/// A process it could not read (`588: Permission denied`) is missing.
pub(crate) fn parse_pwdx(text: &str) -> HashMap<u32, String> {
    text.lines()
        .filter_map(|line| {
            let (pid, path) = line.split_once(": ")?;
            let pid = pid.trim().parse::<u32>().ok()?;
            let path = path.trim_end();
            let path = path.strip_suffix(" (deleted)").unwrap_or(path);
            path.starts_with('/').then(|| (pid, path.to_string()))
        })
        .collect()
}

/// Names of dev tools, by the stem of their program or script name. A listener
/// with one of these names is a dev server wherever it runs from, unless it is
/// a globally installed tool.
const DEV_TOOLS: &[&str] = &[
    "next",
    "next-server",
    "vite",
    "webpack",
    "webpack-dev-server",
    "nuxt",
    "nuxi",
    "astro",
    "remix",
    "parcel",
    "esbuild",
    "wrangler",
    "workerd",
    "miniflare",
    "storybook",
    "start-storybook",
    "ng",
    "gatsby",
    "expo",
    "metro",
    "svelte-kit",
    "rspack",
    "rsbuild",
    "turbopack",
    "live-server",
    "http-server",
    "serve",
    "json-server",
    "nodemon",
    "ts-node-dev",
    "tsx",
    "uvicorn",
    "gunicorn",
    "flask",
    "rails",
    "puma",
    "hugo",
    "jekyll",
    "mkdocs",
];

/// Interpreters whose scripts count as dev servers when they run from a
/// project folder (Python is told apart by [`is_python`]).
const DEV_RUNTIMES: &[&str] = &["node", "nodejs", "bun", "deno", "ruby", "php"];

/// Folders a daemon runs from, never a project.
const SYSTEM_FOLDERS: &[&str] = &[
    "/usr", "/var", "/etc", "/snap", "/run", "/proc", "/sys", "/dev", "/boot", "/lib", "/bin",
    "/sbin",
];

/// Where installed tools live, as a part of a script's path: a script there is
/// a tool the user installed or an `npx` download, not the project's code.
const TOOL_HOMES: &[&str] = &[
    "/lib/node_modules/",
    "/.nvm/versions/",
    "/.npm/_npx/",
    "/.bun/install/",
    "/.cache/",
    "/.local/share/pnpm/",
    "/.local/pipx/",
    "/.cargo/bin/",
    "/.rustup/",
    "/usr/lib/",
    "/usr/share/",
    "/usr/libexec/",
    "/usr/bin/",
    "/snap/",
];

/// Whether a working folder can be a project: not `/`, not the home folder
/// itself, not a system folder.
fn project_folder(cwd: &str, home: Option<&str>) -> bool {
    let cwd = cwd.trim_end_matches('/');
    if cwd.is_empty() || home.is_some_and(|h| h.trim_end_matches('/') == cwd) {
        return false;
    }
    !SYSTEM_FOLDERS
        .iter()
        .any(|d| cwd == *d || cwd.starts_with(&format!("{d}/")))
}

/// Whether one listening process is a dev server. `None` when it cannot be
/// told because its working folder is unknown and the answer depends on it.
fn is_dev_server(row: &PsRow<'_>, cwd: Option<&str>, home: Option<&str>) -> Option<bool> {
    let Some((label, agent)) = label(&row.comm, row.args) else {
        return Some(false);
    };
    if agent || matches!(label.as_str(), "Docker" | "Ollama") || editor_label(row.args).is_some() {
        return Some(false);
    }
    let in_project = cwd.map(|c| project_folder(c, home));
    if in_project == Some(false) {
        return Some(false);
    }
    let mut words = row.args.split_whitespace();
    let first = words.next().unwrap_or("");
    let exe = stem(file_name(first));
    let script = words.find(|w| !w.starts_with('-'));
    let named = |name: &str| DEV_TOOLS.contains(&stem(file_name(name)).as_str());
    let installed = |path: &str| TOOL_HOMES.iter().any(|h| path.contains(h));

    if DEV_RUNTIMES.contains(&exe.as_str()) || is_python(&exe) {
        let Some(script) = script else {
            return Some(false);
        };
        if installed(script) {
            return Some(false);
        }
        return if named(script) {
            Some(true)
        } else {
            in_project
        };
    }
    if named(first) {
        return Some(!installed(first));
    }
    // A compiled program run from a project: a Rust build folder, `go run`'s
    // temporary folder, or a path under the working folder.
    let local = first.starts_with("./")
        || first.contains("/target/debug/")
        || first.contains("/target/release/")
        || first.contains("/go-build")
        || cwd.is_some_and(|c| first.starts_with(&format!("{}/", c.trim_end_matches('/'))));
    if local && !installed(first) {
        return in_project;
    }
    Some(false)
}

/// Finds the dev servers from the three port parts of one query (see
/// [`super::PS_COMMAND`]): the `ss` lines, the process rows of the listening
/// processes and their working folders (`cwds` also holds the folders of the
/// first 400 processes, which fills any gap). `None` when the data cannot be
/// read: no `ss`, listening processes whose rows came back empty, or a process
/// whose working folder is unknown and decides the answer. A wrong count is
/// worse than none.
pub(crate) fn find_servers(
    ports: &str,
    lps: &str,
    lcwd: &str,
    cwds: &HashMap<u32, String>,
    stats: &HashMap<u32, Stat>,
    home: Option<&str>,
) -> Option<Servers> {
    let listeners = parse_listeners(ports)?;
    let rows: HashMap<u32, PsRow<'_>> = lps
        .lines()
        .filter_map(parse_line)
        .map(|r| (r.pid, r))
        .collect();
    let mut folders = cwds.clone();
    folders.extend(parse_pwdx(lcwd));

    let seen = listeners.iter().flat_map(|l| &l.pids).count();
    if seen > 0 && rows.is_empty() {
        return None;
    }

    // The counted listening processes, each with its ports and label. A
    // process that ended since `ss` ran has no row: it is gone, not counted.
    let mut found: Vec<(u32, BTreeSet<u16>, String)> = Vec::new();
    for l in &listeners {
        for pid in &l.pids {
            let Some(row) = rows.get(pid) else { continue };
            if !is_dev_server(row, folders.get(pid).map(String::as_str), home)? {
                continue;
            }
            match found.iter_mut().find(|f| f.0 == *pid) {
                Some(f) => {
                    f.1.insert(l.port);
                }
                None => found.push((
                    *pid,
                    BTreeSet::from([l.port]),
                    label(&row.comm, row.args).map(|x| x.0).unwrap_or_default(),
                )),
            }
        }
    }

    // One server per set of processes that share a port or are parent and
    // child (a union-find over `found`).
    let mut root: Vec<usize> = (0..found.len()).collect();
    fn top(root: &mut [usize], mut i: usize) -> usize {
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    }
    for i in 0..found.len() {
        for j in 0..i {
            let shared = found[i].1.intersection(&found[j].1).next().is_some();
            let related = stats.get(&found[i].0).is_some_and(|s| s.ppid == found[j].0)
                || stats.get(&found[j].0).is_some_and(|s| s.ppid == found[i].0);
            if shared || related {
                let (a, b) = (top(&mut root, i), top(&mut root, j));
                root[a] = b;
            }
        }
    }
    let count = (0..found.len()).filter(|&i| top(&mut root, i) == i).count();

    let mut by_label: HashMap<String, BTreeSet<u16>> = HashMap::new();
    for (_, ports, label) in found {
        by_label.entry(label).or_default().extend(ports);
    }
    Some(Servers {
        count: u32::try_from(count).unwrap_or(u32::MAX),
        ports: by_label
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().collect()))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: Option<&str> = Some("/home/u");

    fn dev(comm: &str, args: &str, cwd: Option<&str>) -> Option<bool> {
        let line = format!("100 1000 {comm} {args}");
        let row = parse_line(&line).expect("row");
        is_dev_server(&row, cwd, HOME)
    }

    /// The table of the README: program, folder, counted.
    #[test]
    fn the_definition_on_a_calibration_table() {
        let app = Some("/home/u/projects/shop-site");
        let yes = |c: &str, a: &str, d: Option<&str>| assert_eq!(dev(c, a, d), Some(true), "{a}");
        let no = |c: &str, a: &str, d: Option<&str>| assert_eq!(dev(c, a, d), Some(false), "{a}");
        // Counted.
        yes("next-server (v1", "next-server (v16.4.6)", app);
        yes(
            "node",
            "node /home/u/projects/shop-site/node_modules/.bin/vite --port 5173",
            app,
        );
        yes("node", "/usr/bin/node src/serve-dashboards.js", app);
        yes(
            "node",
            "node /srv/x/node_modules/next/dist/bin/next dev",
            app,
        );
        yes(
            "MainThread",
            "node /home/u/projects/shop-site/node_modules/wrangler/bin/wrangler.js dev",
            app,
        );
        yes("workerd", "workerd serve --binary config.capnp", app);
        yes("python3", "python3 -m http.server 8000", app);
        yes("python3", "/usr/bin/python3 app.py", app);
        yes(
            "uvicorn",
            "/home/u/projects/api/.venv/bin/python3.12 -m uvicorn main:app",
            app,
        );
        yes("bun", "bun --hot server.ts", app);
        yes("deno", "deno run -A server.ts", app);
        yes("ruby", "ruby bin/rails server", app);
        yes("php", "php -S localhost:8000 -t public", app);
        yes(
            "api-server",
            "/home/u/projects/api/target/debug/api-server --port 9000",
            app,
        );
        yes("main", "./main", app);
        yes("exe", "/tmp/go-build123/b001/exe/server", app);
        yes("tool", "/home/u/projects/shop-site/bin/tool serve", app);
        // Not counted: agents, editors, Docker, Ollama.
        no("claude", "claude --resume x", app);
        no(
            "MainThread",
            "/home/u/.vscode-server/bin/abc/node /home/u/.vscode-server/bin/abc/out/server-main.js",
            app,
        );
        no("dockerd", "/usr/bin/dockerd -H fd://", Some("/"));
        no("ollama", "/usr/local/bin/ollama serve", app);
        // Not counted: an agent's own helper installed globally, an npx
        // download, a daemon from `/`, the home folder or a system folder.
        no(
            "MainThread",
            "node /home/u/.nvm/versions/node/v24.1.0/lib/node_modules/chrome-bridge/dist/bridge.js",
            Some("/tmp/x"),
        );
        no(
            "MainThread",
            "node /home/u/.npm/_npx/ab12/node_modules/.bin/mcp-server --port 1",
            app,
        );
        no("sccache", "/home/u/.cargo/bin/sccache", Some("/"));
        no("sshd", "sshd: /usr/sbin/sshd -D", Some("/"));
        no(
            "systemd-resolve",
            "/usr/lib/systemd/systemd-resolved",
            Some("/"),
        );
        no(
            "python3",
            "/usr/bin/python3 /usr/bin/unattended-upgrade-shutdown",
            app,
        );
        no("python3", "python3 worker.py", Some("/home/u"));
        no("node", "node server.js", Some("/var/lib/thing"));
        no("postgres", "/usr/lib/postgresql/16/bin/postgres -D /x", app);
        no("redis-server", "redis-server *:6379", app);
        no("java", "java -jar app.jar", app);
        // A runtime with no script is a shell for typing, not a server.
        no("node", "node", app);
        // The folder decides for a plain script and is not known: no answer.
        assert_eq!(dev("node", "node server.js", None), None);
        // A known tool needs no folder.
        assert_eq!(dev("vite", "vite", None), Some(true));
    }

    #[test]
    fn reads_ss_lines() {
        let text = "\
LISTEN 0      511                                 127.0.0.1:9224  0.0.0.0:* users:((\"MainThread\",pid=66035,fd=22))
LISTEN 0      4096                 100.92.219.16%tailscale0:443   0.0.0.0:*
LISTEN 0      4096                            127.0.0.53%lo:53    0.0.0.0:*
LISTEN 0      511                                         *:7665        *:* users:((\"node\",pid=588,fd=20),(\"node\",pid=590,fd=20))
LISTEN 0      4096   [fd7a::af37:db11]%tailscale0:38433    [::]:*
LISTEN 0      511                                      [::]:3000      [::]:* users:((\"next-server (v1\",pid=675,fd=22))
ESTAB 0 0 1.2.3.4:5 6.7.8.9:10
junk
";
        let l = parse_listeners(text).unwrap();
        assert_eq!(l.len(), 6);
        assert_eq!(
            l[0],
            Listener {
                port: 9224,
                pids: vec![66035]
            }
        );
        assert!(l[1].pids.is_empty() && l[1].port == 443);
        assert_eq!(l[3].pids, vec![588, 590]);
        assert_eq!(l[4].port, 38433);
        assert_eq!(l[5].port, 3000);
        assert_eq!(l[5].pids, vec![675]);
        // `ss` missing: no answer, not an empty list.
        assert_eq!(parse_listeners("@@PULSE_NOSS\n"), None);
        assert_eq!(parse_listeners("").unwrap().len(), 0);
    }

    #[test]
    fn reads_working_folders_of_listeners() {
        let m = parse_pwdx(
            "588: /home/u/my app\n66035: /tmp/prev\n7: Permission denied\n9: /x (deleted)\njunk\n",
        );
        assert_eq!(m[&588], "/home/u/my app");
        assert_eq!(m[&9], "/x");
        assert_eq!(m.len(), 3);
    }

    fn stat(ppid: u32) -> Stat {
        Stat {
            ticks: 0,
            ppid,
            start: 0,
        }
    }

    const ROWS: &str = "\
 675 300000 next-server (v1 next-server (v16.4.6)
 600 90000 node node /home/u/projects/shop-site/node_modules/.bin/next dev
 700 50000 node node /home/u/projects/api/src/index.js
 710 50000 node node /home/u/projects/api/src/index.js
 720 20000 sshd sshd: /usr/sbin/sshd -D
 730 20000 sccache /home/u/.cargo/bin/sccache
 740 50000 python3 python3 -m http.server 8000
";
    const DIRS: &str = "\
675: /home/u/projects/shop-site
600: /home/u/projects/shop-site
700: /home/u/projects/api
710: /home/u/projects/api
720: /
730: /
740: /home/u/projects/docs
";

    fn run(ports: &str, stats: &HashMap<u32, Stat>) -> Option<Servers> {
        find_servers(ports, ROWS, DIRS, &HashMap::new(), stats, HOME)
    }

    #[test]
    fn counts_each_server_once() {
        // 600 (wrapper) and 675 (worker) listen on 3000 and 3001; 700 and 710
        // (cluster workers) share 4000; 740 listens on 8000 alone; sshd and
        // sccache are not servers.
        let ports = "\
LISTEN 0 1 *:3000 *:* users:((\"next-server (v1\",pid=675,fd=22),(\"node\",pid=600,fd=22))
LISTEN 0 1 *:3001 *:* users:((\"node\",pid=600,fd=23))
LISTEN 0 1 *:4000 *:* users:((\"node\",pid=700,fd=9),(\"node\",pid=710,fd=9))
LISTEN 0 1 0.0.0.0:22 0.0.0.0:* users:((\"sshd\",pid=720,fd=3))
LISTEN 0 1 127.0.0.1:4226 0.0.0.0:* users:((\"sccache\",pid=730,fd=12))
LISTEN 0 1 [::]:8000 [::]:* users:((\"python3\",pid=740,fd=3))
LISTEN 0 1 127.0.0.53%lo:53 0.0.0.0:*
";
        let s = run(ports, &HashMap::new()).unwrap();
        assert_eq!(s.count, 3);
        assert_eq!(s.ports["next-server (v16.4.6)"], vec![3000]);
        assert_eq!(s.ports["node: next"], vec![3000, 3001]);
        assert_eq!(s.ports["node: index.js"], vec![4000]);
        assert_eq!(s.ports["python3: http.server"], vec![8000]);
        assert!(!s.ports.contains_key("sshd"));
    }

    #[test]
    fn a_parent_and_child_that_both_listen_count_once() {
        let ports = "\
LISTEN 0 1 *:3000 *:* users:((\"node\",pid=600,fd=22))
LISTEN 0 1 *:3010 *:* users:((\"next-server (v1\",pid=675,fd=22))
";
        assert_eq!(run(ports, &HashMap::new()).unwrap().count, 2);
        let stats: HashMap<u32, Stat> = [(675, stat(600))].into();
        assert_eq!(run(ports, &stats).unwrap().count, 1);
    }

    #[test]
    fn the_same_port_on_v4_and_v6_is_one_server() {
        let ports = "\
LISTEN 0 1 127.0.0.1:8000 0.0.0.0:* users:((\"python3\",pid=740,fd=3))
LISTEN 0 1 [::1]:8000 [::]:* users:((\"python3\",pid=740,fd=4))
";
        assert_eq!(run(ports, &HashMap::new()).unwrap().count, 1);
    }

    #[test]
    fn no_servers_and_unreadable_data() {
        let none = run("", &HashMap::new()).unwrap();
        assert_eq!(none, Servers::default());
        // Sockets of other users have no process: nothing to count.
        let other = run("LISTEN 0 1 *:5432 *:*\n", &HashMap::new()).unwrap();
        assert_eq!(other.count, 0);
        // No `ss`: no count at all.
        assert_eq!(run("@@PULSE_NOSS\n", &HashMap::new()), None);
        // Listening processes whose rows could not be read: no count.
        let ports = "LISTEN 0 1 *:3000 *:* users:((\"node\",pid=600,fd=22))\n";
        assert_eq!(
            find_servers(ports, "", "", &HashMap::new(), &HashMap::new(), HOME),
            None
        );
        // A process that ended after `ss` ran has no row: it is skipped.
        let ports = "\
LISTEN 0 1 *:3000 *:* users:((\"node\",pid=9999,fd=22))
LISTEN 0 1 *:3001 *:* users:((\"next-server (v1\",pid=675,fd=22))
";
        assert_eq!(run(ports, &HashMap::new()).unwrap().count, 1);
        // A script whose folder is not known: no count rather than a guess.
        let ports = "LISTEN 0 1 *:4000 *:* users:((\"node\",pid=700,fd=9))\n";
        assert_eq!(
            find_servers(ports, ROWS, "", &HashMap::new(), &HashMap::new(), HOME),
            None
        );
        // The folder from the general list fills the gap.
        let general: HashMap<u32, String> = [(700, "/home/u/projects/api".to_string())].into();
        assert_eq!(
            find_servers(ports, ROWS, "", &general, &HashMap::new(), HOME)
                .unwrap()
                .count,
            1
        );
    }
}
