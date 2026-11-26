//! The tray app: window life cycle, tray icon and menu, settings, shortcut,
//! reader thread and test hooks.

use crate::record::{self, Mods, Pressed, Step};
use crate::tray;
use crate::view::{self, History, RowKind};
use crate::win::{self, Hwnd};
use pulse_platform::hotkey::{Combo, HotkeyError, Key as HotKey, Listener};
use pulse_platform::{Settings, Sort, Theme as ThemeSetting, autostart, single};
use pulse_readings::{AppRow, IconImage, Sample, Snapshot, Source, Wsl, icon_rgba};
use slint::winit_030::winit::dpi::PhysicalPosition;
use slint::winit_030::winit::event::{ElementState, WindowEvent};
use slint::winit_030::winit::keyboard::{Key, NamedKey};
use slint::winit_030::winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{CloseRequestResponse, ComponentHandle, Model, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

slint::include_modules!();

/// Reader tick while the panel is shown.
const TICK: Duration = Duration::from_secs(2);
/// A tray click this soon after a blur-hide is the click that caused the blur.
const REOPEN_GUARD: Duration = Duration::from_millis(300);
/// The choices of "Refresh while hidden", in seconds.
const REFRESH_CHOICES: [u8; 3] = [2, 5, 10];

struct Config {
    sample: bool,
    show: bool,
    theme: i32,
    selftest_report: Option<String>,
    /// PULSE_OPEN=settings: start with the Settings screen open.
    open_settings: bool,
    /// Any of PULSE_SAMPLE, PULSE_SHOW or PULSE_SELFTEST is set: a test run,
    /// which never touches the Run key.
    test_run: bool,
}

impl Config {
    fn from_env() -> Self {
        let on = |k: &str| std::env::var(k).is_ok_and(|v| v == "1");
        let set = |k: &str| std::env::var_os(k).is_some();
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
            open_settings: std::env::var("PULSE_OPEN").is_ok_and(|v| v == "settings"),
            test_run: set("PULSE_SAMPLE") || set("PULSE_SHOW") || set("PULSE_SELFTEST"),
        }
    }
}

/// Newest reading, kept by the reader thread whether or not the panel is shown.
struct Latest {
    snap: Snapshot,
    extra: view::Extra,
    hist: History,
    at: Instant,
}

enum ReaderCmd {
    /// Read now. The flag forces the per-process scan even while hidden.
    Wake(bool),
}

type Measure = Arc<Mutex<Option<(Instant, Sender<f64>)>>>;

/// Executable icons: `icon_rgba` runs on a worker thread (it touches the shell
/// and can be slow). A found icon is kept for the session; a failed lookup is
/// retried on later reader ticks, up to `ICON_TRIES` times, before the letter stays.
struct Icons {
    /// Lookups the worker has finished, shared with it.
    done: Arc<Mutex<HashMap<String, IconSlot>>>,
    /// Paths with a request in flight.
    pending: RefCell<HashSet<String>>,
    /// Slint images built from `done`, on the UI thread.
    images: RefCell<HashMap<String, slint::Image>>,
    to_worker: Sender<String>,
}

struct IconSlot {
    /// `None` = the lookup failed.
    img: Option<IconImage>,
    tries: u8,
    at: Instant,
}

const ICON_TRIES: u8 = 3;
/// Failed lookups wait for a later tick (the reader ticks every 2 s).
const ICON_RETRY_AFTER: Duration = Duration::from_millis(1500);

impl Icons {
    fn spawn() -> Self {
        let done: Arc<Mutex<HashMap<String, IconSlot>>> = Arc::default();
        let (to_worker, rx) = mpsc::channel::<String>();
        let shared = done.clone();
        std::thread::spawn(move || {
            // COM once for the worker's life: `icon_rgba` then only re-enters it.
            let _com = pulse_readings::IconThread::init();
            while let Ok(path) = rx.recv() {
                let img = icon_rgba(&path, 32);
                {
                    let mut map = shared
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let tries = map.get(&path).map_or(0, |s| s.tries) + 1;
                    map.insert(
                        path,
                        IconSlot {
                            img,
                            tries,
                            at: Instant::now(),
                        },
                    );
                }
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
            pending: RefCell::default(),
            images: RefCell::default(),
            to_worker,
        }
    }

    fn request(&self, path: &str) {
        if self.pending.borrow_mut().insert(path.to_owned()) {
            let _ = self.to_worker.send(path.to_owned());
        }
    }

    /// The icon if it is ready; otherwise asks the worker (again, if an earlier
    /// try failed and there are tries left) and returns `None`.
    fn get(&self, path: &str) -> Option<slint::Image> {
        if let Some(i) = self.images.borrow().get(path) {
            return Some(i.clone());
        }
        let ready = self
            .done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(path)
            .map(|s| (s.img.clone(), s.tries, s.at));
        match ready {
            Some((Some(img), ..)) => {
                let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    &img.rgba, img.width, img.height,
                );
                let image = slint::Image::from_rgba8(buf);
                self.images
                    .borrow_mut()
                    .insert(path.to_owned(), image.clone());
                Some(image)
            }
            Some((None, tries, at)) => {
                if tries < ICON_TRIES && at.elapsed() >= ICON_RETRY_AFTER {
                    self.pending.borrow_mut().remove(path);
                    self.request(path);
                }
                None
            }
            None => {
                self.request(path);
                None
            }
        }
    }
}

struct TrayState {
    icon: TrayIcon,
    /// What the icon currently shows; it is redrawn only when this changes.
    key: Cell<Option<tray::Key>>,
    tip: RefCell<String>,
    keep: CheckMenuItem,
    auto: CheckMenuItem,
}

struct Panel {
    ui: PanelWindow,
    model: Rc<VecModel<AppItem>>,
    hwnd: Cell<Option<Hwnd>>,
    visible: Cell<bool>,
    last_blur_hide: Cell<Option<Instant>>,
    /// Hide when the panel loses focus (off for PULSE_SHOW and PULSE_SELFTEST).
    base_blur: bool,
    sample: bool,
    latest: Arc<Mutex<Option<Latest>>>,
    shown: Arc<AtomicBool>,
    reader: Sender<ReaderCmd>,
    measure: Measure,
    icons: Icons,
    trim: bool,
    /// Logical height the panel may use on the monitor it opens on.
    avail_h: Cell<f32>,
    layout: Cell<view::Layout>,
    settings: RefCell<Settings>,
    hotkey: Listener,
    /// The saved shortcut is registered (the footer only names a working one).
    hotkey_ok: Cell<bool>,
    recording: Cell<bool>,
    mods: Cell<Mods>,
    reading_at: Cell<Instant>,
    /// Counts the footer's "Updated Ns ago"; runs only while the panel is shown.
    ago: slint::Timer,
    /// Seconds between readings while hidden, read by the reader thread.
    refresh: Arc<AtomicU8>,
    tray: RefCell<Option<TrayState>>,
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

fn refresh_index(secs: u8) -> i32 {
    REFRESH_CHOICES
        .iter()
        .position(|&s| s == secs)
        .map_or(0, |i| i as i32)
}

/// What one key press means to the recorder.
fn pressed_from(ev: &slint::winit_030::winit::event::KeyEvent) -> Pressed {
    if let Key::Named(n) = &ev.logical_key {
        match n {
            NamedKey::Escape => return Pressed::Escape,
            NamedKey::Shift
            | NamedKey::Control
            | NamedKey::Alt
            | NamedKey::AltGraph
            | NamedKey::Super
            | NamedKey::Meta => return Pressed::Modifier,
            _ => {}
        }
    }
    match ev.key_without_modifiers() {
        Key::Named(n) => match n {
            NamedKey::Space => Pressed::Main(HotKey::Space),
            NamedKey::ArrowUp => Pressed::Main(HotKey::Up),
            NamedKey::ArrowDown => Pressed::Main(HotKey::Down),
            NamedKey::ArrowLeft => Pressed::Main(HotKey::Left),
            NamedKey::ArrowRight => Pressed::Main(HotKey::Right),
            other => {
                // The F keys are named F1 to F35.
                let name = format!("{other:?}");
                name.strip_prefix('F')
                    .and_then(|n| n.parse::<u8>().ok())
                    .filter(|n| (1..=24).contains(n))
                    .map_or(Pressed::Other, |n| Pressed::Main(HotKey::F(n)))
            }
        },
        Key::Character(s) => s
            .chars()
            .next()
            .and_then(record::key_from_char)
            .map_or(Pressed::Other, Pressed::Main),
        _ => Pressed::Other,
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

    fn blur_hides(&self) -> bool {
        self.base_blur && !self.settings.borrow().keep_open && !self.recording.get()
    }

    fn apply_latest(&self) {
        let guard = self
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(l) = guard.as_ref() else { return };
        let lay = view::fit(self.avail_h.get(), l.extra.battery.is_some());
        let sort = self.settings.borrow().sort;
        let v = view::build(&l.snap, &l.extra, &l.hist, self.sample, sort);
        self.reading_at.set(l.at);
        drop(guard);
        let ui = &self.ui;
        ui.set_shrink(lay.shrink);
        if lay != self.layout.replace(lay)
            && self.visible.get()
            && let Some(h) = self.hwnd.get()
        {
            // The battery strip came or went: the height changed with it.
            win::place(h, lay.height());
        }
        ui.set_status(v.status.text.into());
        ui.set_status_warn(v.status.warn);
        ui.set_cpu_big(v.cpu_big.into());
        ui.set_cpu_sub(v.cpu_sub.into());
        ui.set_cpu_restart(v.cpu_restart);
        ui.set_cpu_line(v.cpu_line.into());
        ui.set_cpu_fill(v.cpu_fill.into());
        ui.set_mem_big(v.mem_big.into());
        ui.set_mem_sub(v.mem_sub.into());
        ui.set_mem_line(v.mem_line.into());
        ui.set_mem_fill(v.mem_fill.into());
        let t = v.tiles;
        ui.set_gpu_big(t.gpu_big.into());
        ui.set_gpu_missing(t.gpu_missing);
        ui.set_gpu_frac(t.gpu_frac);
        ui.set_drive_big(t.drive_big.into());
        ui.set_drive_sub(t.drive_sub.into());
        ui.set_drive_frac(t.drive_frac);
        ui.set_net_big(t.net_big.into());
        ui.set_net_sub(t.net_sub.into());
        ui.set_has_battery(t.battery.is_some());
        if let Some(b) = t.battery {
            ui.set_bat_big(b.big.into());
            ui.set_bat_sub(b.sub.into());
            ui.set_bat_frac(b.frac);
        }
        let rows: Vec<AppItem> = v
            .apps
            .into_iter()
            .map(|a| {
                let icon = a.path.as_deref().and_then(|p| self.icons.get(p));
                AppItem {
                    kind: match a.kind {
                        RowKind::App => 0,
                        RowKind::Wsl => 1,
                        RowKind::Sub => 2,
                    },
                    has_icon: icon.is_some(),
                    icon: icon.unwrap_or_default(),
                    letter: SharedString::from(a.letter),
                    color: slint::Color::from_rgb_u8(a.rgb[0], a.rgb[1], a.rgb[2]),
                    name: a.name.into(),
                    detail: a.detail.into(),
                    value: a.value.into(),
                    frac: a.frac,
                    agent: a.agent,
                    count: a.count.into(),
                    group_end: a.group_end,
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
        self.update_ago();
    }

    fn update_ago(&self) {
        let secs = self.reading_at.get().elapsed().as_secs();
        self.ui.set_ago(view::updated_ago(secs).into());
    }

    /// A new reading: the tray follows it whether or not the panel is shown.
    fn on_reading(&self) {
        self.update_tray();
        if self.visible.get() {
            self.apply_latest();
        }
    }

    /// Redraws the tray icon only when its picture changes, and its tooltip
    /// only when the text changes.
    fn update_tray(&self) {
        let guard = self
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(l) = guard.as_ref() else { return };
        let tray = self.tray.borrow();
        let Some(t) = tray.as_ref() else { return };
        #[allow(clippy::cast_precision_loss)]
        let mem = if l.snap.mem_total == 0 {
            0.0
        } else {
            (l.snap.mem_used as f64 / l.snap.mem_total as f64) as f32
        };
        let key = tray::Key::new(
            tray::size_for_dpi(win::system_dpi()),
            l.snap.cpu_pct / 100.0,
            mem,
            view::status(&l.snap, false).warn,
            self.settings.borrow().theme == ThemeSetting::Neutral,
        );
        if t.key.get() != Some(key)
            && let Ok(icon) = Icon::from_rgba(tray::render(&key), key.size, key.size)
            && t.icon.set_icon(Some(icon)).is_ok()
        {
            t.key.set(Some(key));
        }
        let tip = view::tooltip(&l.snap);
        if *t.tip.borrow() != tip && t.icon.set_tooltip(Some(&tip)).is_ok() {
            *t.tip.borrow_mut() = tip;
        }
    }

    /// Shows the saved settings on the Settings screen, the footer and the tray menu.
    fn sync_settings_ui(&self) {
        let s = self.settings.borrow();
        let ui = &self.ui;
        ui.global::<Theme>()
            .set_neutral(s.theme == ThemeSetting::Neutral);
        ui.set_sort_cpu(s.sort == Sort::Processor);
        ui.set_keep_open(s.keep_open);
        ui.set_refresh(refresh_index(s.hidden_refresh_s));
        ui.set_hotkey_text(
            if s.hotkey.is_empty() {
                "Off".to_owned()
            } else {
                s.hotkey.clone()
            }
            .into(),
        );
        let shown = if self.hotkey_ok.get() { &s.hotkey } else { "" };
        let (cap, foot) = view::footer_hint(shown);
        ui.set_cap(cap.into());
        ui.set_foot(foot.into());
        if let Some(t) = self.tray.borrow().as_ref() {
            t.keep.set_checked(s.keep_open);
        }
    }

    fn save(&self) {
        let _ = self.settings.borrow().save();
    }

    fn set_note(&self, text: &str, warn: bool) {
        self.ui.set_hotkey_note(text.into());
        self.ui.set_hotkey_warn(warn);
    }

    fn set_sort(&self, sort: Sort) {
        self.settings.borrow_mut().sort = sort;
        self.save();
        self.sync_settings_ui();
        self.apply_latest();
    }

    fn set_theme(&self, theme: ThemeSetting) {
        self.settings.borrow_mut().theme = theme;
        self.save();
        self.sync_settings_ui();
        self.update_tray();
    }

    fn set_keep_open(&self, v: bool) {
        self.settings.borrow_mut().keep_open = v;
        self.save();
        self.sync_settings_ui();
    }

    fn toggle_keep_open(&self) {
        let v = !self.settings.borrow().keep_open;
        self.set_keep_open(v);
    }

    /// Asks Windows, then shows what is true now (the Run key can refuse).
    fn set_autostart(&self, want: bool) {
        let _ = autostart::set(want);
        self.settings.borrow_mut().autostart = want;
        self.save();
        self.show_real_autostart();
    }

    fn toggle_autostart(&self) {
        let v = !autostart::is_enabled();
        self.set_autostart(v);
    }

    fn show_real_autostart(&self) {
        let real = autostart::is_enabled();
        self.ui.set_autostart(real);
        if let Some(t) = self.tray.borrow().as_ref() {
            t.auto.set_checked(real);
        }
    }

    fn set_refresh(&self, i: i32) {
        let secs = usize::try_from(i)
            .ok()
            .and_then(|i| REFRESH_CHOICES.get(i))
            .copied()
            .unwrap_or(2);
        self.settings.borrow_mut().hidden_refresh_s = secs;
        self.refresh.store(secs, Ordering::Relaxed);
        self.save();
        self.sync_settings_ui();
    }

    fn open_settings(&self) {
        self.show_real_autostart();
        self.ui.set_settings_open(true);
    }

    fn close_settings(&self) {
        if self.recording.get() {
            self.stop_recording();
        }
        self.ui.set_settings_open(false);
    }

    /// The saved combination, if there is a valid one.
    fn saved_combo(&self) -> Option<Combo> {
        Combo::parse(&self.settings.borrow().hotkey)
    }

    fn start_recording(&self) {
        self.recording.set(true);
        self.ui.set_recording(true);
        self.set_note(record::RECORDING, false);
        // Let the current combination reach the recorder instead of toggling.
        let _ = self.hotkey.set(None);
    }

    /// Ends recording and puts the saved shortcut back.
    fn stop_recording(&self) {
        self.recording.set(false);
        self.ui.set_recording(false);
        self.mods.set(Mods::default());
        let ok = match self.saved_combo() {
            Some(c) => self.hotkey.set(Some(c)).is_ok(),
            None => false,
        };
        self.hotkey_ok.set(ok);
        self.sync_settings_ui();
    }

    fn cancel_recording(&self) {
        self.stop_recording();
        self.set_note("", false);
    }

    fn turn_off_shortcut(&self) {
        let _ = self.hotkey.set(None);
        self.settings.borrow_mut().hotkey.clear();
        self.hotkey_ok.set(false);
        self.save();
        self.set_note("", false);
        self.sync_settings_ui();
    }

    /// A key press while the panel has focus. True when it was used.
    fn on_key(&self, ev: &slint::winit_030::winit::event::KeyEvent) -> bool {
        if self.recording.get() {
            match record::step(self.mods.get(), pressed_from(ev)) {
                Step::Ignore => {}
                Step::Cancel => self.cancel_recording(),
                Step::NeedTwo => self.set_note(record::HOLD_TWO, true),
                Step::BadKey => self.set_note(record::BAD_KEY, true),
                Step::Try(combo) => self.try_combo(combo),
            }
            return true;
        }
        if ev.logical_key == Key::Named(NamedKey::Escape) {
            if self.ui.get_settings_open() {
                self.close_settings();
            } else {
                self.hide(false);
            }
            return true;
        }
        false
    }

    fn try_combo(&self, combo: Combo) {
        match self.hotkey.set(Some(combo)) {
            Ok(()) => {
                self.recording.set(false);
                self.ui.set_recording(false);
                self.mods.set(Mods::default());
                self.settings.borrow_mut().hotkey = combo.to_string();
                self.hotkey_ok.set(true);
                self.save();
                self.set_note("", false);
                self.sync_settings_ui();
            }
            Err(e) => {
                self.stop_recording();
                let text = match e {
                    HotkeyError::InUse => record::in_use(combo),
                    HotkeyError::Failed => record::FAILED.to_owned(),
                };
                self.set_note(&text, true);
            }
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
        self.avail_h.set(win::avail_height(h));
        self.apply_latest();
        win::place(h, self.layout.get().height());
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
        win::place(h, self.layout.get().height());
        // The footer's age counts up once a second, only while shown.
        self.ago
            .start(slint::TimerMode::Repeated, Duration::from_secs(1), || {
                with_panel(|p| p.update_ago())
            });
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
        self.ago.stop();
        self.close_settings();
        if from_blur {
            self.last_blur_hide.set(Some(Instant::now()));
        }
        win::hide(h);
        if self.trim {
            win::trim_working_set();
        }
    }

    /// Opens or closes the panel. A tray click just after a blur-hide is the
    /// click that caused the blur, so it is ignored (`guard`).
    fn toggle(&self, guard: bool) {
        if guard
            && self
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

type CachedRows = (Vec<AppRow>, Vec<AppRow>, Option<Wsl>);

fn spawn_reader(
    sample: bool,
    latest: Arc<Mutex<Option<Latest>>>,
    shown: Arc<AtomicBool>,
    refresh: Arc<AtomicU8>,
    rx: mpsc::Receiver<ReaderCmd>,
) {
    std::thread::spawn(move || {
        let mut src: Box<dyn Source> = if sample {
            Box::new(Sample::default())
        } else {
            pulse_readings::system()
        };
        let stress = std::env::var_os("PULSE_STRESS").is_some();
        let warn = std::env::var_os("PULSE_WARN").is_some();
        let mut hist = History::default();
        // Rows from the last scan, kept for readings taken while hidden.
        let mut rows: CachedRows = Default::default();
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
                snap.apps.clone_from(&rows.0);
                snap.apps_by_cpu.clone_from(&rows.1);
                snap.wsl.clone_from(&rows.2);
            } else {
                rows = (
                    snap.apps.clone(),
                    snap.apps_by_cpu.clone(),
                    snap.wsl.clone(),
                );
            }
            if warn {
                // A development knob: shows the warning colours.
                snap.cpu_pct = 92.0;
                snap.uptime_s = 9 * 86_400;
            }
            force_apps = false;
            hist.push(&snap);
            let mut extra = view::Extra::from_snapshot(&snap);
            if stress {
                extra = view::Extra::stress();
            }
            *latest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Latest {
                snap,
                extra,
                hist: hist.clone(),
                at: Instant::now(),
            });
            // The UI thread hears of every reading: the tray icon follows it
            // even while hidden; the panel itself only redraws while shown.
            let _ = slint::invoke_from_event_loop(|| with_panel(|p| p.on_reading()));
            let wait = if visible {
                TICK
            } else {
                Duration::from_secs(u64::from(refresh.load(Ordering::Relaxed).max(2)))
            };
            match rx.recv_timeout(wait) {
                Ok(ReaderCmd::Wake(f)) => force_apps = f,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
}

fn initial_tray_icon() -> Icon {
    let key = tray::Key::new(
        tray::size_for_dpi(win::system_dpi()),
        0.0,
        0.0,
        false,
        false,
    );
    Icon::from_rgba(tray::render(&key), key.size, key.size).expect("valid icon buffer")
}

#[allow(clippy::too_many_lines)]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = Config::from_env();
    let settings = Settings::load();

    // A second start opens the first one's panel and exits quietly.
    let Some(_instance) = single::Instance::acquire(|| {
        let _ = slint::invoke_from_event_loop(|| {
            with_panel(|p| {
                if !p.visible.get() {
                    p.show(true, None);
                }
            });
        });
    }) else {
        return Ok(());
    };
    if !cfg.test_run && settings.autostart != autostart::is_enabled() {
        let _ = autostart::set(settings.autostart);
    }

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
    ui.global::<Theme>()
        .set_animate(pulse_platform::animations_enabled());
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    let model = Rc::new(VecModel::<AppItem>::default());
    ui.set_apps(model.clone().into());

    let latest = Arc::new(Mutex::new(None));
    let shown = Arc::new(AtomicBool::new(false));
    let refresh = Arc::new(AtomicU8::new(settings.hidden_refresh_s));
    let (tx, rx) = mpsc::channel();
    spawn_reader(
        cfg.sample,
        latest.clone(),
        shown.clone(),
        refresh.clone(),
        rx,
    );

    // The global shortcut: registered from the saved settings; if it is
    // taken or invalid the app still starts and Settings says so.
    let hotkey = Listener::spawn(|| {
        let _ = slint::invoke_from_event_loop(|| with_panel(|p| p.toggle(false)));
    });
    let mut hotkey_note = None;
    let mut hotkey_ok = false;
    if !settings.hotkey.is_empty() {
        match Combo::parse(&settings.hotkey) {
            Some(c) => match hotkey.set(Some(c)) {
                Ok(()) => hotkey_ok = true,
                Err(HotkeyError::InUse) => hotkey_note = Some(record::in_use(c)),
                Err(HotkeyError::Failed) => hotkey_note = Some(record::FAILED.to_owned()),
            },
            None => {
                hotkey_note = Some("The saved shortcut is not valid. Choose a new one.".to_owned());
            }
        }
    }

    let measure: Measure = Arc::new(Mutex::new(None));
    let panel = Rc::new(Panel {
        ui,
        model,
        hwnd: Cell::new(None),
        visible: Cell::new(false),
        last_blur_hide: Cell::new(None),
        base_blur: !cfg.show && cfg.selftest_report.is_none(),
        sample: cfg.sample,
        latest,
        shown,
        reader: tx.clone(),
        measure: measure.clone(),
        icons: Icons::spawn(),
        trim: std::env::var_os("PULSE_NOTRIM").is_none(),
        avail_h: Cell::new(f32::MAX),
        layout: Cell::new(view::fit(f32::MAX, false)),
        settings: RefCell::new(settings.clone()),
        hotkey,
        hotkey_ok: Cell::new(hotkey_ok),
        recording: Cell::new(false),
        mods: Cell::new(Mods::default()),
        reading_at: Cell::new(Instant::now()),
        ago: slint::Timer::default(),
        refresh,
        tray: RefCell::new(None),
    });
    PANEL.with(|p| *p.borrow_mut() = Some(panel.clone()));
    panel.sync_settings_ui();
    panel.ui.set_autostart(settings.autostart);
    if let Some(note) = hotkey_note {
        panel.set_note(&note, true);
    }

    panel.ui.window().on_close_requested(|| {
        with_panel(|p| p.hide(false));
        CloseRequestResponse::KeepWindowShown
    });
    panel.ui.on_close_clicked(|| with_panel(|p| p.hide(false)));
    panel
        .ui
        .on_settings_shown(|| with_panel(|p| p.open_settings()));
    panel
        .ui
        .on_settings_hidden(|| with_panel(|p| p.close_settings()));
    panel.ui.on_sort_picked(|i| {
        with_panel(|p| {
            p.set_sort(if i == 1 {
                Sort::Processor
            } else {
                Sort::Memory
            });
        });
    });
    panel.ui.on_theme_picked(|i| {
        with_panel(|p| {
            p.set_theme(if i == 1 {
                ThemeSetting::Neutral
            } else {
                ThemeSetting::Mint
            });
        });
    });
    panel
        .ui
        .on_keep_toggled(|v| with_panel(|p| p.set_keep_open(v)));
    panel
        .ui
        .on_autostart_toggled(|v| with_panel(|p| p.set_autostart(v)));
    panel
        .ui
        .on_refresh_picked(|i| with_panel(|p| p.set_refresh(i)));
    panel.ui.on_change_clicked(|| {
        with_panel(|p| {
            if p.recording.get() {
                p.cancel_recording();
            } else {
                p.start_recording();
            }
        });
    });
    panel
        .ui
        .on_off_clicked(|| with_panel(|p| p.turn_off_shortcut()));

    // Tray: left click toggles, right click opens the menu.
    let menu = Menu::new();
    let open = MenuItem::new("Open Pulse", true, None);
    let keep = CheckMenuItem::new(
        "Keep open when I click away",
        true,
        settings.keep_open,
        None,
    );
    let auto = CheckMenuItem::new(
        "Start with Windows",
        true,
        if cfg.test_run {
            settings.autostart
        } else {
            autostart::is_enabled()
        },
        None,
    );
    let settings_item = MenuItem::new("Settings", true, None);
    let exit = MenuItem::new("Exit", true, None);
    menu.append_items(&[
        &open,
        &PredefinedMenuItem::separator(),
        &keep,
        &auto,
        &settings_item,
        &PredefinedMenuItem::separator(),
        &exit,
    ])?;
    let ids: [MenuId; 5] = [
        open.id().clone(),
        keep.id().clone(),
        auto.id().clone(),
        settings_item.id().clone(),
        exit.id().clone(),
    ];
    let _ = MenuEvent::set_event_handler(Some(Arc::new(move |e: MenuEvent| {
        let Some(which) = ids.iter().position(|id| *id == e.id) else {
            return;
        };
        let _ = slint::invoke_from_event_loop(move || {
            with_panel(|p| match which {
                0 => {
                    if !p.visible.get() {
                        p.show(true, None);
                    }
                }
                1 => p.toggle_keep_open(),
                2 => p.toggle_autostart(),
                3 => {
                    p.show(true, None);
                    p.open_settings();
                }
                _ => {
                    let _ = slint::quit_event_loop();
                }
            });
        });
    })));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = e
        {
            let _ = slint::invoke_from_event_loop(|| with_panel(|p| p.toggle(true)));
        }
    }));
    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("Pulse")
        .with_icon(initial_tray_icon())
        .build()?;
    *panel.tray.borrow_mut() = Some(TrayState {
        icon: tray_icon,
        key: Cell::new(None),
        tip: RefCell::default(),
        keep,
        auto,
    });

    // The native window exists once the event loop runs: finish setting it up
    // then, keep it hidden, and show it at once for PULSE_SHOW.
    panel.ui.window().show()?;
    let timer = Rc::new(slint::Timer::default());
    let t2 = timer.clone();
    let start_shown = cfg.show;
    let open_settings = cfg.open_settings;
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
                            if p.blur_hides() {
                                p.hide(true);
                            }
                        });
                        EventResult::Propagate
                    }
                    WindowEvent::ModifiersChanged(m) => {
                        let s = m.state();
                        with_panel(|p| {
                            p.mods.set(Mods {
                                ctrl: s.control_key(),
                                alt: s.alt_key(),
                                shift: s.shift_key(),
                            });
                        });
                        EventResult::Propagate
                    }
                    WindowEvent::KeyboardInput { event, .. }
                        if event.state == ElementState::Pressed && !event.repeat =>
                    {
                        let mut used = false;
                        with_panel(|p| used = p.on_key(event));
                        if used {
                            EventResult::PreventDefault
                        } else {
                            EventResult::Propagate
                        }
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
                if open_settings {
                    p.open_settings();
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
