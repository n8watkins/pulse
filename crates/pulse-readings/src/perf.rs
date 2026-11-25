//! Graphics, drive, network and battery readings, Windows only.

use crate::Battery;
use core::ffi::c_void;
use core::ptr::null_mut;
use std::time::{Duration, Instant};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2,
};
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows_sys::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

/// Space and battery change slowly: read them at most this often.
const SLOW: Duration = Duration::from_secs(30);
/// A rate over a shorter span than this is noise: keep the previous figure.
const MIN_SPAN: Duration = Duration::from_millis(250);
/// The graphics counter's instances are fixed when it is added: add it again
/// this often so new processes' engines show up.
const GPU_REFRESH_READS: u32 = 15;

const IF_TYPE_ETHERNET: u32 = 6;
const IF_TYPE_WIFI: u32 = 71;
const IF_OPER_UP: i32 = 1;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// A PDH query with the counters added to it.
struct Query {
    handle: *mut c_void,
    counters: Vec<*mut c_void>,
}

impl Query {
    fn new(paths: &[&str]) -> Option<Self> {
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

    fn collect(&self) -> bool {
        // SAFETY: the handle is a live query.
        unsafe { PdhCollectQueryData(self.handle) == 0 }
    }

    /// One counter's value.
    fn value(&self, i: usize) -> Option<f64> {
        // SAFETY: a live counter and a valid out pointer.
        unsafe {
            let mut v: PDH_FMT_COUNTERVALUE = core::mem::zeroed();
            let r =
                PdhGetFormattedCounterValue(self.counters[i], PDH_FMT_DOUBLE, null_mut(), &mut v);
            (r == 0 && matches!(v.CStatus, 0 | 1)).then_some(v.Anonymous.doubleValue)
        }
    }

    /// The sum of every instance of a wildcard counter, and how many there were.
    fn sum(&self, i: usize) -> Option<(f64, u32)> {
        // SAFETY: the buffer is sized by the first call; items stay inside it.
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
            let sum = items
                .iter()
                .filter(|it| matches!(it.FmtValue.CStatus, 0 | 1))
                .map(|it| it.FmtValue.Anonymous.doubleValue)
                .sum();
            Some((sum, count))
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
    pub disk_free: u64,
    pub disk_total: u64,
    pub disk_read_bps: u64,
    pub disk_write_bps: u64,
    pub net_down_bps: u64,
    pub net_up_bps: u64,
    pub battery: Option<Battery>,
}

pub(crate) struct Extras {
    gpu: Option<Query>,
    gpu_reads: u32,
    gpu_last: Option<f32>,
    disk: Option<Query>,
    disk_last: (u64, u64),
    net: Option<(Instant, u64, u64)>,
    net_last: (u64, u64),
    space: Option<(Instant, u64, u64)>,
    battery: Option<(Instant, Option<Battery>)>,
}

impl Extras {
    pub fn new() -> Self {
        Self {
            gpu: Query::new(&[r"\GPU Engine(*engtype_3D)\Utilization Percentage"]),
            gpu_reads: 0,
            gpu_last: None,
            disk: Query::new(&[
                r"\PhysicalDisk(_Total)\Disk Read Bytes/sec",
                r"\PhysicalDisk(_Total)\Disk Write Bytes/sec",
            ]),
            disk_last: (0, 0),
            net: None,
            net_last: (0, 0),
            space: None,
            battery: None,
        }
    }

    pub fn read(&mut self) -> Extra {
        let gpu_pct = self.gpu();
        let (disk_read_bps, disk_write_bps) = self.disk();
        let (net_down_bps, net_up_bps) = self.network();
        let (disk_free, disk_total) = self.space();
        Extra {
            gpu_pct,
            disk_free,
            disk_total,
            disk_read_bps,
            disk_write_bps,
            net_down_bps,
            net_up_bps,
            battery: self.battery(),
        }
    }

    fn gpu(&mut self) -> Option<f32> {
        self.gpu_reads += 1;
        if self.gpu_reads.is_multiple_of(GPU_REFRESH_READS) {
            self.gpu = Query::new(&[r"\GPU Engine(*engtype_3D)\Utilization Percentage"]);
            return self.gpu_last;
        }
        let q = self.gpu.as_ref()?;
        if !q.collect() {
            return self.gpu_last;
        }
        if let Some((sum, count)) = q.sum(0) {
            // No 3D engine at all means the PC does not report one.
            self.gpu_last = (count > 0).then(|| sum.clamp(0.0, 100.0) as f32);
        }
        self.gpu_last
    }

    fn disk(&mut self) -> (u64, u64) {
        if let Some(q) = &self.disk
            && q.collect()
        {
            let f = |v: Option<f64>| v.map_or(0, |x| x.max(0.0) as u64);
            self.disk_last = (f(q.value(0)), f(q.value(1)));
        }
        self.disk_last
    }

    /// Bytes per second since the previous read, over real adapters only.
    fn network(&mut self) -> (u64, u64) {
        let now = Instant::now();
        let Some((down, up)) = octets() else {
            return self.net_last;
        };
        match self.net {
            Some((t, pd, pu)) => {
                let dt = now.duration_since(t);
                if dt < MIN_SPAN {
                    return self.net_last;
                }
                let secs = dt.as_secs_f64();
                self.net_last = (
                    (down.saturating_sub(pd) as f64 / secs) as u64,
                    (up.saturating_sub(pu) as f64 / secs) as u64,
                );
            }
            None => self.net_last = (0, 0),
        }
        self.net = Some((now, down, up));
        self.net_last
    }

    fn space(&mut self) -> (u64, u64) {
        if let Some((t, free, total)) = self.space
            && t.elapsed() < SLOW
        {
            return (free, total);
        }
        let root = wide("C:\\");
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        // SAFETY: valid out pointers.
        let ok = unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut avail, &mut total, &mut free) };
        let r = if ok != 0 { (avail, total) } else { (0, 0) };
        self.space = Some((Instant::now(), r.0, r.1));
        r
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

/// Total bytes received and sent over real adapters: Ethernet and Wi-Fi that
/// are up, hardware, with a connector, and not filter interfaces (virtual
/// adapters and filters would count the same traffic twice).
fn octets() -> Option<(u64, u64)> {
    // SAFETY: the table is read inside its NumEntries and freed once.
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = null_mut();
        if GetIfTable2(&mut table) != 0 || table.is_null() {
            return None;
        }
        let rows = (&raw const (*table).Table).cast::<MIB_IF_ROW2>();
        let (mut down, mut up) = (0u64, 0u64);
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
                down += r.InOctets;
                up += r.OutOctets;
            }
        }
        FreeMibTable(table.cast());
        Some((down, up))
    }
}
