//! Fixed drives: which volumes count, their space and their speed.
//!
//! The pure part (which drives to keep, the counter names, one counter set per
//! drive as drives come and go) builds everywhere, so its tests run on Linux.
//! The Windows calls are in [`Drives`].

use crate::Drive;

/// `GetDriveTypeW`'s value for a fixed disk (`DRIVE_FIXED`). Removable
/// (2), network (4), optical (5) and RAM disks (6) are left out: asking a
/// sleeping or disconnected one for its size can stall the reader.
pub(crate) const DRIVE_FIXED: u32 = 3;

/// The drive names ("A:" to "Z:") of the letters set in a `GetLogicalDrives`
/// mask, in letter order.
pub(crate) fn letters(mask: u32) -> Vec<String> {
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:", char::from(b'A' + i)))
        .collect()
}

/// The drives to show, in letter order: fixed drives whose size could be
/// read. `kind` is asked first and `space` (free, total) only for a fixed
/// drive; a drive whose size fails, or reads as zero or as more free than
/// total, is skipped (never shown as an empty drive). Speeds start at 0.
pub(crate) fn scan(
    mask: u32,
    mut kind: impl FnMut(&str) -> u32,
    mut space: impl FnMut(&str) -> Option<(u64, u64)>,
    mut label: impl FnMut(&str) -> Option<String>,
) -> Vec<Drive> {
    letters(mask)
        .into_iter()
        .filter(|name| kind(name) == DRIVE_FIXED)
        .filter_map(|name| {
            let (free, total) = space(&name).filter(|&(f, t)| t > 0 && f <= t)?;
            let label = label(&name).filter(|l| !l.trim().is_empty());
            Some(Drive {
                name,
                label,
                free,
                total,
                read_bps: 0,
                write_bps: 0,
            })
        })
        .collect()
}

/// The two counters of one drive (read, then write), for example
/// `\LogicalDisk(D:)\Disk Read Bytes/sec`.
pub(crate) fn counter_paths(name: &str) -> [String; 2] {
    [
        format!(r"\LogicalDisk({name})\Disk Read Bytes/sec"),
        format!(r"\LogicalDisk({name})\Disk Write Bytes/sec"),
    ]
}

/// One meter (a counter query) per listed drive, with its last rates.
///
/// A meter belongs to one drive name: when the drive leaves the list its
/// meter is dropped (which closes its counters), and a new drive gets a new
/// one. A meter that could not be opened is tried again on the next listing.
pub(crate) struct Meters<Q> {
    list: Vec<(String, Option<Q>, (u64, u64))>,
}

impl<Q> Default for Meters<Q> {
    fn default() -> Self {
        Self { list: Vec::new() }
    }
}

impl<Q> Meters<Q> {
    /// Keeps exactly one entry per name in `names`, in that order.
    pub(crate) fn sync(&mut self, names: &[&str], mut open: impl FnMut(&str) -> Option<Q>) {
        self.list.retain(|(n, _, _)| names.contains(&n.as_str()));
        for &name in names {
            match self.list.iter_mut().find(|(n, _, _)| n == name) {
                Some((_, q @ None, _)) => *q = open(name),
                Some(_) => {}
                None => self.list.push((name.to_owned(), open(name), (0, 0))),
            }
        }
        self.list
            .sort_by_key(|(n, _, _)| names.iter().position(|x| x == n));
    }

    /// Reads every meter: `sample` gives (read, write) bytes per second, or
    /// `None` to keep the previous figures (a failed collect).
    pub(crate) fn update(&mut self, mut sample: impl FnMut(&Q) -> Option<(u64, u64)>) {
        for (_, q, last) in &mut self.list {
            if let Some(v) = q.as_ref().and_then(&mut sample) {
                *last = v;
            }
        }
    }

    /// The last (read, write) of a drive, 0 when it has no meter.
    pub(crate) fn rates(&self, name: &str) -> (u64, u64) {
        self.list
            .iter()
            .find(|(n, _, _)| n == name)
            .map_or((0, 0), |(_, _, r)| *r)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.list.len()
    }
}

#[cfg(windows)]
pub(crate) use win::Drives;

#[cfg(windows)]
mod win {
    use super::{Meters, counter_paths, scan};
    use crate::Drive;
    use crate::perf::Query;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    };

    /// Drives rarely come and go and space changes slowly: the list, the
    /// sizes and the labels are read at most this often. Speeds are read on
    /// every call.
    pub(crate) const LIST_EVERY: Duration = Duration::from_secs(30);

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn root(name: &str) -> Vec<u16> {
        wide(&format!("{name}\\"))
    }

    fn kind(name: &str) -> u32 {
        let r = root(name);
        // SAFETY: a valid null-terminated path.
        unsafe { GetDriveTypeW(r.as_ptr()) }
    }

    /// (free to the user, total), as Explorer shows them.
    fn space(name: &str) -> Option<(u64, u64)> {
        let r = root(name);
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        // SAFETY: a valid path and out pointers.
        let ok = unsafe { GetDiskFreeSpaceExW(r.as_ptr(), &mut avail, &mut total, &mut free) };
        (ok != 0).then_some((avail, total))
    }

    fn label(name: &str) -> Option<String> {
        let r = root(name);
        let mut buf = [0u16; 261];
        // SAFETY: a valid path and a buffer of the length given; the other
        // out pointers may be null.
        let ok = unsafe {
            GetVolumeInformationW(
                r.as_ptr(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                0,
            )
        };
        if ok == 0 {
            return None;
        }
        let n = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]))
    }

    pub(crate) struct Drives {
        list: Vec<Drive>,
        listed: Option<Instant>,
        meters: Meters<Query>,
    }

    impl Drives {
        pub(crate) fn new() -> Self {
            Self {
                list: Vec::new(),
                listed: None,
                meters: Meters::default(),
            }
        }

        pub(crate) fn read(&mut self) -> Vec<Drive> {
            if self.listed.is_none_or(|t| t.elapsed() >= LIST_EVERY) {
                // SAFETY: no arguments.
                let mask = unsafe { GetLogicalDrives() };
                self.list = scan(mask, kind, space, label);
                let names: Vec<&str> = self.list.iter().map(|d| d.name.as_str()).collect();
                self.meters.sync(&names, |name| {
                    let [r, w] = counter_paths(name);
                    Query::new(&[&r, &w])
                });
                self.listed = Some(Instant::now());
            }
            self.meters.update(|q| {
                q.collect().then(|| {
                    let f = |v: Option<f64>| v.map_or(0, |x| x.max(0.0) as u64);
                    (f(q.value(0)), f(q.value(1)))
                })
            });
            for d in &mut self.list {
                (d.read_bps, d.write_bps) = self.meters.rates(&d.name);
            }
            self.list.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    const GB: u64 = 1 << 30;

    fn mask(names: &str) -> u32 {
        names.bytes().map(|b| 1u32 << (b - b'A')).sum()
    }

    #[test]
    fn letters_follow_the_mask_in_order() {
        assert_eq!(letters(mask("CDZ")), ["C:", "D:", "Z:"]);
        assert!(letters(0).is_empty());
        // Bits past Z are not drives.
        assert_eq!(letters(1 << 26 | 1), ["A:"]);
    }

    #[test]
    fn only_fixed_drives_with_a_readable_size_are_kept() {
        let asked = std::cell::RefCell::new(Vec::new());
        let drives = scan(
            mask("CDEFGHX"),
            |n| match n {
                // E: removable, F: network, G: optical, X: RAM disk.
                "E:" => 2,
                "F:" => 4,
                "G:" => 5,
                "X:" => 6,
                _ => DRIVE_FIXED,
            },
            |n| {
                asked.borrow_mut().push(n.to_owned());
                match n {
                    "C:" => Some((296 * GB, 953 * GB)),
                    "D:" => Some((40 * GB, 931 * GB)),
                    // A fixed drive that fails to read (a locked BitLocker one).
                    "H:" => None,
                    _ => Some((GB, 2 * GB)),
                }
            },
            |n| (n == "D:").then(|| "Data".to_owned()),
        );
        let names: Vec<&str> = drives.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["C:", "D:"]);
        assert_eq!(drives[1].label.as_deref(), Some("Data"));
        assert_eq!(drives[0].label, None);
        assert_eq!((drives[0].free, drives[0].total), (296 * GB, 953 * GB));
        // The size is never asked of a drive that is not fixed.
        assert_eq!(*asked.borrow(), ["C:", "D:", "H:"]);
    }

    #[test]
    fn a_zero_size_or_impossible_reading_is_skipped() {
        let drives = scan(
            mask("CDE"),
            |_| DRIVE_FIXED,
            |n| match n {
                "C:" => Some((0, 0)),
                "D:" => Some((5 * GB, 4 * GB)),
                _ => Some((0, 4 * GB)),
            },
            |_| Some("  ".into()),
        );
        assert_eq!(drives.len(), 1);
        assert_eq!(drives[0].name, "E:");
        // A blank label is no label.
        assert_eq!(drives[0].label, None);
    }

    #[test]
    fn counters_are_named_per_drive() {
        assert_eq!(
            counter_paths("D:"),
            [
                r"\LogicalDisk(D:)\Disk Read Bytes/sec",
                r"\LogicalDisk(D:)\Disk Write Bytes/sec"
            ]
        );
    }

    /// A stand-in meter that counts how many are alive.
    struct Fake(Rc<Cell<i32>>, u64);
    impl Drop for Fake {
        fn drop(&mut self) {
            self.0.set(self.0.get() - 1);
        }
    }

    #[test]
    fn a_drive_that_vanishes_drops_its_meter_and_one_that_appears_gets_one() {
        let alive = Rc::new(Cell::new(0));
        let opened = Rc::new(Cell::new(0));
        let open = |name: &str| {
            alive.set(alive.get() + 1);
            opened.set(opened.get() + 1);
            Some(Fake(alive.clone(), u64::from(name.as_bytes()[0])))
        };
        let mut m: Meters<Fake> = Meters::default();
        m.sync(&["C:", "D:"], open);
        m.update(|f| Some((f.1, 1)));
        assert_eq!((m.len(), alive.get()), (2, 2));
        assert_eq!(m.rates("D:"), (u64::from(b'D'), 1));
        // D: is unplugged between two listings.
        m.sync(&["C:"], open);
        assert_eq!((m.len(), alive.get()), (1, 1), "the meter of D: is closed");
        assert_eq!(m.rates("D:"), (0, 0));
        assert_eq!(m.rates("C:"), (u64::from(b'C'), 1), "C: keeps its meter");
        // Listing again with no change opens nothing new.
        m.sync(&["C:"], open);
        assert_eq!(opened.get(), 2);
        // D: comes back, and E: is new: fresh meters, rates start at 0.
        m.sync(&["C:", "D:", "E:"], open);
        assert_eq!((m.len(), alive.get(), opened.get()), (3, 3, 4));
        assert_eq!(m.rates("E:"), (0, 0));
        drop(m);
        assert_eq!(alive.get(), 0, "nothing leaks");
    }

    #[test]
    fn a_meter_that_failed_to_open_is_tried_again_and_failed_reads_keep_the_last_rate() {
        let tries = Cell::new(0);
        let mut m: Meters<u64> = Meters::default();
        let open = |_: &str| {
            tries.set(tries.get() + 1);
            (tries.get() > 1).then_some(7)
        };
        m.sync(&["C:"], open);
        m.update(|v| Some((*v, *v)));
        assert_eq!(m.rates("C:"), (0, 0));
        m.sync(&["C:"], open);
        m.update(|v| Some((*v, *v)));
        assert_eq!(m.rates("C:"), (7, 7));
        m.update(|_| None);
        assert_eq!(m.rates("C:"), (7, 7));
    }

    #[cfg(windows)]
    #[test]
    fn the_system_drive_is_listed_and_its_counters_open() {
        let mut d = Drives::new();
        let first = d.read();
        let c = first.iter().find(|x| x.name == "C:").expect("C: is listed");
        assert!(c.total > 0 && c.free <= c.total);
        let [r, w] = counter_paths("C:");
        assert!(crate::perf::Query::new(&[&r, &w]).is_some());
        std::thread::sleep(std::time::Duration::from_millis(300));
        // A second read inside the listing pace keeps the same drives.
        let again = d.read();
        let names = |v: &[Drive]| v.iter().map(|x| x.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&first), names(&again));
    }

    #[test]
    fn meters_follow_the_listed_order() {
        let mut m: Meters<()> = Meters::default();
        m.sync(&["D:"], |_| Some(()));
        m.sync(&["C:", "D:"], |_| Some(()));
        let names: Vec<&str> = m.list.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(names, ["C:", "D:"]);
    }
}
