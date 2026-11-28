//! Graphics cards on Windows: the engine and adapter memory counters, read on
//! every call, and the adapter list (names, video memory, which adapters are
//! software), read from DXGI at start and again only when the counters name a
//! card the list does not have.

use crate::gpu::{self, Adapter, Luid};
use crate::perf::Query;
use crate::{Gpu, GpuEngine};
use core::ffi::c_void;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{FreeLibrary, LUID};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::core::GUID;

/// Every engine of every process, one instance per (process, engine), all
/// kinds of work: a card running a model is busy on its compute engines while
/// its 3D engine idles.
///
/// Added once and never rebuilt: PDH re-expands the wildcard on every collect.
/// Observed on this PC (Windows 11, 2026-11-25) with a query held open while a
/// process created a Direct3D 11 device and released it again: the instance
/// of the new process appeared in the same sample and vanished again after the
/// release, and the instance count matched a freshly built query on every
/// sample. (A rebuild every 15 reads only dropped a reading and the history.)
const ENGINES: &str = r"\GPU Engine(*)\Utilization Percentage";
/// Dedicated video memory in use, one instance per (card, physical GPU).
const MEMORY: &str = r"\GPU Adapter Memory(*)\Dedicated Usage";
/// The adapter list is read again at most this often, however many unknown
/// cards the counters name (a card DXGI does not list stays unknown).
const RELIST: Duration = Duration::from_secs(60);

pub(crate) struct Cards {
    engines: Option<Query>,
    memory: Option<Query>,
    adapters: Vec<Adapter>,
    /// LUIDs of software adapters, left out of the cards.
    software: Vec<Luid>,
    listed_at: Instant,
    last: Vec<Gpu>,
}

impl Cards {
    pub fn new() -> Self {
        let (adapters, software) = list().unwrap_or_default();
        Self {
            engines: Query::new(&[ENGINES]),
            memory: Query::new(&[MEMORY]),
            adapters,
            software,
            listed_at: Instant::now(),
            last: Vec::new(),
        }
    }

    pub fn read(&mut self) -> Vec<Gpu> {
        let engines = read_all(self.engines.as_ref()).map(|items| {
            gpu::busiest_by_card(items.iter().map(|(n, v, ok)| (n.as_str(), *v, *ok)))
        });
        let memory = read_all(self.memory.as_ref())
            .map(|items| gpu::memory_by_card(items.iter().map(|(n, v, ok)| (n.as_str(), *v, *ok))));
        if let Some(e) = &engines {
            self.relist_if_stale(e);
        }
        let Some(engines) = engines else {
            // A failed collect keeps the previous figures; no engine counter
            // at all means the PC does not report one.
            if self.engines.is_some() && !self.last.is_empty() {
                return self.last.clone();
            }
            return gpu::cards(&self.adapters, None, memory.as_ref());
        };
        let mut engines = engines;
        engines.retain(|l, _| !self.software.contains(l));
        self.last = gpu::cards(&self.adapters, Some(&engines), memory.as_ref());
        self.last.clone()
    }

    fn relist_if_stale(&mut self, engines: &HashMap<Luid, (f64, GpuEngine)>) {
        let unknown = gpu::unknown_luids(engines, &self.adapters, &self.software)
            .next()
            .is_some();
        if unknown && self.listed_at.elapsed() >= RELIST {
            if let Some((adapters, software)) = list() {
                self.adapters = adapters;
                self.software = software;
            }
            self.listed_at = Instant::now();
        }
    }
}

/// Collects the query and returns every instance, `None` on any failure.
fn read_all(q: Option<&Query>) -> Option<Vec<(String, f64, bool)>> {
    let q = q?;
    if !q.collect() {
        return None;
    }
    q.instances(0)
}

#[repr(C)]
struct AdapterDesc1 {
    description: [u16; 128],
    vendor_id: u32,
    device_id: u32,
    sub_sys_id: u32,
    revision: u32,
    dedicated_video_memory: usize,
    dedicated_system_memory: usize,
    shared_system_memory: usize,
    adapter_luid: LUID,
    flags: u32,
}

const IID_IDXGI_FACTORY1: GUID = GUID {
    data1: 0x770a_ae78,
    data2: 0xf26f,
    data3: 0x4dba,
    data4: [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
};
const DXGI_ADAPTER_FLAG_SOFTWARE: u32 = 2;
/// Vtable slots: IUnknown (3), IDXGIObject (4), IDXGIFactory (5), then
/// IDXGIFactory1::EnumAdapters1; IDXGIAdapter (3) after IDXGIObject, then
/// IDXGIAdapter1::GetDesc1.
const RELEASE: usize = 2;
const ENUM_ADAPTERS1: usize = 12;
const GET_DESC1: usize = 10;

type CreateFactory1 = unsafe extern "system" fn(*const GUID, *mut *mut c_void) -> i32;
type Release = unsafe extern "system" fn(*mut c_void) -> u32;
type EnumAdapters1 = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
type GetDesc1 = unsafe extern "system" fn(*mut c_void, *mut AdapterDesc1) -> i32;

/// The slot of a COM object's vtable.
///
/// # Safety
/// `obj` is a live COM object whose vtable has more than `slot` entries.
unsafe fn method(obj: *mut c_void, slot: usize) -> *const c_void {
    // SAFETY: the first field of a COM object is the vtable pointer.
    unsafe { *(*obj.cast::<*const *const c_void>()).add(slot) }
}

/// The hardware cards and the LUIDs of software adapters, from DXGI.
///
/// `dxgi.dll` is loaded for this call only and released after it, so a
/// hidden Pulse keeps no DXGI factory or library in memory.
fn list() -> Option<(Vec<Adapter>, Vec<Luid>)> {
    let name: Vec<u16> = "dxgi.dll".encode_utf16().chain(Some(0)).collect();
    // SAFETY: a system DLL by full search path; freed once below.
    let lib = unsafe {
        LoadLibraryExW(
            name.as_ptr(),
            core::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if lib.is_null() {
        return None;
    }
    // SAFETY: the export has this signature; every object is released once
    // and the library is freed after the last of them.
    let out = unsafe {
        let create = GetProcAddress(lib, c"CreateDXGIFactory1".as_ptr().cast());
        create.and_then(|f| {
            let create: CreateFactory1 = core::mem::transmute(f);
            let mut factory: *mut c_void = core::ptr::null_mut();
            if create(&IID_IDXGI_FACTORY1, &mut factory) < 0 || factory.is_null() {
                return None;
            }
            let enum_adapters: EnumAdapters1 =
                core::mem::transmute(method(factory, ENUM_ADAPTERS1));
            let (mut cards, mut software) = (Vec::new(), Vec::new());
            // DXGI_ERROR_NOT_FOUND (or any failure) ends the list.
            for i in 0..64 {
                let mut adapter: *mut c_void = core::ptr::null_mut();
                if enum_adapters(factory, i, &mut adapter) < 0 || adapter.is_null() {
                    break;
                }
                let get_desc: GetDesc1 = core::mem::transmute(method(adapter, GET_DESC1));
                let mut d: AdapterDesc1 = core::mem::zeroed();
                if get_desc(adapter, &mut d) >= 0 {
                    let luid =
                        gpu::luid_from_parts(d.adapter_luid.LowPart, d.adapter_luid.HighPart);
                    if d.flags & DXGI_ADAPTER_FLAG_SOFTWARE != 0 {
                        software.push(luid);
                    } else if !cards.iter().any(|c: &Adapter| c.luid == luid) {
                        let n = d.description.iter().position(|&u| u == 0).unwrap_or(128);
                        cards.push(Adapter {
                            luid,
                            name: tidy(&String::from_utf16_lossy(&d.description[..n])),
                            total: d.dedicated_video_memory as u64,
                        });
                    }
                }
                let release: Release = core::mem::transmute(method(adapter, RELEASE));
                release(adapter);
            }
            let release: Release = core::mem::transmute(method(factory, RELEASE));
            release(factory);
            Some((cards, software))
        })
    };
    // SAFETY: loaded above, freed once; no object from it is still alive.
    unsafe { FreeLibrary(lib) };
    out
}

/// One space between words ("AMD  Radeon RX 7800 XT" has two).
fn tidy(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}
