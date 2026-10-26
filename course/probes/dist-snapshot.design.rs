//! probe = "dist/snapshot@design"
//! title = "The log keeps its place: a snapshot replaces a prefix, never the numbering"
//! hint = "Take a snapshot only of committed state, refuse one that disagrees with the log, keep the next entry at index + 1, and refuse a snapshot older than what is already applied."
//!
//! Step `meshkv/5.4-snapshots` is the step where the log stops growing: the state at an index is
//! kept, the entries before it are thrown away, and the log keeps its place in the cluster's
//! numbering. This probe is the bookkeeping, not the payload.
//!
//! Four decisions, each with a failure behind it:
//!
//! 1. **a snapshot is of committed state and nothing else.** A picture of an uncommitted entry is a
//!    picture of a state that may still change, and a node that installs it has applied something
//!    that may never have happened;
//! 2. **compaction keeps the numbering.** After compacting to 600 the next entry belongs at 601,
//!    and position 600 is the last one the node remembers having applied. An index at or below the
//!    base is not an entry the log holds, exactly like an index in the future;
//! 3. **the snapshot has to agree with the log.** Same index, different term means a different
//!    history; compacting to it would throw away the wrong prefix, so it is refused and the log is
//!    untouched;
//! 4. **a late snapshot is refused.** One older than what a node has already applied would undo a
//!    state that is already there. Reinstalling the snapshot a node already holds, at exactly its
//!    commit index, is not "older" and is harmless.
//!
//! ## What you write
//!
//! * [`Snapshot::new`] — a snapshot covers at least one entry, so index zero is refused;
//! * [`Log::compact_to`] — drop the covered prefix, move the base, and refuse an index past the
//!   end; compacting to the base again is a no-op because those entries are already gone;
//! * [`Replica::take_snapshot`] — refuse when nothing is committed, otherwise take the state at
//!   the commit index and its term;
//! * [`Replica::compact`] — check the log holds that index with that term, then compact;
//! * [`Replica::install_snapshot`] — refuse one older than the applied state; otherwise replace
//!   the log with a log that starts after it and move the commit index.
//!
//! [`Index`], [`Term`], [`Entry`] and a [`Log`] whose `at`, `base` and `last_index` already
//! account for the base are given, because the vocabulary is the interface: after a compaction an
//! index still means the same thing on every node. [`Snapshot::covers`] and
//! [`Replica::needs_snapshot`] are also given, so the tests can ask where the snapshot ends
//! without re-deriving it; what is missing is when a snapshot may be taken and what it may
//! replace.
//!
//! ## What the tests ask
//!
//! That a snapshot refuses index zero and covers its own index but not the next one; that
//! compaction drops exactly the covered entries and leaves the next position where the cluster
//! expects it, with every index at or below the base absent; that compacting past the end is
//! refused while compacting to the base again drops nothing; that an uncommitted state cannot be
//! snapshotted and a committed one is taken at the commit index, even when that is the tail of the
//! log; that a snapshot with the right index and the wrong term is refused with the log untouched;
//! that installing one replaces the log and the next entry belongs after it; and that an older
//! snapshot is refused while one at the commit index is accepted.
//!
//! ## Where this is honest, and where it is not
//!
//! There is no disk, no file, no transfer and no `InstallSnapshot` message on a wire: the snapshot
//! is a value with a byte vector, and the probe never asks what the bytes mean. The state machine
//! that makes those bytes deterministic is `meshkv/5.10`'s subject, and the commit rule that says
//! which prefix is safe to snapshot is `dist/consensus@apply`'s. What is re-drilled is the
//! bookkeeping every compaction bug hides in: an off-by-one base, a commit index that moves past
//! the entries a node actually holds, or a snapshot from another history quietly replacing a
//! prefix. Those failures are invisible until a node restarts, which is why the boundary is worth
//! pinning here.

use std::fmt;

/// A position in the log: one-based, never reused.
pub type Index = u64;

/// How many elections the cluster has been through. A term only moves up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Term(u64);

impl Term {
    /// Term zero, where a brand-new node starts.
    #[must_use]
    pub fn zero() -> Self {
        Self(0)
    }

    /// A term as it is written on the wire.
    #[must_use]
    pub fn new(number: u64) -> Self {
        Self(number)
    }

    /// The number.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// One write, as the log keeps it: the term that proposed it and where it sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The term of the leader that proposed it.
    pub term: Term,
    /// Its position, one-based.
    pub index: Index,
}

impl Entry {
    /// An entry proposed in `term` at `index`.
    #[must_use]
    pub fn new(term: Term, index: Index) -> Self {
        Self { term, index }
    }
}

/// Why a snapshot or a compaction was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    fn invalid(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    fn protocol(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// A state, and the entry it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The index of the last entry this snapshot covers.
    pub last_included_index: Index,
    /// The term of that entry.
    pub last_included_term: Term,
    data: Vec<u8>,
}

impl Snapshot {
    /// A snapshot of `data` as of the entry at `index` and `term`.
    ///
    /// # Errors
    ///
    /// [`Error`] when the index is zero — there is no entry zero to be the state of, and a
    /// snapshot that covers nothing says nothing.
    pub fn new(index: Index, term: Term, data: Vec<u8>) -> Result<Self, Error> {
        todo!()
    }

    /// `true` when this snapshot covers `index` — that is, when the state at `index` is inside it.
    #[must_use]
    pub fn covers(&self, index: Index) -> bool {
        index <= self.last_included_index
    }

    /// The size of the payload, in bytes.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.data.len()
    }

    /// The state itself, opaque as far as this step is concerned.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

/// The log every node keeps: a base, and the entries after it.
///
/// After a compaction the log holds fewer entries than the cluster's numbering has positions, so
/// an index is still the cluster's index: the first entry held is always at `base + 1`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Log {
    base: Index,
    entries: Vec<Entry>,
}

impl Log {
    /// An empty log that holds everything.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A log holding entries 1 through `terms.len()`, each of the term given.
    #[must_use]
    pub fn with_terms(terms: &[Term]) -> Self {
        Self {
            base: 0,
            entries: terms
                .iter()
                .enumerate()
                .map(|(position, term)| Entry::new(*term, position as Index + 1))
                .collect(),
        }
    }

    /// An empty log that starts after `index`.
    ///
    /// This is the log a node has after it installed a snapshot: it holds nothing, and the next
    /// entry belongs at `index + 1`.
    #[must_use]
    pub fn starting_at(index: Index) -> Self {
        Self {
            base: index,
            entries: Vec::new(),
        }
    }

    /// The index of the last entry this log no longer holds, or zero when it holds everything.
    #[must_use]
    pub fn base(&self) -> Index {
        self.base
    }

    /// How many entries it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when it holds nothing. A log that installed a snapshot is empty and has a base.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The index of the last entry this log holds, or the base when it holds none.
    ///
    /// This is not `len()`: after a compaction the log holds fewer entries than the cluster's
    /// numbering has positions.
    #[must_use]
    pub fn last_index(&self) -> Index {
        self.base + self.entries.len() as Index
    }

    /// The entry at `index`, one-based.
    ///
    /// An index the log has compacted away is not an entry it holds: the answer is `None`, exactly
    /// like an index beyond its end.
    #[must_use]
    pub fn at(&self, index: Index) -> Option<&Entry> {
        if index <= self.base {
            return None;
        }
        let position = index.checked_sub(self.base + 1)?;
        let position = usize::try_from(position).ok()?;
        self.entries.get(position)
    }

    /// Append an entry that belongs next, or refuse it and say why.
    ///
    /// # Errors
    ///
    /// [`Error`] when the entry's index is not `last_index() + 1`.
    pub fn append(&mut self, entry: Entry) -> Result<(), Error> {
        let expected = self.last_index() + 1;
        if entry.index != expected {
            return Err(Error::protocol(format!(
                "entry {} cannot follow a log whose next position is {expected}",
                entry.index
            )));
        }
        self.entries.push(entry);
        Ok(())
    }

    /// Drop every entry up to and including `index`, and answer how many went.
    ///
    /// An index at or below the base was already dropped, so nothing is dropped again and the
    /// answer is zero. An index past the end is refused: a node cannot compact what it never
    /// received.
    ///
    /// # Errors
    ///
    /// [`Error`] when the log does not hold `index` and `index` is after the base.
    pub fn compact_to(&mut self, index: Index) -> Result<usize, Error> {
        todo!()
    }
}

/// One node's log, the prefix it has applied, and the snapshot it last took or installed.
#[derive(Debug, Clone, Default)]
pub struct Replica {
    log: Log,
    commit: Index,
    snapshot: Option<Snapshot>,
}

impl Replica {
    /// A node with an empty log and nothing applied.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The log this node keeps.
    #[must_use]
    pub fn log(&self) -> &Log {
        &self.log
    }

    /// How far this node's state is applied.
    #[must_use]
    pub fn commit(&self) -> Index {
        self.commit
    }

    /// The snapshot this node last took or installed.
    #[must_use]
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }

    /// Append an entry of `term` at the next position, and answer the position it took.
    pub fn append(&mut self, term: Term) -> Index {
        let index = self.log.last_index() + 1;
        self.log
            .append(Entry::new(term, index))
            .expect("the next position is the one just computed");
        index
    }

    /// Mark the log applied up to `index`, and no further than the log reaches.
    pub fn commit_to(&mut self, index: Index) {
        self.commit = index.min(self.log.last_index());
    }

    /// `true` when a peer whose next position is `next_index` is behind the snapshot and cannot be
    /// caught up by entries: the entries that lead there are the ones the snapshot replaced.
    #[must_use]
    pub fn needs_snapshot(&self, next_index: Index) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| next_index <= snapshot.last_included_index)
    }

    /// Take a snapshot of the state as of the commit index.
    ///
    /// A snapshot is of committed state and nothing else: one taken of an uncommitted entry is a
    /// picture of a state that may still change.
    ///
    /// # Errors
    ///
    /// [`Error`] when nothing is committed yet, or when the log does not hold the commit index.
    pub fn take_snapshot(&mut self, data: Vec<u8>) -> Result<Snapshot, Error> {
        todo!()
    }

    /// Compact this node's log against `snapshot`.
    ///
    /// The snapshot and the log have to agree: the log must hold the entry the snapshot claims to
    /// cover, and that entry must be of the term the snapshot names. A snapshot that disagrees is
    /// from another history, and the log is left untouched.
    ///
    /// # Errors
    ///
    /// [`Error`] when the log does not hold the snapshot's index, or holds a different term there.
    pub fn compact(&mut self, snapshot: &Snapshot) -> Result<usize, Error> {
        todo!()
    }

    /// Adopt `snapshot`: this node's log is replaced by the state it carries.
    ///
    /// A snapshot older than what this node has already applied would undo a state that is
    /// already there, and is refused. One at exactly the commit index is not older: it is the
    /// state this node has, and installing it is harmless.
    ///
    /// # Errors
    ///
    /// [`Error`] when the snapshot's index is before this node's commit index.
    pub fn install_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), Error> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(number: u64) -> Term {
        Term::new(number)
    }

    #[test]
    fn a_snapshot_refuses_index_zero_and_covers_up_to_its_own_index() {
        let snapshot = Snapshot::new(10, t(3), b"state".to_vec()).expect("a snapshot");
        assert_eq!(snapshot.last_included_index, 10);
        assert_eq!(snapshot.last_included_term, t(3));
        assert_eq!(snapshot.bytes(), 5);
        for index in 0..=10 {
            assert!(
                snapshot.covers(index),
                "the snapshot covers entry {index}, which is at or before the entry it came from"
            );
        }
        for index in 11..=12 {
            assert!(
                !snapshot.covers(index),
                "entry {index} was written after the snapshot and is not inside it"
            );
        }

        let error =
            Snapshot::new(0, Term::zero(), Vec::new()).expect_err("index 0 is before the log");
        assert!(
            error.to_string().contains("index 0"),
            "a snapshot of nothing says nothing, so there is no entry zero to name: {error}"
        );
    }

    #[test]
    fn compaction_drops_the_covered_prefix_and_keeps_the_cluster_numbering() {
        let mut log = Log::with_terms(&[t(1); 5]);
        assert_eq!(
            log.compact_to(3),
            Ok(3),
            "the three entries the snapshot covers are the three that go"
        );
        assert_eq!(log.len(), 2, "the log keeps what comes after the snapshot");
        assert_eq!(log.base(), 3, "and it knows where it now starts");
        assert_eq!(
            log.last_index(),
            5,
            "the numbering is the cluster's: the last position did not move"
        );

        for index in 1..=3 {
            assert!(
                log.at(index).is_none(),
                "index {index} is at or below the base, so the log does not hold it"
            );
        }
        for index in 4..=5 {
            assert_eq!(
                log.at(index).map(|entry| entry.index),
                Some(index),
                "index {index} is after the snapshot and is still held"
            );
        }

        log.append(Entry::new(t(1), 6))
            .expect("the entry after the snapshot belongs at 6");
        assert_eq!(
            log.at(6).map(|entry| entry.index),
            Some(6),
            "and the next entry is taken at index + 1, not at len() + 1"
        );
    }

    #[test]
    fn compacting_past_the_end_is_refused_and_compacting_to_the_base_is_a_no_op() {
        let mut log = Log::with_terms(&[t(1); 5]);
        let error = log
            .compact_to(9)
            .expect_err("the log does not hold an entry at 9");
        assert!(
            error.to_string().contains('9'),
            "the refusal names the index it could not compact to: {error}"
        );
        assert_eq!(log.len(), 5, "and the log is untouched");
        assert_eq!(log.base(), 0);

        assert_eq!(
            log.compact_to(0),
            Ok(0),
            "compacting to nothing drops nothing: there is no entry zero"
        );
        log.compact_to(3).expect("an index the log holds compacts");
        assert_eq!(
            log.compact_to(3),
            Ok(0),
            "compacting to the base again drops nothing, because those entries are already gone"
        );
        assert_eq!(log.base(), 3);
    }

    #[test]
    fn a_snapshot_is_taken_of_the_committed_state_and_of_nothing_else() {
        let mut replica = Replica::new();
        for _ in 0..3 {
            replica.append(t(1));
        }
        let error = replica
            .take_snapshot(b"state".to_vec())
            .expect_err("nothing is committed yet");
        assert!(
            error.to_string().contains("committed"),
            "a snapshot of uncommitted state is a picture of a state that may still change: {error}"
        );

        replica.commit_to(2);
        let snapshot = replica
            .take_snapshot(b"state".to_vec())
            .expect("a committed state");
        assert_eq!(
            snapshot.last_included_index, 2,
            "a snapshot is of the last committed entry, not of the end of the log"
        );
        assert_eq!(snapshot.last_included_term, t(1));
        assert_eq!(
            replica.snapshot().map(|held| held.last_included_index),
            Some(2),
            "the node holds the snapshot it took"
        );

        replica.commit_to(3);
        let at_tail = replica
            .take_snapshot(b"tail".to_vec())
            .expect("a commit index at the end of the log is still a committed state");
        assert_eq!(
            at_tail.last_included_index, 3,
            "the boundary is the commit index, even when it is the last entry"
        );
    }

    #[test]
    fn a_snapshot_from_another_history_is_refused_and_the_log_is_untouched() {
        let mut replica = Replica::new();
        for _ in 0..5 {
            replica.append(t(1));
        }
        replica.commit_to(5);
        let snapshot = replica
            .take_snapshot(b"state".to_vec())
            .expect("a committed state");

        let other_history = Snapshot::new(3, t(2), b"other".to_vec()).expect("a snapshot shape");
        let error = replica
            .compact(&other_history)
            .expect_err("same index, different term is another history");
        assert!(
            error.to_string().contains("term"),
            "the refusal says the log holds a different term there, because compacting to it \
             would replace the wrong entries: {error}"
        );
        assert_eq!(replica.log().len(), 5, "and the log is untouched");
        assert_eq!(replica.log().base(), 0);

        let dropped = replica
            .compact(&snapshot)
            .expect("a snapshot that agrees with the log compacts");
        assert_eq!(
            dropped, 5,
            "the entries the snapshot covers are exactly the ones it replaces"
        );
        assert_eq!(replica.log().len(), 0);
        assert_eq!(replica.log().base(), 5);
    }

    #[test]
    fn installing_a_snapshot_replaces_the_log_and_the_next_entry_follows_it() {
        let mut replica = Replica::new();
        for _ in 0..3 {
            replica.append(t(1));
        }
        replica.commit_to(3);
        assert_eq!(replica.log().len(), 3);

        let snapshot = Snapshot::new(9, t(2), b"state".to_vec()).expect("a snapshot");
        replica
            .install_snapshot(&snapshot)
            .expect("a snapshot from further ahead installs");
        assert_eq!(replica.log().len(), 0, "the state replaced the log");
        assert_eq!(
            replica.log().base(),
            9,
            "and the log starts after the snapshot, not at zero"
        );
        assert_eq!(replica.commit(), 9, "the state is applied");
        assert_eq!(
            replica.snapshot().map(|held| held.last_included_index),
            Some(9),
            "the node holds the snapshot it installed"
        );

        let next = replica.append(t(2));
        assert_eq!(
            next, 10,
            "the next entry belongs right after the snapshot, so every other node's prev agrees"
        );
        assert_eq!(replica.log().at(10).map(|entry| entry.index), Some(10));
    }

    #[test]
    fn a_late_snapshot_is_refused_and_one_at_the_commit_index_is_not_older() {
        let mut replica = Replica::new();
        for _ in 0..5 {
            replica.append(t(1));
        }
        replica.commit_to(5);
        let late = Snapshot::new(3, t(1), b"old".to_vec()).expect("a snapshot");
        let error = replica
            .install_snapshot(&late)
            .expect_err("this node has already applied 5");
        assert!(
            error.to_string().contains("committed") || error.to_string().contains("applied"),
            "a snapshot older than the applied state would undo it, so it is refused rather \
             than ignored: {error}"
        );
        assert_eq!(replica.commit(), 5, "and the applied state is untouched");
        assert_eq!(replica.log().len(), 5);

        let mut fresh = Replica::new();
        for _ in 0..3 {
            fresh.append(t(1));
        }
        fresh.commit_to(3);
        let at_commit = Snapshot::new(3, t(1), b"same".to_vec()).expect("a snapshot");
        fresh
            .install_snapshot(&at_commit)
            .expect("a snapshot at the commit index is not older");
        assert_eq!(fresh.commit(), 3);
        assert_eq!(
            fresh.log().base(),
            3,
            "installing it is harmless: the log holds the state the snapshot already describes"
        );
    }
}
