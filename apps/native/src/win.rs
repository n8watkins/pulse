//! Win32 details the toolkit does not cover: the tool-window style, showing
//! without activation, and placing the panel on the monitor under the cursor.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForMonitor, GetDpiForSystem, GetDpiForWindow, MDT_EFFECTIVE_DPI,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GWL_EXSTYLE, GetCursorPos, GetForegroundWindow, GetWindowLongPtrW,
    GetWindowThreadProcessId, HWND_TOPMOST, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

/// Logical size of the panel and its distance from the work-area edges.
pub const PANEL_W: f32 = 380.0;
pub const MARGIN: f32 = 12.0;

#[cfg(test)]
mod tests {
    use super::parse_scale;

    #[test]
    fn scale_override_parses_only_sane_factors() {
        assert_eq!(parse_scale(Some("1.5")), Some(1.5));
        assert_eq!(parse_scale(Some(" 1.25 ")), Some(1.25));
        assert_eq!(parse_scale(Some("2")), Some(2.0));
        for bad in ["", "0", "0.5", "5", "x", "NaN", "inf", "-1"] {
            assert_eq!(parse_scale(Some(bad)), None, "{bad:?}");
        }
        assert_eq!(parse_scale(None), None);
    }
}

#[derive(Clone, Copy)]
pub struct Hwnd(pub isize);

impl Hwnd {
    fn raw(self) -> HWND {
        self.0 as HWND
    }
}

/// `WS_EX_TOOLWINDOW` (never in Alt+Tab, no taskbar button) and no `WS_EX_APPWINDOW`.
pub fn style_tool_window(h: Hwnd) {
    unsafe {
        let style = GetWindowLongPtrW(h.raw(), GWL_EXSTYLE);
        let wanted = (style | WS_EX_TOOLWINDOW as isize) & !(WS_EX_APPWINDOW as isize);
        if wanted != style {
            SetWindowLongPtrW(h.raw(), GWL_EXSTYLE, wanted);
            SetWindowPos(
                h.raw(),
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

/// The panel draws its own rounded corners on a transparent window, so ask
/// Windows 11 not to add its own (smaller) rounding or border on top.
pub fn no_system_corners(h: Hwnd) {
    let pref: i32 = DWMWCP_DONOTROUND;
    unsafe {
        DwmSetWindowAttribute(
            h.raw(),
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&raw const pref).cast(),
            size_of::<i32>() as u32,
        );
    }
}

pub fn show_inactive(h: Hwnd) {
    unsafe {
        ShowWindow(h.raw(), SW_SHOWNOACTIVATE);
    }
}

/// True when `h` is the window that has the keyboard focus right now.
pub fn is_foreground(h: Hwnd) -> bool {
    unsafe { GetForegroundWindow() == h.raw() }
}

/// Shows the panel and makes it the foreground window, so Esc and a click
/// away reach it. Windows refuses `SetForegroundWindow` to a process that did
/// not just get input (a tray menu command, a second start); then the panel
/// borrows the foreground window's input queue for the call. No key presses
/// are synthesised: a lone Alt tap would put windows into menu mode.
pub fn show_active(h: Hwnd) {
    unsafe {
        ShowWindow(h.raw(), SW_SHOW);
        SetForegroundWindow(h.raw());
        if GetForegroundWindow() == h.raw() {
            return;
        }
        let front = GetForegroundWindow();
        let front_thread = if front.is_null() {
            0
        } else {
            GetWindowThreadProcessId(front, std::ptr::null_mut())
        };
        let mine = GetCurrentThreadId();
        let attached = front_thread != 0
            && front_thread != mine
            && AttachThreadInput(mine, front_thread, 1) != 0;
        BringWindowToTop(h.raw());
        SetForegroundWindow(h.raw());
        if attached {
            AttachThreadInput(mine, front_thread, 0);
        }
    }
}

pub fn hide(h: Hwnd) {
    unsafe {
        ShowWindow(h.raw(), SW_HIDE);
    }
}

/// The work area of the monitor under the cursor, and that monitor's scale.
fn work_area(h: Hwnd) -> Option<(RECT, f32)> {
    unsafe {
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&raw mut p);
        let mon = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &raw mut info) == 0 {
            return None;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &raw mut dx, &raw mut dy) != 0 {
            dx = GetDpiForWindow(h.raw()).max(96);
        }
        Some((info.rcWork, scale_override().unwrap_or(dx as f32 / 96.0)))
    }
}

/// Logical height the panel can use on the monitor under the cursor (work
/// area minus the margins). `PULSE_WORK_H` (logical px) overrides it, to
/// check a short display without changing the real one.
pub fn avail_height(h: Hwnd) -> f32 {
    if let Some(v) = std::env::var("PULSE_WORK_H")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        return v - 2.0 * MARGIN;
    }
    work_area(h).map_or(f32::MAX, |(work, scale)| {
        (work.bottom - work.top) as f32 / scale - 2.0 * MARGIN
    })
}

/// Puts the panel, `height` logical px tall, bottom-right of the work area of
/// the monitor under the cursor, 12 px from the edges, at that monitor's scale.
/// Height is clamped to the work area.
pub fn place(h: Hwnd, height: f32) {
    unsafe {
        let Some((work, scale)) = work_area(h) else {
            return;
        };
        let margin = (MARGIN * scale).round() as i32;
        let w = (PANEL_W * scale).round() as i32;
        let max_h = (work.bottom - work.top) - 2 * margin;
        let height = ((height * scale).round() as i32).min(max_h);
        let x = work.right - w - margin;
        let y = work.bottom - height - margin;
        SetWindowPos(h.raw(), HWND_TOPMOST, x, y, w, height, SWP_NOACTIVATE);
    }
}

/// `PULSE_SCALE=<factor>` (1.0 to 4.0): pretend every display is at this
/// scale, so a 150 percent layout can be drawn on a 100 percent monitor. A
/// development knob; the real display settings are never touched.
pub fn scale_override() -> Option<f32> {
    parse_scale(std::env::var("PULSE_SCALE").ok().as_deref())
}

fn parse_scale(v: Option<&str>) -> Option<f32> {
    v?.trim()
        .parse::<f32>()
        .ok()
        .filter(|f| f.is_finite() && (1.0..=4.0).contains(f))
}

/// The system DPI, which sets the tray icon's size (`PULSE_SCALE` overrides it).
pub fn system_dpi() -> u32 {
    scale_override().map_or_else(
        || unsafe { GetDpiForSystem() },
        |f| (96.0 * f).round() as u32,
    )
}

/// Pixel size to ask for the 24 px (logical) app icons at: the scaled size,
/// never below 32 (36 at 150 percent, 48 at 200 percent).
pub fn icon_px() -> u32 {
    let scale = system_dpi().max(96) as f32 / 96.0;
    ((24.0 * scale).round() as u32).max(32)
}

/// Hands the process's idle pages back to Windows (the same trim Windows
/// applies to a hidden webview), so a hidden panel counts for almost nothing.
pub fn trim_working_set() {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
    unsafe {
        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}
