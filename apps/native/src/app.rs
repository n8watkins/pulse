//! The tray app: window life cycle, tray icon, reader thread and test hooks.

use crate::view::{self, History};
use crate::win::{self, Hwnd};
use pulse_readings::{IconImage, Sample, Snapshot, Source, icon_rgba};
use slint::winit_030::winit::dpi::PhysicalPosition;
use slint::winit_030::winit::event::{ElementState, WindowEvent};
use slint::winit_030::winit::keyboard::{Key, NamedKey};
use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{CloseRequestResponse, ComponentHandle, Model, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

slint::include_modules!();

const TICK: Duration = Duration::from_secs(2);
/// A tray click this soon after a blur-hide is the click that caused the blur.
const REOPEN_GUARD: Duration = Duration::from_millis(300);

struct Config {
    sample: bool,
    show: bool,
    theme: i32,
    selftest_report: Option<String>,
}

impl Config {
    fn from_env() -> Self {
        let on = |k: &str| std::env::var(k).is_ok_and(|v| v == "1");
        Self {
            sample: on("PULSE_SAMPLE"),
            show: on("PULSE_SHOW"),
            theme: match std::env::var("PULSE_THEME").as_deref() {
                Ok("light") => 1,
                Ok("dark") => 2,
                _ => 0,
            },
            selftest_report: on("PULSE_SELFTEST")
                .then(|| std::env::var("PULSE_REPORT").unwrap_or_default()),
        }
    }
}

/// Newest reading, kept by the reader thread whether or not the panel is shown.
struct Latest {
    snap: Snapshot,
    hist: History,
}

enum ReaderCmd {
    /// Read now. The flag forces the per-process scan even while hidden.
    Wake(bool),
}

type Measure = Arc<Mutex<Option<(Instant, Sender<f64>)>>>;

/// Executable icons: `icon_rgba` runs once per path on a worker thread (it
/// touches the shell and can be slow), the result is kept for the session.
struct Icons {
    /// Finished lookups, shared with the worker. `None` = no icon, keep the letter.
    done: Arc<Mutex<HashMap<String, Option<IconImage>>>>,
    /// Paths already handed to the worker.
    asked: RefCell<HashSet<String>>,
    /// Slint images built from `done`, on the UI thread.
    images: RefCell<HashMap<String, slint::Image>>,
    to_worker: Sender<String>,
}

impl Icons {
    fn spawn() -> Self {
        let done: Arc<Mutex<HashMap<String, Option<IconImage>>>> = Arc::default();
        let (to_worker, rx) = mpsc::channel::<String>();
        let shared = done.clone();
        std::thread::spawn(move || {
            while let Ok(path) = rx.recv() {
                let img = icon_rgba(&path, 32);
                shared
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(path, img);
                let _ = slint::invoke_from_event_loop(|| {
                    with_panel(|p| {
                        if p.visible.get() {
                            p.apply_latest();
                        }
                    });
                });
            }
        });
        Self {
            done,
            asked: RefCell::default(),
            images: RefCell::default(),
            to_worker,
        }
    }

    /// The icon if it is ready; otherwise asks the worker once and returns `None`.
    fn get(&self, path: &str) -> Option<slint::Image> {
        if let Some(i) = self.images.borrow().get(path) {
            return Some(i.clone());
        }
        let ready = self
            .done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(path)
            .cloned();
        match ready {
            Some(Some(img)) => {
                let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    &img.rgba, img.width, img.height,
                );
                let image = slint::Image::from_rgba8(buf);
                self.images
                    .borrow_mut()
                    .insert(path.to_owned(), image.clone());
                Some(image)
            }
            Some(None) => None,
            None => {
                if self.asked.borrow_mut().insert(path.to_owned()) {
                    let _ = self.to_worker.send(path.to_owned());
                }
                None
            }
        }
    }
}

struct Panel {
    ui: PanelWindow,
    model: Rc<VecModel<AppItem>>,
    hwnd: Cell<Option<Hwnd>>,
    visible: Cell<bool>,
    last_blur_hide: Cell<Option<Instant>>,
    blur_hides: bool,
    sample: bool,
    latest: Arc<Mutex<Option<Latest>>>,
    shown: Arc<AtomicBool>,
    reader: Sender<ReaderCmd>,
    measure: Measure,
    icons: Icons,
    trim: bool,
}

thread_local! {
    static PANEL: RefCell<Option<Rc<Panel>>> = const { RefCell::new(None) };
}

fn with_panel(f: impl FnOnce(&Rc<Panel>)) {
    let p = PANEL.with(|p| p.borrow().clone());
    if let Some(p) = p {
        f(&p);
    }
}

impl Panel {
    fn find_hwnd(&self) -> Option<Hwnd> {
        self.ui
            .window()
            .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
                RawWindowHandle::Win32(h) => Some(Hwnd(h.hwnd.get())),
                _ => None,
            })
            .flatten()
    }

    fn apply_latest(&self) {
        let guard = self
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(l) = guard.as_ref() else { return };
        let v = view::build(&l.snap, &l.hist, self.sample);
        drop(guard);
        let ui = &self.ui;
        ui.set_status(v.status.into());
        ui.set_cpu_big(v.cpu_big.into());
        ui.set_cpu_sub(v.cpu_sub.into());
        ui.set_cpu_line(v.cpu_line.into());
        ui.set_cpu_fill(v.cpu_fill.into());
        ui.set_mem_big(v.mem_big.into());
        ui.set_mem_sub(v.mem_sub.into());
        ui.set_mem_line(v.mem_line.into());
        ui.set_mem_fill(v.mem_fill.into());
        let rows: Vec<AppItem> = v
            .apps
            .into_iter()
            .map(|a| {
                let icon = a.path.as_deref().and_then(|p| self.icons.get(p));
                AppItem {
                    has_icon: icon.is_some(),
                    icon: icon.unwrap_or_default(),
                    letter: SharedString::from(a.letter),
                    color: slint::Color::from_rgb_u8(a.rgb[0], a.rgb[1], a.rgb[2]),
                    name: a.name.into(),
                    detail: a.detail.into(),
                    value: a.value.into(),
                    frac: a.frac,
                }
            })
            .collect();
        if self.model.row_count() == rows.len() {
            for (i, r) in rows.into_iter().enumerate() {
                self.model.set_row_data(i, r);
            }
        } else {
            self.model.set_vec(rows);
        }
    }

    fn show(&self, activate: bool, measure: Option<Sender<f64>>) {
        let Some(h) = self.hwnd.get() else { return };
        if self.visible.get() {
            return;
        }
        let t0 = Instant::now();
        self.visible.set(true);
        self.shown.store(true, Ordering::Relaxed);
        self.apply_latest();
        win::place(h);
        if let Some(tx) = measure {
            *self
                .measure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((t0, tx));
        }
        if activate {
            win::show_active(h);
        } else {
            win::show_inactive(h);
        }
        // Moving between monitors of different scale can resize the window
        // behind our back; place it once more now that it is shown.
        win::place(h);
        let _ = self.reader.send(ReaderCmd::Wake(false));
    }

    /// Called as a redraw starts. The first one after an armed `show` ends the
    /// open measurement; the timer runs once the draw and present have returned.
    fn frame_drawn(&self) {
        let m = self
            .measure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some((t0, tx)) = m {
            slint::Timer::single_shot(Duration::ZERO, move || {
                let _ = tx.send(t0.elapsed().as_secs_f64() * 1000.0);
            });
        }
    }

    fn hide(&self, from_blur: bool) {
        let Some(h) = self.hwnd.get() else { return };
        if !self.visible.get() {
            return;
        }
        self.visible.set(false);
        self.shown.store(false, Ordering::Relaxed);
        if from_blur {
            self.last_blur_hide.set(Some(Instant::now()));
        }
        win::hide(h);
        if self.trim {
            win::trim_working_set();
        }
    }

    fn toggle(&self) {
        if self
            .last_blur_hide
            .get()
            .is_some_and(|t| t.elapsed() < REOPEN_GUARD)
        {
            return;
        }
        if self.visible.get() {
            self.hide(false);
        } else {
            self.show(true, None);
        }
    }
}

fn spawn_reader(
    sample: bool,
    latest: Arc<Mutex<Option<Latest>>>,
    shown: Arc<AtomicBool>,
    rx: mpsc::Receiver<ReaderCmd>,
) {
    std::thread::spawn(move || {
        let mut src: Box<dyn Source> = if sample {
            Box::new(Sample::default())
        } else {
            pulse_readings::system()
        };
        let mut hist = History::default();
        let mut apps = Vec::new();
        let mut force_apps = false;
        if sample {
            // Like the reference page, which starts with 60 samples of history.
            for _ in 0..60 {
                hist.push(&src.read(false));
            }
        }
        loop {
            let visible = shown.load(Ordering::Relaxed);
            let mut snap = src.read(visible || force_apps);
            if snap.apps.is_empty() {
                snap.apps.clone_from(&apps);
            } else {
                apps.clone_from(&snap.apps);
            }
            force_apps = false;
            hist.push(&snap);
            *latest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Latest {
                snap,
                hist: hist.clone(),
            });
            if visible {
                // Only while the panel is shown does the UI thread hear of it.
                let _ = slint::invoke_from_event_loop(|| {
                    with_panel(|p| {
                        if p.visible.get() {
                            p.apply_latest();
                        }
                    })
                });
            }
            match rx.recv_timeout(TICK) {
                Ok(ReaderCmd::Wake(f)) => force_apps = f,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
}

/// The tray icon, drawn in code: dark rounded square with two bars, like the
/// reference page's tray icon.
fn tray_image() -> Icon {
    const N: usize = 32;
    let mut rgba = vec![0u8; N * N * 4];
    let s = 64.0 / N as f32;
    let cover = |px: usize, py: usize, f: &dyn Fn(f32, f32) -> bool| -> f32 {
        let mut c = 0;
        for sy in 0..4 {
            for sx in 0..4 {
                let x = (px as f32 + (sx as f32 + 0.5) / 4.0) * s;
                let y = (py as f32 + (sy as f32 + 0.5) / 4.0) * s;
                c += usize::from(f(x, y));
            }
        }
        c as f32 / 16.0
    };
    let round_rect = |x: f32, y: f32| {
        let (cx, cy) = (x.clamp(16.0, 48.0), y.clamp(16.0, 48.0));
        (2.0..=62.0).contains(&x) && (2.0..=62.0).contains(&y) && (x - cx).hypot(y - cy) <= 14.0
    };
    let bars = [
        (13.0f32, 0.24f32, [184u8, 245, 208]),
        (37.0, 0.43, [255, 255, 255]),
    ];
    for py in 0..N {
        for px in 0..N {
            let i = (py * N + px) * 4;
            let base = cover(px, py, &round_rect);
            if base == 0.0 {
                continue;
            }
            let mut col = [13.0f32, 15.0, 14.0];
            for (x0, frac, rgb) in bars {
                let h = (42.0 * frac).max(5.0);
                let track = cover(px, py, &|x, y| {
                    x >= x0 && x < x0 + 15.0 && (11.0..53.0).contains(&y)
                });
                let fill = cover(px, py, &|x, y| {
                    x >= x0 && x < x0 + 15.0 && y >= 53.0 - h && y < 53.0
                });
                for k in 0..3 {
                    col[k] += (f32::from(rgb[k]) - col[k]) * (track * 0.22);
                    col[k] += (f32::from(rgb[k]) - col[k]) * fill;
                }
            }
            rgba[i] = col[0] as u8;
            rgba[i + 1] = col[1] as u8;
            rgba[i + 2] = col[2] as u8;
            rgba[i + 3] = (base * 255.0) as u8;
        }
    }
    Icon::from_rgba(rgba, N as u32, N as u32).expect("valid icon buffer")
}

#[allow(clippy::too_many_lines)]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = Config::from_env();

    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(std::env::var("PULSE_RENDERER").unwrap_or_else(|_| "skia-software".into()))
        .with_winit_window_attributes_hook(|a| {
            // Created hidden offscreen, never activated, fixed size: the real
            // position and visibility come from `Panel::show`.
            a.with_active(false)
                .with_resizable(false)
                .with_transparent(true)
                .with_position(PhysicalPosition::new(-32000, -32000))
        })
        .select()?;

    let ui = PanelWindow::new()?;
    ui.global::<Theme>().set_force(cfg.theme);
    let model = Rc::new(VecModel::<AppItem>::default());
    ui.set_apps(model.clone().into());

    let latest = Arc::new(Mutex::new(None));
    let shown = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    spawn_reader(cfg.sample, latest.clone(), shown.clone(), rx);

    let measure: Measure = Arc::new(Mutex::new(None));
    let panel = Rc::new(Panel {
        ui,
        model,
        hwnd: Cell::new(None),
        visible: Cell::new(false),
        last_blur_hide: Cell::new(None),
        blur_hides: !cfg.show && cfg.selftest_report.is_none(),
        sample: cfg.sample,
        latest,
        shown,
        reader: tx.clone(),
        measure: measure.clone(),
        icons: Icons::spawn(),
        trim: std::env::var_os("PULSE_NOTRIM").is_none(),
    });
    PANEL.with(|p| *p.borrow_mut() = Some(panel.clone()));

    panel.ui.window().on_close_requested(|| {
        with_panel(|p| p.hide(false));
        CloseRequestResponse::KeepWindowShown
    });
    panel.ui.on_close_clicked(|| with_panel(|p| p.hide(false)));

    // Tray: left click toggles, right click opens the menu.
    let menu = Menu::new();
    let open = MenuItem::new("Open Pulse", true, None);
    let exit = MenuItem::new("Exit", true, None);
    menu.append_items(&[&open, &exit])?;
    let (open_id, exit_id) = (open.id().clone(), exit.id().clone());
    let _ = MenuEvent::set_event_handler(Some(Arc::new(move |e: MenuEvent| {
        if e.id == open_id {
            let _ = slint::invoke_from_event_loop(|| {
                with_panel(|p| {
                    if !p.visible.get() {
                        p.show(true, None);
                    }
                });
            });
        } else if e.id == exit_id {
            let _ = slint::invoke_from_event_loop(|| {
                let _ = slint::quit_event_loop();
            });
        }
    })));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = e
        {
            let _ = slint::invoke_from_event_loop(|| with_panel(|p| p.toggle()));
        }
    }));
    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("Pulse")
        .with_icon(tray_image())
        .build()?;

    // The native window exists once the event loop runs: finish setting it up
    // then, keep it hidden, and show it at once for PULSE_SHOW.
    panel.ui.window().show()?;
    let timer = Rc::new(slint::Timer::default());
    let t2 = timer.clone();
    let start_shown = cfg.show;
    let selftest = cfg.selftest_report.clone();
    let selftest_tx = tx;
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(5),
        move || {
            with_panel(|p| {
                let Some(h) = p.find_hwnd() else { return };
                t2.stop();
                win::hide(h);
                win::style_tool_window(h);
                win::no_system_corners(h);
                p.hwnd.set(Some(h));
                p.ui.window().on_winit_window_event(|_, ev| match ev {
                    WindowEvent::RedrawRequested => {
                        with_panel(|p| p.frame_drawn());
                        EventResult::Propagate
                    }
                    WindowEvent::Focused(false) => {
                        with_panel(|p| {
                            if p.blur_hides {
                                p.hide(true);
                            }
                        });
                        EventResult::Propagate
                    }
                    WindowEvent::KeyboardInput { event, .. }
                        if event.state == ElementState::Pressed
                            && event.logical_key == Key::Named(NamedKey::Escape) =>
                    {
                        with_panel(|p| p.hide(false));
                        EventResult::PreventDefault
                    }
                    WindowEvent::CloseRequested => {
                        with_panel(|p| p.hide(false));
                        EventResult::PreventDefault
                    }
                    _ => EventResult::Propagate,
                });
                if start_shown {
                    p.show(false, None);
                } else if p.trim {
                    win::trim_working_set();
                }
                if let Some(report) = selftest.clone() {
                    spawn_selftest(report, selftest_tx.clone());
                }
            });
        },
    );

    slint::run_event_loop_until_quit()?;
    Ok(())
}

/// PULSE_SELFTEST: after 3 s open and close the panel five times without
/// activating, one second apart, and write how long each open took.
fn spawn_selftest(report: String, reader: Sender<ReaderCmd>) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        // One scan with apps so the first open has rows to draw, like any
        // open after the first one does.
        let _ = reader.send(ReaderCmd::Wake(true));
        std::thread::sleep(Duration::from_millis(500));
        let mut times = Vec::new();
        for _ in 0..5 {
            let (tx, rx) = mpsc::channel();
            let _ = slint::invoke_from_event_loop(move || with_panel(|p| p.show(false, Some(tx))));
            times.push(rx.recv_timeout(Duration::from_secs(5)).unwrap_or(-1.0));
            std::thread::sleep(Duration::from_millis(500));
            let _ = slint::invoke_from_event_loop(|| with_panel(|p| p.hide(false)));
            std::thread::sleep(Duration::from_millis(500));
        }
        let list: Vec<String> = times.iter().map(|t| format!("{t:.1}")).collect();
        let json = format!("{{\"shell\":\"native\",\"open_ms\":[{}]}}", list.join(","));
        if !report.is_empty() {
            let _ = std::fs::write(&report, json);
        }
    });
}
