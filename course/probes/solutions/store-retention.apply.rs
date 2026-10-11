//! probe = "store/retention@apply"
//! title = "Old data leaves in whole buckets, and a floor says so"
//! hint = "Snap the cutoff down to a bucket boundary, expire the segments whose last timestamp is below it, keep the one that straddles, and never move the floor backwards."
//!
//! Step `lakelet/4.9-retention` is the first thing in this store that gives a byte back, and it is
//! two claims that have to hold together: everything older than the cutoff is unreachable, and
//! nothing newer than the cutoff was touched. Everything in this file is a rule somebody paid for
//! once:
//!
//! * **old data leaves in whole segments, never records inside one.** Deleting the file is one line;
//!   the hard case is the segment that holds records on both sides of the cutoff. It cannot be
//!   deleted — that takes newer records with it — and it cannot be rewritten, because rewriting it
//!   pays exactly the write amplification `lakelet/4.8-compaction` spent two budgets measuring. The
//!   consequence is the **straddler**: kept, still counted by the index, and no longer an answer;
//! * **a segment expires by its *last* timestamp, and the comparison is half-open.** A record whose
//!   timestamp is exactly the cutoff is kept, so a segment whose last record *is* the cutoff stays and
//!   one a millisecond older goes. The consequence of turning that around is the shortest mistake in
//!   the step: `<=` deletes the bucket the policy meant to keep;
//! * **the cutoff is a bucket boundary, not an instant.** `now - keep`, snapped *down* through the
//!   bucket grid, so the same policy at 00:59 and at 01:01 deletes the same thing and an operator can
//!   say the boundary out loud. The price is coarse — data is kept for somewhere between `keep` and
//!   `keep + width` — and it is the price `lakelet/4.10-downsampling` pays on the same arithmetic;
//! * **the floor is durable and one-way.** The index states the millisecond the store has already
//!   forgotten up to, and a pass acts at its own cutoff or at the floor, whichever is later: a promise
//!   that data is gone cannot be taken back. A pass that expires nothing writes nothing at all — same
//!   epoch, same index — or a policy that runs every minute rewrites the index every minute to say
//!   what it already said.
//!
//! ## What you write
//!
//! * [`Partition::of`] — the bucket a timestamp belongs to;
//! * [`Partition::contains`] — the half-open test that makes one boundary belong to one bucket;
//! * [`Policy::cutoff`] — `now - keep`, snapped down, and never below the epoch;
//! * [`Policy::plan`] — what expires, what stays, and what straddles;
//! * [`Segments::expire`] — the pass: move the floor, drop the expired segments, account for itself;
//! * [`Snapshot::answers`] — what a read at or after a floor returns.
//!
//! [`Record`], [`SegmentInfo`], [`Partition`]'s accessors, [`Policy`]'s accessors, [`Plan`],
//! [`ExpireReport`], [`Segments::seal`], [`Segments::snapshot`] and [`Snapshot`]'s other accessors are
//! given: the index's shape and the report's vocabulary are the project's, and a probe that let you
//! invent them would be checking a different store.
//!
//! ## What the tests ask
//!
//! The bucket arithmetic: a boundary timestamp belongs to the bucket that starts on it, a timestamp
//! one millisecond past a boundary belongs to the bucket *below* it, a width of zero is a bucket per
//! millisecond rather than a bucket that covers nothing, and a width that does not divide the
//! timestamp leaves the remainder in the bucket above. The cutoff: three and a half hours past the
//! epoch snapped down to the third hour, a window that is already a whole number of buckets not
//! snapped any further, a window that reaches past the epoch saturating at zero, and a policy that
//! keeps everything answering zero at every reading. The plan: `last < cutoff` expires, `last ==
//! cutoff` stays, a retained segment whose `first < cutoff` straddles, and a segment whose age nobody
//! stated is kept and is *not* straddling — the one case where the conservative answer is the whole
//! rule. Then a pass that expires nothing and writes nothing, a pass that expires two segments of
//! eight and accounts for the bytes, a floor that never moves backwards, a reader that took its
//! snapshot before the pass and still gets every record it would have got, and the straddler: on the
//! disk, counted by the index, and not an answer.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no manifest and no disk**: the records live in memory, `expire` drops
//! a segment by dropping a value rather than by deleting a file, and the "reader that still gets its
//! answer" is a reader holding a clone of the set it named rather than one holding an [`Arc`] the
//! store deferred its deletion for. What a probe can check is the arithmetic of the boundary — which
//! segment leaves, which stays, which straddles, what the floor does — and that is what a re-check of
//! this habit is worth: the trap is a comparison, not a file system.
//!
//! The real step's index is version 2 of the manifest's layout, with a floor in its header and two
//! timestamps per entry, and the version byte is what makes an older index a *refusal* rather than a
//! misreading — none of those bytes are here. Also missing on purpose: the straddler's old records on
//! the disk (the pass neither deletes nor rewrites it, and this file simply keeps them), the merge
//! that states the union of its inputs' ranges, the refusal a pass earns while a merge is written and
//! not installed, and the budget lane's numbers (`bytes_rewritten_max`, `records_below_cutoff_max`),
//! which are measured from files that exist.

use std::fmt;

/// The bucket width a policy counts in when nobody says otherwise, in milliseconds.
pub const HOUR: u64 = 3_600_000;

/// One record as a client sent it.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// When, in milliseconds since the epoch: the unit the index and this store share.
    pub ts_millis: u64,
    /// What it measured.
    pub value: f64,
}

/// One segment as the store's index states it: what it holds, and the range of timestamps it holds.
///
/// `first` and `last` are `Option` because an entry can state *no* range: that is the value a caller
/// builds by hand for a decision about **size** (`lakelet/4.8-compaction`'s plans are made from it),
/// and a policy never deletes on a number nobody wrote down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentInfo {
    /// The ordinal its file is named by.
    pub index: u64,
    /// How many bytes its file holds.
    pub bytes: u64,
    /// How many records it holds.
    pub records: u64,
    /// The earliest timestamp it holds, or nothing when nobody stated one.
    pub first_millis: Option<u64>,
    /// The latest timestamp it holds, or nothing when nobody stated one.
    pub last_millis: Option<u64>,
}

impl SegmentInfo {
    /// A segment that states no time range at all.
    #[must_use]
    pub const fn new(index: u64, bytes: u64, records: u64) -> Self {
        Self {
            index,
            bytes,
            records,
            first_millis: None,
            last_millis: None,
        }
    }

    /// A segment whose records span `first_millis` to `last_millis`, inclusive at both ends.
    #[must_use]
    pub const fn spanning(
        index: u64,
        bytes: u64,
        records: u64,
        first_millis: u64,
        last_millis: u64,
    ) -> Self {
        Self {
            index,
            bytes,
            records,
            first_millis: Some(first_millis),
            last_millis: Some(last_millis),
        }
    }
}

/// A bucket of absolute time: half-open, `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Partition {
    start_millis: u64,
    width_millis: u64,
}

impl Partition {
    /// The bucket `ts_millis` belongs to, `width_millis` wide.
    ///
    /// A width of zero is treated as one millisecond: a bucket that covered no time would hold no
    /// timestamp at all, including the one it was made from.
    #[must_use]
    pub fn of(ts_millis: u64, width_millis: u64) -> Self {
        let width_millis = width_millis.max(1);
        Self {
            start_millis: ts_millis - ts_millis % width_millis,
            width_millis,
        }
    }

    /// The millisecond this bucket starts at.
    #[must_use]
    pub const fn start(&self) -> u64 {
        self.start_millis
    }

    /// The millisecond after this bucket's last one: buckets are half-open, `[start, end)`.
    #[must_use]
    pub const fn end(&self) -> u64 {
        self.start_millis.saturating_add(self.width_millis)
    }

    /// How many milliseconds this bucket covers.
    #[must_use]
    pub const fn width(&self) -> u64 {
        self.width_millis
    }

    /// Which bucket of this width this is, counted from the epoch.
    #[must_use]
    pub const fn index(&self) -> u64 {
        self.start_millis / self.width_millis
    }

    /// `true` when `ts_millis` is this bucket's.
    ///
    /// A bucket is half-open: a timestamp exactly on a boundary belongs to the bucket that starts
    /// there, so a bucket and its neighbour never both hold one.
    #[must_use]
    pub fn contains(&self, ts_millis: u64) -> bool {
        self.start_millis <= ts_millis && ts_millis < self.end()
    }
}

impl fmt::Display for Partition {
    /// The bucket the way a cutoff is written in a report: `[start, end)`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[{}, {})", self.start(), self.end())
    }
}

/// What a store keeps: a window of time, counted in buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    keep_millis: u64,
    width_millis: u64,
}

impl Policy {
    /// Keep everything, in buckets of an hour: the value that cannot lose data.
    pub const DEFAULT: Self = Self {
        keep_millis: 0,
        width_millis: HOUR,
    };

    /// A policy that keeps `keep_millis` of data, counted in buckets of `width_millis`.
    #[must_use]
    pub const fn new(keep_millis: u64, width_millis: u64) -> Self {
        Self {
            keep_millis,
            width_millis,
        }
    }

    /// How much time it keeps.
    #[must_use]
    pub const fn keep_millis(&self) -> u64 {
        self.keep_millis
    }

    /// How wide one of its buckets is.
    #[must_use]
    pub const fn width_millis(&self) -> u64 {
        self.width_millis
    }

    /// `true` when this policy keeps everything: a window of zero is the largest window there is.
    #[must_use]
    pub const fn keeps_everything(&self) -> bool {
        self.keep_millis == 0
    }

    /// The millisecond this policy forgets at or below at `now_millis`: `now - keep`, snapped
    /// **down** to a bucket boundary.
    ///
    /// A raw `now - keep` moves with the clock, so the same store and the same policy would delete
    /// different things a minute apart. Snapping down is the direction that never deletes a bucket the
    /// policy did not name, and a window that reaches back past the epoch saturates at zero — there is
    /// no millisecond below zero to expire.
    #[must_use]
    pub fn cutoff(&self, now_millis: u64) -> u64 {
        if self.keeps_everything() {
            return 0;
        }
        Partition::of(
            now_millis.saturating_sub(self.keep_millis),
            self.width_millis,
        )
        .start()
    }

    /// Which of `segments` expires at `cutoff_millis`, which of them stays, and which of the ones that
    /// stay reach below the cutoff.
    ///
    /// A segment expires exactly when its **last** timestamp is below the cutoff, and the comparison
    /// is half-open: a record whose timestamp is exactly the cutoff is kept. A segment whose age
    /// nobody stated is retained and is *not* straddling — nobody compared it with the cutoff, and
    /// listing it as reaching below one would be a claim about a range that is not there.
    #[must_use]
    pub fn plan(&self, segments: &[SegmentInfo], cutoff_millis: u64) -> Plan {
        let mut expired: Vec<SegmentInfo> = Vec::new();
        let mut retained: Vec<SegmentInfo> = Vec::new();
        let mut straddling: Vec<u64> = Vec::new();
        for info in segments {
            let Some(last) = info.last_millis else {
                retained.push(*info);
                continue;
            };
            if last < cutoff_millis {
                expired.push(*info);
            } else {
                if info.first_millis.is_some_and(|first| first < cutoff_millis) {
                    straddling.push(info.index);
                }
                retained.push(*info);
            }
        }
        Plan::new(cutoff_millis, expired, retained, straddling)
    }
}

/// What one retention pass would do, as a value: the cutoff it was made at, what expires, what stays,
/// and which of the retained segments straddle the cutoff.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    cutoff: u64,
    expired: Vec<SegmentInfo>,
    retained: Vec<SegmentInfo>,
    straddling: Vec<u64>,
}

impl Plan {
    /// The plan a policy made: taken as it is given.
    #[must_use]
    pub const fn new(
        cutoff: u64,
        expired: Vec<SegmentInfo>,
        retained: Vec<SegmentInfo>,
        straddling: Vec<u64>,
    ) -> Self {
        Self {
            cutoff,
            expired,
            retained,
            straddling,
        }
    }

    /// The cutoff it was made at.
    #[must_use]
    pub const fn cutoff(&self) -> u64 {
        self.cutoff
    }

    /// The segments that go, in the manifest's order.
    #[must_use]
    pub fn expired(&self) -> &[SegmentInfo] {
        &self.expired
    }

    /// The segments that stay, in the manifest's order, with the manifest's own values.
    #[must_use]
    pub fn retained(&self) -> &[SegmentInfo] {
        &self.retained
    }

    /// The ordinals of the retained segments that reach below the cutoff.
    #[must_use]
    pub fn straddling(&self) -> &[u64] {
        &self.straddling
    }

    /// `true` when this plan expires nothing: the test a caller makes before touching the store.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.expired.is_empty()
    }

    /// How many bytes expiring would give back.
    #[must_use]
    pub fn expired_bytes(&self) -> u64 {
        self.expired
            .iter()
            .fold(0u64, |total, info| total.saturating_add(info.bytes))
    }

    /// How many records that is, as the index counts them.
    #[must_use]
    pub fn expired_records(&self) -> u64 {
        self.expired
            .iter()
            .fold(0u64, |total, info| total.saturating_add(info.records))
    }

    /// What the segments that stay cost.
    #[must_use]
    pub fn retained_bytes(&self) -> u64 {
        self.retained
            .iter()
            .fold(0u64, |total, info| total.saturating_add(info.bytes))
    }
}

/// What one pass did, and what it cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpireReport {
    /// The epoch the store ended at: a new one when something expired, the same one when nothing did.
    pub epoch: u64,
    /// The cutoff the pass acted at.
    pub cutoff: u64,
    /// The floor the store is at afterwards.
    pub floor: u64,
    /// How many segments it found.
    pub segments_before: usize,
    /// How many it left.
    pub segments_after: usize,
    /// The segments it expired.
    pub expired: Vec<SegmentInfo>,
    /// The segments it kept, with the index's own values.
    pub retained: Vec<SegmentInfo>,
    /// The ordinals of the retained segments that straddle its cutoff.
    pub straddling: Vec<u64>,
    /// How many bytes the disk gave back.
    pub bytes_reclaimed: u64,
    /// How many records the index stopped naming.
    pub records_expired: u64,
}

impl ExpireReport {
    /// The report of a pass that made `plan` at `epoch` and `floor`, over `segments_before` segments.
    ///
    /// Given because it is bookkeeping over the plan: every number here is one of the plan's, and the
    /// part of the pass worth re-checking is which segments the plan named.
    #[must_use]
    pub fn of(plan: &Plan, epoch: u64, floor: u64, segments_before: usize) -> Self {
        Self {
            epoch,
            cutoff: plan.cutoff(),
            floor,
            segments_before,
            segments_after: plan.retained().len(),
            expired: plan.expired().to_vec(),
            retained: plan.retained().to_vec(),
            straddling: plan.straddling().to_vec(),
            bytes_reclaimed: plan.expired_bytes(),
            records_expired: plan.expired_records(),
        }
    }
}

/// One segment and the records its file holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// What the store's index states about it.
    pub info: SegmentInfo,
    /// The records its file holds, in the order it accepted them.
    pub records: Vec<Record>,
}

/// The store's index and its records: an epoch, a floor, and the segments it is made of.
#[derive(Debug, Clone, PartialEq)]
pub struct Segments {
    epoch: u64,
    floor: u64,
    blocks: Vec<Block>,
}

impl Segments {
    /// An empty store: no segments, a floor of nothing, an epoch nothing has happened at.
    #[must_use]
    pub fn open() -> Self {
        Self {
            epoch: 0,
            floor: 0,
            blocks: Vec::new(),
        }
    }

    /// Close `records` into a segment whose file costs `bytes`, and answer its ordinal.
    ///
    /// The range the entry states comes from the records themselves, which is what `seal` is for: a
    /// policy has to be able to decide what is old without opening a file.
    pub fn seal(&mut self, records: &[Record], bytes: u64) -> u64 {
        let index = self.blocks.last().map_or(0, |block| block.info.index + 1);
        let first = records.iter().map(|record| record.ts_millis).min();
        let last = records.iter().map(|record| record.ts_millis).max();
        let info = match (first, last) {
            (Some(first), Some(last)) => {
                SegmentInfo::spanning(index, bytes, records.len() as u64, first, last)
            }
            _ => SegmentInfo::new(index, bytes, 0),
        };
        self.blocks.push(Block {
            info,
            records: records.to_vec(),
        });
        self.epoch += 1;
        index
    }

    /// How many times the store has changed shape.
    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The millisecond this store has forgotten up to: everything below it is not an answer.
    #[must_use]
    pub const fn floor(&self) -> u64 {
        self.floor
    }

    /// The segments it is made of, oldest first.
    #[must_use]
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// How many records the index still counts — which is not what a read returns.
    #[must_use]
    pub fn records(&self) -> u64 {
        self.blocks.iter().fold(0u64, |total, block| {
            total.saturating_add(block.info.records)
        })
    }

    /// What a reader takes when it starts: the set of segments that are this store now, and the floor
    /// of this epoch.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            floor: self.floor,
            blocks: self.blocks.clone(),
        }
    }

    /// Carry `policy` out at `now_millis`: move the floor, drop what expires, and account for the
    /// pass.
    ///
    /// The pass acts at its own cutoff or at the store's floor, whichever is later — a promise that
    /// data is gone cannot be taken back — and a pass that expires nothing writes nothing at all.
    pub fn expire(&mut self, policy: &Policy, now_millis: u64) -> ExpireReport {
        let before = self.blocks.len();
        let cutoff = policy.cutoff(now_millis).max(self.floor);
        let segments: Vec<SegmentInfo> = self.blocks.iter().map(|block| block.info).collect();
        let plan = policy.plan(&segments, cutoff);
        if plan.is_empty() {
            // Nothing to expire is nothing to write: no new epoch, no new index, no file touched.
            return ExpireReport::of(&plan, self.epoch, self.floor, before);
        }
        let expired: Vec<u64> = plan.expired().iter().map(|info| info.index).collect();
        self.blocks
            .retain(|block| !expired.contains(&block.info.index));
        self.floor = cutoff;
        self.epoch += 1;
        ExpireReport::of(&plan, self.epoch, self.floor, before)
    }
}

/// What one reader may see: the set of segments it named, and the floor of that epoch.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    floor: u64,
    blocks: Vec<Block>,
}

impl Snapshot {
    /// The millisecond the store it named had already forgotten up to.
    #[must_use]
    pub const fn floor(&self) -> u64 {
        self.floor
    }

    /// The segments it named, oldest first.
    #[must_use]
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// How many records the index of the epoch it named still counts: the *segments* hold these, and a
    /// straddler's records below the floor are among them.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.blocks.iter().fold(0u64, |total, block| {
            total.saturating_add(block.info.records)
        })
    }

    /// `true` when it named no segments at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Every record at or after this snapshot's floor, in the order the store kept them.
    ///
    /// The floor is the epoch's, not the store's: a pass that runs beside this reader raises the
    /// store's floor and leaves this one where it was.
    #[must_use]
    pub fn answers(&self) -> Vec<Record> {
        self.blocks
            .iter()
            .flat_map(|block| block.records.iter())
            .filter(|record| record.ts_millis >= self.floor)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Milliseconds between two records of one segment: a sample a minute.
    const STEP: u64 = 60_000;

    /// The first timestamp of the first segment, on a bucket boundary so the arithmetic reads.
    const EPOCH: u64 = 472_222 * HOUR;

    /// Segments a case seals, unless it builds its own list.
    const SEGMENTS: usize = 8;

    /// Records one sealed segment holds: ninety minutes of one series, a sample a minute.
    const PER_SEGMENT: usize = 90;

    /// What one sealed segment's file costs.
    const SEGMENT_BYTES: u64 = 4096;

    /// What the cases' policy keeps: six and a half hours, deliberately not a whole number of buckets.
    const KEEP: u64 = 6 * HOUR + 30 * 60_000;

    /// The reading every case passes in: ten hours after [`EPOCH`], so the whole store is in the past.
    const NOW: u64 = EPOCH + 10 * HOUR;

    /// The cutoff [`KEEP`] has at [`NOW`]: three and a half hours after the epoch, snapped to the
    /// third hour. Two of the eight segments are below it and the third straddles it.
    const CUTOFF: u64 = EPOCH + 3 * HOUR;

    /// The policy the cases use.
    fn policy() -> Policy {
        Policy::new(KEEP, HOUR)
    }

    /// One record of segment `index`, `offset` minutes past its first.
    fn record(index: usize, offset: usize) -> Record {
        Record {
            ts_millis: EPOCH + index as u64 * HOUR + offset as u64 * STEP,
            value: offset as f64 / 4.0,
        }
    }

    /// The records segment `index` holds: decided by the two numbers that name them, so a case can
    /// rebuild the whole store's record list without reading the store it is judging.
    fn batch(index: usize) -> Vec<Record> {
        (0..PER_SEGMENT)
            .map(|offset| record(index, offset))
            .collect()
    }

    /// Every record a store of [`SEGMENTS`] sealed segments holds, in the order it accepted them.
    fn sealed() -> Vec<Record> {
        (0..SEGMENTS).flat_map(batch).collect()
    }

    /// A store in a directory of its own, with [`SEGMENTS`] sealed segments in it.
    fn sealed_store() -> Segments {
        let mut store = Segments::open();
        for index in 0..SEGMENTS {
            store.seal(&batch(index), SEGMENT_BYTES);
        }
        store
    }

    /// One segment as a plan sees it, with the range the index states for it.
    fn spanned(index: u64, bytes: u64, records: u64, first: u64, last: u64) -> SegmentInfo {
        SegmentInfo::spanning(index, bytes, records, first, last)
    }

    /// The ordinals of a segment list, in order.
    fn indices(segments: &[SegmentInfo]) -> Vec<u64> {
        segments.iter().map(|info| info.index).collect()
    }

    /// The ordinals a store's index names, in order.
    fn named(blocks: &[Block]) -> Vec<u64> {
        blocks.iter().map(|block| block.info.index).collect()
    }

    /// The records a read answers from a store, once the policy has run at [`NOW`].
    fn answered(store: &Segments) -> Vec<Record> {
        store.snapshot().answers()
    }

    #[test]
    fn a_timestamp_belongs_to_the_bucket_that_starts_at_it() {
        let hour = Partition::of(0, HOUR);
        assert_eq!(hour.start(), 0, "the first bucket starts at the epoch");
        assert_eq!(hour.end(), HOUR, "and ends one width later");
        assert_eq!(hour.width(), HOUR, "which is the width it was made with");
        assert_eq!(hour.index(), 0, "counted from the epoch: `start / width`");
        assert!(
            hour.contains(0) && hour.contains(HOUR - 1),
            "a bucket is half-open `[start, end)`: its first millisecond is in it and its last \
             millisecond before the end is too"
        );
        assert!(
            !hour.contains(HOUR),
            "and the boundary is not: a timestamp exactly on one belongs to the bucket that starts \
             there, which is what keeps two neighbours from both holding it"
        );

        let third = Partition::of(CUTOFF, HOUR);
        assert_eq!(
            third.start(),
            CUTOFF,
            "a timestamp already on a boundary is its own bucket's start"
        );
        assert_eq!(
            Partition::of(CUTOFF + 1, HOUR).start(),
            CUTOFF,
            "and one millisecond past a boundary is in the bucket below it: a cutoff is snapped \
             *down*, never up, so a pass never deletes a bucket the policy did not name"
        );
        assert_eq!(
            third.index(),
            472_225,
            "which bucket that is, counted from the epoch"
        );

        let odd = Partition::of(1_000, 7 * 60_000);
        assert_eq!(
            odd.start(),
            0,
            "a width that does not divide the timestamp leaves the remainder in the bucket above"
        );
        assert_eq!(odd.end(), 7 * 60_000, "and the bucket is `[0, 7 minutes)`");
        assert!(
            odd.contains(1_000) && odd.contains(419_999) && !odd.contains(420_000),
            "which is half-open at both ends of the remainder"
        );

        let single = Partition::of(86_399_999, 1);
        assert_eq!(
            single.width(),
            1,
            "a width of one millisecond is a bucket per millisecond"
        );
        assert_eq!(
            single.start(),
            86_399_999,
            "which holds exactly the timestamp it was made from"
        );
        assert!(single.contains(86_399_999) && !single.contains(86_400_000));

        let zero = Partition::of(41, 0);
        assert_eq!(
            zero.width(),
            1,
            "a width of zero is treated as one: a bucket that covered no time would hold no \
             timestamp at all, including the one it was made from"
        );
        assert_eq!(
            zero.start(),
            41,
            "so the timestamp is in a bucket of its own"
        );
        assert!(
            zero.contains(41) && !zero.contains(42),
            "and that bucket holds exactly one millisecond"
        );

        assert_eq!(
            Partition::of(CUTOFF, HOUR).to_string(),
            format!("[{CUTOFF}, {})", CUTOFF + HOUR),
            "a bucket prints as the half-open range it is: an operator reading a cutoff in a log line \
             has to see which bucket it is"
        );
    }

    #[test]
    fn a_cutoff_is_a_bucket_boundary_and_zero_keeps_everything() {
        assert_eq!(
            policy().keep_millis(),
            KEEP,
            "a policy states how much time it keeps"
        );
        assert_eq!(
            policy().width_millis(),
            HOUR,
            "and how wide a bucket of it is"
        );
        assert!(
            !policy().keeps_everything(),
            "a policy that keeps six and a half hours does not keep everything"
        );
        assert_eq!(
            policy().cutoff(NOW),
            CUTOFF,
            "the cutoff is `now - keep` snapped down to the hour below it: three and a half hours \
             past the epoch is the third hour"
        );
        assert_eq!(
            policy().cutoff(NOW) % HOUR,
            0,
            "so it is a multiple of the width — the same number on every machine and in every \
             timezone, whatever the clock said a millisecond ago"
        );
        assert_eq!(
            Policy::new(3 * HOUR, HOUR).cutoff(NOW),
            EPOCH + 7 * HOUR,
            "a window that is already a whole number of buckets lands on a boundary and is not \
             snapped any further"
        );

        for now in [0u64, 1, NOW, u64::MAX] {
            assert_eq!(
                Policy::DEFAULT.cutoff(now),
                0,
                "a policy that keeps everything answers zero at `now = {now}`, which is below every \
                 timestamp a store can hold: an unconfigured store deletes nothing"
            );
        }
        assert!(
            Policy::DEFAULT.keeps_everything() && Policy::DEFAULT.width_millis() == HOUR,
            "which is what `keeps_everything` says, while its width is the one an operator reads \
             telemetry in"
        );
        assert_eq!(
            Policy::new(u64::MAX, HOUR).cutoff(1_000),
            0,
            "a window that reaches back past the epoch saturates: there is no millisecond below zero \
             to expire"
        );
        assert_eq!(
            Policy::new(HOUR, 0).cutoff(NOW),
            NOW - HOUR,
            "a width of zero is a millisecond bucket, so nothing is snapped away"
        );
    }

    #[test]
    fn a_plan_names_what_expires_what_stays_and_what_straddles() {
        let segments = vec![
            spanned(0, 100, 10, 0, CUTOFF - 1),
            spanned(1, 200, 20, 0, CUTOFF),
            spanned(2, 300, 30, CUTOFF - 1, CUTOFF + 1),
            spanned(3, 400, 40, CUTOFF, CUTOFF + 5),
            spanned(4, 500, 50, CUTOFF + 1, CUTOFF + 9),
            spanned(5, 600, 60, CUTOFF + 10, CUTOFF + 10),
        ];
        let plan = policy().plan(&segments, CUTOFF);
        assert_eq!(
            plan.cutoff(),
            CUTOFF,
            "the plan says which cutoff it was made at"
        );
        assert_eq!(
            indices(plan.expired()),
            [0],
            "a segment expires exactly when its *last* timestamp is below the cutoff: one whose last \
             record is a millisecond older than it is old data"
        );
        assert_eq!(
            indices(plan.retained()),
            [1, 2, 3, 4, 5],
            "and the ones that stay keep the manifest's order — including the segment whose last \
             timestamp is *exactly* the cutoff, which is the `<=`-for-`<` trap this rule turns on"
        );
        assert_eq!(
            plan.retained(),
            &segments[1..],
            "with the manifest's own values: same ordinal, same bytes, same records, same timestamps. \
             `nothing newer than the cutoff was touched` is an equality claim about the index"
        );
        assert_eq!(
            plan.straddling(),
            [1, 2],
            "a retained segment whose *first* timestamp is below the cutoff straddles it: it holds \
             records on both sides, which is what makes it neither deletable nor rewritable"
        );
        assert!(
            !plan.straddling().contains(&3),
            "while a segment whose first timestamp is exactly the cutoff does not: it has nothing \
             below it, so rewriting it would spend bytes to remove nothing"
        );
        assert!(
            !plan.is_empty(),
            "a plan with something in `expired` is not empty — that is the test a caller makes \
             before touching a store"
        );
        assert_eq!(
            plan.expired_bytes(),
            100,
            "a plan adds up what it would delete"
        );
        assert_eq!(plan.expired_records(), 10, "and how many records that is");
        assert_eq!(
            plan.retained_bytes(),
            2000,
            "and what the segments that stay cost"
        );
        assert_eq!(
            plan.retained().len() + plan.expired().len(),
            segments.len(),
            "every segment is on exactly one side of the plan: an expiry that dropped one silently, \
             or listed one twice, would show up here before it touched anything"
        );

        assert!(
            policy().plan(&segments, 0).is_empty(),
            "a cutoff of zero expires nothing: every timestamp is at or after it"
        );
        assert_eq!(
            policy().plan(&segments, 0).retained().len(),
            segments.len(),
            "and every segment is retained"
        );
        assert!(
            policy().plan(&segments, 0).straddling().is_empty(),
            "with nothing straddling it: a bucket is half-open, so zero is not below a first \
             timestamp of zero"
        );
        assert!(
            policy().plan(&[], CUTOFF).is_empty(),
            "and a store with no segments has nothing to expire"
        );
        assert_eq!(
            policy().plan(&segments, u64::MAX).expired().len(),
            segments.len(),
            "while a cutoff past every timestamp expires the whole store: the rule is a comparison, \
             and it does not care how large the numbers are"
        );

        // The one case where the conservative answer *is* the rule: a segment whose age nobody wrote
        // down is kept, and it is not straddling either — nobody read it against the cutoff.
        let silent = [SegmentInfo::new(0, 100, 10), SegmentInfo::new(1, 200, 20)];
        let plan = policy().plan(&silent, CUTOFF);
        assert!(
            plan.is_empty(),
            "a plan made from segments whose ranges nobody stated expires nothing: `None` is not an \
             old timestamp, it is an unstated one, and deleting a file is the one decision this store \
             cannot take back"
        );
        assert_eq!(
            plan.retained(),
            &silent[..],
            "so every one of them stays, with the index's own values"
        );
        assert!(
            plan.straddling().is_empty(),
            "and none of them is called a straddler: that would claim it reaches below a boundary it \
             was never read against"
        );
        let mixed = policy().plan(
            &[
                SegmentInfo::new(0, 100, 10),
                spanned(1, 200, 20, 0, CUTOFF - 1),
            ],
            CUTOFF,
        );
        assert_eq!(
            indices(mixed.expired()),
            [1],
            "while a segment whose range *is* stated expires by the ordinary rule, in the same plan: \
             the default is about the segment nobody wrote down, not about the plan"
        );
        assert_eq!(indices(mixed.retained()), [0], "and the silent one stays");
    }

    #[test]
    fn a_pass_that_expires_nothing_writes_nothing() {
        let mut store = sealed_store();
        let epoch = store.epoch();
        let blocks = store.blocks().to_vec();
        let report = store.expire(&Policy::DEFAULT, NOW);

        assert_eq!(
            report.epoch, epoch,
            "a pass that expires nothing moves nothing: its report carries the epoch the store is \
             already at"
        );
        assert_eq!(
            store.epoch(),
            epoch,
            "and the store's epoch did not move either"
        );
        assert!(
            report.expired.is_empty(),
            "nothing was below a cutoff of zero"
        );
        assert_eq!(
            report.cutoff, 0,
            "which is what a policy that keeps everything asks for"
        );
        assert_eq!(
            store.floor(),
            0,
            "and the floor it never had is still nothing"
        );
        assert_eq!(
            store.blocks(),
            &blocks[..],
            "every segment is where it was, byte for byte the same index: a background task that ran \
             every minute would otherwise rewrite the index every minute to say what it already said"
        );
        assert_eq!(report.segments_before, report.segments_after);
        assert_eq!(report.bytes_reclaimed, 0, "and the disk gave nothing back");

        // A pass whose cutoff is below the oldest record is the same answer with a smaller policy.
        let report = store.expire(&Policy::new(HOUR, HOUR), EPOCH + HOUR);
        assert_eq!(
            report.epoch, epoch,
            "a cutoff below every segment expires nothing, and a pass that expires nothing writes \
             nothing"
        );
        assert!(report.expired.is_empty() && store.blocks().len() == SEGMENTS);
    }

    #[test]
    fn a_pass_deletes_whole_segments_and_accounts_for_itself() {
        let mut store = sealed_store();
        let before = store.blocks().to_vec();
        let epoch = store.epoch();
        let report = store.expire(&policy(), NOW);

        assert_eq!(
            indices(&report.expired),
            [0, 1],
            "the segments whose last record is below the cutoff: two of eight, whole segments and \
             never a record inside one"
        );
        assert_eq!(
            indices(&report.retained),
            [2, 3, 4, 5, 6, 7],
            "and six that stay, in the manifest's order"
        );
        assert_eq!(
            report.straddling,
            [2],
            "with one straddling the cutoff: the segment that holds records on both sides of it"
        );
        assert_eq!(
            report.cutoff, CUTOFF,
            "the pass acted at the policy's bucket boundary"
        );
        assert_eq!(report.floor, CUTOFF, "and the store's floor is that cutoff");
        assert_eq!(store.floor(), CUTOFF);
        assert_eq!(
            report.epoch,
            epoch + 1,
            "the store changed shape once: one pass, one new epoch"
        );
        assert_eq!(report.segments_before, SEGMENTS);
        assert_eq!(report.segments_after, 6);
        assert_eq!(
            report.segments_before - report.expired.len(),
            report.segments_after,
            "a pass accounts for itself: what it found less what it expired is what it left"
        );
        assert_eq!(
            report.bytes_reclaimed,
            2 * SEGMENT_BYTES,
            "and what it reclaimed is the files it expired, counted in the index's own bytes"
        );
        assert_eq!(report.records_expired, 2 * PER_SEGMENT as u64);
        assert_eq!(
            named(store.blocks()),
            [2, 3, 4, 5, 6, 7],
            "the store's index names what the report says it left"
        );
        assert_eq!(
            store.blocks(),
            &before[2..],
            "and the segments that stay are the ones that were: same values, same ranges, same bytes \
             — `nothing newer than the cutoff was touched` is that equality and not a count"
        );
    }

    #[test]
    fn the_floor_never_moves_backwards() {
        let mut store = sealed_store();
        let report = store.expire(&policy(), NOW);
        assert_eq!(
            store.floor(),
            CUTOFF,
            "a pass moves the floor to its cutoff"
        );
        let epoch = store.epoch();
        assert_eq!(report.epoch, epoch);

        // A later policy with a shorter window, or a clock that went backwards: a promise that data is
        // gone cannot be taken back, so this pass writes nothing at all.
        let behind = store.expire(&policy(), EPOCH + HOUR);
        assert!(
            behind.expired.is_empty(),
            "a cutoff below the floor expires nothing: the pass acts at the floor, whichever cutoff \
             its policy computed"
        );
        assert_eq!(behind.epoch, epoch, "the epoch did not move");
        assert_eq!(
            store.floor(),
            CUTOFF,
            "and the floor is where it was: a store that moved it back would answer records it \
             promised were gone"
        );

        // Forward is the one direction it has, and it still moves in whole buckets.
        let later = store.expire(&policy(), NOW + 3 * HOUR);
        assert_eq!(
            later.cutoff,
            CUTOFF + 3 * HOUR,
            "the same policy at a later reading: `now - keep` snapped down to the hour below it"
        );
        assert_eq!(later.cutoff % HOUR, 0, "which is a bucket boundary again");
        assert_eq!(indices(&later.expired), [2, 3, 4]);
        assert_eq!(store.floor(), CUTOFF + 3 * HOUR);
        assert_eq!(
            later.epoch,
            epoch + 1,
            "one pass that changed something: one new epoch"
        );
    }

    #[test]
    fn a_reader_that_took_its_snapshot_before_the_pass_keeps_its_answer() {
        let mut store = sealed_store();
        let reader = store.snapshot();
        let everything = sealed();
        store.expire(&policy(), NOW);

        assert_eq!(
            reader.floor(),
            0,
            "the reader named an epoch that had forgotten nothing"
        );
        assert_eq!(
            reader.answers(),
            everything,
            "so it answers every record the store had accepted when it started — the same records, \
             once each, in the same order: a pass may not rewrite an answer that is in flight"
        );
        assert_eq!(
            reader.blocks().len(),
            SEGMENTS,
            "it still names the set it took"
        );

        // A read that begins after the pass answers what the store still keeps, and that is the other
        // half of the claim: the store is one store before the pass and after it.
        let after = answered(&store);
        let expected: Vec<Record> = everything
            .iter()
            .filter(|record| record.ts_millis >= CUTOFF)
            .cloned()
            .collect();
        assert_eq!(
            after, expected,
            "a read after the pass answers the records at or after the cutoff and no older one — \
             while the reader that began before it answers exactly what it would have answered"
        );
        assert_eq!(
            after.len(),
            480,
            "which is 480 of the 720 the store accepted"
        );
        assert_eq!(
            store.snapshot().floor(),
            CUTOFF,
            "the store's own floor moved with the pass"
        );
    }

    #[test]
    fn the_index_still_counts_what_a_read_no_longer_answers() {
        let mut store = sealed_store();
        let reader = store.snapshot();
        store.expire(&policy(), NOW);
        let after = store.snapshot();

        assert_eq!(
            after.floor(),
            CUTOFF,
            "the read that begins after the pass is confined to what the store still keeps"
        );
        assert_eq!(
            store.records(),
            6 * PER_SEGMENT as u64,
            "the index still counts every record of the segments it names — a straddler's old ones \
             included, because nothing rewrote them"
        );
        assert_eq!(
            after.len(),
            store.records(),
            "and the snapshot of that epoch counts the same"
        );
        assert_eq!(
            after.answers().len(),
            480,
            "while a read answers only what is at or after the floor"
        );
        assert_eq!(
            store.records() - after.answers().len() as u64,
            60,
            "and the sixty in between are exactly the straddling segment's records below the cutoff: \
             both numbers are honest, and they answer different questions"
        );

        let straddler = store
            .blocks()
            .iter()
            .find(|block| block.info.index == 2)
            .expect("the straddler is still named by the index");
        assert_eq!(
            straddler.info.records, PER_SEGMENT as u64,
            "with every record it holds"
        );
        assert_eq!(
            straddler.info.first_millis,
            Some(EPOCH + 2 * HOUR),
            "and the range it stated: a straddler is kept, reported, and never rewritten"
        );
        assert_eq!(reader.answers().len(), SEGMENTS * PER_SEGMENT);
    }
}
