//! Executable icons and version-info names, Windows only.

use crate::IconImage;
use core::ffi::c_void;
use core::ptr::null_mut;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, ReleaseDC,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::Shell::{
    ExtractIconExW, SHDefExtractIconW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Full path of the executable of one process, or `None` when it cannot be opened.
pub(crate) fn process_path(pid: u32) -> Option<String> {
    // SAFETY: plain calls with valid buffers; the handle is closed.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// The `FileDescription` of an executable's version info, when it has one.
pub(crate) fn file_description(path: &str) -> Option<String> {
    let p = wide(path);
    // SAFETY: buffers are sized by the API's own answers; pointers returned by
    // VerQueryValueW point into `data`, which outlives their use.
    unsafe {
        let size = GetFileVersionInfoSizeW(p.as_ptr(), null_mut());
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(p.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let query = |sub: &str| -> Option<(*const c_void, u32)> {
            let s = wide(sub);
            let mut ptr: *mut c_void = null_mut();
            let mut len = 0u32;
            (VerQueryValueW(data.as_ptr().cast(), s.as_ptr(), &mut ptr, &mut len) != 0
                && !ptr.is_null()
                && len > 0)
                .then_some((ptr.cast_const(), len))
        };
        let (tr, tlen) = query("\\VarFileInfo\\Translation")?;
        let pairs = (tlen as usize) / 4;
        let mut langs: Vec<(u16, u16)> = (0..pairs)
            .map(|i| {
                let q = tr.cast::<u16>().add(i * 2);
                (q.read_unaligned(), q.add(1).read_unaligned())
            })
            .collect();
        langs.push((0x0409, 0x04b0));
        langs.push((0x0409, 0x04e4));
        for (lang, page) in langs {
            let key = format!("\\StringFileInfo\\{lang:04x}{page:04x}\\FileDescription");
            if let Some((v, n)) = query(&key) {
                let units = core::slice::from_raw_parts(v.cast::<u16>(), n as usize);
                let text = String::from_utf16_lossy(units);
                let text = text.trim_end_matches('\0').trim();
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
        None
    }
}

/// Reads a bitmap as top-down 32-bit BGRA.
///
/// # Safety
/// `bmp` must be a valid GDI bitmap not selected into a device context.
unsafe fn bgra(bmp: *mut c_void, w: i32, h: i32) -> Option<Vec<u8>> {
    // SAFETY: per the contract above; the buffer is w*h*4 bytes.
    unsafe {
        let mut bi: BITMAPINFO = core::mem::zeroed();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..core::mem::zeroed()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(null_mut());
        let lines = GetDIBits(
            dc,
            bmp,
            0,
            h as u32,
            px.as_mut_ptr().cast(),
            &mut bi,
            DIB_RGB_COLORS,
        );
        ReleaseDC(null_mut(), dc);
        (lines != 0).then_some(px)
    }
}

/// Initialises COM (single-threaded apartment) on this thread; true when the
/// call must be balanced with [`com_uninit`].
pub(crate) fn com_init() -> bool {
    // SAFETY: balanced by `com_uninit` when this returns true.
    unsafe { CoInitializeEx(core::ptr::null(), COINIT_APARTMENTTHREADED as u32) >= 0 }
}

pub(crate) fn com_uninit() {
    // SAFETY: only called after a successful `com_init` on this thread.
    unsafe { CoUninitialize() };
}

/// How one attempt at an icon ended.
enum Once {
    Done(IconImage),
    /// The shell failed this time (load, a busy shell); a retry may work.
    Transient,
    /// Nothing will ever come of this path (no file, no colour bitmap).
    Never,
}

/// The icon of an executable as straight RGBA, resized to `size` x `size`.
pub(crate) fn rgba(path: &str, size: u32) -> Option<IconImage> {
    if size == 0 || !std::path::Path::new(path).is_file() {
        return None;
    }
    let p = wide(path);
    // The shell is flaky on a thread that never initialised COM, and under
    // load a first call can fail: initialise COM for the call (a cheap no-op
    // on a thread that did so already) and retry the transient failure.
    let com = com_init();
    let mut out = None;
    for attempt in 0..3 {
        match rgba_once(&p, size) {
            Once::Done(img) => {
                out = Some(img);
                break;
            }
            Once::Never => break,
            Once::Transient => {}
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
    }
    if com {
        com_uninit();
    }
    out
}

/// The icon handle at `size` px: the file's first icon resource asked for at
/// exactly that size, else the shell's (which is the generic application icon
/// for a file with none), else the first resource at its own size.
///
/// # Safety
/// `p` must be a NUL-terminated wide string.
unsafe fn icon_handle(p: &[u16], size: u32) -> *mut c_void {
    // SAFETY: per the contract above; zeroed plain data.
    unsafe {
        let mut exact: *mut c_void = null_mut();
        if SHDefExtractIconW(p.as_ptr(), 0, 0, &mut exact, null_mut(), size) >= 0
            && !exact.is_null()
        {
            return exact;
        }
        let mut fi: SHFILEINFOW = core::mem::zeroed();
        let r = SHGetFileInfoW(
            p.as_ptr(),
            0,
            &mut fi,
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if r != 0 && !fi.hIcon.is_null() {
            return fi.hIcon;
        }
        let mut large: *mut c_void = null_mut();
        if ExtractIconExW(p.as_ptr(), 0, &mut large, null_mut(), 1) >= 1 && !large.is_null() {
            return large;
        }
        null_mut()
    }
}

fn rgba_once(p: &[u16], size: u32) -> Once {
    // SAFETY: zeroed plain-data structs; every GDI object made here is deleted.
    unsafe {
        let hicon = icon_handle(p, size);
        if hicon.is_null() {
            return Once::Transient;
        }
        let mut ii: ICONINFO = core::mem::zeroed();
        let got = GetIconInfo(hicon, &mut ii);
        let out = if got == 0 {
            Once::Transient
        } else if ii.hbmColor.is_null() {
            // A monochrome icon: there is no colour bitmap to read.
            Once::Never
        } else {
            let mut b: BITMAP = core::mem::zeroed();
            GetObjectW(ii.hbmColor, size_of::<BITMAP>() as i32, (&raw mut b).cast());
            let (w, h) = (b.bmWidth, b.bmHeight);
            if w <= 0 || h <= 0 {
                Once::Never
            } else {
                let mut px = bgra(ii.hbmColor, w, h);
                if let Some(px) = px.as_mut()
                    && px.chunks_exact(4).all(|c| c[3] == 0)
                {
                    // No alpha channel in the colour bitmap: use the mask
                    // (black = opaque).
                    let mask = if ii.hbmMask.is_null() {
                        None
                    } else {
                        bgra(ii.hbmMask, w, h)
                    };
                    for (i, c) in px.chunks_exact_mut(4).enumerate() {
                        let opaque = mask.as_ref().is_none_or(|m| m[i * 4] == 0);
                        c[3] = if opaque { 255 } else { 0 };
                    }
                }
                px.map_or(Once::Transient, |px| {
                    Once::Done(IconImage {
                        width: size,
                        height: size,
                        rgba: crate::scale::bgra_to_rgba(&px, w as u32, h as u32, size),
                    })
                })
            }
        };
        if got != 0 {
            if !ii.hbmColor.is_null() {
                DeleteObject(ii.hbmColor);
            }
            if !ii.hbmMask.is_null() {
                DeleteObject(ii.hbmMask);
            }
        }
        DestroyIcon(hicon);
        out
    }
}
