//! Network byte deltas from per-interface counters. Pure code, so it is tested everywhere.

use std::collections::HashMap;

/// Lifetime (received, sent) octets per interface, keyed by `InterfaceLuid`.
pub(crate) type Counters = HashMap<u64, (u64, u64)>;

/// Bytes (received, sent) moved since `prev`.
///
/// Only interfaces present in both reads count: one that just came up has no
/// baseline (its lifetime counter is not traffic of this interval) and one
/// that went away took its counter with it. A counter that went backwards
/// (reset, driver reload) counts as 0 for that interface.
pub(crate) fn delta(prev: &Counters, cur: &Counters) -> (u64, u64) {
    let (mut down, mut up) = (0u64, 0u64);
    for (luid, &(in_now, out_now)) in cur {
        if let Some(&(in_then, out_then)) = prev.get(luid) {
            down = down.saturating_add(in_now.saturating_sub(in_then));
            up = up.saturating_add(out_now.saturating_sub(out_then));
        }
    }
    (down, up)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(items: &[(u64, u64, u64)]) -> Counters {
        items.iter().map(|&(l, i, o)| (l, (i, o))).collect()
    }

    #[test]
    fn steady_interfaces_sum_their_own_deltas() {
        let prev = c(&[(1, 100, 10), (2, 1000, 50)]);
        let cur = c(&[(1, 150, 15), (2, 1300, 60)]);
        assert_eq!(delta(&prev, &cur), (350, 15));
    }

    #[test]
    fn an_adapter_coming_up_adds_nothing_this_tick() {
        let prev = c(&[(1, 100, 10)]);
        let cur = c(&[(1, 150, 15), (2, 25_000_000_000, 9_000_000_000)]);
        assert_eq!(delta(&prev, &cur), (50, 5));
    }

    #[test]
    fn an_adapter_going_down_does_not_cancel_real_traffic() {
        let prev = c(&[(1, 100, 10), (2, 5_000_000, 5_000_000)]);
        let cur = c(&[(1, 600, 110)]);
        assert_eq!(delta(&prev, &cur), (500, 100));
    }

    #[test]
    fn a_counter_that_went_backwards_counts_as_zero() {
        let prev = c(&[(1, 1000, 1000), (2, 10, 10)]);
        let cur = c(&[(1, 5, 5), (2, 20, 30)]);
        assert_eq!(delta(&prev, &cur), (10, 20));
    }

    #[test]
    fn nothing_before_means_nothing_now() {
        assert_eq!(delta(&Counters::new(), &c(&[(1, 9, 9)])), (0, 0));
    }
}
