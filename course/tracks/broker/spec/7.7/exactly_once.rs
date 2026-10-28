//! Spec for `broker/7.7-exactly-once` — an effect per record, once, however many times the range
//! is processed.

use broker::Topic;
use broker::exactly_once::{Sink, process};

fn topic() -> Topic {
    let mut topic = Topic::new(1);
    for payload in ["one", "two", "three"] {
        topic.append(Some("k"), payload);
    }
    topic
}

#[test]
fn one_pass_applies_every_record_in_order() {
    let topic = topic();
    let mut sink = Sink::new();
    assert_eq!(process(&topic, &mut sink, 0, 0, 3), 3);
    let payloads: Vec<&str> = sink
        .effects()
        .iter()
        .map(|effect| effect.payload.as_str())
        .collect();
    assert_eq!(payloads, ["one", "two", "three"]);
}

#[test]
fn a_replay_of_the_same_range_applies_nothing() {
    let topic = topic();
    let mut sink = Sink::new();
    process(&topic, &mut sink, 0, 0, 3);
    assert_eq!(
        process(&topic, &mut sink, 0, 0, 3),
        0,
        "a record was applied twice"
    );
    assert_eq!(sink.effects().len(), 3);
}

#[test]
fn a_partial_pass_resumes_without_duplicating() {
    let topic = topic();
    let mut sink = Sink::new();
    assert_eq!(process(&topic, &mut sink, 0, 0, 2), 2);
    assert_eq!(
        process(&topic, &mut sink, 0, 0, 3),
        1,
        "the first two were re-applied"
    );
    assert_eq!(sink.effects().len(), 3);
    assert_eq!(sink.effects()[2].offset, 2);
}

#[test]
fn the_end_is_clamped_to_the_log() {
    let topic = topic();
    let mut sink = Sink::new();
    assert_eq!(process(&topic, &mut sink, 0, 1, 99), 2);
    assert_eq!(sink.effects()[0].payload, "two");
}

#[test]
fn the_effect_remembers_where_it_came_from() {
    let topic = topic();
    let mut sink = Sink::new();
    process(&topic, &mut sink, 0, 1, 2);
    let effect = &sink.effects()[0];
    assert_eq!(effect.partition, 0);
    assert_eq!(effect.offset, 1);
    assert!(sink.applied(0, 1));
    assert!(!sink.applied(0, 0));
}

#[test]
fn a_partition_the_log_does_not_have_is_empty() {
    let topic = topic();
    let mut sink = Sink::new();
    assert_eq!(process(&topic, &mut sink, 9, 0, 4), 0);
    assert!(sink.effects().is_empty());
}
