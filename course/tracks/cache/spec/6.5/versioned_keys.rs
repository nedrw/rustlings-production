//! Spec for `cache/6.5-versioned-keys` — a namespace is invalidated by moving its prefix, not by
//! walking the cache.

use cache::Store;
use cache::versioned::Versioned;

fn fixture() -> (Store, Versioned) {
    let mut store = Store::new();
    store.put("a", "first");
    store.put("b", "second");
    (store, Versioned::new(60))
}

#[test]
fn a_second_read_in_the_same_version_is_a_hit() {
    let (mut store, mut cache) = fixture();
    cache.get(&mut store, "subscribers", "a", 0);
    assert_eq!(
        cache.get(&mut store, "subscribers", "a", 1),
        Some("first".to_owned())
    );
    assert_eq!(store.reads(), 1);
}

#[test]
fn a_bump_makes_the_namespace_unreachable() {
    let (mut store, mut cache) = fixture();
    cache.get(&mut store, "subscribers", "a", 0);
    store.put("a", "second version");

    cache.bump("subscribers");
    assert_eq!(
        cache.get(&mut store, "subscribers", "a", 1),
        Some("second version".to_owned())
    );
    assert_eq!(store.reads(), 2, "the old entry was still reachable");
}

#[test]
fn a_bump_does_not_walk_the_cache() {
    let (mut store, mut cache) = fixture();
    cache.get(&mut store, "subscribers", "a", 0);
    let before = cache.entries();
    cache.bump("subscribers");
    assert_eq!(cache.entries(), before, "the bump swept the cache");
}

#[test]
fn another_namespace_is_unaffected() {
    let (mut store, mut cache) = fixture();
    cache.get(&mut store, "subscribers", "a", 0);
    cache.get(&mut store, "newsletters", "a", 0);
    cache.bump("subscribers");

    assert_eq!(
        cache.get(&mut store, "newsletters", "a", 1),
        Some("first".to_owned())
    );
    assert_eq!(store.reads(), 2, "bumping one namespace dropped another");
}

// Pins given code: `version` and `bump` are in the scaffold; the prefix work in `get` is the step.
#[test]
fn a_never_bumped_namespace_is_version_zero() {
    let (_, cache) = fixture();
    assert_eq!(cache.version("subscribers"), 0);
}

// Pins given code: `bump`; a namespace that is never read through `get` still counts its bumps.
#[test]
fn the_version_only_moves_forward() {
    let (_, mut cache) = fixture();
    cache.bump("subscribers");
    cache.bump("subscribers");
    assert_eq!(cache.version("subscribers"), 2);
}
