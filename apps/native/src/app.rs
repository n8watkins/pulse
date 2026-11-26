//! The tray app: window life cycle, tray icon and menu, settings, shortcut,
//! reader thread and test hooks.

use crate::logic::{self, ClickAction, Pending, PendingAction, Throttle, Toggle};
use crate::record::{self, Mods, Pressed, Step};
use crate::tray;
use crate::view::{self, History, RowKind};
use crate::win::{self, Hwnd};
use pulse_platform::hotkey::{Combo, Listener};
use pulse_platform::{Settings, Sort, Theme as ThemeSetting, autostart, single};
use pulse_readings::{AppRow, IconImage, Sample, Snapshot, Source, Wsl, icon_rgba};
use slint::winit_030::winit::dpi::PhysicalPosition;
use slint::winit_030::winit::event::{ElementState, WindowEvent};
use slint::winit_030::winit::keyboard::{Key, NamedKey, PhysicalKey};
use slint::winit_030::winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use slint::winit_030::winit::platform::windows::WindowAttributesExtWindows;
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
/// The tooltip is refreshed when the pointer is over the tray icon, at most this often.
const TIP_EVERY: Duration = Duration::from_secs(1);
/// The choices of "Refresh while hidden", in seconds.
const REFRESH_CHOICES: [u8; 3] = [2, 5, 10];

struct Config {
    sample: bool,
    show: bool,
    theme: i32,
    selftest_report: Option<String>,
    /// PULSE_OPEN=settings: start with the Settings screen open.
    open_settings: bool,
    /// PULSE_DEMO_SHORTCUT=recording|keys|taken|failed|invalid: show that state of the
    /// shortcut field without any key press (for screenshots).
    demo_shortcut: Option<String>,
    /// Any of PULSE_SAMPLE, PULSE_SHOW, PULSE_SELFTEST, PULSE_SETTINGS_DIR or
    /// PULSE_INSTANCE_NAME is set: a test run, which never touches the Run key.
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
            demo_shortcut: std::env::var("PULSE_DEMO_SHORTCUT")
                .ok()
                .filter(|v| !v.is_empty()),
            test_run: set("PULSE_SAMPLE")
                || set("PULSE_SHOW")
                || set("PULSE_SELFTEST")
                || set("PULSE_SETTINGS_DIR")
                || set("PULSE_INSTANCE_NAME"),
        }
    }
}

/// Newest reading, kept by the reader thread whether or not the panel is shown.
struct Latest {
    snap: Snapshot,
    extra: view::Extra,
    hist: History,
    at: Instant,
    /// When the app rows in `snap` were scanned.
    apps_at: Instant,
}

enum ReaderCmd {
    /// Read now. The flag forces the per-process scan even while hidden.
    Wake(bool),
}

type Measure = Arc<Mutex<Option<(Instant, Sender<f64>)>>>;

/// Executable icons: `icon_rgba` runs on a worker thread (it touches the shell
/// and can be slow). A found icon is kept for the session; a failed lookup is
/// asked again no sooner than `ICON_RETRY_AFTER` later, `ICON_TRIES` tries in all,
/// before the letter stays.
struct Icons {
    /// Lookups the worker has finished and the UI has not taken yet.
    done: Arc<Mutex<HashMap<String, Option<IconImage>>>>,
    /// Paths with a request in flight (cleared when its result is taken).
    pending: RefCell<HashSet<String>>,
    /// Lookups started per path, and when the last one started.
    tries: RefCell<HashMap<String, (u8, Instant)>>,
    /// Slint images built from found icons, on the UI thread.
    images: RefCell<HashMap<String, slint::Image>>,
    to_worker: Sender<String>,
}

impl Icons {
    fn spawn() -> Self {
        let done: Arc<Mutex<HashMap<String, Option<IconImage>>>> = Arc::default();
        let (to_worker, rx) = mpsc::channel::<String>();
        let shared = done.clone();
        std::thread::spawn(move || {
            // COM once for the worker's life: `icon_rgba` then only re-enters it.
            let _com = pulse_readings::IconThread::init();
            while let Ok(path) = rx.recv() {
                let img = icon_rgba(&path, win::icon_px());
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
            pending: RefCell::default(),
            tries: RefCell::default(),
            images: RefCell::default(),
            to_worker,
        }
    }

    /// The icon if it is ready; otherwise asks the worker (again, if an earlier
    /// try failed and the policy allows) and returns `None`.
    fn get(&self, path: &str) -> Option<slint::Image> {
        if let Some(i) = self.images.borrow().get(path) {
            return Some(i.clone());
        }
        let result = self
            .done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(path);
        if let Some(found) = result {
            // The request is answered: it is no longer in flight.
            self.pending.borrow_mut().remove(path);
            if let Some(img) = found {
                let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    &img.rgba, img.width, img.height,
                );
                let image = slint::Image::from_rgba8(buf);
                self.images
                    .borrow_mut()
                    .insert(path.to_owned(), image.clone());
                return Some(image);
            }
        }
        if !self.pending.borrow().contains(path) {
            let (n, since) = self
                .tries
                .borrow()
                .get(path)
                .map_or((0, None), |&(n, at)| (n, Some(at.elapsed())));
            if logic::icon_try_due(n, since) {
                self.tries
                    .borrow_mut()
                    .insert(path.to_owned(), (n + 1, Instant::now()));
                self.pending.borrow_mut().insert(path.to_owned());
                let _ = self.to_worker.send(path.to_owned());
            }
        }
        None
    }
}

struct TrayState {
    icon: TrayIcon,
    /// What the icon currently shows; it is redrawn only when this changes.
    key: Cell<Option<tray::Key>>,
    /// The tooltip text last set; refreshed while the pointer is over the icon.
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
    /// The message under the shortcut field (empty: the usual help line) and
    /// whether it is a warning.
    note_msg: RefCell<String>,
    note_warn: Cell<bool>,
    reading_at: Cell<Instant>,
    /// Counts the footer's "Updated Ns ago"; runs only while the panel is shown.
    ago: slint::Timer,
    /// Seconds between readings while hidden, read by the reader thread.
    refresh: Arc<AtomicU8>,
    /// The Neutral theme is on (the tray icon's colours), read by the reader thread.
    neutral: Arc<AtomicBool>,
    tray: RefCell<Option<TrayState>>,
    /// At mouse-down on the tray icon: the panel counted as open.
    click_was_open: Cell<bool>,
}

/// A second start or `--quit` that arrived before the window existed.
static PENDING: Pending = Pending::new();
/// Throttles the tooltip refresh while the pointer moves over the tray icon.
static TIP_THROTTLE: Throttle = Throttle::new(TIP_EVERY);

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

/// What one key press means to the recorder: the physical key (so digit rows
/// and non-Latin layouts work) plus the layout's letter, see
/// [`record::pressed_from_code`].
fn pressed_from(ev: &slint::winit_030::winit::event::KeyEvent) -> Pressed {
    let PhysicalKey::Code(code) = ev.physical_key else {
        return Pressed::Other;
    };
    let latin = match ev.key_without_modifiers() {
        Key::Character(s) => s.chars().next().filter(char::is_ascii_alphabetic),
        _ => None,
    };
    // winit's `KeyCode` names are the variant names (`KeyA`, `Digit1`, `F5`).
    record::pressed_from_code(&format!("{code:?}"), latin)
}

/// Applies a request that arrived before the window existed, once it does.
fn apply_pending() {
    with_panel(|p| match PENDING.take(p.hwnd.get().is_some()) {
        Some(PendingAction::Quit) => {
            let _ = slint::quit_event_loop();
        }
        Some(PendingAction::Show) => p.present(),
        None => {}
    });
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

    fn is_foreground(&self) -> bool {
        self.hwnd.get().is_some_and(win::is_foreground)
    }

    fn blur_hides(&self) -> bool {
        self.base_blur && !self.settings.borrow().keep_open
    }

    fn apply_latest(&self) {
        let guard = self
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(l) = guard.as_ref() else { return };
        let lay = view::fit(self.avail_h.get(), l.extra.battery.is_some());
        let sort = self.settings.borrow().sort;
        let v = view::build(
            &l.snap,
            &l.extra,
            &l.hist,
            self.sample,
            sort,
            Instant::now(),
            l.apps_at,
        );
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
            ui.set_bat_low(b.low);
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
                        RowKind::Reading => 3,
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

    /// A new reading while the panel is shown: the panel and the tray follow it.
    fn on_reading(&self) {
        self.update_tray();
        if self.visible.get() {
            self.apply_latest();
        }
    }

    /// Redraws the tray icon when its picture changes. The tooltip is not
    /// touched here (see `refresh_tooltip`).
    fn update_tray(&self) {
        let guard = self
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(l) = guard.as_ref() else { return };
        let tray = self.tray.borrow();
        let Some(t) = tray.as_ref() else { return };
        let key = tray::key_for(
            &l.snap,
            win::system_dpi(),
            self.settings.borrow().theme == ThemeSetting::Neutral,
        );
        if t.key.get() != Some(key)
            && let Ok(icon) = Icon::from_rgba(tray::render(&key), key.size, key.size)
            && t.icon.set_icon(Some(icon)).is_ok()
        {
            t.key.set(Some(key));
        }
    }

    /// Puts the newest figures into the tooltip, when the pointer is over the
    /// tray icon (the text includes network rates, so it is not kept fresh on
    /// every reading).
    fn refresh_tooltip(&self) {
        let guard = self
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(l) = guard.as_ref() else { return };
        let tray = self.tray.borrow();
        let Some(t) = tray.as_ref() else { return };
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
        self.neutral
            .store(s.theme == ThemeSetting::Neutral, Ordering::Relaxed);
        ui.set_sort_cpu(s.sort == Sort::Processor);
        ui.set_keep_open(s.keep_open);
        ui.set_refresh(refresh_index(s.hidden_refresh_s));
        let shown = if self.hotkey_ok.get() { &s.hotkey } else { "" };
        let (cap, foot) = view::footer_hint(shown);
        ui.set_cap(cap.into());
        ui.set_foot(foot.into());
        ui.set_foot_short(view::FOOT_SHORT.into());
        if let Some(t) = self.tray.borrow().as_ref() {
            t.keep.set_checked(s.keep_open);
        }
        drop(s);
        self.refresh_field();
    }

    /// Shows the shortcut field, its switch and the line under it.
    fn refresh_field(&self) {
        let s = self.settings.borrow();
        let on = s.shortcut_on();
        let recording = self.recording.get();
        let msg = self.note_msg.borrow();
        let warn = self.note_warn.get() && !msg.is_empty();
        let state = record::field(on, recording, warn);
        let keys = s.remembered_hotkey();
        let text = if recording {
            record::preview(self.mods.get())
        } else if keys.is_empty() {
            "Not set".to_owned()
        } else {
            Combo::parse(keys).map_or_else(|| keys.to_owned(), |c| c.to_string())
        };
        let ui = &self.ui;
        ui.set_shortcut_on(on);
        ui.set_key_text(text.into());
        ui.set_field_state(match state {
            record::Field::Idle => 0,
            record::Field::Off => 1,
            record::Field::Recording => 2,
            record::Field::Error => 3,
        });
        ui.set_key_hint(
            if recording {
                record::HINT_RECORDING
            } else {
                record::HINT_IDLE
            }
            .into(),
        );
        ui.set_hotkey_note(record::note(on, recording, &msg).into());
        ui.set_hotkey_warn(!msg.is_empty() && self.note_warn.get());
    }

    /// Saves, once more if the first try fails; Settings says so when it
    /// still fails, until a save works again.
    fn save(&self) {
        let ok = logic::save_with_retry(|| self.settings.borrow().save());
        self.ui.set_save_failed(!ok);
    }

    /// Sets the message under the shortcut field; empty text brings back the
    /// usual help line for the current state.
    fn set_note(&self, text: &str, warn: bool) {
        text.clone_into(&mut self.note_msg.borrow_mut());
        self.note_warn.set(warn);
        self.refresh_field();
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
        // Shown but not in front: nothing would ever blur it now.
        if logic::hide_on_keep_off(v, self.visible.get(), self.is_foreground(), self.base_blur) {
            self.hide(false);
        }
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

    /// Shows what the Run key says: on only when it starts this exe; when it
    /// starts another existing copy the switch is off and says so.
    fn show_real_autostart(&self) {
        let reg = autostart::registered();
        let this = reg == autostart::Registered::ThisCopy;
        self.ui.set_autostart(this);
        self.ui
            .set_autostart_other(reg == autostart::Registered::OtherCopy);
        if let Some(t) = self.tray.borrow().as_ref() {
            t.auto.set_checked(this);
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
            self.cancel_recording();
        }
        self.ui.set_settings_open(false);
    }

    /// The saved combination, if there is a valid one.
    fn saved_combo(&self) -> Option<Combo> {
        Combo::parse(&self.settings.borrow().hotkey)
    }

    fn start_recording(&self) {
        self.recording.set(true);
        self.mods.set(Mods::default());
        self.set_note("", false);
        // Let the current combination reach the recorder instead of toggling.
        let _ = self.hotkey.set(None);
    }

    /// Ends recording and puts the saved shortcut back. When that cannot be
    /// registered any more, returns the note to show (and the footer stops
    /// naming the shortcut).
    fn stop_recording(&self) -> Option<String> {
        self.recording.set(false);
        self.mods.set(Mods::default());
        let mut failed = None;
        let ok = match self.saved_combo() {
            Some(c) => match self.hotkey.set(Some(c)) {
                Ok(()) => true,
                Err(e) => {
                    failed = Some(record::error_note(e, c));
                    false
                }
            },
            None => false,
        };
        self.hotkey_ok.set(ok);
        self.sync_settings_ui();
        failed
    }

    fn cancel_recording(&self) {
        match self.stop_recording() {
            Some(note) => self.set_note(&note, true),
            None => self.set_note("", false),
        }
    }

    /// PULSE_DEMO_SHORTCUT: puts the shortcut field into one state for a
    /// screenshot. Registers nothing and reads no keys.
    fn demo_shortcut(&self, what: &str) {
        let saved = self.settings.borrow().remembered_hotkey().to_owned();
        let combo = Combo::parse(&saved).unwrap_or(Combo {
            ctrl: false,
            alt: true,
            shift: true,
            key: pulse_platform::hotkey::Key::Letter('P'),
        });
        match what {
            "recording" => {
                self.recording.set(true);
                self.set_note("", false);
            }
            "keys" => {
                self.recording.set(true);
                self.mods.set(Mods {
                    ctrl: false,
                    alt: true,
                    shift: true,
                });
                self.set_note(record::HOLD_TWO, true);
            }
            "taken" => self.set_note(&record::in_use(combo), true),
            "failed" => self.set_note(record::FAILED, true),
            "invalid" => self.set_note(record::NOT_VALID, true),
            _ => {}
        }
    }

    /// The shortcut's switch. Off keeps the combination for later; on
    /// registers the remembered one, and says why when that does not work.
    fn set_shortcut_enabled(&self, want: bool) {
        if self.recording.get() {
            self.cancel_recording();
        }
        if want {
            let text = self.settings.borrow().remembered_hotkey().to_owned();
            match Combo::parse(&text) {
                Some(c) => match self.hotkey.set(Some(c)) {
                    Ok(()) => {
                        self.settings.borrow_mut().set_shortcut(&c.to_string());
                        self.hotkey_ok.set(true);
                        self.save();
                        self.set_note("", false);
                    }
                    Err(e) => self.set_note(&record::error_note(e, c), true),
                },
                None => self.set_note(record::NOT_VALID, true),
            }
        } else {
            let _ = self.hotkey.set(None);
            self.settings.borrow_mut().switch_shortcut_off();
            self.hotkey_ok.set(false);
            self.save();
            self.set_note("", false);
        }
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
                Step::CtrlAlt => self.set_note(record::CTRL_ALT, true),
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
                self.mods.set(Mods::default());
                self.settings.borrow_mut().set_shortcut(&combo.to_string());
                self.hotkey_ok.set(true);
                self.save();
                self.set_note("", false);
                self.sync_settings_ui();
            }
            Err(e) => {
                let mut text = record::error_note(e, combo);
                if self.stop_recording().is_some() {
                    text.push_str(" Saved one is off too.");
                }
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

    /// Opens the panel, or brings an open one to the foreground.
    fn present(&self) {
        if !self.visible.get() {
            self.show(true, None);
        } else if let Some(h) = self.hwnd.get()
            && !win::is_foreground(h)
        {
            win::show_active(h);
        }
    }

    /// The global shortcut: opens, focuses an open panel that is not in
    /// front, or (a second press) closes it.
    fn on_shortcut(&self) {
        match logic::shortcut_action(self.visible.get(), self.is_foreground()) {
            Toggle::Open => self.show(true, None),
            Toggle::Focus => self.present(),
            Toggle::Close => self.hide(false),
        }
    }

    /// Mouse-down on the tray icon: note whether the panel counts as open.
    /// The blur that this click causes arrives before the click is released.
    fn click_down(&self) {
        let since = self.last_blur_hide.get().map(|t| t.elapsed());
        self.click_was_open.set(logic::open_at_click(
            self.visible.get(),
            since,
            REOPEN_GUARD,
        ));
    }

    /// Mouse-up on the tray icon: open or close, as decided at mouse-down.
    fn click_up(&self) {
        match logic::click_action(self.click_was_open.replace(false), self.visible.get()) {
            ClickAction::Open => self.present(),
            ClickAction::Close => self.hide(false),
            ClickAction::Nothing => {}
        }
    }
}

type CachedRows = (Vec<AppRow>, Vec<AppRow>, Option<Wsl>);

fn spawn_reader(
    sample: bool,
    latest: Arc<Mutex<Option<Latest>>>,
    shown: Arc<AtomicBool>,
    refresh: Arc<AtomicU8>,
    neutral: Arc<AtomicBool>,
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
        let no_apps = std::env::var_os("PULSE_NOAPPS").is_some();
        let battery = std::env::var("PULSE_BATTERY")
            .ok()
            .and_then(|v| logic::parse_battery(&v));
        let mut hist = History::default();
        // Rows from the last scan, kept for readings taken while hidden.
        let mut rows: CachedRows = Default::default();
        let mut force_apps = false;
        // When the rows were scanned (they are kept while hidden between scans).
        let mut apps_at = Instant::now();
        // The tray picture last sent to the UI thread.
        let mut tray_key: Option<tray::Key> = None;
        if sample {
            // Like the reference page, which starts with 60 samples, 2 s apart.
            let now = Instant::now();
            for i in 0..60u32 {
                let at = now.checked_sub(TICK * (60 - i)).unwrap_or(now);
                hist.push(&src.read(false), at);
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
                apps_at = Instant::now();
            }
            if no_apps {
                // A development knob: shows the "Reading apps" row.
                snap.apps.clear();
                snap.apps_by_cpu.clear();
                snap.wsl = None;
            }
            if let Some((pct, plugged)) = battery {
                // A development knob: a pretend battery.
                snap.battery = Some(pulse_readings::Battery { pct, plugged });
            }
            if stress {
                // A development knob: very long names everywhere.
                view::stress_snapshot(&mut snap);
            }
            if warn {
                // A development knob: shows the warning colours.
                snap.cpu_pct = 92.0;
                snap.uptime_s = 9 * 86_400;
            }
            force_apps = false;
            let now = Instant::now();
            hist.push(&snap, now);
            let key = tray::key_for(&snap, win::system_dpi(), neutral.load(Ordering::Relaxed));
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
                at: now,
                apps_at,
            });
            // Shown: the UI thread hears of every reading. Hidden: only of a
            // change of the tray picture, so a hidden Pulse wakes the UI
            // thread as rarely as it can.
            if visible {
                let _ = slint::invoke_from_event_loop(|| with_panel(|p| p.on_reading()));
            } else if tray_key != Some(key) {
                let _ = slint::invoke_from_event_loop(|| with_panel(|p| p.update_tray()));
            }
            tray_key = Some(key);
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
    // `--quit` (the installer): ask the running app to quit, do not start one.
    if std::env::args_os().any(|a| a == "--quit") {
        let gone = single::request_quit(Duration::from_secs(5));
        std::process::exit(i32::from(!gone));
    }
    if let Some(f) = win::scale_override() {
        // SAFETY: nothing else runs yet; this is the first thing the process does.
        unsafe { std::env::set_var("SLINT_SCALE_FACTOR", f.to_string()) };
    }
    let cfg = Config::from_env();
    let settings = Settings::load();

    // Owns the panel so it is never a taskbar or Alt+Tab window, not even
    // while Slint shows it once to create it (see `win::create_owner`).
    let owner = win::create_owner();
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(std::env::var("PULSE_RENDERER").unwrap_or_else(|_| "skia-software".into()))
        .with_winit_window_attributes_hook(move |a| {
            // Offscreen, never activated, fixed size, no taskbar button: the
            // real position and visibility come from `Panel::show`. Slint
            // makes the window visible itself when it is shown, so it cannot
            // be created invisible here (`with_visible(false)` is overridden
            // by `Window::show`); the timer below hides it as soon as it exists.
            let a = a
                .with_active(false)
                .with_resizable(false)
                .with_transparent(true)
                .with_skip_taskbar(true)
                .with_position(PhysicalPosition::new(-32000, -32000));
            if owner == 0 {
                a
            } else {
                a.with_owner_window(owner)
            }
        })
        .select()?;

    // One running copy. Taken once the backend is selected, so a second start
    // or `--quit` that arrives from here on is kept (`PENDING`) until the
    // window exists, not dropped. A second start opens the first one's panel
    // and exits quietly.
    let Some(instance) = single::Instance::acquire_with(
        || {
            PENDING.request_show();
            let _ = slint::invoke_from_event_loop(apply_pending);
        },
        // The same path as the tray's Exit: the loop ends and `run` removes the icon.
        || {
            PENDING.request_quit();
            let _ = slint::invoke_from_event_loop(apply_pending);
        },
    ) else {
        return Ok(());
    };
    // Start with Windows follows the setting only for the installed exe
    // (`Pulse.exe`; development builds are `pulse-native.exe`) and never in a
    // test run; it never takes the value over from another existing copy.
    let exe = std::env::current_exe().unwrap_or_default();
    if autostart::auto_sync_allowed(&exe, cfg.test_run) {
        let _ = autostart::sync(settings.autostart);
    }

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
    let neutral = Arc::new(AtomicBool::new(settings.theme == ThemeSetting::Neutral));
    let (tx, rx) = mpsc::channel();
    spawn_reader(
        cfg.sample,
        latest.clone(),
        shown.clone(),
        refresh.clone(),
        neutral.clone(),
        rx,
    );

    // The global shortcut: registered from the saved settings; if it is
    // taken or invalid the app still starts and Settings says so.
    let hotkey = Listener::spawn(|| {
        let _ = slint::invoke_from_event_loop(|| with_panel(|p| p.on_shortcut()));
    });
    let mut hotkey_note = None;
    let mut hotkey_ok = false;
    if settings.shortcut_on() {
        match Combo::parse(&settings.hotkey) {
            Some(c) => match hotkey.set(Some(c)) {
                Ok(()) => hotkey_ok = true,
                Err(e) => hotkey_note = Some(record::error_note(e, c)),
            },
            None => {
                hotkey_note = Some(record::NOT_VALID.to_owned());
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
        note_msg: RefCell::default(),
        note_warn: Cell::new(false),
        reading_at: Cell::new(Instant::now()),
        ago: slint::Timer::default(),
        refresh,
        neutral,
        tray: RefCell::new(None),
        click_was_open: Cell::new(false),
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
    panel.ui.on_field_clicked(|| {
        with_panel(|p| match record::click(p.recording.get()) {
            record::Click::Start => p.start_recording(),
            record::Click::Cancel => p.cancel_recording(),
        });
    });
    panel
        .ui
        .on_shortcut_toggled(|v| with_panel(|p| p.set_shortcut_enabled(v)));

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
                0 => p.present(),
                1 => p.toggle_keep_open(),
                2 => p.toggle_autostart(),
                3 => {
                    p.present();
                    p.open_settings();
                }
                _ => {
                    let _ = slint::quit_event_loop();
                }
            });
        });
    })));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| match e {
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Down,
            ..
        } => {
            let _ = slint::invoke_from_event_loop(|| with_panel(|p| p.click_down()));
        }
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } => {
            let _ = slint::invoke_from_event_loop(|| with_panel(|p| p.click_up()));
        }
        // The tooltip is brought up to date when the pointer reaches the icon.
        TrayIconEvent::Enter { .. } | TrayIconEvent::Move { .. }
            if TIP_THROTTLE.allow(Instant::now()) =>
        {
            let _ = slint::invoke_from_event_loop(|| with_panel(|p| p.refresh_tooltip()));
        }
        _ => {}
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
    let demo_shortcut = cfg.demo_shortcut.clone();
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
                            // Recording ends when the panel loses focus, and the
                            // saved shortcut comes back.
                            if p.recording.get() {
                                p.cancel_recording();
                            }
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
                            // The field shows the modifiers held so far.
                            if p.recording.get() {
                                p.refresh_field();
                            }
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
                    if let Some(demo) = demo_shortcut.as_deref() {
                        p.demo_shortcut(demo);
                    }
                }
                if let Some(report) = selftest.clone() {
                    spawn_selftest(report, selftest_tx.clone());
                }
                // A second start or `--quit` that came in while starting.
                apply_pending();
            });
        },
    );

    slint::run_event_loop_until_quit()?;
    // Take the panel apart here, while the other threads are alive: the tray
    // icon leaves the notification area, the shortcut listener and the single
    // instance thread are joined. Left to the thread-local's destructor at
    // process exit it would join them during teardown, where that can panic
    // (and a panic aborts). Then leave without running destructors.
    drop(panel.tray.borrow_mut().take());
    let in_tls = PANEL.with(|p| p.borrow_mut().take());
    drop(in_tls);
    drop(panel);
    drop(instance);
    std::process::exit(0);
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
