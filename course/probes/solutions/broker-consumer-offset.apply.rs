//! probe = "broker/consumer-offset@apply"
//! title = "The commit is where the next life starts"
//! hint = "A commit is the next offset to read, clamped to the end; a partition the log does not have is not a position at all."
//!
//! Step `broker/7.2-consumer-offsets` is the contract between a group and its restart. This probe
//! keeps the log in a `BTreeMap` and asks for the commit alone: the poll is given, because the
//! position it produces is only half the promise.
//!
//! ## What you write
//!
//! [`Consumer::commit`] — record the group's position for a partition:
//!
//! * a partition the log does not have: ignore it;
//! * an offset past the partition's end: clamp to the end;
//! * anything else: record it as given.
//!
//! ## What the tests ask
//!
//! A commit resumes at the next record; a commit past the end is clamped; an unknown partition is
//! ignored; a commit may skip records (that is the group's choice, and a restart then starts there);
//! and two consumers do not share a position.

use std::collections::BTreeMap;

/// A log of partitions, each a list of payloads.
#[derive(Debug, Default)]
pub struct Log {
    parts: BTreeMap<usize, Vec<String>>,
}

impl Log {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a payload to `partition`.
    pub fn append(&mut self, partition: usize, payload: &str) {
        self.parts
            .entry(partition)
            .or_default()
            .push(payload.to_owned());
    }

    /// The partition's end offset.
    #[must_use]
    pub fn end(&self, partition: usize) -> u64 {
        self.parts
            .get(&partition)
            .map_or(0, |records| records.len() as u64)
    }

    /// The payload at `offset`.
    #[must_use]
    pub fn read(&self, partition: usize, offset: u64) -> Option<&str> {
        self.parts
            .get(&partition)?
            .get(usize::try_from(offset).ok()?)
            .map(String::as_str)
    }
}

/// One consumer's position in a log.
#[derive(Debug, Default)]
pub struct Consumer {
    committed: BTreeMap<usize, u64>,
    next: BTreeMap<usize, u64>,
}

impl Consumer {
    /// A consumer at the beginning.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A consumer that starts where `committed` says a previous one stopped.
    #[must_use]
    pub fn resume(committed: &BTreeMap<usize, u64>) -> Self {
        Self {
            committed: committed.clone(),
            next: committed.clone(),
        }
    }

    /// The committed offset (`0` when nothing was committed).
    #[must_use]
    pub fn committed(&self, partition: usize) -> u64 {
        self.committed.get(&partition).copied().unwrap_or(0)
    }

    /// The next payload to process, in order.
    pub fn poll(&mut self, log: &Log, partition: usize) -> Option<String> {
        let offset = self.next.get(&partition).copied().unwrap_or(0);
        let payload = log.read(partition, offset)?.to_owned();
        self.next.insert(partition, offset + 1);
        Some(payload)
    }

    /// Commit the group's position in `partition`.
    pub fn commit(&mut self, log: &Log, partition: usize, offset: u64) {
        if !log.parts.contains_key(&partition) {
            return;
        }
        let offset = offset.min(log.end(partition));
        self.committed.insert(partition, offset);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log() -> Log {
        let mut log = Log::new();
        for payload in ["one", "two", "three"] {
            log.append(0, payload);
        }
        log
    }

    #[test]
    fn a_commit_resumes_at_the_next_record() {
        let log = log();
        let mut consumer = Consumer::new();
        assert_eq!(consumer.poll(&log, 0).as_deref(), Some("one"));
        consumer.commit(&log, 0, 1);

        let mut restarted = Consumer::resume(&BTreeMap::from([(0, 1)]));
        assert_eq!(restarted.poll(&log, 0).as_deref(), Some("two"));
    }

    #[test]
    fn a_commit_past_the_end_is_clamped() {
        let log = log();
        let mut consumer = Consumer::new();
        consumer.commit(&log, 0, 99);
        assert_eq!(consumer.committed(0), 3);
    }

    #[test]
    fn an_unknown_partition_has_no_position() {
        let log = log();
        let mut consumer = Consumer::new();
        consumer.commit(&log, 9, 5);
        assert_eq!(consumer.committed(9), 0);
    }

    #[test]
    fn a_commit_may_skip_records() {
        let log = log();
        let mut consumer = Consumer::new();
        consumer.commit(&log, 0, 2);
        let mut restarted = Consumer::resume(&BTreeMap::from([(0, consumer.committed(0))]));
        assert_eq!(restarted.poll(&log, 0).as_deref(), Some("three"));
    }

    #[test]
    fn two_consumers_do_not_share_a_position() {
        let log = log();
        let mut one = Consumer::new();
        let mut two = Consumer::new();
        one.poll(&log, 0);
        one.commit(&log, 0, 1);
        assert_eq!(two.committed(0), 0);
        assert_eq!(two.poll(&log, 0).as_deref(), Some("one"));
    }
}
