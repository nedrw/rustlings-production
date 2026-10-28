//! probe = "cache/expiry-jitter@apply"
//! title = "One second of fills, many seconds of expiries"
//! hint = "A deterministic per-key spread inside base ± spread: the same key always answers the same value, and keys filled together do not die together."
//!
//! Step `cache/6.4-expiry-jitter` spreads a warmed cache's expiries so the store sees a drizzle
//! instead of a wave. This probe asks for the schedule itself: given a base, a spread and a list of
//! keys filled at the same instant, when does each one expire?
//!
//! ## What you write
//!
//! [`schedule`] — one `(key, ttl)` pair per key, in the order given:
//!
//! * `spread == 0` gives every key `base`, exactly;
//! * otherwise the TTL is inside `base - spread ..= base + spread`, decided by the key itself: the
//!   same key always gets the same TTL, and keys sharing a base do not all get one value;
//! * a spread wider than the base saturates at zero rather than wrapping.
//!
//! ## What the tests ask
//!
//! Zero spread is the base; the band holds for fifty keys; the same key twice is the same TTL;
//! twenty keys produce more than one TTL; and a wide spread stays under `base + spread`.

use std::collections::BTreeSet;

/// The TTL each key gets, in the order given.
#[must_use]
pub fn schedule<'a>(base: u64, spread: u64, keys: &[&'a str]) -> Vec<(&'a str, u64)> {
    keys.iter()
        .map(|key| (*key, ttl_for(base, spread, key)))
        .collect()
}

/// A stable hash places each key in the band; the same key always lands in the same place.
fn ttl_for(base: u64, spread: u64, key: &str) -> u64 {
    if spread == 0 {
        return base;
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let span = u128::from(spread) * 2 + 1;
    let offset = u128::from(hash) % span;
    let signed = i128::try_from(offset).unwrap_or(i128::MAX) - i128::from(spread);
    let ttl = i128::from(base) + signed;
    u64::try_from(ttl.max(0)).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_spread_is_the_base() {
        let keys = ["a", "b", "a-longer-key"];
        let ttls = schedule(60, 0, &keys);
        assert_eq!(ttls.len(), 3);
        assert!(ttls.iter().all(|(_, ttl)| *ttl == 60), "{ttls:?}");
    }

    #[test]
    fn every_ttl_is_inside_the_band() {
        let keys: Vec<String> = (0..50).map(|index| format!("key-{index}")).collect();
        let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
        for (key, ttl) in schedule(60, 10, &borrowed) {
            assert!((50..=70).contains(&ttl), "{key}: {ttl}");
        }
    }

    #[test]
    fn the_same_key_gets_the_same_ttl() {
        let first = schedule(60, 10, &["key-7"]);
        let second = schedule(60, 10, &["key-7"]);
        assert_eq!(first, second);
    }

    #[test]
    fn keys_filled_together_do_not_expire_together() {
        let keys: Vec<String> = (0..20).map(|index| format!("key-{index}")).collect();
        let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
        let ttls: BTreeSet<u64> = schedule(60, 10, &borrowed)
            .into_iter()
            .map(|(_, ttl)| ttl)
            .collect();
        assert!(ttls.len() > 1, "every key got one ttl: {ttls:?}");
    }

    #[test]
    fn a_wide_spread_stays_under_the_top_of_the_band() {
        for (key, ttl) in schedule(5, 100, &["a", "b", "c", "d"]) {
            assert!(ttl <= 105, "{key}: {ttl}");
        }
    }

    #[test]
    fn the_order_of_the_answer_is_the_order_of_the_keys() {
        let keys = ["first", "second", "third"];
        let answer: Vec<&str> = schedule(60, 10, &keys)
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        assert_eq!(answer, keys);
    }
}
