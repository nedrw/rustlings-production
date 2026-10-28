//! Spec for `broker/7.2-consumer-offsets` — commit after processing, resume at the commit, and
//! lose only what was not committed.

use std::collections::BTreeMap;

use broker::Topic;
use broker::consumer::Consumer;

fn topic() -> Topic {
    let mut topic = Topic::new(1);
    for payload in ["one", "two", "three"] {
        topic.append(Some("k"), payload);
    }
    topic
}

// Pins given code: `poll` walks a partition in order; the step is `commit`, which the other tests fail on.
#[test]
fn polling_returns_the_records_in_order() {
    let topic = topic();
    let mut consumer = Consumer::new();
    let payloads: Vec<String> = (0..3)
        .filter_map(|_| consumer.poll(&topic, 0))
        .map(|record| record.payload)
        .collect();
    assert_eq!(payloads, ["one", "two", "three"]);
    assert!(consumer.poll(&topic, 0).is_none(), "poll ran past the end");
}

#[test]
fn a_committed_consumer_resumes_where_it_stopped() {
    let topic = topic();
    let mut consumer = Consumer::new();
    let first = consumer.poll(&topic, 0).expect("a record");
    assert_eq!(first.payload, "one");
    consumer.commit(&topic, 0, 1);

    let mut restarted = Consumer::resume(&BTreeMap::from([(0, 1)]));
    let next = restarted.poll(&topic, 0).expect("a record");
    assert_eq!(next.payload, "two");
}

// Pins the at-least-once baseline: with nothing committed a restart begins at zero — true even before `commit` is written.
#[test]
fn uncommitted_progress_is_delivered_again() {
    let topic = topic();
    let mut consumer = Consumer::new();
    consumer.poll(&topic, 0);
    consumer.poll(&topic, 0);

    // Nothing was committed: a restart begins at the beginning, which is at-least-once.
    let mut restarted = Consumer::resume(&BTreeMap::new());
    let first = restarted.poll(&topic, 0).expect("a record");
    assert_eq!(first.payload, "one");
}

#[test]
fn a_commit_past_the_end_is_clamped_to_the_end() {
    let topic = topic();
    let mut consumer = Consumer::new();
    consumer.commit(&topic, 0, 99);
    assert_eq!(consumer.committed(0), 3);
}

#[test]
fn committing_a_partition_the_topic_does_not_have_is_ignored() {
    let topic = topic();
    let mut consumer = Consumer::new();
    consumer.commit(&topic, 9, 5);
    assert_eq!(consumer.committed(9), 0);
    assert!(consumer.poll(&topic, 9).is_none());
}

// Pins given code: positions live in the consumer, not the topic.
#[test]
fn two_consumers_do_not_share_a_position() {
    let topic = topic();
    let mut one = Consumer::new();
    let mut two = Consumer::new();
    one.poll(&topic, 0);
    assert_eq!(two.poll(&topic, 0).unwrap().payload, "one");
}
