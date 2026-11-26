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
    pub struct Inner;

    pub fn acquire(_name: &str, _on_second: Box<dyn Fn() + Send>) -> Option<Inner> {
        Some(Inner)
    }
}
