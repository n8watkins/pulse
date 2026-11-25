//! Win32 details the toolkit does not cover: the tool-window style, showing
//! without activation, and placing the panel on the monitor under the cursor.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetCursorPos, GetWindowLongPtrW, HWND_TOPMOST, SW_HIDE, SW_SHOW,
    SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_EX_APPWINDOW,
    WS_EX_TOOLWINDOW,
};

/// Logical size of the panel and its distance from the work-area edges.
pub const PANEL_W: f32 = 380.0;
pub const PANEL_H: f32 = 700.0;
pub const MARGIN: f32 = 12.0;

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

pub fn show_active(h: Hwnd) {
    unsafe {
        ShowWindow(h.raw(), SW_SHOW);
        SetForegroundWindow(h.raw());
    }
}

pub fn hide(h: Hwnd) {
    unsafe {
        ShowWindow(h.raw(), SW_HIDE);
    }
}

/// Puts the panel bottom-right of the work area of the monitor under the
/// cursor, 12 px from the edges, at that monitor's scale. Height is clamped
/// to the work area.
pub fn place(h: Hwnd) {
    unsafe {
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&raw mut p);
        let mon = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &raw mut info) == 0 {
            return;
        }
        let work: RECT = info.rcWork;
        let (mut dx, mut dy) = (96u32, 96u32);
        if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &raw mut dx, &raw mut dy) != 0 {
            dx = GetDpiForWindow(h.raw()).max(96);
        }
        let scale = dx as f32 / 96.0;
        let margin = (MARGIN * scale).round() as i32;
        let w = (PANEL_W * scale).round() as i32;
        let max_h = (work.bottom - work.top) - 2 * margin;
        let height = ((PANEL_H * scale).round() as i32).min(max_h);
        let x = work.right - w - margin;
        let y = work.bottom - height - margin;
        SetWindowPos(h.raw(), HWND_TOPMOST, x, y, w, height, SWP_NOACTIVATE);
    }
}
