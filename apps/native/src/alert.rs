//! The low memory alert as plain logic: when to notify, and what the notice
//! says. No toolkit, no Windows types and no clock of its own (the caller
//! passes the time in), so every rule is unit tested on any machine.

use pulse_platform::{DEFAULT_LOW_MEM_LEVEL, LOW_MEM_LEVELS};
use pulse_readings::AppRow;
use std::time::{Duration, Instant};

/// How the alert waits. The real values are `Rules::NORMAL`; `Rules::TEST`
/// is what `PULSE_MEM_PCT` switches to, so the alert can be triggered on
/// purpose without a minute of waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// Memory must stay at or above the level this long before the first notice.
    pub sustain: Duration,
    /// Nothing fires this soon after Pulse started.
    pub warmup: Duration,
    /// Never two notices closer than this.
    pub cooldown: Duration,
    /// After a notice, memory must fall this many points below the level
    /// (and rise to it again) before the next one.
    pub rearm_drop: u8,
}

impl Rules {
    pub const NORMAL: Rules = Rules {
        sustain: Duration::from_secs(30),
        warmup: Duration::from_secs(60),
        cooldown: Duration::from_secs(600),
        rearm_drop: 3,
    };

    pub const TEST: Rules = Rules {
        sustain: Duration::from_secs(3),
        warmup: Duration::from_secs(4),
        cooldown: Duration::from_secs(20),
        rearm_drop: 3,
    };
}

/// The level for Settings' choice number `i` (0 to 3), or the default.
pub fn level_at(i: i32) -> u8 {
    usize::try_from(i)
        .ok()
        .and_then(|i| LOW_MEM_LEVELS.get(i))
        .copied()
        .unwrap_or(DEFAULT_LOW_MEM_LEVEL)
}

/// Settings' choice number for a level (the default's when it is not one).
pub fn index_of(level: u8) -> i32 {
    let i = LOW_MEM_LEVELS.iter().position(|l| *l == level).or_else(|| {
        LOW_MEM_LEVELS
            .iter()
            .position(|l| *l == DEFAULT_LOW_MEM_LEVEL)
    });
    i.map_or(0, |i| i as i32)
}

/// `PULSE_MEM_PCT`: a whole percent from 0 to 100 that replaces the real
/// reading in the alert logic.
pub fn parse_forced_pct(text: &str) -> Option<u8> {
    text.trim().parse::<u8>().ok().filter(|p| *p <= 100)
}

/// Memory in use as a whole percent.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
pub fn percent(used: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    (used as f64 / total as f64 * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8
}

/// The alert's memory of what happened. One per run, owned by the reader.
#[derive(Debug)]
pub struct Alert {
    started: Instant,
    /// Since when memory has been at or above the level, without a break.
    above_since: Option<Instant>,
    /// A notice may fire (memory was below the level by `rearm_drop` points
    /// since the last one, or none was sent yet).
    armed: bool,
    last_fired: Option<Instant>,
    /// The level seen last; a change starts over.
    level: u8,
}

impl Alert {
    pub fn new(started: Instant) -> Self {
        Self {
            started,
            above_since: None,
            armed: true,
            last_fired: None,
            level: 0,
        }
    }

    /// One reading. `level` is 0 when the alert is off. True when a notice
    /// should be shown now.
    pub fn observe(&mut self, now: Instant, pct: u8, level: u8, rules: &Rules) -> bool {
        if level == 0 {
            self.above_since = None;
            self.level = 0;
            return false;
        }
        if level != self.level {
            // Switched on, or a new level: judge the new level on its own.
            self.level = level;
            self.above_since = None;
            self.armed = true;
        }
        if pct < level {
            self.above_since = None;
            if u16::from(pct) + u16::from(rules.rearm_drop) <= u16::from(level) {
                self.armed = true;
            }
            return false;
        }
        let since = *self.above_since.get_or_insert(now);
        let held = now.saturating_duration_since(since) >= rules.sustain;
        let warm = now.saturating_duration_since(self.started) >= rules.warmup;
        let rested = self
            .last_fired
            .is_none_or(|t| now.saturating_duration_since(t) >= rules.cooldown);
        if self.armed && held && warm && rested {
            self.armed = false;
            self.last_fired = Some(now);
            return true;
        }
        false
    }
}

/// The notice's title.
pub fn title(pct: u8) -> String {
    format!("Memory is {pct}% full")
}

/// The notice's body: the app using the most memory. `None` when there is no
/// app list.
pub fn body(apps: &[AppRow]) -> Option<String> {
    let top = apps.iter().max_by_key(|a| a.mem_bytes)?;
    Some(format!(
        "{} is using the most: {}",
        top.name,
        crate::view::format_bytes(top.mem_bytes)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rules = Rules::NORMAL;

    fn at(start: Instant, s: u64) -> Instant {
        start + Duration::from_secs(s)
    }

    /// Feeds one reading per second from `from` to `to` (inclusive) and
    /// returns the seconds at which a notice fired.
    fn run(a: &mut Alert, t0: Instant, from: u64, to: u64, pct: u8, level: u8) -> Vec<u64> {
        (from..=to)
            .filter(|s| a.observe(at(t0, *s), pct, level, &R))
            .collect()
    }

    fn app(name: &str, mb: u64) -> AppRow {
        AppRow {
            name: name.to_owned(),
            mem_bytes: mb << 20,
            ..AppRow::default()
        }
    }

    #[test]
    fn nothing_fires_in_the_first_minute() {
        let t0 = Instant::now();
        let mut a = Alert::new(t0);
        assert!(run(&mut a, t0, 0, 59, 99, 90).is_empty());
    }

    #[test]
    fn fires_once_the_level_has_held_for_the_sustain_time() {
        let t0 = Instant::now();
        let mut a = Alert::new(t0);
        // Above from the start; the warm-up (60 s) is the later gate.
        assert_eq!(run(&mut a, t0, 0, 100, 92, 90), vec![60]);
        // Starting above only after the warm-up: 30 s later.
        let mut b = Alert::new(t0);
        assert!(run(&mut b, t0, 0, 69, 50, 90).is_empty());
        assert_eq!(run(&mut b, t0, 70, 130, 92, 90), vec![100]);
    }

    #[test]
    fn a_dip_below_the_level_restarts_the_sustain_time() {
        let t0 = Instant::now();
        let mut a = Alert::new(t0);
        assert!(run(&mut a, t0, 100, 120, 92, 90).is_empty());
        assert!(run(&mut a, t0, 121, 121, 89, 90).is_empty());
        // 20 s above again is not 30 s.
        assert!(run(&mut a, t0, 122, 141, 92, 90).is_empty());
        assert_eq!(run(&mut a, t0, 142, 160, 92, 90), vec![152]);
    }

    #[test]
    fn exactly_the_level_counts() {
        let t0 = Instant::now();
        let mut a = Alert::new(t0);
        assert_eq!(run(&mut a, t0, 100, 140, 90, 90), vec![130]);
    }

    #[test]
    fn it_does_not_repeat_while_memory_stays_high() {
        let t0 = Instant::now();
        let mut a = Alert::new(t0);
        assert_eq!(run(&mut a, t0, 100, 5000, 95, 90), vec![130]);
    }

    #[test]
    fn a_small_dip_does_not_rearm_but_a_real_drop_does() {
        let t0 = Instant::now();
        let mut a = Alert::new(t0);
        assert_eq!(run(&mut a, t0, 100, 140, 95, 90), vec![130]);
        // 2 points under the level is not "a few points below".
        assert!(run(&mut a, t0, 141, 150, 88, 90).is_empty());
        assert!(run(&mut a, t0, 151, 1000, 95, 90).is_empty());
        // 3 points under: armed again; the cooldown still holds until 730.
        assert!(run(&mut a, t0, 1001, 1010, 87, 90).is_empty());
        assert_eq!(run(&mut a, t0, 1011, 1100, 95, 90), vec![1041]);
    }

    #[test]
    fn never_more_often_than_every_ten_minutes() {
        let t0 = Instant::now();
        let mut a = Alert::new(t0);
        assert_eq!(run(&mut a, t0, 100, 140, 95, 90), vec![130]);
        // Drops and rises right away, held for 30 s: cooldown blocks it.
        assert!(run(&mut a, t0, 141, 150, 70, 90).is_empty());
        assert!(run(&mut a, t0, 151, 400, 95, 90).is_empty());
        // The first reading at or after 130 + 600 fires.
        assert_eq!(run(&mut a, t0, 401, 800, 95, 90), vec![730]);
    }

    #[test]
    fn off_never_fires_and_switching_on_starts_the_wait_afresh() {
        let t0 = Instant::now();
        let mut a = Alert::new(t0);
        assert!(run(&mut a, t0, 0, 500, 99, 0).is_empty());
        // On at 500 with memory already high: 30 s of holding first.
        assert_eq!(run(&mut a, t0, 500, 560, 99, 90), vec![530]);
    }

    #[test]
    fn a_new_level_is_judged_on_its_own() {
        let t0 = Instant::now();
        let mut a = Alert::new(t0);
        assert_eq!(run(&mut a, t0, 100, 140, 92, 90), vec![130]);
        // Raised to 95: 92 is below it, nothing. Lowered to 80 later: the
        // cooldown (to 730) still protects against a repeat.
        assert!(run(&mut a, t0, 141, 200, 92, 95).is_empty());
        assert!(run(&mut a, t0, 201, 700, 92, 80).is_empty());
        assert_eq!(run(&mut a, t0, 701, 800, 92, 80), vec![730]);
    }

    #[test]
    fn the_test_timings_are_short() {
        let t0 = Instant::now();
        let mut a = Alert::new(t0);
        let fired: Vec<u64> = (0..60)
            .filter(|s| a.observe(at(t0, *s), 93, 90, &Rules::TEST))
            .collect();
        // Held for 3 s and past the 4 s warm-up, then quiet while it stays high.
        assert_eq!(fired, vec![4]);
        // A drop and a rise again fires once the 20 s rest is over.
        let again: Vec<u64> = (60..70)
            .filter(|s| a.observe(at(t0, *s), if *s < 62 { 80 } else { 93 }, 90, &Rules::TEST))
            .collect();
        assert_eq!(again, vec![65]);
    }

    #[test]
    fn percent_rounds_and_handles_no_total() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(92, 100), 92);
        assert_eq!(percent(8_995, 10_000), 90);
        assert_eq!(percent(8_994, 10_000), 90);
        assert_eq!(percent(8_949, 10_000), 89);
        assert_eq!(percent(200, 100), 100);
    }

    #[test]
    fn forced_percent_parses_whole_percents_only() {
        assert_eq!(parse_forced_pct("93"), Some(93));
        assert_eq!(parse_forced_pct(" 0 "), Some(0));
        assert_eq!(parse_forced_pct("100"), Some(100));
        for bad in ["", "101", "-1", "9.5", "x", "300"] {
            assert_eq!(parse_forced_pct(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn choices_map_to_levels_and_back() {
        for (i, l) in LOW_MEM_LEVELS.into_iter().enumerate() {
            assert_eq!(level_at(i as i32), l);
            assert_eq!(index_of(l), i as i32);
        }
        assert_eq!(level_at(-1), 90);
        assert_eq!(level_at(9), 90);
        assert_eq!(index_of(91), 2);
    }

    #[test]
    fn the_notice_names_the_top_app() {
        assert_eq!(title(92), "Memory is 92% full");
        let apps = vec![
            app("Code", 900),
            app("Google Chrome", 6246),
            app("WSL (Linux)", 3000),
        ];
        assert_eq!(
            body(&apps).as_deref(),
            Some("Google Chrome is using the most: 6.1 GB")
        );
        let apps = vec![app("Google Chrome", 2000), app("WSL (Linux)", 8000)];
        assert_eq!(
            body(&apps).as_deref(),
            Some("WSL (Linux) is using the most: 7.8 GB")
        );
        assert_eq!(body(&[]), None);
    }
}
