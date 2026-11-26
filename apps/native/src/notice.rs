//! Showing a notice to the user: the one place that talks to the system's
//! notification. Windows uses the tray icon's own notification (the
//! `Shell_NotifyIconW` balloon, which Windows 10 and 11 show as a toast and
//! which follows Do Not Disturb). A Mac or Linux version supplies its own
//! `show` and `on_click` here and nothing else changes.
//!
//! The notice comes from the tray icon, so it carries the app's own name
//! ("Pulse", from the exe's version details) and its icon.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{
    DefSubclassProc, NIF_INFO, NIIF_WARNING, NIM_MODIFY, NOTIFYICONDATAW, SetWindowSubclass,
    Shell_NotifyIconW,
};

/// The tray icon's own (hidden) window, which owns the notification.
static TARGET: AtomicIsize = AtomicIsize::new(0);
/// What a click on the notice does.
static CLICKED: OnceLock<fn()> = OnceLock::new();

/// The message the tray icon sends its window, as `tray-icon` registers it.
const WM_TRAYICON: u32 = 6002;
/// `NIN_BALLOONUSERCLICK`: the user clicked the notice.
const BALLOON_USER_CLICK: u32 = 0x405;
const SUBCLASS_ID: usize = 0x50_554c_5345;

/// Hooks the tray icon's window: from now on `show` has a window to use and
/// a click on the notice calls `on_click` (on the thread that runs the
/// window's messages, so keep it short and hand over to the event loop).
pub fn attach(tray_window: HWND, on_click: fn()) {
    let _ = CLICKED.set(on_click);
    // SAFETY: `tray_window` is the live window of the tray icon, which
    // outlives the subclass (both end with the process).
    let ok = unsafe { SetWindowSubclass(tray_window, Some(subclass), SUBCLASS_ID, 0) };
    if ok != 0 {
        TARGET.store(tray_window as isize, Ordering::Release);
    }
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // Without version 4 behavior the notification event is the whole lparam.
    if msg == WM_TRAYICON
        && lparam as u32 == BALLOON_USER_CLICK
        && let Some(f) = CLICKED.get()
    {
        f();
    }
    // SAFETY: the arguments are the ones this procedure was called with.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

fn fill<const N: usize>(dst: &mut [u16; N], text: &str) {
    // Room for the closing zero.
    for (slot, unit) in dst.iter_mut().take(N - 1).zip(text.encode_utf16()) {
        *slot = unit;
    }
}

/// Shows a notice with a warning symbol. False when it could not be sent
/// (no tray icon yet, or the notification area refused).
pub fn show(title: &str, body: &str) -> bool {
    let hwnd = TARGET.load(Ordering::Acquire) as HWND;
    if hwnd.is_null() {
        return false;
    }
    // `tray-icon` numbers its icons from 1 and keeps the number to itself;
    // Pulse has one icon, so the first number the shell accepts is ours.
    for uid in 1..=8u32 {
        let mut nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: uid,
            uFlags: NIF_INFO,
            dwInfoFlags: NIIF_WARNING,
            ..Default::default()
        };
        fill(&mut nid.szInfoTitle, title);
        fill(&mut nid.szInfo, body);
        // SAFETY: `nid` is fully initialised and lives through the call.
        if unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) } != 0 {
            return true;
        }
    }
    false
}
