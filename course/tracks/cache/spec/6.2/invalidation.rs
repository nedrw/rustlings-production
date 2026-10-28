//! Spec for `cache/6.2-invalidation` — a write goes to the store first, then the cache is told
//! that what it holds is no longer true.

use cache::invalidation::write;
use cache::{Cache, Store};

/// A store with two keys, both cached.
fn filled() -> (Store, Cache) {
    let mut store = Store::new();
    store.put("a", "old");
    store.put("b", "other");
    let mut cache = Cache::new(60);
    cache.get(&mut store, "a", 0);
    cache.get(&mut store, "b", 0);
    (store, cache)
}

#[test]
fn a_write_updates_the_store_and_drops_the_entry() {
    let (mut store, mut cache) = filled();
    let reads_before = store.reads();

    write(&mut store, &mut cache, "a", "new");

    assert_eq!(cache.len(), 1, "the written key is still cached");
    assert_eq!(store.reads(), reads_before, "the write read the store");

    assert_eq!(cache.get(&mut store, "a", 1), Some("new".to_owned()));
    assert_eq!(
        store.reads(),
        reads_before + 1,
        "the cache served the old value: it was updated, not invalidated"
    );
}

#[test]
fn the_store_holds_the_new_value() {
    let (mut store, mut cache) = filled();
    write(&mut store, &mut cache, "a", "new");
    assert_eq!(store.get("a"), Some("new".to_owned()));
}

#[test]
fn writing_a_key_that_is_not_cached_is_harmless() {
    let (mut store, mut cache) = filled();
    write(&mut store, &mut cache, "c", "fresh");
    assert_eq!(cache.len(), 2, "an unrelated cached key was dropped");
    assert_eq!(store.get("c"), Some("fresh".to_owned()));
}

#[test]
fn a_write_leaves_the_other_keys_cached() {
    let (mut store, mut cache) = filled();
    let reads_before = store.reads();
    write(&mut store, &mut cache, "a", "new");

    assert_eq!(cache.get(&mut store, "b", 2), Some("other".to_owned()));
    assert_eq!(
        store.reads(),
        reads_before,
        "writing one key emptied another"
    );
}

#[test]
fn a_write_then_read_twice_still_sees_the_new_value() {
    let (mut store, mut cache) = filled();
    write(&mut store, &mut cache, "a", "new");
    assert_eq!(cache.get(&mut store, "a", 1), Some("new".to_owned()));
    assert_eq!(cache.get(&mut store, "a", 2), Some("new".to_owned()));
    assert_eq!(
        store.reads(),
        3,
        "the refilled entry was dropped or refetched twice"
    );
}
