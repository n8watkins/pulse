//! Graphics, drive, network and battery readings, Windows only.

use crate::drives::Drives;
use crate::net::Counters;
use crate::{Battery, Drive};
use core::ffi::c_void;
use core::ptr::null_mut;
use std::time::{Duration, Instant};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2,
};
use windows_sys::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

/// The battery changes slowly: read it at most this often (drives keep their
/// own pace, see `drives.rs`).
const SLOW: Duration = Duration::from_secs(30);
/// A rate over a shorter span than this is noise: keep the previous figure.
const MIN_SPAN: Duration = Duration::from_millis(250);

/// The per-process 3D engine counter, one instance per (process, engine).
///
/// Added once and never rebuilt: PDH re-expands the wildcard on every collect.
/// Observed on this PC (Windows 11, 2026-11-25) with a query held open while a
/// process created a Direct3D 11 device and released it again: the instance
/// of the new process appeared in the same sample and vanished again after the
/// release, and the instance count matched a freshly built query on every
/// sample. (A rebuild every 15 reads only dropped a reading and the history.)
const GPU_COUNTER: &str = r"\GPU Engine(*engtype_3D)\Utilization Percentage";

const IF_TYPE_ETHERNET: u32 = 6;
const IF_TYPE_WIFI: u32 = 71;
const IF_OPER_UP: i32 = 1;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// A PDH query with the counters added to it.
pub(crate) struct Query {
    handle: *mut c_void,
    counters: Vec<*mut c_void>,
}

impl Query {
    pub(crate) fn new(paths: &[&str]) -> Option<Self> {
        let mut handle: *mut c_void = null_mut();
        // SAFETY: valid out pointers; the query is closed on drop.
        unsafe {
            if PdhOpenQueryW(core::ptr::null(), 0, &mut handle) != 0 {
                return None;
            }
            let mut q = Self {
                handle,
                counters: Vec::new(),
            };
            for p in paths {
                let w = wide(p);
                let mut c: *mut c_void = null_mut();
                if PdhAddEnglishCounterW(handle, w.as_ptr(), 0, &mut c) != 0 {
                    return None;
                }
                q.counters.push(c);
            }
            PdhCollectQueryData(handle);
            Some(q)
        }
    }

    pub(crate) fn collect(&self) -> bool {
        // SAFETY: the handle is a live query.
        unsafe { PdhCollectQueryData(self.handle) == 0 }
    }

    /// One counter's value.
    pub(crate) fn value(&self, i: usize) -> Option<f64> {
        // SAFETY: a live counter and a valid out pointer.
        unsafe {
            let mut v: PDH_FMT_COUNTERVALUE = core::mem::zeroed();
            let r =
                PdhGetFormattedCounterValue(self.counters[i], PDH_FMT_DOUBLE, null_mut(), &mut v);
            (r == 0 && matches!(v.CStatus, 0 | 1)).then_some(v.Anonymous.doubleValue)
        }
    }

    /// Every instance of a wildcard counter: (name, value, valid).
    pub(crate) fn instances(&self, i: usize) -> Option<Vec<(String, f64, bool)>> {
        // SAFETY: the buffer is sized by the first call; items and the names
        // they point to stay inside it.
        unsafe {
            let (mut size, mut count) = (0u32, 0u32);
            let r = PdhGetFormattedCounterArrayW(
                self.counters[i],
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                null_mut(),
            );
            if r != PDH_MORE_DATA {
                return None;
            }
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let r = PdhGetFormattedCounterArrayW(
                self.counters[i],
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                buf.as_mut_ptr().cast(),
            );
            if r != 0 {
                return None;
            }
            let items = core::slice::from_raw_parts(
                buf.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>(),
                count as usize,
            );
            let base = buf.as_ptr() as usize;
            let end = base + buf.len() * 8;
            Some(
                items
                    .iter()
                    .map(|it| {
                        let mut name = String::new();
                        let at = it.szName as usize;
                        if at >= base && at < end {
                            let max = (end - at) / 2;
                            let units = core::slice::from_raw_parts(it.szName, max);
                            let n = units.iter().position(|&u| u == 0).unwrap_or(max);
                            name = String::from_utf16_lossy(&units[..n]);
                        }
                        (
                            name,
                            it.FmtValue.Anonymous.doubleValue,
                            matches!(it.FmtValue.CStatus, 0 | 1),
                        )
                    })
                    .collect(),
            )
        }
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: the handle is a live query, closed once.
        unsafe { PdhCloseQuery(self.handle) };
    }
}

// The handles are only used from the thread that owns the reader.
// SAFETY: PDH handles are not tied to a thread.
unsafe impl Send for Query {}

/// What one read adds to a snapshot.
#[derive(Default)]
pub(crate) struct Extra {
    pub gpu_pct: Option<f32>,
    pub drives: Vec<Drive>,
    pub net_down_bps: u64,
    pub net_up_bps: u64,
    pub battery: Option<Battery>,
}

pub(crate) struct Extras {
    gpu: Option<Query>,
    gpu_last: Option<f32>,
    drives: Drives,
    net: Option<(Instant, Counters)>,
    net_last: (u64, u64),
    battery: Option<(Instant, Option<Battery>)>,
}

impl Extras {
    pub fn new() -> Self {
        Self {
            gpu: Query::new(&[GPU_COUNTER]),
            gpu_last: None,
            drives: Drives::new(),
            net: None,
            net_last: (0, 0),
            battery: None,
        }
    }

    pub fn read(&mut self) -> Extra {
        let gpu_pct = self.gpu();
        let drives = self.drives.read();
        let (net_down_bps, net_up_bps) = self.network();
        Extra {
            gpu_pct,
            drives,
            net_down_bps,
            net_up_bps,
            battery: self.battery(),
        }
    }

    fn gpu(&mut self) -> Option<f32> {
        let q = self.gpu.as_ref()?;
        if !q.collect() {
            return self.gpu_last;
        }
        if let Some(items) = q.instances(0) {
            // No valid 3D engine at all means the PC does not report one.
            self.gpu_last =
                crate::gpu::busiest(items.iter().map(|(n, v, ok)| (n.as_str(), *v, *ok)))
                    .map(|v| v as f32);
        }
        self.gpu_last
    }

    /// Bytes per second since the previous read, over real adapters only.
    fn network(&mut self) -> (u64, u64) {
        let now = Instant::now();
        let Some(cur) = octets() else {
            return self.net_last;
        };
        match &self.net {
            Some((t, prev)) => {
                let dt = now.duration_since(*t);
                if dt < MIN_SPAN {
                    return self.net_last;
                }
                let (down, up) = crate::net::delta(prev, &cur);
                let secs = dt.as_secs_f64();
                self.net_last = ((down as f64 / secs) as u64, (up as f64 / secs) as u64);
            }
            None => self.net_last = (0, 0),
        }
        self.net = Some((now, cur));
        self.net_last
    }

    fn battery(&mut self) -> Option<Battery> {
        if let Some((t, b)) = &self.battery
            && t.elapsed() < SLOW
        {
            return b.clone();
        }
        // SAFETY: a zeroed plain-data struct and a valid out pointer.
        let b = unsafe {
            let mut s: SYSTEM_POWER_STATUS = core::mem::zeroed();
            // BatteryFlag 128 is "no system battery", a percent of 255 "unknown".
            (GetSystemPowerStatus(&mut s) != 0
                && s.BatteryFlag & 128 == 0
                && s.BatteryLifePercent <= 100)
                .then_some(Battery {
                    pct: s.BatteryLifePercent,
                    plugged: s.ACLineStatus == 1,
                })
        };
        self.battery = Some((Instant::now(), b.clone()));
        b
    }
}

/// Lifetime bytes (received, sent) of each real adapter, by `InterfaceLuid`.
/// Real means Ethernet or Wi-Fi that is up, hardware, with a connector, and not
/// a filter interface (virtual adapters and filters would count the same
/// traffic twice).
fn octets() -> Option<Counters> {
    // SAFETY: the table is read inside its NumEntries and freed once.
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = null_mut();
        if GetIfTable2(&mut table) != 0 || table.is_null() {
            return None;
        }
        let rows = (&raw const (*table).Table).cast::<MIB_IF_ROW2>();
        let mut out = Counters::new();
        for i in 0..(*table).NumEntries as usize {
            let r = &*rows.add(i);
            let flags = r.InterfaceAndOperStatusFlags._bitfield;
            let hardware = flags & 1 != 0;
            let filter = flags & 2 != 0;
            let connector = flags & 4 != 0;
            if (r.Type == IF_TYPE_ETHERNET || r.Type == IF_TYPE_WIFI)
                && r.OperStatus == IF_OPER_UP
                && hardware
                && connector
                && !filter
            {
                out.insert(r.InterfaceLuid.Value, (r.InOctets, r.OutOctets));
            }
        }
        FreeMibTable(table.cast());
        Some(out)
    }
}
