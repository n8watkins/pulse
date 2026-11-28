# Pulse

A native Windows tray app that shows live system health in a small panel.
It lives only in the system tray: no taskbar button and no Alt+Tab entry.

## Install

Download `Pulse-<version>-setup.exe` from the [latest release](https://github.com/n8watkins/pulse/releases/latest) and run it.
It installs for the current user only and needs no administrator rights.
The installer is not code signed, so Windows SmartScreen may say "Windows protected your PC": choose "More info", then "Run anyway".
Pulse then sits in the system tray; click its icon or press Alt+Shift+P to open the panel.
To remove it, use "Installed apps" in Windows Settings.

## Status

The panel is drawn natively with Slint (software Skia renderer), with no webview.
The owner chose this on 2026-11-25 after comparing it with a Tauri webview shell: the native panel used about 5 MB while open against about 100 MB for the webview.

- `apps/native`: the tray app.
- `crates/pulse-readings`: the system reader (processor, memory, top apps with icons, graphics, drive, network, battery).
- `scripts/measure.ps1`: measures memory, open time and takes screenshots without touching input or focus.
- `docs/reference/panel-preview.html`: the panel design (the `.pulse` rules and markup).

The webview shell is parked, not deleted: branch `spike/webview`, tag `parked/webview`.

Two views: the expanded one (380 x 580, the "B Tighter" layout) and a compact one (380 wide, about 287 tall: Processor and Memory as mini tiles, three one-line apps, a collapsible Linux row with its program and agent counts, one footer row of figures).
The Settings sheet has a one-line-per-setting form in the compact view and never changes the panel's height.

Built: tray icon with live bars and a tooltip, open and close by click or a global shortcut, Processor and Memory tiles with sparklines, Top apps with real icons sorted by memory or processor, the WSL breakdown with coding agents marked, Graphics, Drive, Network and Battery tiles, Settings, a low memory alert (a notification, on at 90 percent by default), start with Windows, a light, dark or system appearance, and a per-user installer (`scripts/build-installer.ps1`).

Slint is used under its royalty-free desktop license, which requires an "About Slint" notice in the app or the Slint badge on a public download page.
Pulse shows the `AboutSlint` widget at the bottom of Settings.

## Low memory alert

Pulse shows a Windows notification when memory in use stays at or above a level you pick, for example "Memory is 92% full" with "Google Chrome is using the most: 6.1 GB" (the top app by memory, or "WSL (Linux)" when that is the top).
It works while the panel is hidden, and clicking the notification opens the panel.
Settings has one row, "Low memory alert", with a switch and a level of 80, 85, 90 or 95 percent.
The default is on at 90 percent; a `settings.json` from before the alert loads with that default (the saved fields are `low_mem_alert` and `low_mem_level`).
The header line ("Memory is nearly full") and the tray icon's warning color turn at the same level, with the same rounding, so they never disagree with the alert; with the alert switched off they turn at 90 percent.

Rules (the logic is `apps/native/src/alert.rs`, tested with an injected clock):

- Memory must stay at or above the level for 30 seconds before the first notification.
- Nothing is shown in the first minute after Pulse starts.
- After a notification the next one waits until memory has dropped at least 3 points below the level and risen to it again, and is never sooner than 10 minutes after the last one.
- Switching the alert on or changing the level starts the wait afresh.

The notification is the tray icon's own (`Shell_NotifyIconW`, shown by Windows 10 and 11 as a toast, silenced by Do Not Disturb), so it carries Pulse's name and icon and needs nothing from the installer.
The alert runs on the reader that already refreshes while the panel is hidden; the per-process scan for the top app happens once, when a notification is due.
The Windows call lives in `apps/native/src/notice.rs`, so another platform can supply its own.

## Drives

The reader lists every fixed local drive with a letter, for example C: and D:.
Network drives, removable drives (USB sticks, card readers), optical and RAM drives are left out, since asking a sleeping or disconnected one for its size can stall the reader: `GetDriveTypeW` decides, and no size is asked of a drive that is not fixed.
Volumes without a letter are left out, and so is a drive whose size cannot be read (for example a locked BitLocker drive); it is skipped, never shown as an empty drive.
The list and the free space are read again every 30 seconds (a volume label once, when its drive shows up), so a drive that is plugged in or removed shows up or goes away within 30 seconds, without a restart.
The speed of each drive comes from its own `\LogicalDisk(<letter>:)\Disk Read Bytes/sec` and `Disk Write Bytes/sec` counters, read on every refresh; a drive that goes away has its counters closed.

The Drive tile shows the fullest drive (the smallest share free) with its own free space and speed, titled with its letter, for example "Drive D: free".
With several drives it adds a small "1 of 3"; the compact footer adds it too when the row still has room.
On a PC with one drive the tile reads as before.
"Drive D: is nearly full" (the header line and the warning color of the tray icon) shows when any drive is under 8 percent free; with several, it names the fullest.
Processor and memory warnings still come first.

## Coding-agent sessions in the WSL list

Inside "WSL (Linux)", each coding-agent session (Claude Code, Codex CLI, Gemini CLI, Cursor Agent, Aider, opencode and the other agents the labeller marks) is one row: the product mark, the agent, "·" and the name of the folder it runs in, a dot (green when busy, grey when idle) and the value of the active sort (memory in Memory sort, percent in Processor sort).
Programs that are not agents stay grouped by name, as before.
Pulse reads this from outside, so it shows only what it can know: which sessions exist, the folder name (never the full path, `~` for the home folder), busy or idle, processor, memory and how long the session has run.
It cannot know what an agent is thinking or which step it is on, and shows nothing that implies it.

What a session is:

- The root process of an agent plus its helpers: agent processes whose direct parent is an agent of the same kind (Claude Code's spare process, a native binary under a script wrapper).
- Programs the agent started (a dev server, a test run, a headless browser) are no part of it.
  They keep their own rows and are not counted in the session's memory or processor.
  Another agent started by an agent is a session of its own.
- `Wsl::sessions` replaces the agent groups that used to be in `Wsl::procs`, so the number of agents is `Wsl::agent_count()` (the number of sessions); `procs` holds only non-agent programs and `WslProc::agent` is always false there.

Busy and idle:

- A session is busy when it and everything it started (all descendants, except other agents' sessions) used at least 10 percent of one processor core since the last query, and stays busy until that falls under 4 percent of a core.
A waiting session with its helper servers measured 0 to 6 percent of a core on the owner's PC, a working one far more.
- The gap between 10 and 4 percent is the hysteresis that keeps the dot from flickering.
  The first query after WSL starts has nothing to compare with, so all sessions read idle for one round.

Which rows are shown (the Linux list never grows past 6 rows, so many sessions do not push the rest out):

1. Sessions are ordered busy first, then by the value of the active sort, then by name, folder and process id.
2. Up to 4 sessions are one row each.
3. With more than 4, the first 3 are rows and all the others fold into one line: "Claude Code · 9 idle" with their memory or processor summed ("9 idle agents" when they are not all one agent, "more" in place of "idle" when one of them is busy; the dot is green then).
4. The programs fill the rows left of the 6.

## Rows with sub items

A row with sub items (today only "WSL (Linux)") has a chevron in both views: down when closed, up when open.
Click anywhere on the row, or press Space or Enter while it has the keyboard focus (a ring shows it), to open or close it; nothing else on a row is clickable, so there is no conflict.
A screen reader hears "Expand WSL (Linux)" or "Collapse WSL (Linux)" and the open state.
A machine with nothing running inside has no sub items, so no chevron.
The state is kept per view in `settings.json` under `groups_open`, keyed by a group id (`wsl`): `{"groups_open": {"expanded": {"wsl": true}, "compact": {"wsl": false}}}`.
Expanded defaults to open, compact to closed.
A file from before has a single `wsl_open`: the compact view takes that value, the expanded view starts open, and the next save drops the old field.
A new row with sub items needs no new setting, only a group id.

Closed, the expanded row shows the summary on its second line, where "Linux virtual machine · inside:" is when open; the apps below move up.
In both views the closed WSL row reads "N programs · N agents · N dev servers": the programs are the processes of the Linux programs that are not coding agents, the agents are the sessions (one each, however many helper processes it has), so the two never overlap, and the dev servers are described below.
Zero (or a count that could not be read) is left out of the line.
Open in the expanded view, the count moves to the detail line: "Linux VM · 3 dev servers · inside:" ("VM" because the bar leaves room for about 35 characters).
The compact row has room for about 30 characters, so there the parts come in the order "3 dev servers · 5 agents · 18 programs" (open or closed): what does not fit ends in an ellipsis at the end, and the name "WSL (Linux)" keeps its width.
Opened, the same session rows, folding and order apply, in the compact row style.

Two sessions of the same agent in the same folder show how long each has run ("2h 10m", "14m"), small, after the name.
Long folder names end in an ellipsis.
Without a readable folder, the row shows the agent alone.

How it is read: the same single `wsl.exe -e sh -c "..."` call as before (WSL is never started by Pulse), now also printing the parent and start of every process (`/proc/<pid>/stat`), the working folder of every process (`ls -l /proc/[0-9]*/cwd`, at most 400 lines), the home folder and the uptime.
The `ps` part is cut to the 120 biggest processes by memory with each line cut to 240 characters, so the output stays bounded and an agent, being big, is not cut off.

## Dev servers in the WSL list

Many agents leave dev servers running, so the WSL row counts them ("3 dev servers") and the Linux row of a program that is a dev server shows its port, small and muted, after the name: ":3000", or the lowest port and how many more (":8000 +1").
A dev server that is not among the shown rows (the list shows at most 6) is still counted.

### What counts

A dev server is a Linux process that

1. listens on a TCP port and belongs to the user running the query (the sockets of other users, and of the system's services, come without a process and are never counted), and
2. is a development program: Node, Bun, Deno, Python, Ruby or PHP running code from a project folder; a dev tool known by name wherever it runs (`next-server`, `next`, `vite`, `webpack`, `wrangler`, `workerd`, `esbuild`, `storybook`, `uvicorn`, `gunicorn`, `rails`, `puma`, `hugo` and a few more, the list is `DEV_TOOLS` in `crates/pulse-readings/src/wsl/servers.rs`); or a compiled program (Go, Rust and others) run from a project folder (a path under the working folder, `./name`, a Cargo `target/debug` or `target/release` folder, `go run`'s temporary build folder).
   A project folder is any working folder that is not `/`, not the home folder itself and not a system folder (`/usr`, `/var`, `/etc`, `/snap`, `/run`, `/lib` and so on), and
3. is not a coding agent, not an editor's server (VS Code server and the like), not Docker or Ollama, and not a tool installed on the machine: a script under a global `lib/node_modules`, `.nvm/versions`, an `npx` download (`.npm/_npx`), `.cache`, pnpm or pipx folders, `/usr/lib` or `/usr/share` is a tool, not the project's server (this is what keeps an agent's own helper bridge or an MCP server run through `npx` out of the count).

Not counted on purpose: databases (`postgres`, `redis-server`, `mysqld`, `mongod`), because they are services that stay up by design and not what agents leave behind; Java programs, because language servers and build daemons listen on ports; `sshd`, `systemd-resolved`, the WSL relay and every other system service (no project folder, and they belong to root).
A plain script run by Node or Python needs its working folder to be known (it is read for every listening process); if it cannot be read, the whole count is dropped instead of guessing.

Processes of one server count once: the same port (several workers, IPv4 and IPv6), or a parent and its child that both listen.
Ports are attached to the Linux row of the same label as the process that holds them.

The table below is the one used to check the definition (program, folder, result).
The first rows are what this PC's live list looked like when it was built; the folder names are made up.
The rest are the unit-test cases in `servers.rs`.

| Program | Port | Working folder | Counted | Why |
| --- | --- | --- | --- | --- |
| `node src/serve-dashboards.js` | 7665 | `~/projects/reports` | yes | Node, script in a project folder |
| `python3 -m http.server 18731` | 18731 | `/tmp/site-a` | yes | Python in a project folder |
| `node serve.js` | 18732 | `/tmp/site-b` | yes | Node, script in a project folder |
| `node .../lib/node_modules/chrome-bridge/dist/bridge.js` | 9224 | `/tmp/prev` | no | script of a globally installed tool (an agent's helper) |
| `sccache` | 4226 | `/` | no | not a dev program, no project folder |
| systemd-resolved, tailscaled | 53, 443 | n/a | no | other users' sockets: no process to count |
| `next-server (v16.4.6)` | 3000 | `~/projects/shop-site` | yes | dev tool by name |
| `node .../node_modules/.bin/vite` | 5173 | `~/projects/shop-site` | yes | dev tool by name |
| `wrangler dev` / `workerd` | 8787 | `~/projects/edge` | yes (once) | dev tool by name, wrapper and worker share a port |
| `uvicorn main:app` | 8000 | `~/projects/api` | yes | dev tool by name |
| `bun --hot server.ts`, `deno run -A server.ts`, `ruby bin/rails server`, `php -S` | any | project folder | yes | runtimes running project code |
| `api-server` (Cargo `target/debug`), `./main`, `go run` build | any | project folder | yes | compiled program run from a project |
| `claude`, other coding agents | any | any | no | coding agent |
| VS Code server | any | any | no | editor server |
| `dockerd`, `ollama` | any | any | no | Docker, Ollama |
| `node .../.npm/_npx/.../mcp-server` | any | project folder | no | `npx` download, a tool |
| `python3 /usr/bin/unattended-upgrade-shutdown` | any | project folder | no | system script |
| `python3 worker.py` | any | `~` | no | home folder is not a project |
| `node server.js` | any | `/var/lib/x` | no | system folder |
| `postgres`, `redis-server`, `java -jar` | any | any | no | databases and Java are not counted (see above) |

### How the ports are read

The same single `wsl.exe -e sh -c "..."` call (no extra spawn, WSL never started by Pulse) also runs `ss -ltnpH` once (at most 300 lines), then `ps -p` and `pwdx` for the processes that own the sockets (at most 100), because a small dev server is not among the 120 biggest processes that the main `ps` lists.
`ss` was chosen over reading `/proc/net/tcp` and the socket links under `/proc/<pid>/fd`: one program, IPv4 and IPv6 in one list, the process of every socket of the user in one answer.
The `/proc` route needs the fd listing of every process (about 75 KB here) and a second pass to match socket numbers, and its output is not bounded without a filter that cannot be written without quote characters (which `wsl.exe` mangles).
Without `ss` (or when it fails) the part says so and no count is shown; so is it when listening processes cannot be read, or when a script's working folder is unknown.
Another user's server (run with `sudo`, say) has no visible process and is not counted: the count is of the user's own servers.
The coding-agent session rows do not show "1 server": that would need each server tied to the session that started it, and it is left out.

Measured on this PC through `wsl.exe` (median of 30 runs each, the machine busy with other agents, so compare the pairs and not the absolute numbers): the call took about 100 to 170 ms before and about 160 to 210 ms after, and printed about the same number of bytes plus 1 to 2 KB.
Most of the added time is `ss` itself (about 30 ms inside Linux here, whatever flags it gets).
The query runs on its own thread every 6 seconds and never blocks the panel.

## Build and measure

Build with the Windows toolchain: `cargo.exe build --release -p pulse-native`, with `--target-dir` pointing at a Windows folder when the source is inside WSL.

Environment hooks, for tests and screenshots:

- `PULSE_SAMPLE=1` uses the sample readings instead of the real reader.
- `PULSE_SHOW=1` shows the panel at start without activating it and keeps it open on blur.
- `PULSE_THEME=light|dark` forces the theme for that run.
  It wins over the Appearance setting (System, Light, Dark, saved as `appearance` in `settings.json`; System follows the window's light or dark state, which winit reports on every platform).
- `PULSE_VIEW=compact|expanded` starts in that view whatever `settings.json` says (the saved field is `view`, expanded by default).
  The header button switches between the views and saves the choice.
- `PULSE_WSL_OPEN=0|1` starts both views with their Linux (WSL) list closed or open, whatever `settings.json` says (the saved state is `groups_open`, see "Rows with sub items"; without it expanded is open and compact is closed).
- `PULSE_SELFTEST=1` with `PULSE_REPORT=<path>` opens and closes the panel five times and writes `{"shell":"native","open_ms":[...]}`.
- `PULSE_NOTRIM=1` turns off the working-set trim on hide.
- `PULSE_RENDERER=skia` uses the GPU renderer (about 108 MB; the default software renderer is the reason memory stays low).
- `PULSE_STRESS=1` feeds worst-case tile values and very long names (app names, exe lines, Linux program labels with a count, 14 agent sessions (busy and idle, a long folder name, the home folder, none readable, two in one folder), 128 dev servers with 5-digit and several ports, the busy status line), to check that every text ends in an ellipsis or wraps and nothing is cut.
- `PULSE_SCALE=<factor>` (1.0 to 4.0) forces the display scale: Slint draws at that factor, the window is sized and placed (12 logical px from the work-area corner) at that factor, the app icons are asked for at the scaled pixel size (36 px at 1.5) and the tray icon size follows it.
  It simulates scaling on a 100% monitor for screenshots; it is not a real scaled monitor, and per-monitor DPI changes when dragging between monitors are untested.
- `PULSE_DRIVES=<count>` adds made-up drives after the real ones (or the sample's C:) until there are that many (taking the free letters from D: to Z:), to see several drives on a PC with one: the first one added, D: "Games", is nearly full, so the Drive tile shows it with "1 of 3" and the header warns "Drive D: is nearly full".
  With `PULSE_STRESS=1` the Drive tile instead gets that many drives with the widest figures ("Drive W: free", "999 GB", "999 MB/s").
- `PULSE_BATTERY=<percent>,<plugged 0|1>` replaces the battery reading (for example `12,0` or `80,1`), to see the Battery tile on a desktop PC.
- `PULSE_WORK_H=<logical px>` pretends the work area is that tall, to check the panel on a short display.
- `PULSE_OPEN=settings` starts with the Settings screen open (for screenshots).
- `PULSE_DEMO_SHORTCUT=recording|keys|taken|failed|invalid` (with `PULSE_OPEN=settings`) shows one state of the shortcut field without any key press: `recording` (waiting for keys), `keys` (Alt and Shift held, with the "hold two" warning), `taken` (another app has it), `failed`, `invalid`.
  It registers nothing.
  The off state needs no hook: a `settings.json` with `"hotkey": ""` and `"hotkey_last": "Ctrl+Shift+F20"` shows the switch off with the combination dimmed.
  Screenshots of a test copy should use F20 to F24 shortcuts, never the owner's real one.
- `PULSE_WARN=1` makes the reader report 92% processor and 9 days of uptime, to see the warning colors and "restart?".
- `PULSE_NOAPPS=1` makes the reader report no apps, to see the "Reading apps..." row.
- `PULSE_MEM_PCT=<0 to 100>` replaces the memory percent the low memory alert sees and shortens its waits (3 s of holding, nothing in the first 4 s, 20 s between notifications), so the alert can be triggered on purpose.
  The alert still has to be on in Settings (it is by default) and the level at or below the forced percent.
  It counts as a test run, so it never touches the Run key.
  A notification it sends is also logged as one line (`sent ...` or `refused ...`) in `alert.log` next to `settings.json`, since a toast can be hidden by Do Not Disturb or a full-screen app.
- `PULSE_SETTINGS_DIR=<folder>` keeps `settings.json` in that folder instead of `%APPDATA%\Pulse` (test runs use it).
- `PULSE_INSTANCE_NAME=<name>` gives a test run its own single-instance name, so it never pokes the real panel.
- `PULSE_AUTOSTART_NAME=<name>` names the Run-key value a test run would write.
- With any of `PULSE_SAMPLE`, `PULSE_SHOW`, `PULSE_SELFTEST`, `PULSE_MEM_PCT`, `PULSE_SETTINGS_DIR` or `PULSE_INSTANCE_NAME` set, the app does not bring the Run key in line with the setting at start.
  Neither does any exe not named `Pulse.exe` (development builds are `pulse-native.exe`).
  The check at start never takes the Run value over from another existing copy of Pulse; the Settings switch and the tray item write it for the running exe.
- `scripts/measure.ps1` gives every start its own `PULSE_SETTINGS_DIR` and `PULSE_INSTANCE_NAME`.
- `--quit` as an argument asks the running app to quit and exits (0 when it is gone, 1 after 5 s); the installer uses it.

Measure with `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\measure.ps1 -Exe <exe> -Out <folder>`.
It sends no keys, makes no clicks, does not touch the clipboard and never takes focus.

Lint and tests:

- CI (`.github/workflows/ci.yml`, GitHub-hosted runners only) runs fmt, clippy with `-D warnings`, the tests and a release build on Windows, the pure-logic tests on Linux and `scripts/check-dashes.sh` (no em dash in tracked text files).
- Tests that need this particular PC (WSL running with a "Claude Code" process, `explorer.exe` icons from the shell icon cache, the live comparison with the PDH counters) do nothing unless `PULSE_LIVE_TESTS=1`.
  Run them locally on Windows with `PULSE_LIVE_TESTS=1 cargo test -p pulse-readings -- --nocapture`.
  `compare_with_pdh_counter` samples the reader and the `Process(*)` counter over the same window (`PULSE_COMPARE_SECS`, default 10) and prints both.

From WSL:

- `cargo fmt --all --check`
- `cargo clippy -p pulse-native -p pulse-readings --all-targets --target x86_64-pc-windows-msvc -- -D warnings`
- `cargo test --workspace`
- The Windows-only reader tests run with `cargo.exe test -p pulse-readings`.

## Release

The version is the workspace version in the root `Cargo.toml`; the exe, its version details, the About line in Settings and the installer name all take it from there.

Every installer that is handed out has its own version number, so two builds are never told apart by date alone.
Raise the version in a pull request before building one: the middle number for a round with new features (0.2.0 to 0.3.0), the last number for a round of fixes only (0.2.0 to 0.2.1).
Run `cargo update --workspace` so `Cargo.lock` follows, then publish the installer as a GitHub release tagged `v<version>` on the commit it was built from.

1. Build the exe with the Windows toolchain: `cargo.exe build --release -p pulse-native`, with `--target-dir` pointing at a Windows folder when the source is inside WSL.
   `apps/native/build.rs` embeds the icon (`apps/native/assets/pulse.ico`), the version details (ProductName and FileDescription "Pulse", company "n8builds.dev") and a manifest (per-monitor-v2 DPI, Windows 10 and 11) through `rc.exe`.
   The resource step runs only when the build host is Windows, so `cargo clippy --target x86_64-pc-windows-msvc` from WSL needs no resource tools.
2. The icon is generated, not drawn by hand: `python3 scripts/make-icon.py` (needs Python 3 and Pillow) rewrites `apps/native/assets/pulse.ico` with 16, 20, 24, 32, 48, 64 and 256 px images in the style of `apps/native/ui/logo.svg`.
3. `THIRD-PARTY-NOTICES.md` is generated from `cargo metadata` for the Windows target: `python3 scripts/gen-notices.py` (hand-written part in `scripts/notices-head.md`).
   Regenerate it after dependency changes; the script prints any crate whose license is copyleft-only or unclear.
4. Build the installer on Windows: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-installer.ps1 -Exe <pulse-native.exe> -Out <folder>` writes `Pulse-<version>-setup.exe`.
   It uses the `makensis.exe` of the NSIS that Tauri downloaded (`%LOCALAPPDATA%\tauri\NSIS`), or one on PATH, or `-Makensis <path>`.
   `-TestName "Pulse Test"` builds an installer that uses that name for the folder, shortcut, uninstall entry, Run-key value and settings folder, so it never touches a real Pulse install.

The installer is per-user (no administrator rights): it installs to `%LOCALAPPDATA%\Programs\Pulse`, adds a Start menu shortcut and an Add/Remove Programs entry under HKCU, and closes a running Pulse from that folder first: it runs the installed `Pulse.exe --quit` (only when that exe is version 0.1.0 or later; waits up to 10 s, and ends that one launched process if it is still there), then, as a last resort only, ends any process still running from that exact folder (matched by full path, so a copy running from another folder is left alone).
Silent install: `/S` (starts Pulse at the end unless `/NORUN` is given).
Uninstall keeps the settings in `%APPDATA%\Pulse` unless you answer yes (a silent uninstall always keeps them).
"Start with Windows" is owned by the app; the uninstaller only removes its Run value when it points into the install folder.

The installer and the exe are not code signed, so Windows SmartScreen will warn ("Windows protected your PC") on first run; choose "More info", then "Run anyway".
