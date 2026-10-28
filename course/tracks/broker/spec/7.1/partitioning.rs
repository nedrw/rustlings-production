//! Spec for `broker/7.1-partitioning` — the same key, the same partition; order inside it; and an
//! unkeyed producer that still spreads.

use broker::Topic;

#[test]
fn a_key_always_lands_in_the_same_partition() {
    let mut topic = Topic::new(4);
    let (first, offset) = topic.append(Some("tenant-a"), "one");
    let (second, next) = topic.append(Some("tenant-a"), "two");
    assert_eq!(first, second, "the same key moved partitions");
    assert_eq!(offset, 0);
    assert_eq!(next, 1);
}

#[test]
fn order_inside_a_partition_is_the_append_order() {
    let mut topic = Topic::new(2);
    for payload in ["one", "two", "three"] {
        topic.append(Some("tenant-a"), payload);
    }
    let partition = broker::partition_for(2, "tenant-a");
    assert_eq!(topic.end(partition), 3);
    let payloads: Vec<&str> = (0..3)
        .map(|offset| topic.read(partition, offset).unwrap().payload.as_str())
        .collect();
    assert_eq!(payloads, ["one", "two", "three"]);
}

#[test]
fn a_record_lands_in_the_partition_its_key_names() {
    let mut topic = Topic::new(4);
    let (partition, offset) = topic.append(Some("tenant-a"), "one");
    assert_eq!(partition, broker::partition_for(4, "tenant-a"));
    assert_eq!(offset, 0);
    assert_eq!(
        topic.read(partition, 0).unwrap().key.as_deref(),
        Some("tenant-a")
    );
}

#[test]
fn offsets_are_per_partition() {
    let mut topic = Topic::new(4);
    let tenants = ["a", "b", "c", "d", "e", "f", "g", "h"];
    for tenant in tenants {
        topic.append(Some(tenant), tenant);
    }
    let total: u64 = (0..4).map(|partition| topic.end(partition)).sum();
    assert_eq!(total, 8);
    for partition in 0..4 {
        for offset in 0..topic.end(partition) {
            assert!(topic.read(partition, offset).is_some());
        }
    }
}

#[test]
fn an_unkeyed_producer_still_spreads_records() {
    let mut topic = Topic::new(4);
    for payload in ["one", "two", "three", "four"] {
        topic.append(None, payload);
    }
    for partition in 0..4 {
        assert_eq!(
            topic.end(partition),
            1,
            "partition {partition} got no record"
        );
    }
}

// Pins given code: `read` answers `None` out of range; the step is `append`.
#[test]
fn reading_past_the_end_is_not_a_panic() {
    let topic = Topic::new(2);
    assert!(topic.read(0, 0).is_none());
    assert!(topic.read(9, 0).is_none());
}
