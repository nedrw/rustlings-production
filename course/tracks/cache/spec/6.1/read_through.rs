//! Spec for `cache/6.1-read-through` — a hit does not touch the store, and an entry expires.
//!
//! The store counts its reads, so "the cache served this" is an assertion, not a feeling.

use cache::{Cache, Store};

fn fixture() -> (Store, Cache) {
    let mut store = Store::new();
    store.put("a", "first");
    store.put("b", "second");
    (store, Cache::new(10))
}

#[test]
fn a_miss_reads_the_store_once_and_caches_the_value() {
    let (mut store, mut cache) = fixture();
    assert_eq!(cache.get(&mut store, "a", 0), Some("first".to_owned()));
    assert_eq!(store.reads(), 1);
    assert_eq!(cache.len(), 1);
}

#[test]
fn a_hit_does_not_touch_the_store() {
    let (mut store, mut cache) = fixture();
    cache.get(&mut store, "a", 0);
    assert_eq!(cache.get(&mut store, "a", 5), Some("first".to_owned()));
    assert_eq!(store.reads(), 1, "a cached read went to the store");
}

#[test]
fn an_entry_is_valid_until_its_deadline_and_not_at_it() {
    let (mut store, mut cache) = fixture();
    cache.get(&mut store, "a", 0);

    // `Cache::new(10)` filled at 0: the deadline is 10, and the deadline second is expired.
    assert_eq!(cache.get(&mut store, "a", 9), Some("first".to_owned()));
    assert_eq!(store.reads(), 1);
    assert_eq!(cache.get(&mut store, "a", 10), Some("first".to_owned()));
    assert_eq!(
        store.reads(),
        2,
        "an expired entry was served from the cache"
    );
}

#[test]
fn a_key_the_store_does_not_have_is_not_cached() {
    let (mut store, mut cache) = fixture();
    assert_eq!(cache.get(&mut store, "missing", 0), None);
    assert_eq!(cache.len(), 0);
    assert_eq!(cache.get(&mut store, "missing", 1), None);
    assert_eq!(
        store.reads(),
        2,
        "a missing key was cached as a present one"
    );
}

#[test]
fn an_expired_entry_is_replaced_by_the_new_value() {
    let (mut store, mut cache) = fixture();
    cache.get(&mut store, "a", 0);
    store.put("a", "changed");

    assert_eq!(cache.get(&mut store, "a", 10), Some("changed".to_owned()));
    assert_eq!(cache.get(&mut store, "a", 11), Some("changed".to_owned()));
    assert_eq!(store.reads(), 2, "the old entry came back after expiry");
}

#[test]
fn keys_are_independent() {
    let (mut store, mut cache) = fixture();
    cache.get(&mut store, "a", 0);
    cache.get(&mut store, "b", 0);
    assert_eq!(cache.len(), 2);
    assert_eq!(store.reads(), 2);
    assert_eq!(cache.get(&mut store, "a", 1), Some("first".to_owned()));
    assert_eq!(cache.get(&mut store, "b", 1), Some("second".to_owned()));
    assert_eq!(store.reads(), 2);
}
