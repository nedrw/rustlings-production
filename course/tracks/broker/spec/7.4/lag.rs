//! Spec for `broker/7.4-lag` — how far behind each partition is, and which ones are past the
//! threshold.

use std::collections::BTreeMap;

use broker::Topic;
use broker::lag::{backlog, behind, lag};

fn topic() -> Topic {
    let mut topic = Topic::new(3);
    for payload in ["one", "two"] {
        topic.append(None, payload);
    }
    topic.append(None, "three");
    topic.append(None, "four");
    topic
}

#[test]
fn lag_is_the_end_offset_minus_the_commit() {
    let topic = topic();
    let committed = BTreeMap::from([(0, 1), (1, 0)]);
    let lag = lag(&topic, &committed);
    assert_eq!(lag[&0], topic.end(0) - 1);
    assert_eq!(lag[&1], topic.end(1));
}

#[test]
fn a_partition_with_no_commit_is_fully_behind() {
    let topic = topic();
    let lag = lag(&topic, &BTreeMap::new());
    for partition in 0..3 {
        assert_eq!(lag[&partition], topic.end(partition));
    }
}

#[test]
fn a_commit_past_the_end_is_not_negative_lag() {
    let topic = topic();
    let committed = BTreeMap::from([(0, 999), (1, 0), (2, 0)]);
    let lag = lag(&topic, &committed);
    assert_eq!(lag[&0], 0, "a commit past the end went below zero");
}

#[test]
fn the_backlog_is_the_sum_of_the_lags() {
    let lag = BTreeMap::from([(0, 2), (1, 0), (2, 5)]);
    assert_eq!(backlog(&lag), 7);
}

#[test]
fn behind_lists_only_the_partitions_past_the_threshold() {
    let lag = BTreeMap::from([(0, 2), (1, 5), (2, 6)]);
    assert_eq!(behind(&lag, 5), [2], "the boundary is not behind");
    assert_eq!(behind(&lag, 4), [1, 2]);
    assert!(behind(&lag, 100).is_empty());
}
