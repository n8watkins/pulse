# Pulse

A native Windows tray app that shows live system health in a small panel.
It lives only in the system tray: no taskbar button and no Alt+Tab entry.

## Status: spike

Two bare shells are being built to compare, before the real build starts:

- `apps/webview`: Tauri 2, the panel is a web page in a hidden WebView2 window.
- `apps/native`: no webview, the panel is drawn by a Rust UI toolkit.

Both draw the same snapshot from `crates/pulse-readings` and are measured by the same script, so memory, open time and looks can be compared on the owner's PC.

`docs/reference/panel-preview.html` holds the panel design both shells copy (the `.pulse` rules and markup).

## Build and measure

Build on Windows with `cargo build --release -p pulse-webview` (the exe is `target\release\pulse-webview.exe`).

Both shells follow the same environment hooks:

- `PULSE_SAMPLE=1` uses the sample readings instead of the real reader.
- `PULSE_SHOW=1` shows the panel at start without activating it and keeps it open on blur.
- `PULSE_THEME=light|dark` forces the theme.
- `PULSE_SELFTEST=1` with `PULSE_REPORT=<path>` opens and closes the panel five times and writes `{"shell":"...","open_ms":[...]}`.

Measure any shell that follows the hooks with `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\measure.ps1 -Exe <exe> -Out <folder>`.
It sends no keys, makes no clicks, does not touch the clipboard and never takes focus.
It writes `report.json`, `panel-light.png` and `panel-dark.png` to the folder.
