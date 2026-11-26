//! Single instance: a named mutex plus a named event the second instance
//! signals so the first one opens its panel.
//!
//! Names are `Local\Pulse-instance` and `Local\Pulse-show`; the `Pulse` part
//! can be overridden with `PULSE_INSTANCE_NAME` (tests).

/// Held by the first instance for as long as it runs.
pub struct Instance {
    _inner: imp::Inner,
}

impl Instance {
    /// `Some` for the first instance. `None` for a later one, after it has
    /// signalled the first so it calls `on_second`.
    pub fn acquire(on_second: impl Fn() + Send + 'static) -> Option<Instance> {
        acquire_named(&base_name(), Box::new(on_second))
    }
}

fn base_name() -> String {
    match std::env::var("PULSE_INSTANCE_NAME") {
        Ok(name) if !name.is_empty() => name,
        _ => "Pulse".to_owned(),
    }
}

fn acquire_named(name: &str, on_second: Box<dyn Fn() + Send>) -> Option<Instance> {
    imp::acquire(name, on_second).map(|inner| Instance { _inner: inner })
}

#[cfg(not(windows))]
mod imp {
    pub struct Inner;

    pub fn acquire(_name: &str, _on_second: Box<dyn Fn() + Send>) -> Option<Inner> {
        Some(Inner)
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, INFINITE, SetEvent, WaitForMultipleObjects,
    };

    // Handles are process-wide kernel objects, safe to move between threads.
    struct Handle(HANDLE);
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}

    pub struct Inner {
        mutex: Handle,
        show: Handle,
        stop: Handle,
        join: Option<std::thread::JoinHandle<()>>,
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn acquire(name: &str, on_second: Box<dyn Fn() + Send>) -> Option<Inner> {
        let mutex_name = wide(&format!("Local\\{name}-instance"));
        let show_name = wide(&format!("Local\\{name}-show"));
        // SAFETY: names are NUL-terminated; every handle is closed on each path.
        unsafe {
            let mutex = CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr());
            if mutex.is_null() {
                // Cannot tell; behave as the first instance rather than refusing to start.
                return Some(detached());
            }
            let already = GetLastError() == ERROR_ALREADY_EXISTS;
            // Auto-reset, initially not signalled.
            let show = CreateEventW(std::ptr::null(), 0, 0, show_name.as_ptr());
            if already {
                if !show.is_null() {
                    SetEvent(show);
                    CloseHandle(show);
                }
                CloseHandle(mutex);
                return None;
            }
            if show.is_null() {
                return Some(Inner {
                    mutex: Handle(mutex),
                    show: Handle(std::ptr::null_mut()),
                    stop: Handle(std::ptr::null_mut()),
                    join: None,
                });
            }
            let stop = CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
            let (show_t, stop_t) = (Handle(show), Handle(stop));
            let join = std::thread::Builder::new()
                .name("pulse-single".into())
                .spawn(move || {
                    let (show, stop) = (show_t, stop_t);
                    let handles = [show.0, stop.0];
                    loop {
                        let r = WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE);
                        if r == WAIT_OBJECT_0 {
                            on_second();
                        } else {
                            break;
                        }
                    }
                });
            Some(Inner {
                mutex: Handle(mutex),
                show: Handle(show),
                stop: Handle(stop),
                join: join.ok(),
            })
        }
    }

    fn detached() -> Inner {
        Inner {
            mutex: Handle(std::ptr::null_mut()),
            show: Handle(std::ptr::null_mut()),
            stop: Handle(std::ptr::null_mut()),
            join: None,
        }
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            // SAFETY: handles were created by acquire and are closed exactly once;
            // the thread is joined before its handles are closed.
            unsafe {
                if !self.stop.0.is_null() {
                    SetEvent(self.stop.0);
                }
                if let Some(join) = self.join.take() {
                    let _ = join.join();
                }
                for h in [&self.stop, &self.show, &self.mutex] {
                    if !h.0.is_null() {
                        CloseHandle(h.0);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn first_then_second() {
        let name = format!("Pulse-test-{}", std::process::id());
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        let first = acquire_named(
            &name,
            Box::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
            }),
        );
        assert!(first.is_some());
        #[cfg(windows)]
        {
            assert!(acquire_named(&name, Box::new(|| {})).is_none());
            let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while count.load(Ordering::SeqCst) < 1 && std::time::Instant::now() < end {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_eq!(count.load(Ordering::SeqCst), 1);
            assert!(acquire_named(&name, Box::new(|| {})).is_none());
            let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while count.load(Ordering::SeqCst) < 2 && std::time::Instant::now() < end {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_eq!(count.load(Ordering::SeqCst), 2);
            drop(first);
            // Released: a new first instance can take it, then it is cleaned up.
            let again = acquire_named(&name, Box::new(|| {}));
            assert!(again.is_some());
        }
    }
}
