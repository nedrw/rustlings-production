//! probe = "broker/partitioning@apply"
//! title = "One key, one partition; one partition, one order"
//! hint = "Route keyed records by the given stable hash and unkeyed ones in rotation; offsets are per partition, so two partitions both start at zero."
//!
//! Step `broker/7.1-partitioning` is the two promises a partitioned log makes. This probe keeps the
//! log in a `BTreeMap` of partitions instead of a `Vec` of `Vec`s — the same routing judgement from a
//! different shape.
//!
//! ## What you write
//!
//! [`Log::append`] — a keyed record goes to `partition_for`, an unkeyed one to the next partition
//! in rotation; answer `(partition, offset)`, where the offset is the record's index in its own
//! partition.
//!
//! ## What the tests ask
//!
//! Same key, same partition and consecutive offsets; order inside a partition; four unkeyed records
//! spread over four partitions; offsets per partition; and a read past the end is `None`.

use std::collections::BTreeMap;

/// The stable hash the log routes by. Given: a hash is not the exercise.
#[must_use]
pub fn partition_for(partitions: usize, key: &str) -> usize {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash % partitions as u64) as usize
}

/// A log of partitions, each a list of payloads.
#[derive(Debug)]
pub struct Log {
    parts: BTreeMap<usize, Vec<String>>,
    partitions: usize,
    next_unkeyed: usize,
}

impl Log {
    /// A log with `partitions` empty partitions.
    #[must_use]
    pub fn new(partitions: usize) -> Self {
        Self {
            parts: BTreeMap::new(),
            partitions,
            next_unkeyed: 0,
        }
    }

    /// How many records a partition holds (its end offset).
    #[must_use]
    pub fn end(&self, partition: usize) -> u64 {
        self.parts
            .get(&partition)
            .map_or(0, |records| records.len() as u64)
    }

    /// The payload at `offset` in `partition`.
    #[must_use]
    pub fn read(&self, partition: usize, offset: u64) -> Option<&str> {
        self.parts
            .get(&partition)?
            .get(usize::try_from(offset).ok()?)
            .map(String::as_str)
    }

    /// Append a payload and answer `(partition, offset)`.
    pub fn append(&mut self, key: Option<&str>, payload: &str) -> (usize, u64) {
        todo!("keyed records go to their key's partition; unkeyed ones rotate")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_keeps_its_partition_and_its_offsets_are_consecutive() {
        let mut log = Log::new(4);
        let first = log.append(Some("tenant-a"), "one");
        let second = log.append(Some("tenant-a"), "two");
        assert_eq!(first.0, second.0);
        assert_eq!(first.1, 0);
        assert_eq!(second.1, 1);
    }

    #[test]
    fn order_inside_a_partition_is_the_append_order() {
        let mut log = Log::new(2);
        for payload in ["one", "two", "three"] {
            log.append(Some("tenant-a"), payload);
        }
        let partition = partition_for(2, "tenant-a");
        assert_eq!(log.read(partition, 0), Some("one"));
        assert_eq!(log.read(partition, 1), Some("two"));
        assert_eq!(log.read(partition, 2), Some("three"));
    }

    #[test]
    fn an_unkeyed_producer_rotates_through_the_partitions() {
        let mut log = Log::new(4);
        for payload in ["one", "two", "three", "four"] {
            log.append(None, payload);
        }
        for partition in 0..4 {
            assert_eq!(log.end(partition), 1, "partition {partition} got no record");
        }
    }

    #[test]
    fn offsets_are_per_partition() {
        let mut log = Log::new(4);
        let a = log.append(Some("a"), "a");
        let b = log.append(Some("b"), "b");
        if a.0 != b.0 {
            assert_eq!(a.1, 0);
            assert_eq!(b.1, 0);
        }
    }

    #[test]
    fn reading_past_the_end_is_none() {
        let mut log = Log::new(2);
        log.append(Some("a"), "one");
        assert_eq!(log.read(0, 90), None);
        assert_eq!(log.read(9, 0), None);
    }
}
