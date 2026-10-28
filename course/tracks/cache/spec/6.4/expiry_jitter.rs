//! Spec for `cache/6.4-expiry-jitter` — entries that share a base do not all expire at once.

use std::collections::BTreeSet;

use cache::expiry::ttl_for;

#[test]
fn a_spread_of_zero_is_the_base() {
    for key in ["a", "b", "a-longer-key"] {
        assert_eq!(ttl_for(60, 0, key), 60, "{key}");
    }
}

#[test]
fn the_ttl_stays_inside_the_band() {
    for index in 0..50 {
        let key = format!("key-{index}");
        let ttl = ttl_for(60, 10, &key);
        assert!((50..=70).contains(&ttl), "{key}: {ttl}");
    }
}

#[test]
fn the_same_key_gets_the_same_ttl() {
    assert_eq!(ttl_for(60, 10, "key-7"), ttl_for(60, 10, "key-7"));
}

#[test]
fn keys_sharing_a_base_do_not_all_expire_together() {
    let ttls: BTreeSet<u64> = (0..20)
        .map(|index| ttl_for(60, 10, &format!("key-{index}")))
        .collect();
    assert!(ttls.len() > 1, "every key got the same ttl: {ttls:?}");
}

#[test]
fn a_spread_wider_than_the_base_cannot_go_below_zero() {
    // Wide spreads are a configuration mistake, not a panic: the lower edge saturates at zero.
    let ttl = ttl_for(5, 100, "key");
    assert!(ttl <= 105, "{ttl}");
}

#[test]
fn different_bases_move_the_whole_band() {
    let small = ttl_for(10, 2, "key-3");
    let large = ttl_for(100, 2, "key-3");
    assert!((8..=12).contains(&small), "{small}");
    assert!((98..=102).contains(&large), "{large}");
}
