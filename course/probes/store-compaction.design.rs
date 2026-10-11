//! probe = "store/compaction@design"
//! title = "A merge is a value, and a reader never sees half of one"
//! hint = "Take the oldest run of comparable segments, write the new segment's file before anything names it, and let the last reader of the old set — not the swap — delete what the merge replaced."
//!
//! Step `lakelet/4.8-compaction` is where this store stops writing one file per flush and starts
//! merging them, and it is two claims that have to hold at once: *which* segments to merge is a
//! value, and a reader never sees half of one. Everything in this file is a rule somebody paid for
//! once:
//!
//! * **the decision is a value, not a match arm inside a loop.** The rule is one sentence — the
//!   oldest run of `width` consecutive segments whose sizes are all within a factor of `ratio` of
//!   each other — and a plan can be judged without merging anything: it names the segments it would
//!   take, what they cost and what they hold, and asking the policy twice over the same sizes answers
//!   the same plan. The consequence is that merging pairs "as soon as two vaguely similar segments
//!   exist" stops being a taste and becomes a number a budget can read;
//! * **a merge is visible in exactly one step.** The new file is written, and the set that names it is
//!   swapped afterwards, so a reader of the index sees the store before the merge or after it — never
//!   the half-state in which a run and its copy are both the store. The consequence is that every
//!   stage a crash can land on leaves one of exactly two worlds, and neither of them answers a record
//!   twice;
//! * **the last reader to leave is the one that retires the inputs.** A snapshot names a *set* of
//!   segments, and the files a merge replaced stay on the disk — and stay readable — while any reader
//!   holds the set that named them. The consequence is that letting them go at the swap hands a query
//!   that began before it a file nobody names any more, and no count in the store would say so;
//! * **a file the index does not name is not the store.** The segment a merge wrote is on the disk
//!   before anything names it, so a reader reads the set it holds and never the directory: what a
//!   crash leaves in `segments/` is a leftover, and a listing is not an answer.
//!
//! ## What you write
//!
//! * [`Policy::plan`] — the merge this policy would do next, from segment metadata alone;
//! * [`Set::retire`] — the files a merge replaced, handed to the version that named them;
//! * [`Store::merge`] — carry a plan out as far as the file: write the new segment and answer the
//!   merge value, moving nothing the store is made of;
//! * [`Store::install`] — the one step: make the new set the store, and hand the inputs to the
//!   version being replaced.
//!
//! [`SegmentInfo`], [`Policy`], [`Plan`], [`Files`], [`Set`], [`Snapshot`], [`Store`], [`Merge`] and
//! [`Refused`] are given: the vocabulary is the interface, [`Files`] is the segment directory as a
//! probe can have one, and the refusal classes are what a caller branches on.
//!
//! ## What the tests ask
//!
//! The plan takes the oldest run: four equal segments at a width of four — and never the whole tier;
//! a run that begins after a segment ten times its neighbours' size; and a run that is too short, or
//! not comparable at *both* ends, is no plan at all, while exactly `width` comparable segments is one.
//! That last pair is the off-by-one the policy turns on: a store that never merges its last run grows
//! forever. A plan for nothing selects nothing, and a width of one or zero never asks for a single
//! segment. Then the value half: a merge writes its file and answers what it took, the store is not
//! moved by it, a reader that took its snapshot before it still names exactly the segments it took,
//! and a second merge while one is written is refused. Then the swap: one epoch, one file where the
//! run was, the inputs retired — still on the disk while a reader holds the old set, and freed once by
//! the last reader to leave.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no filesystem and no crash**: the segment directory is a set of
//! ordinals in memory ([`Files`]), a merge copies metadata rather than bytes, and "one of two worlds"
//! here means one of two segment *sets* rather than two states of a device. What a probe can check is
//! the sequencing — what a plan is, what a reader holds, which of the two steps frees a file — and
//! that is what the design half of this slot is a claim about.
//!
//! The real step writes a columnar segment and an index of its own bytes, flushes them, swaps the
//! index by writing a pending file and renaming it onto its own name, flushes the directory after the
//! rename, and measures its rewrite ratio in `tests/budget_compaction.rs` — none of which fits a file
//! with no device under it. Also missing on purpose: the five crash stages (`planned` → `written` →
//! `durable` → `swapped` → `retired`) and what each of them leaves behind, the `Error::Protocol` a
//! damaged segment or a cut index earns, and the byte relations a real merge earns by sharing
//! dictionaries between its inputs' blocks — here a merge's output *is* its input bytes, so the one
//! number this file cannot state is the step's "a merge writes fewer bytes than it read". And two
//! threads retiring at once: the real `Set` locks its retired list, and a probe with one thread has
//! nothing to lock.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

/// One segment as the store's index states it: an ordinal, what its file costs, what it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentInfo {
    /// The ordinal its file is named by.
    pub index: u64,
    /// How many bytes its file holds.
    pub bytes: u64,
    /// How many records it holds.
    pub records: u64,
}

impl SegmentInfo {
    /// A segment whose file costs `bytes` and holds `records`.
    #[must_use]
    pub const fn new(index: u64, bytes: u64, records: u64) -> Self {
        Self {
            index,
            bytes,
            records,
        }
    }
}

/// The size-tiered rule as a value: `width` segments at a time, within a factor of `ratio`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    width: usize,
    ratio: u64,
}

impl Policy {
    /// What this build ships: four segments within a factor of two.
    pub const DEFAULT: Self = Self { width: 4, ratio: 2 };

    /// A policy that merges `width` segments whose sizes are within a factor of `ratio`.
    #[must_use]
    pub const fn new(width: usize, ratio: u64) -> Self {
        Self { width, ratio }
    }

    /// How many segments one merge takes.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// The factor the sizes of one run are within.
    #[must_use]
    pub const fn ratio(&self) -> u64 {
        self.ratio
    }

    /// The merge this policy would do next, from segment sizes and counts alone.
    ///
    /// The rule in one sentence: *the oldest run of `width` consecutive segments whose sizes are all
    /// within a factor of `ratio` of each other.* Consecutive, because a merge writes one segment
    /// where its inputs were, so a run that skipped a segment would move that segment's records
    /// behind records that came after them; the oldest, because those are the files a reader has been
    /// opening longest; and *both* ends for the size comparison, because a rule that looked at one
    /// end would merge a tiny file into a large one. A plan never takes a single segment — that
    /// rewrites bytes and removes none — whatever number a caller asked for.
    #[must_use]
    pub fn plan(&self, segments: &[SegmentInfo]) -> Plan {
        todo!()
    }
}

/// What one merge would take, and what it would cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    segments: Vec<u64>,
    bytes: u64,
    records: u64,
}

impl Plan {
    /// The plan over a run of `segments`, taken as it is given: this is the step's own construction,
    /// not a second rule about which runs are allowed.
    #[must_use]
    pub fn over(segments: &[SegmentInfo]) -> Self {
        Self {
            segments: segments.iter().map(|info| info.index).collect(),
            bytes: segments
                .iter()
                .fold(0u64, |total, info| total.saturating_add(info.bytes)),
            records: segments
                .iter()
                .fold(0u64, |total, info| total.saturating_add(info.records)),
        }
    }

    /// The merge that takes nothing.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            segments: Vec::new(),
            bytes: 0,
            records: 0,
        }
    }

    /// The ordinals the merge would copy, oldest first.
    #[must_use]
    pub fn segments(&self) -> &[u64] {
        &self.segments
    }

    /// How many bytes it would read.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// How many records it would copy.
    #[must_use]
    pub const fn records(&self) -> u64 {
        self.records
    }

    /// `true` when this plan is no merge at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// How many files the merge would remove: it writes one file where its inputs were.
    #[must_use]
    pub fn files_removed(&self) -> usize {
        self.segments.len().saturating_sub(1)
    }
}

/// The files a store is made of, as a probe can have them: not a filesystem, a shared set of ordinals
/// that a write adds to and a retirement deletes from.
///
/// Shared on purpose: the store and every reader of it hold the same one, which is what makes a file
/// the store deleted and a file a reader still names the same file.
#[derive(Debug, Clone, Default)]
pub struct Files {
    present: Rc<RefCell<BTreeSet<u64>>>,
    deletes: Rc<RefCell<Vec<u64>>>,
}

impl Files {
    /// An empty segment directory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Write one segment's file.
    pub fn write(&self, index: u64) {
        self.present.borrow_mut().insert(index);
    }

    /// Delete one segment's file, and answer whether it was there.
    ///
    /// Every attempt is recorded, whether or not it removed anything: a file freed twice is a
    /// retirement that was not a set, and [`Files::deletes`] is where the tests read that off.
    pub fn delete(&self, index: u64) -> bool {
        self.deletes.borrow_mut().push(index);
        self.present.borrow_mut().remove(&index)
    }

    /// `true` when that segment's file is on the disk.
    #[must_use]
    pub fn contains(&self, index: u64) -> bool {
        self.present.borrow().contains(&index)
    }

    /// How many files are on the disk.
    #[must_use]
    pub fn len(&self) -> usize {
        self.present.borrow().len()
    }

    /// `true` when nothing is on the disk.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.present.borrow().is_empty()
    }

    /// Every ordinal on the disk, in order.
    #[must_use]
    pub fn ordinals(&self) -> Vec<u64> {
        self.present.borrow().iter().copied().collect()
    }

    /// Every delete that was asked for, in the order it was asked for — a file freed twice is in here
    /// twice.
    #[must_use]
    pub fn deletes(&self) -> Vec<u64> {
        self.deletes.borrow().clone()
    }
}

/// One version of the store: the segments it is made of, the epoch that names it, and the files a
/// merge has replaced and not deleted yet.
pub struct Set {
    files: Files,
    segments: Vec<SegmentInfo>,
    epoch: u64,
    retired: RefCell<Vec<u64>>,
}

impl Set {
    /// One version of a store: these segments, at this epoch, on these files.
    #[must_use]
    pub fn new(files: Files, segments: Vec<SegmentInfo>, epoch: u64) -> Self {
        Self {
            files,
            segments,
            epoch,
            retired: RefCell::new(Vec::new()),
        }
    }

    /// The files this version is made of.
    #[must_use]
    pub fn files(&self) -> &Files {
        &self.files
    }

    /// The segments this version is, oldest first.
    #[must_use]
    pub fn segments(&self) -> &[SegmentInfo] {
        &self.segments
    }

    /// The epoch this version is: it changes once per installed change of shape.
    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Hand this version the files a merge replaced: they go when the last reader of this version
    /// does.
    ///
    /// The list is a *set* of ordinals — a file retired twice is still one file, and a retirement
    /// that freed it twice would be a delete of something that is not there — and nothing is deleted
    /// here: a version that freed its inputs at the swap would hand a reader that began before it a
    /// file nobody names.
    pub fn retire(&self, indices: &[u64]) {
        todo!()
    }
}

impl Drop for Set {
    /// Delete what a merge replaced, and only now: this runs when the last handle on this version
    /// goes, which is the one moment at which nothing can still name these files.
    ///
    /// Given rather than written, because it is one loop — the interesting half of the rule is *when*
    /// it is allowed to run ([`Set::retire`], [`Store::install`]).
    fn drop(&mut self) {
        for index in self.retired.borrow().iter() {
            self.files.delete(*index);
        }
    }
}

/// What one reader holds: a version of the store, kept alive until the reader lets go.
///
/// A snapshot is an [`Rc`] handle on a [`Set`], and the reader count is that handle's count: the last
/// one to drop is the one whose `Drop` runs [`Set::retire`]'s deletions, which is what makes a merge
/// invisible to a query that began before it. There is no counter to get wrong and none to write.
pub struct Snapshot {
    set: Rc<Set>,
}

impl Snapshot {
    /// The segments this reader may see, oldest first — the set it was taken with, whatever the store
    /// has done since.
    #[must_use]
    pub fn segments(&self) -> &[SegmentInfo] {
        self.set.segments()
    }

    /// The epoch of the version it named.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.set.epoch()
    }

    /// How many bytes the segments it named cost.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.set.segments().iter().map(|info| info.bytes).sum()
    }

    /// How many records the segments it named hold.
    #[must_use]
    pub fn records(&self) -> u64 {
        self.set.segments().iter().map(|info| info.records).sum()
    }
}

/// Why a merge or an install was refused. A refusal is an answer, not a panic: a caller branches on
/// it, and every one of them leaves the store exactly where it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// A merge needs segments to take.
    EmptyPlan,
    /// The plan names a segment this store does not have: a plan is a value taken at a moment.
    StalePlan {
        /// The ordinal the plan named.
        index: u64,
    },
    /// A merge is written and not installed yet: writing a second one would write over its file.
    MergeInFlight {
        /// The ordinal the merge in flight wrote.
        index: u64,
    },
    /// There is no merge in flight at this ordinal, so there is nothing to install.
    NotInstalled {
        /// The ordinal the caller handed over.
        index: u64,
    },
}

/// One merge, as a value: what it copies, and what the file it wrote costs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merge {
    /// The epoch the store will have when this is installed.
    pub epoch: u64,
    /// The ordinal of the file it wrote.
    pub index: u64,
    /// The segments it copies, oldest first.
    pub taken: Vec<u64>,
    /// What those segments cost together.
    pub bytes_in: u64,
    /// What the new file costs.
    pub bytes_out: u64,
    /// How many records the new file holds.
    pub records: u64,
    /// How many segments the store had when it was written.
    pub segments_before: usize,
}

/// The store: what its index names right now, the ordinal the next segment gets, and the merge that
/// has been written and not installed.
pub struct Store {
    set: Rc<Set>,
    next: u64,
    pending: Option<u64>,
}

impl Store {
    /// A store over `set`, whose next segment is the highest ordinal in it plus one.
    #[must_use]
    pub fn open(set: Set) -> Self {
        let next = set
            .segments()
            .iter()
            .map(|info| info.index)
            .max()
            .map_or(0, |index| index + 1);
        Self {
            set: Rc::new(set),
            next,
            pending: None,
        }
    }

    /// What the store is right now.
    #[must_use]
    pub fn set(&self) -> &Rc<Set> {
        &self.set
    }

    /// The segments the store is made of, oldest first.
    #[must_use]
    pub fn segments(&self) -> &[SegmentInfo] {
        self.set.segments()
    }

    /// The epoch the store is at.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.set.epoch()
    }

    /// The segment written and not installed yet, if there is one.
    #[must_use]
    pub const fn pending(&self) -> Option<u64> {
        self.pending
    }

    /// The merge `policy` would do next, from the segments the store names.
    #[must_use]
    pub fn plan(&self, policy: Policy) -> Plan {
        policy.plan(self.set.segments())
    }

    /// Take a reader's view of what the store is now.
    ///
    /// The reader count is [`Rc`]'s, so this is the whole of what a snapshot costs: from here the
    /// files a later merge retires stay on the disk until this handle goes.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            set: Rc::clone(&self.set),
        }
    }

    /// Carry `plan` out as far as the file: write the new segment, and answer the merge value.
    ///
    /// Nothing the store is made of changes here. The file is written under an ordinal no version
    /// names, which is exactly the state a crash between writing and installing leaves — and what
    /// makes a reader never see half of a merge.
    ///
    /// # Errors
    ///
    /// [`Refused::EmptyPlan`] when the plan takes no segments, [`Refused::StalePlan`] when it names
    /// a segment this store does not have, and [`Refused::MergeInFlight`] when a merge is already
    /// written and not installed.
    pub fn merge(&mut self, plan: &Plan) -> Result<Merge, Refused> {
        todo!()
    }

    /// The one step a merge is visible in: make the new set the store, and hand the inputs to the
    /// version being replaced.
    ///
    /// The inputs are *retired*, not deleted: they go when the last reader of the version that named
    /// them lets go, so a query that began before this call goes on answering from the files it
    /// named.
    ///
    /// # Errors
    ///
    /// [`Refused::NotInstalled`] when there is no merge in flight at this ordinal, and
    /// [`Refused::StalePlan`] when the merge does not name a segment of the version it is installed
    /// into.
    pub fn install(&mut self, ran: Merge) -> Result<(), Refused> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four segments of the same size are one run under [`Policy::DEFAULT`].
    const RECORDS: u64 = 10;

    /// Segments of these sizes, in this order, holding [`RECORDS`] records each.
    fn sized(sizes: &[u64]) -> Vec<SegmentInfo> {
        sizes
            .iter()
            .enumerate()
            .map(|(index, bytes)| SegmentInfo::new(index as u64, *bytes, RECORDS))
            .collect()
    }

    /// A store of `count` segments of `bytes` each, on a directory of its own.
    fn store_of(count: usize, bytes: u64) -> Store {
        let files = Files::new();
        let segments: Vec<SegmentInfo> = (0..count as u64)
            .map(|index| SegmentInfo::new(index, bytes, RECORDS))
            .collect();
        for info in &segments {
            files.write(info.index);
        }
        Store::open(Set::new(files, segments, 0))
    }

    /// The ordinal of every segment a version names, in order.
    fn named(segments: &[SegmentInfo]) -> Vec<u64> {
        segments.iter().map(|info| info.index).collect()
    }

    #[test]
    fn a_plan_takes_the_oldest_run_of_comparable_segments() {
        let policy = Policy::new(4, 2);
        let plan = policy.plan(&sized(&[4096; 8]));
        assert_eq!(
            plan.segments(),
            [0, 1, 2, 3],
            "the *oldest* run that qualifies: those are the files a reader has been opening longest, \
             and a policy that took the youngest run would leave the old ones to grow old forever"
        );
        assert_eq!(
            plan.segments().len(),
            4,
            "`width` segments and not a whole tier: a merge whose cost followed the size of the tier \
             would depend on how long the store had been running"
        );
        assert_eq!(
            plan.bytes(),
            4 * 4096,
            "the plan adds up what it would read"
        );
        assert_eq!(plan.records(), 4 * RECORDS, "and what it would copy");
        assert_eq!(
            plan.files_removed(),
            3,
            "it writes one file where four were, which is the number a reader stops opening"
        );
        assert!(
            !plan.is_empty(),
            "four comparable segments is a merge, and a caller tests for that before touching a disk"
        );
        assert_eq!(
            policy.plan(&sized(&[4096; 8])),
            plan,
            "asking twice answers the same plan: what a plan is made from is the sizes in front of \
             it and nothing else"
        );

        // A segment ten times its neighbours is not comparable with them, so the run begins after it.
        assert_eq!(
            policy
                .plan(&sized(&[40_960, 4096, 4096, 4096, 4096]))
                .segments(),
            [1, 2, 3, 4],
            "the run begins at the first segment comparable with the ones that follow it"
        );
        assert_eq!(
            Policy::DEFAULT.plan(&sized(&[4096; 8])).segments().len(),
            4,
            "this build's policy merges a run and not a pair: merging pairs falls fast and rewrites \
             the store over and over to do it, which is the ratio the budget lane measures"
        );
    }

    #[test]
    fn a_run_that_is_too_short_or_not_comparable_is_no_plan() {
        let policy = Policy::new(4, 2);
        assert!(
            policy.plan(&sized(&[4096, 4096, 4096])).is_empty(),
            "three comparable segments and a width of four is nothing to merge: `width` is how many \
             segments one merge takes, and a merge of fewer is the next call's business"
        );
        assert_eq!(
            policy
                .plan(&sized(&[4096, 4096, 4096, 4096]))
                .segments()
                .len(),
            4,
            "exactly `width` comparable segments is a plan: the rule is `at least width`, and an \
             off-by-one here is a store that never merges its last run"
        );
        assert!(
            policy.plan(&sized(&[40_960, 40_960, 10])).is_empty(),
            "a run compares a newcomer with *both* of its ends: 10 bytes is within a factor of two of \
             40 960's largest and not of its smallest, and a rule that looked one way would merge a \
             tiny file into a large one"
        );
        assert!(
            policy.plan(&sized(&[4096, 4096, 10, 10])).is_empty(),
            "two comparable segments and a width of four is nothing to merge, however comparable the \
             four of them are one at a time"
        );
        assert_eq!(
            Policy::new(2, 2)
                .plan(&sized(&[4096, 4096, 40_960, 40_960]))
                .segments(),
            [0, 1],
            "a narrower policy takes the first run that qualifies, not the largest one"
        );

        // Never one segment, whatever a caller asked for: merging a file into a file costs bytes and
        // removes none.
        for width in [0usize, 1, 2] {
            let plan = Policy::new(width, 1).plan(&sized(&[4096, 4096, 4096]));
            assert!(
                plan.segments().len() >= 2,
                "a policy with a width of {width} planned {} segment(s): a merge of one rewrites \
                 bytes and removes none",
                plan.segments().len()
            );
        }

        // Sizes and ratios are numbers a caller can make large, and a comparison that multiplied them
        // in `u64` would overflow — a policy that panics on a large store is a policy a store cannot
        // run.
        let huge = [u64::MAX, u64::MAX - 1, u64::MAX, u64::MAX];
        assert_eq!(
            Policy::new(4, u64::MAX)
                .plan(&sized(&huge))
                .segments()
                .len(),
            4,
            "four segments of nearly the same enormous size are one run, and the plan is computed \
             rather than overflowed"
        );

        assert!(
            Policy::DEFAULT.plan(&[]).is_empty(),
            "a store with no segments has nothing to merge"
        );
        assert!(
            Plan::empty().is_empty() && Plan::empty().files_removed() == 0,
            "a plan for nothing selects nothing: no ordinal, no bytes, and no file to remove"
        );
    }

    #[test]
    fn a_merge_is_a_value_and_the_store_is_not_moved_by_it() {
        let mut store = store_of(4, 4096);
        let plan = store.plan(Policy::DEFAULT);
        let reader = store.snapshot();
        let ran = store
            .merge(&plan)
            .unwrap_or_else(|refused| panic!("the store's own plan is a merge: {refused:?}"));

        assert_eq!(
            ran.taken,
            [0, 1, 2, 3],
            "the merge value names what it copies"
        );
        assert_eq!(
            ran.index, 4,
            "and the file it wrote, under an ordinal no version names yet: the index names the \
             store, and a file it does not name is not the store"
        );
        assert_eq!(ran.bytes_in, 4 * 4096, "what the run it copies costs");
        assert_eq!(ran.records, 4 * RECORDS);
        assert_eq!(ran.segments_before, 4);
        assert_eq!(ran.epoch, 1, "the epoch this merge would give the store");

        assert_eq!(
            store.segments().len(),
            4,
            "a merge moves nothing the store is made of: not its segments and not its epoch"
        );
        assert_eq!(store.epoch(), 0);
        assert_eq!(store.pending(), Some(4));
        assert!(
            store.set().files().contains(4),
            "the file is written and on the disk — which is exactly the state a crash between \
             writing and installing leaves behind"
        );
        assert!(
            !named(reader.segments()).contains(&4),
            "and a segment offered to a reader is never one the merge is already writing: the reader \
             named four segments and still does"
        );
        assert_eq!(
            store.merge(&plan),
            Err(Refused::MergeInFlight { index: 4 }),
            "a second merge while one is written and not installed would write over its file"
        );

        let mut fresh = store_of(4, 4096);
        let files = fresh.set().files().clone();
        assert_eq!(
            fresh.merge(&Plan::empty()),
            Err(Refused::EmptyPlan),
            "a merge needs segments to take: an empty plan is what a caller tests for with \
             `Plan::is_empty`"
        );
        assert_eq!(
            fresh.pending(),
            None,
            "and a refused merge left no merge in flight"
        );
        assert_eq!(
            files.ordinals(),
            [0, 1, 2, 3],
            "nor did it write a file: four segments, and none of them a merge's"
        );
    }

    #[test]
    fn what_the_store_cannot_carry_out_is_refused_and_leaves_it_alone() {
        let mut store = store_of(4, 4096);
        let plan = store.plan(Policy::DEFAULT);
        let ran = store.merge(&plan).expect("the store's own plan is a merge");
        store
            .install(ran.clone())
            .expect("a merge this store wrote is installed");

        // The plan is a value taken at a moment, and this one names segments the store no longer has.
        assert_eq!(
            store.merge(&plan),
            Err(Refused::StalePlan { index: 0 }),
            "a merge that took what a stale plan names without asking the store again would write a \
             segment holding records that are already merged, and every one of them would be \
             answered twice"
        );
        assert_eq!(
            store.install(ran),
            Err(Refused::NotInstalled { index: 4 }),
            "there is no merge in flight at that ordinal: `install` takes the value `merge` \
             returned, and installing one twice would name a segment the store never wrote"
        );
        assert_eq!(store.epoch(), 1, "every refusal left the epoch alone");
        assert_eq!(named(store.segments()), [4], "and the segments it names");
        assert_eq!(store.set().files().ordinals(), [4]);
    }

    #[test]
    fn the_swap_is_the_one_step_a_merge_is_visible_in() {
        let mut store = store_of(4, 4096);
        let files = store.set().files().clone();
        let plan = store.plan(Policy::DEFAULT);
        let ran = store.merge(&plan).expect("the store's own plan is a merge");
        store
            .install(ran.clone())
            .expect("a merge this store wrote is installed");

        assert_eq!(
            store.epoch(),
            1,
            "a merge is visible in exactly one step: an epoch per stage would be a reader watching a \
             store that is neither of its two states"
        );
        assert_eq!(
            named(store.segments()),
            [4],
            "four segments became one file, and it sits where the run was"
        );
        assert_eq!(
            store.segments()[0].records,
            4 * RECORDS,
            "the new file holds every record its inputs held, once each — a merge removes no record, \
             and the count is the evidence"
        );
        assert_eq!(
            ran.segments_before - store.segments().len(),
            3,
            "and the merge removed three files, which is the cost a reader was paying"
        );
        assert_eq!(
            files.ordinals(),
            [4],
            "the files it replaced are gone once nothing names them: a store that kept them would be \
             a directory that grows with every merge"
        );
        assert_eq!(
            files.deletes(),
            [0, 1, 2, 3],
            "each file was deleted exactly once, by the one handle that outlived the swap"
        );
    }

    #[test]
    fn a_snapshot_taken_before_the_swap_sees_exactly_the_segments_it_took() {
        let mut store = store_of(4, 4096);
        let reader = store.snapshot();
        let plan = store.plan(Policy::DEFAULT);
        let ran = store.merge(&plan).expect("the store's own plan is a merge");
        let files = store.set().files().clone();
        store
            .install(ran)
            .expect("a merge this store wrote is installed");

        assert_eq!(reader.epoch(), 0);
        assert!(
            reader.epoch() < store.epoch(),
            "the epoch is how the two views are told apart: the reader names the store as it was"
        );
        assert_eq!(
            named(reader.segments()),
            [0, 1, 2, 3],
            "a read that began before the swap answers from the segments it named, not from the store \
             that replaced them"
        );
        assert_eq!(reader.bytes(), 4 * 4096, "and costs what it named");
        assert_eq!(reader.records(), 4 * RECORDS, "with every record it named");
        assert_eq!(
            named(store.snapshot().segments()),
            [4],
            "while a read that began after the swap answers from the store the merge made: the store \
             is one store before the swap and after it"
        );
        assert!(
            files.contains(0) && files.contains(3),
            "the files the merge replaced are still on the disk while a reader names them"
        );

        drop(reader);
        assert_eq!(
            files.ordinals(),
            [4],
            "once the last reader that named the old set lets go, its files go: that is the one place \
             a merge deletes anything"
        );
    }

    #[test]
    fn the_last_reader_to_leave_is_the_one_that_frees_the_inputs() {
        let mut store = store_of(4, 4096);
        let files = store.set().files().clone();
        let first = store.snapshot();
        let second = store.snapshot();
        let plan = store.plan(Policy::DEFAULT);
        let plan_len = plan.segments().len();
        let ran = store.merge(&plan).expect("the store's own plan is a merge");
        store
            .install(ran)
            .expect("a merge this store wrote is installed");

        assert_eq!(
            files.ordinals(),
            [0, 1, 2, 3, 4],
            "install retires and does not delete: two readers still name the set the inputs belonged \
             to"
        );
        drop(first);
        assert!(
            files.contains(0),
            "one reader left is still a reader, and a file that left now would be a file nobody can \
             answer from"
        );
        drop(second);
        assert_eq!(
            files.ordinals(),
            [4],
            "the last reader to leave is the one that retires its inputs"
        );
        assert_eq!(
            files.deletes().len(),
            plan_len,
            "and each file was deleted exactly once: a file freed twice is a delete of something \
             that is not there"
        );
    }

    #[test]
    fn retiring_twice_frees_once() {
        let files = Files::new();
        let segments = sized(&[4096, 4096]);
        for info in &segments {
            files.write(info.index);
        }
        files.write(2);

        let set = Set::new(files.clone(), segments, 7);
        set.retire(&[0, 1]);
        set.retire(&[1, 2]);
        assert_eq!(
            files.ordinals(),
            [0, 1, 2],
            "retirement is bookkeeping: nothing leaves the disk while the version that named it is \
             alive"
        );
        drop(set);
        assert_eq!(
            files.deletes(),
            [0, 1, 2],
            "a file retired twice is still one file: the retirement list is a set of ordinals, and a \
             delete asked for twice would be a delete of something that is not there"
        );
        assert_eq!(
            files.ordinals(),
            [] as [u64; 0],
            "so every retired file is gone"
        );
    }
}
