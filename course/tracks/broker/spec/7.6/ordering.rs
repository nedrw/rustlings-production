//! Spec for `broker/7.6-ordering` — one key per group that must stay ordered, and no grouping
//! where nothing has to be ordered.

use std::collections::BTreeSet;

use broker::ordering::plan;

fn groups(keys: &std::collections::BTreeMap<String, String>) -> BTreeSet<&str> {
    keys.values().map(String::as_str).collect()
}

#[test]
fn a_chain_becomes_one_group() {
    let keys = plan(&["a", "b", "c"], &[("a", "b"), ("b", "c")]);
    assert_eq!(groups(&keys).len(), 1, "{keys:?}");
}

#[test]
fn two_independent_pairs_are_two_groups() {
    let keys = plan(&["a", "b", "c", "d"], &[("a", "b"), ("c", "d")]);
    assert_eq!(groups(&keys).len(), 2, "{keys:?}");
    assert_eq!(keys["a"], keys["b"]);
    assert_eq!(keys["c"], keys["d"]);
    assert_ne!(keys["a"], keys["c"]);
}

#[test]
fn nothing_to_order_means_no_grouping() {
    let keys = plan(&["a", "b", "c"], &[]);
    assert_eq!(groups(&keys).len(), 3, "{keys:?}");
    for event in ["a", "b", "c"] {
        assert_eq!(keys[event], event);
    }
}

#[test]
fn the_groups_key_is_the_smallest_name_in_it() {
    let keys = plan(&["b", "a"], &[("b", "a")]);
    assert_eq!(keys["a"], "a");
    assert_eq!(keys["b"], "a");
}

#[test]
fn a_rule_naming_an_unknown_event_is_ignored() {
    let keys = plan(&["a"], &[("a", "ghost")]);
    assert_eq!(keys.len(), 1);
    assert_eq!(keys["a"], "a");
}

#[test]
fn every_event_gets_a_key() {
    let events = ["a", "b", "c", "d", "e"];
    let keys = plan(&events, &[("a", "c"), ("d", "e")]);
    assert_eq!(keys.len(), 5);
    for event in events {
        assert!(keys.contains_key(event), "{event} has no key");
    }
}
