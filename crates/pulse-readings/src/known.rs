//! The per-exe cache of icon path and friendly name. Pure code over a
//! [`Probe`], so the cache rules are tested everywhere.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Re-check that a cached path still exists this often (in reads).
pub(crate) const RECHECK_READS: u64 = 30;
/// A failed path lookup is not tried again for this long.
pub(crate) const MISS_RETRY: Duration = Duration::from_secs(60);
/// A missing `FileDescription` is asked for again this often (in reads) ...
pub(crate) const DESC_RETRY_READS: u64 = 10;
/// ... up to this many times in all.
pub(crate) const DESC_TRIES: u8 = 3;

/// What the cache needs from the machine.
pub(crate) trait Probe {
    /// Full path of the executable of one process.
    fn process_path(&self, pid: u32) -> Option<String>;
    /// The built-in stand-in file for an image name.
    fn stand_in(&self, exe: &str) -> Option<String>;
    /// The same name in `System32`.
    fn system32(&self, exe: &str) -> Option<String>;
    fn exists(&self, path: &str) -> bool;
    fn description(&self, path: &str) -> Option<String>;
}

enum Slot {
    Found {
        path: String,
        desc: Option<String>,
        /// Description lookups left (0 once found, or for stand-ins, which never rename).
        desc_left: u8,
        desc_at: u64,
        checked_at: u64,
    },
    /// No path could be found; try again at this time.
    Missed { retry_at: Instant },
}

#[derive(Default)]
pub(crate) struct Known {
    slots: HashMap<String, Slot>,
    reads: u64,
}

/// The file name of a Windows (or any) path.
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

impl Known {
    /// Call once per read.
    pub fn begin_read(&mut self) {
        self.reads += 1;
    }

    /// The path and description for one app row, from the cache when it is
    /// still good. `pids` are tried in turn; a pid is accepted only when its
    /// image file name equals `exe` (a recycled pid belongs to another program).
    pub fn resolve(
        &mut self,
        probe: &impl Probe,
        exe: &str,
        pids: &[u32],
        now: Instant,
    ) -> Option<(String, Option<String>)> {
        let key = exe.to_ascii_lowercase();
        let reads = self.reads;
        match self.slots.get_mut(&key) {
            Some(Slot::Missed { retry_at }) if now < *retry_at => return None,
            Some(Slot::Missed { .. }) => {
                self.slots.remove(&key);
            }
            Some(Slot::Found {
                path, checked_at, ..
            }) if reads.saturating_sub(*checked_at) >= RECHECK_READS => {
                if probe.exists(path) {
                    *checked_at = reads;
                } else {
                    self.slots.remove(&key);
                }
            }
            _ => {}
        }
        if !self.slots.contains_key(&key) {
            let slot = Self::lookup(probe, exe, pids, now, reads);
            self.slots.insert(key.clone(), slot);
        }
        match self.slots.get_mut(&key)? {
            Slot::Missed { .. } => None,
            Slot::Found {
                path,
                desc,
                desc_left,
                desc_at,
                ..
            } => {
                if desc.is_none()
                    && *desc_left > 0
                    && reads.saturating_sub(*desc_at) >= DESC_RETRY_READS
                {
                    *desc_left -= 1;
                    *desc_at = reads;
                    *desc = probe.description(path);
                }
                Some((path.clone(), desc.clone()))
            }
        }
    }

    fn lookup(probe: &impl Probe, exe: &str, pids: &[u32], now: Instant, reads: u64) -> Slot {
        // 1. The built-in stand-ins never rename the row.
        if let Some(path) = probe.stand_in(exe) {
            return Slot::Found {
                path,
                desc: None,
                desc_left: 0,
                desc_at: reads,
                checked_at: reads,
            };
        }
        // 2. A process of the group whose image really is this exe, else 3.
        //    the same name in System32.
        let path = pids
            .iter()
            .filter_map(|&p| probe.process_path(p))
            .find(|p| file_name(p).eq_ignore_ascii_case(exe))
            .or_else(|| probe.system32(exe));
        match path {
            Some(path) => {
                let desc = probe.description(&path);
                Slot::Found {
                    desc_left: if desc.is_some() { 0 } else { DESC_TRIES - 1 },
                    desc,
                    path,
                    desc_at: reads,
                    checked_at: reads,
                }
            }
            None => Slot::Missed {
                retry_at: now + MISS_RETRY,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    struct Fake {
        paths: RefCell<HashMap<u32, String>>,
        existing: RefCell<Vec<String>>,
        descs: RefCell<HashMap<String, String>>,
        open_calls: Cell<u32>,
        desc_calls: Cell<u32>,
        stand: Option<(&'static str, &'static str)>,
    }

    impl Probe for Fake {
        fn process_path(&self, pid: u32) -> Option<String> {
            self.open_calls.set(self.open_calls.get() + 1);
            self.paths.borrow().get(&pid).cloned()
        }
        fn stand_in(&self, exe: &str) -> Option<String> {
            self.stand
                .filter(|(k, _)| k.eq_ignore_ascii_case(exe))
                .map(|(_, v)| v.to_string())
        }
        fn system32(&self, _: &str) -> Option<String> {
            None
        }
        fn exists(&self, path: &str) -> bool {
            self.existing.borrow().iter().any(|p| p == path)
        }
        fn description(&self, path: &str) -> Option<String> {
            self.desc_calls.set(self.desc_calls.get() + 1);
            self.descs.borrow().get(path).cloned()
        }
    }

    fn fake(paths: &[(u32, &str)]) -> Fake {
        let f = Fake::default();
        for (pid, p) in paths {
            f.paths.borrow_mut().insert(*pid, (*p).to_string());
            f.existing.borrow_mut().push((*p).to_string());
        }
        f
    }

    #[test]
    fn a_recycled_pid_with_another_exe_is_skipped() {
        let f = fake(&[(1, r"C:\Windows\notepad.exe"), (2, r"C:\Apps\Chrome.EXE")]);
        let mut k = Known::default();
        let got = k
            .resolve(&f, "chrome.exe", &[1, 2], Instant::now())
            .unwrap();
        assert_eq!(got.0, r"C:\Apps\Chrome.EXE");
    }

    #[test]
    fn only_a_wrong_pid_means_a_miss() {
        let f = fake(&[(1, r"C:\Windows\notepad.exe")]);
        let mut k = Known::default();
        assert!(k.resolve(&f, "chrome.exe", &[1], Instant::now()).is_none());
    }

    #[test]
    fn a_cached_path_is_rechecked_and_resolved_again_when_gone() {
        let f = fake(&[(1, r"C:\D\app-1\Discord.exe")]);
        let mut k = Known::default();
        let t = Instant::now();
        assert_eq!(
            k.resolve(&f, "Discord.exe", &[1], t).unwrap().0,
            r"C:\D\app-1\Discord.exe"
        );
        // The app updated into a new folder; the old one is gone.
        f.existing.borrow_mut().clear();
        f.paths
            .borrow_mut()
            .insert(1, r"C:\D\app-2\Discord.exe".to_string());
        f.existing
            .borrow_mut()
            .push(r"C:\D\app-2\Discord.exe".to_string());
        // Cached until the recheck falls due ...
        for _ in 0..(RECHECK_READS - 1) {
            k.begin_read();
        }
        assert_eq!(
            k.resolve(&f, "Discord.exe", &[1], t).unwrap().0,
            r"C:\D\app-1\Discord.exe"
        );
        // ... then it is looked up again.
        k.begin_read();
        assert_eq!(
            k.resolve(&f, "Discord.exe", &[1], t).unwrap().0,
            r"C:\D\app-2\Discord.exe"
        );
    }

    #[test]
    fn a_failed_lookup_is_cached_for_a_minute() {
        let f = fake(&[]);
        let mut k = Known::default();
        let t = Instant::now();
        assert!(k.resolve(&f, "x.exe", &[1, 2, 3], t).is_none());
        let first = f.open_calls.get();
        assert_eq!(first, 3);
        for _ in 0..10 {
            k.begin_read();
            assert!(
                k.resolve(&f, "x.exe", &[1, 2, 3], t + Duration::from_secs(20))
                    .is_none()
            );
        }
        assert_eq!(f.open_calls.get(), first);
        // After a minute it is tried again and can succeed.
        f.paths.borrow_mut().insert(2, r"C:\x.exe".into());
        f.existing.borrow_mut().push(r"C:\x.exe".into());
        let got = k.resolve(&f, "x.exe", &[1, 2, 3], t + MISS_RETRY);
        assert_eq!(got.unwrap().0, r"C:\x.exe");
    }

    #[test]
    fn a_missing_description_is_retried_a_few_times_not_forever() {
        let f = fake(&[(1, r"C:\a\app.exe")]);
        let mut k = Known::default();
        let t = Instant::now();
        assert_eq!(k.resolve(&f, "app.exe", &[1], t).unwrap().1, None);
        assert_eq!(f.desc_calls.get(), 1);
        // Not asked again straight away.
        k.begin_read();
        k.resolve(&f, "app.exe", &[1], t);
        assert_eq!(f.desc_calls.get(), 1);
        // The version info appears (the file was still being written).
        f.descs
            .borrow_mut()
            .insert(r"C:\a\app.exe".into(), "The App".into());
        for _ in 0..DESC_RETRY_READS {
            k.begin_read();
        }
        let got = k.resolve(&f, "app.exe", &[1], t).unwrap();
        assert_eq!(got.1.as_deref(), Some("The App"));
        let calls = f.desc_calls.get();
        for _ in 0..(DESC_RETRY_READS * 5) {
            k.begin_read();
            k.resolve(&f, "app.exe", &[1], t);
        }
        assert_eq!(f.desc_calls.get(), calls);
    }

    #[test]
    fn a_description_that_never_comes_stops_after_the_tries() {
        let f = fake(&[(1, r"C:\a\app.exe")]);
        let mut k = Known::default();
        let t = Instant::now();
        for _ in 0..(DESC_RETRY_READS * 10) {
            k.begin_read();
            k.resolve(&f, "app.exe", &[1], t);
        }
        assert_eq!(f.desc_calls.get(), u32::from(DESC_TRIES));
    }

    #[test]
    fn stand_ins_are_used_without_opening_processes_or_renaming() {
        let mut f = fake(&[(1, r"C:\Windows\System32\wsl.exe")]);
        f.stand = Some(("vmmemWSL", r"C:\Windows\System32\wsl.exe"));
        let mut k = Known::default();
        let got = k.resolve(&f, "vmmemWSL", &[1], Instant::now()).unwrap();
        assert_eq!(got, (r"C:\Windows\System32\wsl.exe".to_string(), None));
        assert_eq!((f.open_calls.get(), f.desc_calls.get()), (0, 0));
    }
}
