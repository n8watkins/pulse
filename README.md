# Pulse

A native Windows tray app that shows live system health in a small panel.
It lives only in the system tray: no taskbar button and no Alt+Tab entry.

## Status: spike

Two bare shells are being built to compare, before the real build starts:

- `apps/webview`: Tauri 2, the panel is a web page in a hidden WebView2 window.
- `apps/native`: no webview, the panel is drawn by a Rust UI toolkit.

Both draw the same snapshot from `crates/pulse-readings` and are measured by the same script, so memory, open time and looks can be compared on the owner's PC.

`docs/reference/panel-preview.html` holds the panel design both shells copy (the `.pulse` rules and markup).
