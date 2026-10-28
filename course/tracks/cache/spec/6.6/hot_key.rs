//! Spec for `cache/6.6-hot-key` — a local layer in front of a shared one, with the staleness budget
//! written down.

use cache::Store;
use cache::two_level::TwoLevel;

fn fixture() -> (Store, TwoLevel) {
    let mut store = Store::new();
    store.put("hot", "value");
    // Local entries live 3 seconds, shared ones 30: an instance may serve data 3 seconds old.
    (store, TwoLevel::new(3, 30))
}

#[test]
fn the_first_read_misses_and_fills_both_layers() {
    let (mut store, mut cache) = fixture();
    assert_eq!(cache.get(&mut store, "hot", 0), Some("value".to_owned()));
    assert_eq!(cache.misses(), 1);
    assert_eq!(store.reads(), 1);
}

#[test]
fn a_local_hit_does_not_consult_the_shared_layer() {
    let (mut store, mut cache) = fixture();
    cache.get(&mut store, "hot", 0);
    assert_eq!(cache.get(&mut store, "hot", 1), Some("value".to_owned()));
    assert_eq!(cache.local_hits(), 1);
    assert_eq!(
        cache.shared_hits(),
        0,
        "the local hit reached the shared layer"
    );
    assert_eq!(store.reads(), 1);
}

#[test]
fn after_the_local_ttl_the_shared_layer_answers_and_refills_local() {
    let (mut store, mut cache) = fixture();
    cache.get(&mut store, "hot", 0);
    // At 3 the local entry is expired (the deadline second is not valid) and the shared one is not.
    assert_eq!(cache.get(&mut store, "hot", 3), Some("value".to_owned()));
    assert_eq!(cache.shared_hits(), 1);
    assert_eq!(cache.misses(), 1, "a shared hit was counted as a miss");
    assert_eq!(store.reads(), 1);
    // The local layer was refilled: at 4 it answers again.
    assert_eq!(cache.get(&mut store, "hot", 4), Some("value".to_owned()));
    assert_eq!(cache.local_hits(), 1);
}

#[test]
fn after_both_ttls_the_store_answers_again() {
    let (mut store, mut cache) = fixture();
    cache.get(&mut store, "hot", 0);
    assert_eq!(cache.get(&mut store, "hot", 30), Some("value".to_owned()));
    assert_eq!(cache.misses(), 2);
    assert_eq!(store.reads(), 2);
}

#[test]
fn the_staleness_budget_is_the_local_ttl() {
    let (_, cache) = fixture();
    assert_eq!(cache.staleness_budget(), 3);
}
