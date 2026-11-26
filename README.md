# Pulse

A native Windows tray app that shows live system health in a small panel.
It lives only in the system tray: no taskbar button and no Alt+Tab entry.

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

Built: tray icon with live bars and a tooltip, open and close by click or a global shortcut, Processor and Memory tiles with sparklines, Top apps with real icons sorted by memory or processor, the WSL breakdown with coding agents marked, Graphics, Drive, Network and Battery tiles, Settings, start with Windows, Mint and Neutral colors in light and dark, and a per-user installer (`scripts/build-installer.ps1`).

Slint is used under its royalty-free desktop license, which requires an "About Slint" notice in the app or the Slint badge on a public download page.
Pulse shows the `AboutSlint` widget at the bottom of Settings.

## Build and measure

Build with the Windows toolchain: `cargo.exe build --release -p pulse-native`, with `--target-dir` pointing at a Windows folder when the source is inside WSL.

Environment hooks, for tests and screenshots:

- `PULSE_SAMPLE=1` uses the sample readings instead of the real reader.
- `PULSE_SHOW=1` shows the panel at start without activating it and keeps it open on blur.
- `PULSE_THEME=light|dark` forces the theme for that run.
  It wins over the Appearance setting (System, Light, Dark, saved as `appearance` in `settings.json`; System follows the window's light or dark state, which winit reports on every platform).
- `PULSE_VIEW=compact|expanded` starts in that view whatever `settings.json` says (the saved field is `view`, expanded by default).
  The header button switches between the views and saves the choice.
- `PULSE_WSL_OPEN=0|1` starts the compact view with its Linux (WSL) list closed or open (the saved field is `wsl_open`, closed by default; the expanded view always shows the list).
- `PULSE_SELFTEST=1` with `PULSE_REPORT=<path>` opens and closes the panel five times and writes `{"shell":"native","open_ms":[...]}`.
- `PULSE_NOTRIM=1` turns off the working-set trim on hide.
- `PULSE_RENDERER=skia` uses the GPU renderer (about 108 MB; the default software renderer is the reason memory stays low).
- `PULSE_STRESS=1` feeds worst-case tile values and very long names (app names, exe lines, Linux program labels with the agent badge and a count, the busy status line), to check that every text ends in an ellipsis or wraps and nothing is cut.
- `PULSE_SCALE=<factor>` (1.0 to 4.0) forces the display scale: Slint draws at that factor, the window is sized and placed (12 logical px from the work-area corner) at that factor, the app icons are asked for at the scaled pixel size (36 px at 1.5) and the tray icon size follows it.
  It simulates scaling on a 100% monitor for screenshots; it is not a real scaled monitor, and per-monitor DPI changes when dragging between monitors are untested.
- `PULSE_BATTERY=<percent>,<plugged 0|1>` replaces the battery reading (for example `12,0` or `80,1`), to see the Battery tile on a desktop PC.
- `PULSE_WORK_H=<logical px>` pretends the work area is that tall, to check the panel on a short display.
- `PULSE_OPEN=settings` starts with the Settings screen open (for screenshots).
- `PULSE_DEMO_SHORTCUT=recording|keys|taken|failed|invalid` (with `PULSE_OPEN=settings`) shows one state of the shortcut field without any key press: `recording` (waiting for keys), `keys` (Alt and Shift held, with the "hold two" warning), `taken` (another app has it), `failed`, `invalid`.
  It registers nothing.
  The off state needs no hook: a `settings.json` with `"hotkey": ""` and `"hotkey_last": "Ctrl+Shift+F20"` shows the switch off with the combination dimmed.
  Screenshots of a test copy should use F20 to F24 shortcuts, never the owner's real one.
- `PULSE_WARN=1` makes the reader report 92% processor and 9 days of uptime, to see the warning colors and "restart?".
- `PULSE_NOAPPS=1` makes the reader report no apps, to see the "Reading apps..." row.
- `PULSE_SETTINGS_DIR=<folder>` keeps `settings.json` in that folder instead of `%APPDATA%\Pulse` (test runs use it).
- `PULSE_INSTANCE_NAME=<name>` gives a test run its own single-instance name, so it never pokes the real panel.
- `PULSE_AUTOSTART_NAME=<name>` names the Run-key value a test run would write.
- With any of `PULSE_SAMPLE`, `PULSE_SHOW`, `PULSE_SELFTEST`, `PULSE_SETTINGS_DIR` or `PULSE_INSTANCE_NAME` set, the app does not bring the Run key in line with the setting at start.
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

The version is the workspace version in the root `Cargo.toml`; the exe, its version details and the installer name all take it from there.

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
