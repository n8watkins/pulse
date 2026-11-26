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

Built so far: tray icon, open and close, Processor and Memory tiles with sparklines, Top apps with real icons, Graphics, Drive and Network tiles, light and dark.
Not built yet: global shortcut, settings, start with Windows, the WSL breakdown, the tray tooltip and live tray bars, the Neutral colour theme, an installer.

Slint is used under its royalty-free desktop licence, which requires an "About Slint" notice in the app or the Slint badge on a public download page.
Neither exists yet; one must be added before the app is shared.

## Build and measure

Build with the Windows toolchain: `cargo.exe build --release -p pulse-native`, with `--target-dir` pointing at a Windows folder when the source is inside WSL.

Environment hooks, for tests and screenshots:

- `PULSE_SAMPLE=1` uses the sample readings instead of the real reader.
- `PULSE_SHOW=1` shows the panel at start without activating it and keeps it open on blur.
- `PULSE_THEME=light|dark` forces the theme.
- `PULSE_SELFTEST=1` with `PULSE_REPORT=<path>` opens and closes the panel five times and writes `{"shell":"native","open_ms":[...]}`.
- `PULSE_NOTRIM=1` turns off the working-set trim on hide.
- `PULSE_RENDERER=skia` uses the GPU renderer (about 108 MB; the default software renderer is the reason memory stays low).
- `PULSE_STRESS=1` feeds worst-case tile values, to check nothing is clipped.
- `PULSE_WORK_H=<logical px>` pretends the work area is that tall, to check the panel on a short display.

Measure with `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\measure.ps1 -Exe <exe> -Out <folder>`.
It sends no keys, makes no clicks, does not touch the clipboard and never takes focus.

Lint and tests, from WSL:

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
   Regenerate it after dependency changes; the script prints any crate whose licence is copyleft-only or unclear.
4. Build the installer on Windows: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-installer.ps1 -Exe <pulse-native.exe> -Out <folder>` writes `Pulse-<version>-setup.exe`.
   It uses the `makensis.exe` of the NSIS that Tauri downloaded (`%LOCALAPPDATA%\tauri\NSIS`), or one on PATH, or `-Makensis <path>`.
   `-TestName "Pulse Test"` builds an installer that uses that name for the folder, shortcut, uninstall entry, Run-key value and settings folder, so it never touches a real Pulse install.

The installer is per-user (no administrator rights): it installs to `%LOCALAPPDATA%\Programs\Pulse`, adds a Start menu shortcut and an Add/Remove Programs entry under HKCU, and asks a running Pulse from that folder to close first (matched by full path, so a copy running from another folder is left alone).
Silent install: `/S` (starts Pulse at the end unless `/NORUN` is given).
Uninstall keeps the settings in `%APPDATA%\Pulse` unless you answer yes (a silent uninstall always keeps them).
"Start with Windows" is owned by the app; the uninstaller only removes its Run value when it points into the install folder.

The installer and the exe are not code signed, so Windows SmartScreen will warn ("Windows protected your PC") on first run; choose "More info", then "Run anyway".
