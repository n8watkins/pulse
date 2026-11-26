//! Small decisions of the tray app as plain functions, with no toolkit or
//! Windows types, so they are unit tested on any machine.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Runs `save` and, if it fails, once more. True when it ended up saved.
pub fn save_with_retry<E>(mut save: impl FnMut() -> Result<(), E>) -> bool {
    save().is_ok() || save().is_ok()
}

/// What the shortcut does to the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    /// Hidden: open it.
    Open,
    /// Open but not the foreground window: bring it forward.
    Focus,
    /// Open and in front: a second press closes it.
    Close,
}

pub fn shortcut_action(visible: bool, foreground: bool) -> Toggle {
    match (visible, foreground) {
        (false, _) => Toggle::Open,
        (true, false) => Toggle::Focus,
        (true, true) => Toggle::Close,
    }
}

/// Whether a tray click counts the panel as open. The blur arrives at
/// mouse-down and the click is decided at mouse-up, so this is read at
/// mouse-down: the panel is visible, or has only just hidden itself because
/// that very click took its focus.
pub fn open_at_click(visible: bool, since_blur_hide: Option<Duration>, guard: Duration) -> bool {
    visible || since_blur_hide.is_some_and(|t| t < guard)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickAction {
    Open,
    Close,
    Nothing,
}

/// The tray click at mouse-up, from what mouse-down saw (`was_open`) and the
/// panel now.
pub fn click_action(was_open: bool, visible: bool) -> ClickAction {
    match (was_open, visible) {
        (false, false) => ClickAction::Open,
        (true, true) => ClickAction::Close,
        _ => ClickAction::Nothing,
    }
}

/// Turning "Keep open" off hides a panel that is shown but not in front:
/// nothing would ever blur it now.
pub fn hide_on_keep_off(keep_now: bool, visible: bool, foreground: bool, can_blur: bool) -> bool {
    !keep_now && visible && !foreground && can_blur
}

/// Failed icon lookups are tried at most this many times in all...
pub const ICON_TRIES: u8 = 3;
/// ...and no sooner than this after the previous try.
pub const ICON_RETRY_AFTER: Duration = Duration::from_millis(1500);

/// Whether a lookup for a path may start: never asked, or tried fewer than
/// `ICON_TRIES` times and the last try is old enough.
pub fn icon_try_due(tries: u8, since_last: Option<Duration>) -> bool {
    match since_last {
        None => tries == 0,
        Some(t) => tries < ICON_TRIES && t >= ICON_RETRY_AFTER,
    }
}

/// Requests that reach the app before its window exists (a second start or
/// `--quit` while it is starting): kept, and applied once the window is there.
#[derive(Default)]
pub struct Pending {
    show: AtomicBool,
    quit: AtomicBool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingAction {
    Quit,
    Show,
}

impl Pending {
    pub const fn new() -> Self {
        Self {
            show: AtomicBool::new(false),
            quit: AtomicBool::new(false),
        }
    }

    pub fn request_show(&self) {
        self.show.store(true, Ordering::SeqCst);
    }

    pub fn request_quit(&self) {
        self.quit.store(true, Ordering::SeqCst);
    }

    /// The next request to apply, if `ready` (the window exists). A quit
    /// wins over a show. Requests stay kept while not ready.
    pub fn take(&self, ready: bool) -> Option<PendingAction> {
        if !ready {
            return None;
        }
        if self.quit.swap(false, Ordering::SeqCst) {
            self.show.store(false, Ordering::SeqCst);
            return Some(PendingAction::Quit);
        }
        self.show
            .swap(false, Ordering::SeqCst)
            .then_some(PendingAction::Show)
    }
}

/// Lets an event through at most once per `every`.
pub struct Throttle {
    every: Duration,
    last: Mutex<Option<Instant>>,
}

impl Throttle {
    pub const fn new(every: Duration) -> Self {
        Self {
            every,
            last: Mutex::new(None),
        }
    }

    pub fn allow(&self, now: Instant) -> bool {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.is_some_and(|t| now.saturating_duration_since(t) < self.every) {
            return false;
        }
        *last = Some(now);
        true
    }
}

/// `PULSE_BATTERY=<percent>,<plugged 0|1>` (a development knob): a pretend battery.
pub fn parse_battery(v: &str) -> Option<(u8, bool)> {
    let (pct, plugged) = v.split_once(',')?;
    let pct = pct.trim().parse::<u8>().ok().filter(|p| *p <= 100)?;
    let plugged = match plugged.trim() {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    Some((pct, plugged))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_battery_hook_parses() {
        assert_eq!(parse_battery("12,0"), Some((12, false)));
        assert_eq!(parse_battery(" 80 , 1 "), Some((80, true)));
        for bad in ["", "12", "101,0", "x,1", "12,2", "-1,0", "12,"] {
            assert_eq!(parse_battery(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_failed_save_is_tried_once_more() {
        let mut calls = 0;
        assert!(save_with_retry(|| {
            calls += 1;
            if calls == 1 { Err("busy") } else { Ok(()) }
        }));
        assert_eq!(calls, 2);
        let mut calls = 0;
        assert!(!save_with_retry(|| {
            calls += 1;
            Err::<(), _>("locked")
        }));
        assert_eq!(calls, 2);
        let mut calls = 0;
        assert!(save_with_retry(|| {
            calls += 1;
            Ok::<(), ()>(())
        }));
        assert_eq!(calls, 1);
    }

    #[test]
    fn the_shortcut_focuses_before_it_hides() {
        assert_eq!(shortcut_action(false, false), Toggle::Open);
        assert_eq!(shortcut_action(true, false), Toggle::Focus);
        assert_eq!(shortcut_action(true, true), Toggle::Close);
    }

    #[test]
    fn a_slow_tray_click_still_closes_the_panel() {
        let guard = Duration::from_millis(300);
        // Blur hid it at mouse-down; the click ends 2 s later.
        let was_open = open_at_click(false, Some(Duration::from_millis(20)), guard);
        assert!(was_open);
        assert_eq!(click_action(was_open, false), ClickAction::Nothing);
        // Blur did not hide it (Keep open): the click closes it.
        let was_open = open_at_click(true, None, guard);
        assert_eq!(click_action(was_open, true), ClickAction::Close);
        // Hidden a while ago: the click opens it, however long it is held.
        let was_open = open_at_click(false, Some(Duration::from_secs(5)), guard);
        assert!(!was_open);
        assert_eq!(click_action(was_open, false), ClickAction::Open);
        assert_eq!(
            click_action(open_at_click(false, None, guard), false),
            ClickAction::Open
        );
        // Opened by the shortcut while the button was down: leave it.
        assert_eq!(click_action(false, true), ClickAction::Nothing);
    }

    #[test]
    fn keep_open_off_hides_a_panel_nobody_is_in() {
        assert!(hide_on_keep_off(false, true, false, true));
        assert!(!hide_on_keep_off(false, true, true, true));
        assert!(!hide_on_keep_off(false, false, false, true));
        assert!(!hide_on_keep_off(true, true, false, true));
        // Test runs that keep the panel open on blur stay open.
        assert!(!hide_on_keep_off(false, true, false, false));
    }

    #[test]
    fn icon_lookups_retry_at_most_three_times_a_second_and_a_half_apart() {
        assert!(icon_try_due(0, None));
        let ms = |n| Some(Duration::from_millis(n));
        assert!(!icon_try_due(1, ms(1499)));
        assert!(icon_try_due(1, ms(1500)));
        assert!(icon_try_due(2, ms(9000)));
        assert!(!icon_try_due(3, ms(60_000)));
        // Asked for, but the count is not known to be zero: no stacking.
        assert!(!icon_try_due(1, None));
    }

    #[test]
    fn requests_before_the_window_exists_are_kept() {
        let p = Pending::new();
        assert_eq!(p.take(true), None);
        p.request_show();
        assert_eq!(p.take(false), None);
        assert_eq!(p.take(true), Some(PendingAction::Show));
        assert_eq!(p.take(true), None);
        p.request_show();
        p.request_quit();
        assert_eq!(p.take(false), None);
        assert_eq!(p.take(true), Some(PendingAction::Quit));
        assert_eq!(p.take(true), None, "a quit drops the show");
    }

    #[test]
    fn the_throttle_lets_one_event_through_per_interval() {
        let t = Throttle::new(Duration::from_secs(1));
        let t0 = Instant::now();
        assert!(t.allow(t0));
        assert!(!t.allow(t0 + Duration::from_millis(999)));
        assert!(t.allow(t0 + Duration::from_millis(1000)));
        assert!(!t.allow(t0 + Duration::from_millis(1500)));
        assert!(t.allow(t0 + Duration::from_millis(2100)));
    }
}
