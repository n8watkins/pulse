//! The real reader for Windows: no process handles, no sysinfo.
//!
//! Processor from `GetSystemTimes` deltas, memory from `GlobalMemoryStatusEx`,
//! uptime from `GetTickCount64`, and the apps from one
//! `NtQuerySystemInformation(SystemProcessInformation)` call per read.

use crate::{AppRow, Snapshot, Source, group, icon};
use std::collections::HashMap;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::SystemInformation::{
    GetSystemInfo, GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX, SYSTEM_INFO,
};
use windows_sys::Win32::System::Threading::GetSystemTimes;

const SYSTEM_PROCESS_INFORMATION: u32 = 5;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(
        class: u32,
        info: *mut core::ffi::c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

// Offsets inside SYSTEM_PROCESS_INFORMATION on 64-bit Windows.
const OFF_NEXT: usize = 0;
const OFF_PRIVATE_WS: usize = 8;
const OFF_NAME_LEN: usize = 56;
const OFF_NAME_PTR: usize = 64;
const OFF_PID: usize = 80;
const MIN_ENTRY: usize = 88;

/// Shortest span (100 ns units, per core) a processor figure is taken over: 250 ms.
const MIN_SPAN: u64 = 2_500_000;

pub struct Windows {
    last: Option<(u64, u64)>,
    pct: f32,
    cores: u32,
    /// u64 storage so the buffer is 8-byte aligned.
    buf: Vec<u64>,
    /// Per lowercase exe name: the path found and the friendly name from its version info.
    known: HashMap<String, (String, Option<String>)>,
}

impl Windows {
    pub fn new() -> Self {
        // SAFETY: a valid out pointer.
        let cores = unsafe {
            let mut si: SYSTEM_INFO = core::mem::zeroed();
            GetSystemInfo(&mut si);
            si.dwNumberOfProcessors.max(1)
        };
        let mut w = Self {
            last: None,
            pct: 0.0,
            cores,
            buf: vec![0; (1 << 20) / 8],
            known: HashMap::new(),
        };
        w.cpu_pct();
        w
    }

    /// Whole-machine use since the previous call; 0 on the first call.
    fn cpu_pct(&mut self) -> f32 {
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut idle, mut kernel, mut user) = (zero, zero, zero);
        // SAFETY: three valid out pointers.
        let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
        if ok == 0 {
            return 0.0;
        }
        let t = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
        // Kernel time includes idle time.
        let (idle, total) = (t(idle), t(kernel) + t(user));
        let pct = match self.last {
            // Too short a window (a read right after another) is noise: keep
            // the earlier baseline and figure.
            Some((_, pt)) if (total.saturating_sub(pt)) / u64::from(self.cores) < MIN_SPAN => {
                return self.pct;
            }
            Some((pi, pt)) => {
                let dt = (total - pt) as f64;
                let di = idle.saturating_sub(pi) as f64;
                ((1.0 - di / dt) * 100.0).clamp(0.0, 100.0) as f32
            }
            None => 0.0,
        };
        self.last = Some((idle, total));
        self.pct = pct;
        pct
    }

    /// Adds the path of one process and the file's own description as the name
    /// (the built-in names win), cached per exe name once found.
    fn resolve(&mut self, mut row: AppRow, pids: &[u32]) -> AppRow {
        let key = row.exe.to_ascii_lowercase();
        if !self.known.contains_key(&key) {
            // Protected processes cannot be opened: try the group's others,
            // then the same name in System32 (svchost, dwm).
            let path = pids
                .iter()
                .find_map(|&p| icon::process_path(p))
                .or_else(|| {
                    let root = std::env::var("SystemRoot").ok()?;
                    let file = if row.exe.contains('.') {
                        row.exe.clone()
                    } else {
                        format!("{}.exe", row.exe)
                    };
                    let p = std::path::Path::new(&root).join("System32").join(file);
                    p.is_file().then(|| p.to_string_lossy().into_owned())
                });
            if let Some(path) = path {
                let desc = icon::file_description(&path);
                self.known.insert(key.clone(), (path, desc));
            }
        }
        if let Some((path, desc)) = self.known.get(&key) {
            row.path = Some(path.clone());
            if let Some(d) = desc
                && !group::is_builtin(&row.exe)
            {
                row.name.clone_from(d);
            }
        }
        row
    }

    /// One call for every process; returns (image name, private working set).
    fn processes(&mut self) -> Vec<(String, u64, u32)> {
        let mut returned = 0u32;
        for _ in 0..4 {
            let bytes = self.buf.len() * 8;
            // SAFETY: the buffer is `bytes` long and `returned` is a valid out pointer.
            let status = unsafe {
                NtQuerySystemInformation(
                    SYSTEM_PROCESS_INFORMATION,
                    self.buf.as_mut_ptr().cast(),
                    u32::try_from(bytes).unwrap_or(u32::MAX),
                    &mut returned,
                )
            };
            if status >= 0 {
                let bytes_ptr = self.buf.as_ptr().cast::<u8>();
                // SAFETY: the Vec<u64> owns `bytes` initialised bytes.
                let slice = unsafe { core::slice::from_raw_parts(bytes_ptr, bytes) };
                return parse(slice, bytes_ptr as usize);
            }
            if status != STATUS_INFO_LENGTH_MISMATCH {
                return Vec::new();
            }
            let want = (returned as usize + (256 << 10)).div_ceil(8);
            self.buf.resize(want.max(self.buf.len() * 2), 0);
        }
        Vec::new()
    }
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// Walks the linked entries in `buf`; `base` is the address `buf` starts at,
/// which the name pointers are relative to.
fn parse(buf: &[u8], base: usize) -> Vec<(String, u64, u32)> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + MIN_ENTRY <= buf.len() {
        let next = u32_at(buf, at + OFF_NEXT).unwrap_or(0) as usize;
        let private = u64_at(buf, at + OFF_PRIVATE_WS).unwrap_or(0);
        let len = buf
            .get(at + OFF_NAME_LEN..at + OFF_NAME_LEN + 2)
            .map_or(0, |b| usize::from(u16::from_le_bytes([b[0], b[1]])));
        let ptr = u64_at(buf, at + OFF_NAME_PTR).unwrap_or(0) as usize;
        let name = ptr
            .checked_sub(base)
            .and_then(|o| buf.get(o..o.checked_add(len)?))
            .map(|b| {
                let units: Vec<u16> = b
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16_lossy(&units)
            })
            .unwrap_or_default();
        let pid = u64_at(buf, at + OFF_PID).unwrap_or(0) as u32;
        out.push((name, private, pid));
        if next == 0 {
            break;
        }
        at += next;
    }
    out
}

impl Source for Windows {
    fn read(&mut self, with_apps: bool) -> Snapshot {
        let cpu_pct = self.cpu_pct();
        // SAFETY: zeroed plain-data structs with their size fields set.
        let (mem_used, mem_total) = unsafe {
            let mut m: MEMORYSTATUSEX = core::mem::zeroed();
            m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
            if GlobalMemoryStatusEx(&mut m) == 0 {
                (0, 0)
            } else {
                (
                    m.ullTotalPhys.saturating_sub(m.ullAvailPhys),
                    m.ullTotalPhys,
                )
            }
        };
        let cores = self.cores;
        // SAFETY: no arguments.
        let uptime_s = unsafe { GetTickCount64() } / 1000;
        let apps = if with_apps {
            let rows = group::top_apps(self.processes());
            rows.into_iter()
                .map(|(row, pids)| self.resolve(row, &pids))
                .collect()
        } else {
            Vec::new()
        };
        Snapshot {
            cpu_pct,
            cores,
            uptime_s,
            mem_used,
            mem_total,
            apps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_and_names_come_from_the_exe() {
        let path = r"C:\Windows\explorer.exe";
        let img = crate::icon_rgba(path, 32).expect("explorer has an icon");
        assert_eq!((img.width, img.height), (32, 32));
        assert_eq!(img.rgba.len(), 32 * 32 * 4);
        assert!(img.rgba.chunks_exact(4).any(|p| p[3] > 0));
        assert_eq!(
            icon::file_description(path).as_deref(),
            Some("Windows Explorer")
        );
    }

    #[test]
    fn reads_plausible_figures() {
        let mut w = Windows::new();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let s = w.read(true);
        assert!(s.cores >= 1);
        assert!(s.mem_total > 0 && s.mem_used <= s.mem_total);
        assert!((0.0..=100.0).contains(&s.cpu_pct));
        assert!(!s.apps.is_empty() && s.apps.len() <= group::TOP);
        assert!(s.apps.windows(2).all(|p| p[0].mem_bytes >= p[1].mem_bytes));
        assert!(w.read(false).apps.is_empty());
    }
}
