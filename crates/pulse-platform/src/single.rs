//! Single instance: a named mutex plus named events. The second instance
//! signals the show event so the first one opens its panel; the installer
//! signals the quit event so the first one closes itself cleanly.
//!
//! Names are `Local\Pulse-instance`, `Local\Pulse-show` and
//! `Local\Pulse-quit`; the `Pulse` part can be overridden with
//! `PULSE_INSTANCE_NAME` (tests).

use std::time::Duration;

type Callback = Box<dyn Fn() + Send>;

/// Held by the first instance for as long as it runs.
pub struct Instance {
    _inner: imp::Inner,
}

impl Instance {
    /// `Some` for the first instance. `None` for a later one, after it has
    /// signalled the first so it calls `on_second`.
    pub fn acquire(on_second: impl Fn() + Send + 'static) -> Option<Instance> {
        Self::acquire_with(on_second, || {})
    }

    /// Like [`Instance::acquire`], and the first instance also calls
    /// `on_quit` whenever [`request_quit`] signals it.
    pub fn acquire_with(
        on_second: impl Fn() + Send + 'static,
        on_quit: impl Fn() + Send + 'static,
    ) -> Option<Instance> {
        acquire_named(&base_name(), Box::new(on_second), Box::new(on_quit))
    }
}

/// Asks a running instance to quit and waits until it has released the
/// instance mutex. `true` at once when none runs; `false` when it is still
/// there after `timeout`.
pub fn request_quit(timeout: Duration) -> bool {
    imp::request_quit(&base_name(), timeout)
}

fn base_name() -> String {
    match std::env::var("PULSE_INSTANCE_NAME") {
        Ok(name) if !name.is_empty() => name,
        _ => "Pulse".to_owned(),
    }
}

fn acquire_named(name: &str, on_second: Callback, on_quit: Callback) -> Option<Instance> {
    imp::acquire(name, on_second, on_quit).map(|inner| Instance { _inner: inner })
}

#[cfg(not(windows))]
mod imp {
    use super::Callback;
    use std::time::Duration;

    pub struct Inner;

    pub fn acquire(_name: &str, _on_second: Callback, _on_quit: Callback) -> Option<Inner> {
        Some(Inner)
    }

    pub fn request_quit(_name: &str, _timeout: Duration) -> bool {
        true
    }
}

#[cfg(windows)]
mod imp {
    use super::Callback;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, INFINITE, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE, SetEvent,
        WaitForMultipleObjects,
    };

    // Handles are process-wide kernel objects, safe to move between threads.
    #[derive(Clone, Copy)]
    struct Handle(HANDLE);
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}

    const NONE: Handle = Handle(std::ptr::null_mut());

    pub struct Inner {
        mutex: Handle,
        /// Stop event; the waiter thread closes show, quit and stop on exit.
        stop: Handle,
        join: Option<std::thread::JoinHandle<()>>,
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn names(name: &str) -> (Vec<u16>, Vec<u16>, Vec<u16>) {
        (
            wide(&format!("Local\\{name}-instance")),
            wide(&format!("Local\\{name}-show")),
            wide(&format!("Local\\{name}-quit")),
        )
    }

    /// Creates (or opens) an auto-reset event that starts not signalled.
    fn event(name: &[u16]) -> Handle {
        // SAFETY: name is NUL-terminated.
        Handle(unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) })
    }

    fn close(h: Handle) {
        if !h.0.is_null() {
            // SAFETY: closes a handle this module created, once.
            unsafe { CloseHandle(h.0) };
        }
    }

    pub fn acquire(name: &str, on_second: Callback, on_quit: Callback) -> Option<Inner> {
        let (mutex_name, show_name, quit_name) = names(name);
        // SAFETY: names are NUL-terminated; every handle is closed on each path.
        let mutex = Handle(unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) });
        if mutex.0.is_null() {
            // Cannot tell; behave as the first instance rather than refusing to start.
            return Some(Inner {
                mutex: NONE,
                stop: NONE,
                join: None,
            });
        }
        // SAFETY: reads this thread's last error from the call above.
        let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let show = event(&show_name);
        if already {
            if !show.0.is_null() {
                // SAFETY: valid event handle.
                unsafe { SetEvent(show.0) };
                close(show);
            }
            close(mutex);
            return None;
        }
        let quit = event(&quit_name);
        // Manual-reset stop event.
        // SAFETY: no name, default security.
        let stop = Handle(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) });
        if show.0.is_null() || quit.0.is_null() || stop.0.is_null() {
            close(show);
            close(quit);
            close(stop);
            return Some(Inner {
                mutex,
                stop: NONE,
                join: None,
            });
        }
        let join = std::thread::Builder::new()
            .name("pulse-single".into())
            .spawn(move || {
                let handles = [show, quit, stop].map(|h| h.0);
                loop {
                    // SAFETY: three valid handles owned by this thread until it exits.
                    let r = unsafe { WaitForMultipleObjects(3, handles.as_ptr(), 0, INFINITE) };
                    if r == WAIT_OBJECT_0 {
                        on_second();
                    } else if r == WAIT_OBJECT_0 + 1 {
                        on_quit();
                    } else {
                        break;
                    }
                }
                close(show);
                close(quit);
                close(stop);
            });
        match join {
            Ok(join) => Some(Inner {
                mutex,
                stop,
                join: Some(join),
            }),
            Err(_) => Some(Inner {
                mutex,
                stop: NONE,
                join: None,
            }),
        }
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            // The mutex goes first so the instance counts as gone at once, even
            // when this runs on the waiter thread (inside `on_quit`).
            close(self.mutex);
            if !self.stop.0.is_null() {
                // SAFETY: the stop event is still open: only the waiter thread
                // closes it, and only after seeing it signalled.
                unsafe { SetEvent(self.stop.0) };
            }
            if let Some(join) = self.join.take()
                && join.thread().id() != std::thread::current().id()
            {
                let _ = join.join();
            }
        }
    }

    /// Opens (never creates) the instance mutex; `None` when nobody holds it.
    fn open_mutex(name: &[u16]) -> Option<Handle> {
        // SAFETY: name is NUL-terminated.
        let h = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
        (!h.is_null()).then_some(Handle(h))
    }

    pub fn request_quit(name: &str, timeout: Duration) -> bool {
        let (mutex_name, _, quit_name) = names(name);
        let Some(probe) = open_mutex(&mutex_name) else {
            return true;
        };
        close(probe);
        let quit = event(&quit_name);
        if !quit.0.is_null() {
            // SAFETY: valid event handle.
            unsafe { SetEvent(quit.0) };
        }
        let end = Instant::now() + timeout;
        let gone = loop {
            match open_mutex(&mutex_name) {
                None => break true,
                Some(h) => close(h),
            }
            if Instant::now() >= end {
                break false;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        // Closing our handle to the event: if the instance is gone the object
        // goes with it, so a later instance starts with a fresh one.
        close(quit);
        gone
    }
}

#[cfg(all(test, not(windows)))]
mod fallback_tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn fallbacks() {
        assert!(Instance::acquire_with(|| {}, || {}).is_some());
        assert!(request_quit(Duration::from_millis(1)));
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn unique(tag: &str) -> String {
        format!("Pulse-test-{tag}-{}", std::process::id())
    }

    fn noop() -> Callback {
        Box::new(|| {})
    }

    fn wait_for(count: &AtomicUsize, at_least: usize) {
        let end = Instant::now() + Duration::from_secs(5);
        while count.load(Ordering::SeqCst) < at_least && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    use std::time::Instant;

    #[test]
    fn first_then_second() {
        let name = unique("single");
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        let first = acquire_named(
            &name,
            Box::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
            }),
            noop(),
        );
        assert!(first.is_some());
        assert!(acquire_named(&name, noop(), noop()).is_none());
        wait_for(&count, 1);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(acquire_named(&name, noop(), noop()).is_none());
        wait_for(&count, 2);
        assert_eq!(count.load(Ordering::SeqCst), 2);
        drop(first);
        // Released: a new first instance can take it.
        assert!(acquire_named(&name, noop(), noop()).is_some());
    }

    #[test]
    fn quit_request_closes_the_instance() {
        let name = unique("quit");
        let quits = Arc::new(AtomicUsize::new(0));
        let slot: Arc<Mutex<Option<Instance>>> = Arc::new(Mutex::new(None));
        let (q, s) = (Arc::clone(&quits), Arc::clone(&slot));
        let instance = acquire_named(
            &name,
            noop(),
            Box::new(move || {
                q.fetch_add(1, Ordering::SeqCst);
                drop(s.lock().unwrap().take());
            }),
        );
        *slot.lock().unwrap() = instance;
        assert!(imp::request_quit(&name, Duration::from_secs(5)));
        assert_eq!(quits.load(Ordering::SeqCst), 1);
        // Nothing left behind: a new instance is the first one.
        assert!(acquire_named(&name, noop(), noop()).is_some());
    }

    #[test]
    fn quit_request_with_no_instance_is_immediate() {
        let started = Instant::now();
        assert!(imp::request_quit(&unique("none"), Duration::from_secs(5)));
        assert!(started.elapsed() < Duration::from_secs(1));
        // It did not create the mutex: a later start is the first instance.
        assert!(acquire_named(&unique("none"), noop(), noop()).is_some());
    }

    #[test]
    fn quit_request_times_out_when_ignored() {
        let name = unique("ignored");
        let quits = Arc::new(AtomicUsize::new(0));
        let q = Arc::clone(&quits);
        let _first = acquire_named(
            &name,
            noop(),
            Box::new(move || {
                q.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .unwrap();
        let started = Instant::now();
        assert!(!imp::request_quit(&name, Duration::from_millis(300)));
        assert!(started.elapsed() >= Duration::from_millis(300));
        wait_for(&quits, 1);
        assert_eq!(quits.load(Ordering::SeqCst), 1);
    }
}
