//! A partitioned message log.
//!
//! Every step of this track adds to the library; the binary stays a shell.
//!
//! There is no broker process here and no network: the log is a `Vec` per partition, and the
//! semantics under test — routing, offsets, assignment, lag — are the ones a broker gives you.

// The region later scaffolds are allowed to write into (authoring.md §5). A scaffold may only
// *insert* lines inside it, and only *remove* nothing.
// kiln:scaffold:begin
// kiln:scaffold:end

/// One record in a partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The key the record was published with, if any.
    pub key: Option<String>,
    /// The record's payload.
    pub payload: String,
}

/// The partition a key belongs to: the same key lands in the same partition, every time.
///
/// FNV-1a over the key's bytes, modulo the partition count. It is given because a stable hash is
/// not the exercise — the log's routing is.
#[must_use]
pub fn partition_for(partitions: usize, key: &str) -> usize {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash % partitions as u64) as usize
}

/// A topic: a fixed number of partitions, each an append-only log.
#[derive(Debug)]
pub struct Topic {
    partitions: Vec<Vec<Record>>,
    next_unkeyed: usize,
}

impl Topic {
    /// A topic with `partitions` empty partitions.
    #[must_use]
    pub fn new(partitions: usize) -> Self {
        Self {
            partitions: vec![Vec::new(); partitions],
            next_unkeyed: 0,
        }
    }

    /// How many partitions the topic has.
    #[must_use]
    pub fn partitions(&self) -> usize {
        self.partitions.len()
    }

    /// The next offset a producer would write at, per partition.
    #[must_use]
    pub fn end(&self, partition: usize) -> u64 {
        self.partitions
            .get(partition)
            .map_or(0, |records| records.len() as u64)
    }

    /// The record at `offset` in `partition`.
    #[must_use]
    pub fn read(&self, partition: usize, offset: u64) -> Option<&Record> {
        self.partitions
            .get(partition)?
            .get(usize::try_from(offset).ok()?)
    }

    /// Append a record and answer `(partition, offset)`.
    ///
    /// A keyed record goes to its key's partition; an unkeyed one walks the partitions in order,
    /// so a producer that never keys its records still spreads them.
    pub fn append(&mut self, key: Option<&str>, payload: &str) -> (usize, u64) {
        let _ = (&self.next_unkeyed, key, payload);
        todo!("route by key, or round-robin when there is none")
    }
}

/// The version of the service.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
