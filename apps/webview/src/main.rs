//! Pulse, webview shell: a tray icon and one hidden Tauri window.
//!
//! The reader thread takes a snapshot every 2 s. While the panel is hidden it
//! skips the process scan and sends nothing to the page, so Windows can trim
//! the hidden webview. While shown it emits each snapshot as one event.
//!
//! Environment hooks (the native shell implements the same ones):
//! `PULSE_SAMPLE=1` sample data, `PULSE_SHOW=1` show at start without
//! activating and keep open on blur, `PULSE_THEME=light|dark` force the theme,
//! `PULSE_SELFTEST=1` with `PULSE_REPORT=<path>` open and close five times and
//! write the open times.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use pulse_readings::{IconImage, Sample, Snapshot, Source};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewWindow, WindowEvent,
};

const LABEL: &str = "main";
const WIDTH: f64 = 380.0;
const HEIGHT: f64 = 700.0;
const MARGIN: f64 = 12.0;
const HISTORY: usize = 90;
/// Samples of history made up front with sample data.
const SEED: usize = 60;
/// Icon size asked of the reader; the page shows it at 24 px.
const ICON_PX: u32 = 32;
/// Attempts per icon path before giving up on it.
const ICON_TRIES: u8 = 3;
const TICK: Duration = Duration::from_secs(2);
/// A toggle this soon after an automatic hide is the tray click that caused it.
const REOPEN_GUARD_MS: u64 = 300;

struct Shared {
    start: Instant,
    visible: AtomicBool,
    /// Milliseconds since `start` of the last hide caused by losing focus, plus one.
    auto_hide_at: AtomicU64,
    keep_open_on_blur: bool,
    suspend: bool,
    /// Low webview memory target and a host trim on hide; `PULSE_NOTRIM=1` turns it off.
    trim: bool,
    wake: Mutex<Sender<()>>,
    /// When the last show was requested and its first frame has not been reported.
    pending_show: Mutex<Option<Instant>>,
    /// Where the self-test waits for the first-frame time.
    painted: Mutex<Option<Sender<f64>>>,
    theme: Option<String>,
    sample: bool,
}

type SharedState = Arc<Shared>;

#[derive(Serialize)]
struct Payload<'a> {
    snapshot: &'a Snapshot,
    cpu: &'a VecDeque<f32>,
    mem: &'a VecDeque<f32>,
    /// True for the snapshot sent because the panel was just shown.
    first: bool,
}

#[derive(Serialize)]
struct Config {
    theme: Option<String>,
    sample: bool,
}

fn env_on(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}

#[tauri::command]
fn config(state: State<'_, SharedState>) -> Config {
    Config {
        theme: state.theme.clone(),
        sample: state.sample,
    }
}

/// The page calls this once it is listening: if the panel was shown before the
/// page loaded, send it a snapshot now.
#[tauri::command]
fn ready(state: State<'_, SharedState>) {
    if state.visible.load(Ordering::SeqCst)
        && let Ok(wake) = state.wake.lock()
    {
        let _ = wake.send(());
    }
}

#[tauri::command]
fn hide_panel(app: AppHandle, state: State<'_, SharedState>) {
    hide(&app, &state, false);
}

/// The page calls this after the first frame with data was painted.
#[tauri::command]
fn frame_painted(state: State<'_, SharedState>) {
    let t0 = state.pending_show.lock().ok().and_then(|mut p| p.take());
    if let Some(t0) = t0 {
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        if let Some(tx) = state.painted.lock().ok().and_then(|p| p.clone()) {
            let _ = tx.send(ms);
        }
    }
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

/// Bottom-right of the work area of the monitor the cursor is on.
fn place(app: &AppHandle, win: &WebviewWindow) {
    let monitors = app.available_monitors().unwrap_or_default();
    let cursor = app.cursor_position().ok();
    let under_cursor = cursor.and_then(|c| {
        monitors.iter().find(|m| {
            let (p, s) = (m.position(), m.size());
            c.x >= f64::from(p.x)
                && c.x < f64::from(p.x) + f64::from(s.width)
                && c.y >= f64::from(p.y)
                && c.y < f64::from(p.y) + f64::from(s.height)
        })
    });
    let Some(mon) = under_cursor
        .cloned()
        .or_else(|| app.primary_monitor().ok().flatten())
        .or_else(|| monitors.first().cloned())
    else {
        return;
    };
    let scale = mon.scale_factor();
    let wa = mon.work_area();
    let (wa_x, wa_y) = (f64::from(wa.position.x), f64::from(wa.position.y));
    let (wa_w, wa_h) = (f64::from(wa.size.width), f64::from(wa.size.height));
    let margin = (MARGIN * scale).round();
    let w = (WIDTH * scale).round();
    let h = (HEIGHT * scale).round().min(wa_h - 2.0 * margin);
    let x = wa_x + wa_w - w - margin;
    let y = wa_y + wa_h - h - margin;
    let pos = PhysicalPosition::new(x as i32, y as i32);
    // Move first so a different monitor's scale applies before the size does.
    let _ = win.set_position(pos);
    let _ = win.set_size(PhysicalSize::new(w as u32, h as u32));
    let _ = win.set_position(pos);
}

#[cfg(windows)]
fn show_without_activating(win: &WebviewWindow) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
    if let Ok(hwnd) = win.hwnd() {
        // SAFETY: the handle belongs to this live window.
        unsafe { ShowWindow(hwnd.0.cast(), SW_SHOWNOACTIVATE) };
    }
}

#[cfg(not(windows))]
fn show_without_activating(win: &WebviewWindow) {
    let _ = win.show();
}

fn show(app: &AppHandle, state: &SharedState, activate: bool) {
    let Some(win) = window(app) else { return };
    if let Ok(mut p) = state.pending_show.lock() {
        *p = Some(Instant::now());
    }
    place(app, &win);
    state.visible.store(true, Ordering::SeqCst);
    if state.suspend {
        suspend_webview(&win, false);
    }
    if state.trim {
        memory_target(&win, false);
    }
    if activate {
        let _ = win.show();
        let _ = win.set_focus();
    } else {
        show_without_activating(&win);
    }
    if let Ok(wake) = state.wake.lock() {
        let _ = wake.send(());
    }
}

fn hide(app: &AppHandle, state: &SharedState, automatic: bool) {
    state.visible.store(false, Ordering::SeqCst);
    if automatic {
        let ms = state.start.elapsed().as_millis() as u64;
        state.auto_hide_at.store(ms + 1, Ordering::SeqCst);
    }
    if let Some(win) = window(app) {
        let _ = win.hide();
        if state.suspend {
            suspend_webview(&win, true);
        }
        if state.trim {
            trim_when_hidden(&win, state);
        }
    }
}

/// Low memory target for the webview now, then the host's own working set
/// once things have settled, if the panel is still hidden.
fn trim_when_hidden(win: &WebviewWindow, state: &SharedState) {
    memory_target(win, true);
    let state = Arc::clone(state);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        if !state.visible.load(Ordering::SeqCst) {
            trim_host();
        }
    });
}

/// Hands this process's idle pages back to Windows.
#[cfg(windows)]
fn trim_host() {
    use windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: the pseudo handle of the current process is always valid.
    unsafe { K32EmptyWorkingSet(GetCurrentProcess()) };
}

#[cfg(not(windows))]
fn trim_host() {}

/// WebView2's memory usage target: low while hidden, normal while shown.
#[cfg(windows)]
fn memory_target(win: &WebviewWindow, low: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
        ICoreWebView2_19,
    };
    use windows_core::Interface;
    let _ = win.with_webview(move |wv| {
        // SAFETY: the controller belongs to this live webview, called on its thread.
        unsafe {
            let Ok(core) = wv.controller().CoreWebView2() else {
                return;
            };
            let Ok(core19) = core.cast::<ICoreWebView2_19>() else {
                return;
            };
            let level = if low {
                COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
            } else {
                COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
            };
            let _ = core19.SetMemoryUsageTargetLevel(level);
        }
    });
}

#[cfg(not(windows))]
fn memory_target(_win: &WebviewWindow, _low: bool) {}

/// Experiment, off unless `PULSE_SUSPEND=1` (not part of the shared hooks):
/// WebView2's own suspend while hidden, resumed on show.
#[cfg(windows)]
fn suspend_webview(win: &WebviewWindow, suspend: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
    use windows_core::Interface;
    let _ = win.with_webview(move |wv| {
        // SAFETY: the controller belongs to this live webview, called on its thread.
        unsafe {
            let controller = wv.controller();
            let _ = controller.SetIsVisible(!suspend);
            let Ok(core) = controller.CoreWebView2() else {
                return;
            };
            let Ok(core3) = core.cast::<ICoreWebView2_3>() else {
                return;
            };
            if suspend {
                let handler =
                    webview2_com::TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(())));
                let _ = core3.TrySuspend(&handler);
            } else {
                let _ = core3.Resume();
            }
        }
    });
}

#[cfg(not(windows))]
fn suspend_webview(_win: &WebviewWindow, _suspend: bool) {}

fn toggle(app: &AppHandle, state: &SharedState) {
    if state.visible.load(Ordering::SeqCst) {
        hide(app, state, false);
        return;
    }
    let at = state.auto_hide_at.load(Ordering::SeqCst);
    let now = state.start.elapsed().as_millis() as u64;
    if at != 0 && now.saturating_sub(at - 1) < REOPEN_GUARD_MS {
        return;
    }
    show(app, state, true);
}

#[cfg(windows)]
fn make_tool_window(win: &WebviewWindow) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    };
    if let Ok(hwnd) = win.hwnd() {
        let h = hwnd.0.cast();
        // SAFETY: the handle belongs to this live window.
        unsafe {
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
            let ex = (ex | WS_EX_TOOLWINDOW as isize) & !(WS_EX_APPWINDOW as isize);
            SetWindowLongPtrW(h, GWL_EXSTYLE, ex);
        }
    }
}

#[cfg(not(windows))]
fn make_tool_window(_win: &WebviewWindow) {}

/// The icon as a PNG data URL for the page.
fn png_data_url(img: &IconImage) -> Option<String> {
    use base64::Engine;
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().ok()?;
    w.write_image_data(&img.rgba).ok()?;
    w.finish().ok()?;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(out)
    ))
}

/// Appends a snapshot's processor and memory figures to the sparkline history.
fn record(cpu: &mut VecDeque<f32>, mem: &mut VecDeque<f32>, snap: &Snapshot) {
    let used = if snap.mem_total == 0 {
        0.0
    } else {
        snap.mem_used as f64 / snap.mem_total as f64 * 100.0
    };
    cpu.push_back(snap.cpu_pct);
    mem.push_back(used as f32);
    while cpu.len() > HISTORY {
        cpu.pop_front();
        mem.pop_front();
    }
}

/// The reader thread: one snapshot per tick, an extra one when woken by a show.
fn reader(app: AppHandle, state: SharedState, mut source: Box<dyn Source>, wake: Receiver<()>) {
    let mut cpu = VecDeque::with_capacity(HISTORY + 1);
    let mut mem = VecDeque::with_capacity(HISTORY + 1);
    // Per icon path: how many times it was tried (ICON_TRIES once sent).
    let mut tried: HashMap<String, u8> = HashMap::new();
    if state.sample {
        // Sample data starts with a full history, as the reference page does.
        for _ in 0..SEED {
            record(&mut cpu, &mut mem, &source.read(false));
        }
    }
    loop {
        let woken = match wake.recv_timeout(TICK) {
            Ok(()) => true,
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let visible = state.visible.load(Ordering::SeqCst);
        let snap = source.read(visible);
        record(&mut cpu, &mut mem, &snap);
        // Nothing reaches the page while it is hidden.
        if visible {
            // Each icon goes to the page once, before the snapshot that uses it.
            let mut fresh = serde_json::Map::new();
            for path in snap.apps.iter().filter_map(|a| a.path.as_ref()) {
                // A path that failed is tried again on later snapshots, up to
                // ICON_TRIES times; one that worked is never read again.
                let tries = tried.entry(path.clone()).or_insert(0);
                if *tries >= ICON_TRIES {
                    continue;
                }
                match pulse_readings::icon_rgba(path, ICON_PX).and_then(|img| png_data_url(&img)) {
                    Some(url) => {
                        *tries = ICON_TRIES;
                        fresh.insert(path.clone(), url.into());
                    }
                    None => *tries += 1,
                }
            }
            if !fresh.is_empty() {
                let _ = app.emit_to(LABEL, "icons", serde_json::Value::Object(fresh));
            }
            let payload = Payload {
                snapshot: &snap,
                cpu: &cpu,
                mem: &mem,
                first: woken,
            };
            if let Ok(json) = serde_json::to_value(&payload) {
                let _ = app.emit_to(LABEL, "snapshot", json);
            }
        }
    }
}

/// Opens and closes the panel five times without activating it and writes how
/// long each took from the show request to the first painted frame with data.
fn selftest(app: AppHandle, state: SharedState, report: String) {
    std::thread::sleep(Duration::from_secs(3));
    let mut open_ms: Vec<Option<f64>> = Vec::new();
    for _ in 0..5 {
        let (tx, rx) = mpsc::channel();
        if let Ok(mut p) = state.painted.lock() {
            *p = Some(tx);
        }
        show(&app, &state, false);
        let ms = rx.recv_timeout(Duration::from_secs(5)).ok();
        open_ms.push(ms.map(|m| (m * 10.0).round() / 10.0));
        std::thread::sleep(Duration::from_secs(1));
        hide(&app, &state, false);
        std::thread::sleep(Duration::from_secs(1));
    }
    if let Ok(mut p) = state.painted.lock() {
        *p = None;
    }
    let json = serde_json::json!({ "shell": "webview", "open_ms": open_ms });
    let _ = std::fs::write(report, json.to_string());
}

fn main() {
    let sample = env_on("PULSE_SAMPLE");
    let show_at_start = env_on("PULSE_SHOW");
    let theme = std::env::var("PULSE_THEME")
        .ok()
        .filter(|t| t == "light" || t == "dark");
    let (wake_tx, wake_rx) = mpsc::channel();
    let state: SharedState = Arc::new(Shared {
        start: Instant::now(),
        visible: AtomicBool::new(false),
        auto_hide_at: AtomicU64::new(0),
        keep_open_on_blur: show_at_start,
        suspend: env_on("PULSE_SUSPEND"),
        trim: !env_on("PULSE_NOTRIM"),
        wake: Mutex::new(wake_tx),
        pending_show: Mutex::new(None),
        painted: Mutex::new(None),
        theme,
        sample,
    });
    let selftest_report = if env_on("PULSE_SELFTEST") {
        std::env::var("PULSE_REPORT").ok()
    } else {
        None
    };

    let for_events = Arc::clone(&state);
    let for_setup = Arc::clone(&state);
    let wake_rx = Mutex::new(Some(wake_rx));
    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            config,
            ready,
            hide_panel,
            frame_painted
        ])
        .on_window_event(move |win, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                hide(win.app_handle(), &for_events, false);
            }
            WindowEvent::Focused(false)
                if !for_events.keep_open_on_blur && for_events.visible.load(Ordering::SeqCst) =>
            {
                hide(win.app_handle(), &for_events, true);
            }
            _ => {}
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            let state = for_setup;
            if let Some(win) = window(&handle) {
                make_tool_window(&win);
            }

            let open = MenuItemBuilder::with_id("open", "Open Pulse").build(app)?;
            let exit = MenuItemBuilder::with_id("exit", "Exit").build(app)?;
            let menu = MenuBuilder::new(app).items(&[&open, &exit]).build()?;
            let icon = Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
            let on_menu = Arc::clone(&state);
            let on_click = Arc::clone(&state);
            TrayIconBuilder::new()
                .icon(icon)
                .tooltip("Pulse")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "open" if !on_menu.visible.load(Ordering::SeqCst) => {
                        show(app, &on_menu, true);
                    }
                    "exit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(move |tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle(tray.app_handle(), &on_click);
                    }
                })
                .build(app)?;

            let source: Box<dyn Source> = if state.sample {
                Box::new(Sample::default())
            } else {
                pulse_readings::system()
            };
            if let Some(rx) = wake_rx.lock().ok().and_then(|mut r| r.take()) {
                let (a, s) = (handle.clone(), Arc::clone(&state));
                std::thread::spawn(move || reader(a, s, source, rx));
            }
            if state.trim && !show_at_start {
                // Start-up touches far more memory than an idle panel keeps.
                let (a, s) = (handle.clone(), Arc::clone(&state));
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(4));
                    if !s.visible.load(Ordering::SeqCst)
                        && let Some(win) = window(&a)
                    {
                        trim_when_hidden(&win, &s);
                    }
                });
            }
            if state.suspend && !show_at_start {
                // Suspend the hidden page once it has loaded.
                let (a, s) = (handle.clone(), Arc::clone(&state));
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(1500));
                    if !s.visible.load(Ordering::SeqCst)
                        && let Some(win) = window(&a)
                    {
                        suspend_webview(&win, true);
                    }
                });
            }
            if let Some(report) = selftest_report {
                let (a, s) = (handle.clone(), Arc::clone(&state));
                std::thread::spawn(move || selftest(a, s, report));
            } else if show_at_start {
                show(&handle, &state, false);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("the Pulse window runs");
}
