//! Spec for `broker/7.5-replay` — a replay reads a range and leaves every position alone.

use broker::Topic;
use broker::consumer::Consumer;
use broker::replay::Replay;

fn topic() -> Topic {
    let mut topic = Topic::new(2);
    for payload in ["one", "two", "three", "four"] {
        topic.append(Some("k"), payload);
    }
    topic
}

#[test]
fn a_replay_reads_the_half_open_range_in_order() {
    let topic = topic();
    let mut replay = Replay::new(&topic, 0, 1, 3);
    let payloads: Vec<String> = std::iter::from_fn(|| replay.next_record())
        .map(|record| record.payload)
        .collect();
    assert_eq!(
        payloads,
        ["two", "three"],
        "`to` is the first offset not read"
    );
}

#[test]
fn a_replay_does_not_move_the_consumers_position() {
    let topic = topic();
    let mut consumer = Consumer::new();
    consumer.poll(&topic, 0);
    consumer.commit(&topic, 0, 1);

    let mut replay = Replay::new(&topic, 0, 0, 4);
    assert_eq!(replay.next_record().unwrap().payload, "one");
    assert_eq!(consumer.committed(0), 1, "the replay committed something");
    assert_eq!(consumer.poll(&topic, 0).unwrap().payload, "two");
}

#[test]
fn a_replay_from_past_the_end_is_empty() {
    let topic = topic();
    let mut replay = Replay::new(&topic, 0, 9, 12);
    assert!(replay.next_record().is_none());
}

#[test]
fn a_replay_is_clamped_to_the_log() {
    let topic = topic();
    let mut replay = Replay::new(&topic, 0, 2, 99);
    let payloads: Vec<String> = std::iter::from_fn(|| replay.next_record())
        .map(|record| record.payload)
        .collect();
    assert_eq!(payloads, ["three", "four"]);
}

#[test]
fn an_empty_range_reads_nothing() {
    let topic = topic();
    let mut replay = Replay::new(&topic, 0, 2, 2);
    assert!(replay.next_record().is_none());
}

#[test]
fn two_replays_of_one_range_agree() {
    let topic = topic();
    let first: Vec<String> = std::iter::from_fn({
        let mut replay = Replay::new(&topic, 0, 0, 2);
        move || replay.next_record()
    })
    .map(|record| record.payload)
    .collect();
    let second: Vec<String> = std::iter::from_fn({
        let mut replay = Replay::new(&topic, 0, 0, 2);
        move || replay.next_record()
    })
    .map(|record| record.payload)
    .collect();
    assert_eq!(first, second);
}
