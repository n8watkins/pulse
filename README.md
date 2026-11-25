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

Measure with `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\measure.ps1 -Exe <exe> -Out <folder>`.
It sends no keys, makes no clicks, does not touch the clipboard and never takes focus.

Lint and tests, from WSL:

- `cargo fmt --all --check`
- `cargo clippy -p pulse-native -p pulse-readings --all-targets --target x86_64-pc-windows-msvc -- -D warnings`
- `cargo test --workspace`
- The Windows-only reader tests run with `cargo.exe test -p pulse-readings`.
