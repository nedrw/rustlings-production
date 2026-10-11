//! probe = "query/index-pruning@apply"
//! title = "The index sits beside the store and only ever prunes"
//! hint = "Answer `may`, never `does`: state both columns' bounds, filter the series names and their pairs, keep a segment whenever a bound nobody stated or a value that is a NaN makes the answer uncertain, and let the pruning move a number rather than an answer."
//!
//! Step `lakelet/4.14-index-pruning` is where a correct-but-slow store gets a number that can go red:
//! every segment gets a small **sidecar** file beside it — `<store>/index/<ordinal>.lki` — holding
//! the bounds of its ordered columns and a filter over its series vocabulary. Everything in this file
//! is a rule somebody paid for once:
//!
//! * **the index is derived state, and it lives beside the store.** Everything in it can be built
//!   again from the segment it describes, so a missing index, a damaged index and an index for a
//!   segment that no longer exists are all the same event — *this query opens a file it could have
//!   skipped* — and never a different answer. The consequence is that pruning is allowed to be wrong
//!   in exactly one direction: it may open a file it did not need to, and it may never skip one that
//!   holds what was asked for;
//! * **a zone answers *may* and never *does*.** A false positive — "a record here might meet your
//!   range" about a segment that holds none — costs one file read. A false negative costs a
//!   **record**: a query that did not open a segment holding something it asked about answers with a
//!   hole in it, and no count in the store would say so. The consequence is that a bound nobody
//!   stated, a zone built from no records, and a value column holding a **NaN** all answer `true`;
//! * **the range rule is half-open, and both of its comparisons are load-bearing.** A segment is met
//!   when its earliest record is before `to` and its latest is at or after `from`. The consequence of
//!   turning either one around is a record nobody answers with, which is why the tests ask about both
//!   ends;
//! * **the metric is deliberately not a term.** A filter holds whole series names and the `key=value`
//!   pairs they carry, and a query that named no pair is not asked about the vocabulary at all. The
//!   consequence: pruning a *conjunction* is one rule a pair at a time, and `FROM mem` about a store
//!   that never held `mem` still reaches the records and is refused there rather than answered empty.
//!
//! ## What you write
//!
//! * [`Zone::of`] — the bounds of both columns, with a NaN making the value pair unstated;
//! * [`Zone::may_meet`] — the half-open range question;
//! * [`Bloom::of`] — a filter holding every term it is built from;
//! * [`Bloom::may_contain`] — `may`, and only `may`;
//! * [`SegmentIndex::of`] — the zone over the records and the filter over their series vocabulary;
//! * [`SegmentIndex::may_answer`] — the whole pruning decision for one segment and one question.
//!
//! [`Record`], [`Zone`]'s accessors, [`Bloom::insert`], [`Bloom::everything`], [`Bloom::union`],
//! [`SegmentIndex`]'s accessors, [`terms_of`], [`hash`], [`blocks_pruned`] and the layout constants
//! are given: the vocabulary is the interface, and the hash is given on purpose — see below.
//!
//! ## What the tests ask
//!
//! A zone states the bounds of both columns, a single record is a span of no width, and no records at
//! all states nothing. `may_meet` is asked at both ends of the half-open rule: a range that ends where
//! the first record stands does not meet the segment and one that starts at its last record does, and
//! an unstated zone meets every range including `u64::MAX`. A value between the bounds that no record
//! holds says `may` — the false positive a zone is allowed — while one outside them does not. Every
//! term that went into a filter answers `may`, and 10 000 terms that went into none answer `may` at
//! the rate the fixed size promises: at most one in a thousand here, against the one-in-seventy-
//! thousand the real module's table bounds. The vocabulary is whole names and their pairs, a question
//! that named no pair is not asked about it at all, and a question that covers everything prunes
//! nothing. Then the two claims the slot is about: a map that disagrees with the data moves
//! `blocks_pruned` and never an answer, and the index never rules out a segment that holds what the
//! question asked for.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no segment file and no sidecar**: the records of a segment and the
//! index beside it are values here, so there is nothing to open, nothing to be missing, and nothing to
//! be scribbled over. What a probe can check is the decision the sidecar has to make — may, never does
//! — and that is the test a sidecar has to pass to be worth having at all; the store that reads it
//! counts `index_missing` and `index_unusable` where this file has [`SegmentIndex::unstated`].
//!
//! The real index hashes with the crate the project already has: `crc32fast`'s CRC-32 over the term
//! with the hash's index appended, avalanched with `fmix32`. A probe can download nothing, so
//! [`hash`] is FNV-1a 32 written out here — and the *avalanche* is not faked, because it is the part
//! the module's own table is about: a checksum used as a hash answers `yes` about one term in thirty
//! at this size, and the mixing step is what takes that to about one in seventy thousand. Also missing
//! on purpose: the sidecar's thirty-three-byte zone and its sealed encoding, the `Error::Protocol` a
//! flipped byte earns, the merge that writes the union of its inputs' indexes without reading a series
//! name, the plan's `bytes_read`/`bytes_avoided`, and `OR` — a disjunction is answered by the same
//! filter, and it costs a grammar and a printer that are a step of their own.

/// One record: the series it belongs to, when it happened, and what it measured.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// The series name, as a client spelled it: a metric and the pairs it carries.
    pub series: String,
    /// When, in milliseconds since the epoch: the unit the index and the store's own index share.
    pub ts_millis: u64,
    /// What it measured: the second ordered column a zone states bounds for.
    pub value: f64,
}

/// How many bits every filter this build writes holds.
pub const BLOOM_BITS: usize = 1024;

/// How many bit positions of one term a filter sets.
pub const BLOOM_HASHES: u8 = 4;

/// The bytes those bits take.
pub const BLOOM_BYTES: usize = BLOOM_BITS / 8;

/// The hash of one term with the hash's index appended, avalanched.
///
/// Given because it is the one thing a probe cannot have: the real index uses the crate the project
/// already depends on, and a hand-rolled CRC in a course file would teach nothing about indexes while
/// being wrong in ways nobody notices. The avalanche is `fmix32`, and it is the part that matters —
/// see the prose above.
#[must_use]
pub fn hash(term: &str, index: u8) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in term.as_bytes().iter().chain(std::iter::once(&index)) {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x85eb_ca6b);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(0xc2b2_ae35);
    hash ^ (hash >> 16)
}

/// The bit one term's `index`-th hash lands on.
#[must_use]
pub fn bit_of(term: &str, index: u8) -> usize {
    hash(term, index) as usize % BLOOM_BITS
}

/// The terms one series name is made of: the whole name, and every `key=value` pair it carries.
///
/// The metric — the part before the first comma — is deliberately **not** a term. A store could prune
/// with it, and doing so would change an answer: `FROM mem` about a store that never held `mem` is a
/// refusal decided from the records a query read, and a metric probe would turn that refusal into an
/// empty answer whenever every segment in the range got pruned.
#[must_use]
pub fn terms_of(series: &str) -> Vec<String> {
    let mut terms = vec![series.to_owned()];
    for part in series.split(',').skip(1) {
        if part.contains('=') {
            terms.push(part.to_owned());
        }
    }
    terms
}

/// The bounds of one segment's two ordered columns, or the fact that nobody stated them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    first_millis: Option<u64>,
    last_millis: Option<u64>,
    min_value: Option<f64>,
    max_value: Option<f64>,
}

impl Zone {
    /// The bounds of the records this zone is built from.
    ///
    /// A set holding no records states nothing at all, and a set holding a **NaN** states no *value*
    /// bounds: a NaN is neither above nor below anything, so no interval contains it and bounds that
    /// left it out would be bounds claiming a record is not there. The timestamps are a different
    /// column and are stated either way.
    #[must_use]
    pub fn of(records: &[Record]) -> Self {
        todo!()
    }

    /// The zone that states nothing: what a segment whose sidecar is missing or damaged is read as.
    #[must_use]
    pub const fn unstated() -> Self {
        Self {
            first_millis: None,
            last_millis: None,
            min_value: None,
            max_value: None,
        }
    }

    /// The earliest timestamp it states.
    #[must_use]
    pub const fn first_millis(&self) -> Option<u64> {
        self.first_millis
    }

    /// The latest timestamp it states.
    #[must_use]
    pub const fn last_millis(&self) -> Option<u64> {
        self.last_millis
    }

    /// The smallest value it states.
    #[must_use]
    pub const fn min_value(&self) -> Option<f64> {
        self.min_value
    }

    /// The largest value it states.
    #[must_use]
    pub const fn max_value(&self) -> Option<f64> {
        self.max_value
    }

    /// `true` when this zone may hold a record in the half-open range `[from, to)`.
    ///
    /// The rule is the one `lakelet/4.11-query-engine` already prunes a segment's stated span with: a
    /// segment is met when its earliest record is before `to` and its latest is at or after `from`.
    /// A bound nobody stated answers `true`, because `false` here is a segment a query would not open,
    /// and a segment not opened is a record nobody answers with.
    #[must_use]
    pub fn may_meet(&self, from: u64, to: u64) -> bool {
        todo!()
    }

    /// `true` when this zone may hold a record whose value is `value`.
    ///
    /// Given, because it is bounds compared with a number and the interesting half is the direction:
    /// between the bounds is a `may` — a value no record holds is still inside an interval — and a
    /// NaN is in no interval at all. Nothing in this step's grammar asks a *value* column anything;
    /// the pair is here because a zone is what a later predicate would prune with.
    #[must_use]
    pub fn may_contain_value(&self, value: f64) -> bool {
        match (self.min_value, self.max_value) {
            (Some(min), Some(max)) => value >= min && value <= max,
            _ => true,
        }
    }
}

/// A filter over the terms one segment's series vocabulary is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bloom {
    bits: Vec<u8>,
}

impl Bloom {
    /// A filter holding every term of `terms`.
    #[must_use]
    pub fn of<'a>(terms: impl IntoIterator<Item = &'a str>) -> Self {
        todo!()
    }

    /// The filter that may hold everything: what a vocabulary nobody could state is written as.
    ///
    /// Every bit set, so it prunes nothing, which is the only safe answer for an index that cannot
    /// tell.
    #[must_use]
    pub fn everything() -> Self {
        Self {
            bits: vec![0xff; BLOOM_BYTES],
        }
    }

    /// Set every bit of `term`, which is the whole of what a filter does with one.
    ///
    /// Given: the bit arithmetic is not the lesson, and every one of these positions is
    /// [`bit_of`]'s.
    pub fn insert(&mut self, term: &str) {
        for index in 0..BLOOM_HASHES {
            let at = bit_of(term, index);
            if let Some(byte) = self.bits.get_mut(at / 8) {
                *byte |= 1 << (at % 8);
            }
        }
    }

    /// `true` when `term` **may** be one this filter was built from.
    ///
    /// `true` is only an admission that the filter cannot tell: a false positive costs one file read,
    /// and a false *negative* costs a record, because a query that did not open a segment holding
    /// something it asked about answers with a hole in it and no count in the store would say so.
    #[must_use]
    pub fn may_contain(&self, term: &str) -> bool {
        todo!()
    }

    /// How many bits it holds.
    #[must_use]
    pub const fn bits(&self) -> usize {
        BLOOM_BITS
    }

    /// The filter that holds what either of the two holds.
    ///
    /// Given, and the reason the size is fixed: two filters can only be ORed when their bits mean the
    /// same places, and a merge writes its new segment's filter this way without reading one series
    /// name.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        Self {
            bits: self
                .bits
                .iter()
                .zip(&other.bits)
                .map(|(mine, theirs)| mine | theirs)
                .collect(),
        }
    }
}

/// What the sidecar beside one segment states: the bounds of its columns, and the terms its series
/// vocabulary is made of.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentIndex {
    zone: Zone,
    series: Bloom,
}

impl SegmentIndex {
    /// What the records of one segment state.
    ///
    /// The vocabulary is every whole series name and every `key=value` pair one carries
    /// ([`terms_of`]): a query names both shapes — `FROM cpu` names a metric, `WHERE host = 'h0'` names
    /// a pair — and what a segment has is whole names.
    #[must_use]
    pub fn of(records: &[Record]) -> Self {
        todo!()
    }

    /// The index that states nothing: what a segment whose sidecar is missing or damaged is read as.
    #[must_use]
    pub fn unstated() -> Self {
        Self {
            zone: Zone::unstated(),
            series: Bloom::everything(),
        }
    }

    /// The bounds of its columns.
    #[must_use]
    pub const fn zone(&self) -> &Zone {
        &self.zone
    }

    /// Its series vocabulary.
    #[must_use]
    pub const fn series(&self) -> &Bloom {
        &self.series
    }

    /// `true` when this segment **may** answer a question: it may hold a record in `[from, to)` and
    /// every `key=value` pair the question named may be in its vocabulary.
    ///
    /// A question that named no pair is not asked about the vocabulary at all: with nothing to ask it,
    /// the filter is not consulted, and a question about a metric the segment never held is answered
    /// from the records rather than pruned away. Pruning a *conjunction* is one rule — a segment is
    /// skipped when *any* pair the question named cannot be in it, because a record matching all of
    /// them carries every one.
    #[must_use]
    pub fn may_answer(&self, from: u64, to: u64, pairs: &[&str]) -> bool {
        todo!()
    }
}

/// How many of `indexes` a question may skip: the segments whose sidecar says no.
///
/// This is the `blocks_pruned` the step is about, in the units a budget watches: a count of
/// *segments*, comparable with the ones a query opened. Given, because the arithmetic is not the
/// lesson — the decision it counts is [`SegmentIndex::may_answer`].
#[must_use]
pub fn blocks_pruned(indexes: &[SegmentIndex], from: u64, to: u64, pairs: &[&str]) -> u64 {
    indexes
        .iter()
        .filter(|index| !index.may_answer(from, to, pairs))
        .count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minute, in the unit the index counts in.
    const MINUTE: u64 = 60_000;

    /// An hour, in the same unit.
    const HOUR: u64 = 3_600_000;

    /// The instant the workload's first segment starts at, so a range reads as an offset from it.
    const EPOCH: u64 = 472_222 * HOUR;

    /// One segment of a store for the tests to judge: the records it holds and the sidecar beside
    /// it — which is a stale or wrong one for the cases that are about that.
    struct Block {
        records: Vec<Record>,
        index: SegmentIndex,
    }

    /// One record of one series, `offset` minutes past [`EPOCH`].
    fn record(series: &str, offset: u64, value: f64) -> Record {
        Record {
            series: series.to_owned(),
            ts_millis: EPOCH + offset * MINUTE,
            value,
        }
    }

    /// A block whose sidecar states what its records state.
    fn block(records: Vec<Record>) -> Block {
        Block {
            index: SegmentIndex::of(&records),
            records,
        }
    }

    /// Twelve segments of one series each, an hour apart: the store a question is asked of.
    fn workload() -> Vec<Block> {
        (0..12)
            .map(|segment| {
                let series = format!("cpu,host=h{segment}");
                let records = (0..6)
                    .map(|offset| record(&series, segment * 60 + offset, offset as f64))
                    .collect();
                block(records)
            })
            .collect()
    }

    /// The sidecars of a set of blocks, which is what a plan is handed.
    fn indexes(blocks: &[Block]) -> Vec<SegmentIndex> {
        blocks.iter().map(|block| block.index.clone()).collect()
    }

    /// The records of one block that a question asks for.
    fn held(block: &Block, from: u64, to: u64, pairs: &[&str]) -> Vec<Record> {
        block
            .records
            .iter()
            .filter(|record| {
                record.ts_millis >= from
                    && record.ts_millis < to
                    && pairs
                        .iter()
                        .all(|pair| terms_of(&record.series).iter().any(|term| term == pair))
            })
            .cloned()
            .collect()
    }

    /// `true` when a block's own records hold something a question asks for.
    fn holds(block: &Block, from: u64, to: u64, pairs: &[&str]) -> bool {
        !held(block, from, to, pairs).is_empty()
    }

    /// What a question reads out of `blocks` through the index: a block it cannot rule out is read
    /// whole, and one it rules out contributes nothing.
    fn answers_read(
        blocks: &[Block],
        indexes: &[SegmentIndex],
        from: u64,
        to: u64,
        pairs: &[&str],
    ) -> Vec<Record> {
        blocks
            .iter()
            .zip(indexes)
            .filter(|(_, index)| index.may_answer(from, to, pairs))
            .flat_map(|(block, _)| held(block, from, to, pairs))
            .collect()
    }

    /// What the same question reads with every block opened: no index consulted at all.
    fn answers_unpruned(blocks: &[Block], from: u64, to: u64, pairs: &[&str]) -> Vec<Record> {
        blocks
            .iter()
            .flat_map(|block| held(block, from, to, pairs))
            .collect()
    }

    #[test]
    fn a_zone_states_the_bounds_of_both_columns() {
        let records = vec![
            record("cpu,host=h0", 0, 5.0),
            record("cpu,host=h0", 1, -2.5),
            record("cpu,host=h0", 2, 17.0),
        ];
        let zone = Zone::of(&records);
        assert_eq!(
            zone.first_millis(),
            Some(EPOCH),
            "the earliest timestamp of the records it was built from"
        );
        assert_eq!(zone.last_millis(), Some(EPOCH + 2 * MINUTE));
        assert_eq!(
            zone.min_value(),
            Some(-2.5),
            "and the smallest and largest value, which is the column the store's own index states \
             nothing about"
        );
        assert_eq!(zone.max_value(), Some(17.0));

        // One record is a span of no width, which is what makes the range question exact at a point.
        let single = Zone::of(&[record("cpu,host=h0", 7, 2.0)]);
        assert_eq!(single.first_millis(), single.last_millis());
        assert_eq!(single.min_value(), single.max_value());

        // No records at all: nothing is stated, which is a fact about the index and not a claim about
        // the store.
        let empty = Zone::of(&[]);
        assert_eq!(empty.first_millis(), None);
        assert_eq!(empty.last_millis(), None);
        assert_eq!(empty.min_value(), None);
        assert_eq!(empty.max_value(), None);
        assert_eq!(empty, Zone::unstated());

        // A NaN is above and below nothing, so the value pair stops being stated rather than bounding
        // a set that quietly leaves it out.
        let with_nan = Zone::of(&[
            record("cpu,host=h0", 0, 1.0),
            record("cpu,host=h0", 1, f64::NAN),
        ]);
        assert_eq!(
            with_nan.min_value(),
            None,
            "a set holding a NaN states no bounds: bounds that excluded it would be bounds claiming \
             a record is not there"
        );
        assert_eq!(with_nan.max_value(), None);
        assert_eq!(
            with_nan.first_millis(),
            Some(EPOCH),
            "the timestamps are a different column and are still stated"
        );
    }

    #[test]
    fn a_zone_answers_may_and_never_does() {
        let zone = Zone::of(&[
            record("cpu,host=h0", 1, 10.0),
            record("cpu,host=h0", 3, 30.0),
        ]);
        let from = EPOCH + MINUTE;
        let to = EPOCH + 3 * MINUTE;

        assert!(
            zone.may_meet(from, from + 1),
            "a range starting on the first record"
        );
        assert!(
            zone.may_meet(from - 1, from + 1),
            "one starting just before it"
        );
        assert!(
            zone.may_meet(to, to + 1),
            "and one starting on the *last* record meets it: that record is inside the range"
        );
        assert!(
            !zone.may_meet(to + 1, to + 100),
            "a range that begins after the last record does not"
        );
        assert!(
            !zone.may_meet(from - 100, from),
            "and neither does one that ends where the first record stands: the range is half-open, \
             so a record whose timestamp is exactly `to` belongs to the next question"
        );
        assert!(
            zone.may_meet(from - 100, from + 1),
            "and it meets one that reaches one past it"
        );
        assert!(
            zone.may_meet(from - HOUR, to + HOUR),
            "one that swallows it"
        );
        assert!(
            Zone::of(&[]).may_meet(u64::MAX - 1, u64::MAX),
            "and a zone that states nothing meets every range, which is the only answer that cannot \
             lose a record"
        );

        // The other column, and the false positive a zone is allowed to have.
        assert!(
            zone.may_contain_value(15.5),
            "a value no record holds, and it may: a zone is bounds"
        );
        assert!(!zone.may_contain_value(9.99), "one below the smallest");
        assert!(!zone.may_contain_value(30.01), "and one above the largest");
        assert!(
            Zone::unstated().may_contain_value(f64::NAN),
            "while an unstated pair may contain anything, including the value that is in no interval"
        );
    }

    #[test]
    fn a_bloom_may_hold_every_term_that_went_in() {
        let terms = [
            "cpu,host=h0",
            "host=h0",
            "cpu,host=h1",
            "host=h1",
            "dc=east",
        ];
        let bloom = Bloom::of(terms);
        assert_eq!(
            bloom.bits(),
            BLOOM_BITS,
            "the filter states its own geometry, which is what makes two of them mergeable"
        );
        for term in terms {
            assert!(
                bloom.may_contain(term),
                "`{term}` went in and answers `may` — a filter has no false negatives, and that is the \
                 whole reason it is the right shape for an index that may never lose a record"
            );
        }

        let mut grown = bloom.clone();
        grown.insert("cpu,host=h9");
        assert!(
            grown.may_contain("cpu,host=h9"),
            "a term inserted on its own is a term like any other"
        );
        assert!(
            Bloom::everything().may_contain("anything,at=all"),
            "and the filter that states nothing answers `may` about everything: every bit set prunes \
             nothing"
        );

        let union = bloom.union(&grown);
        assert_eq!(
            union.bits(),
            BLOOM_BITS,
            "a union is a filter of the same shape"
        );
        for term in ["cpu,host=h9", "host=h0"] {
            assert!(
                union.may_contain(term),
                "`{term}` went into one of the two, so the union may hold it — which is how a merge \
                 writes its new segment's filter without reading a series name"
            );
        }
    }

    #[test]
    fn a_bloom_answers_yes_at_the_rate_its_size_promises() {
        // Eight series of one segment — sixteen terms, their names and their pairs — in a thousand and
        // twenty-four bits. The real module's table bounds this at about one in seventy thousand; the
        // case asserts a bound two orders of magnitude looser than that, which is still enough to
        // catch a filter that answers `yes` without looking at its bits at all.
        let members: Vec<String> = (0..8)
            .map(|index| format!("cpu,host=member{index}"))
            .collect();
        let records: Vec<Record> = members
            .iter()
            .enumerate()
            .map(|(at, name)| record(name, at as u64 * 1_000, 1.0))
            .collect();
        let bloom = SegmentIndex::of(&records).series().clone();
        assert_eq!(
            bloom.bits(),
            BLOOM_BITS,
            "a segment's vocabulary goes into one filter of the build's fixed size"
        );

        let mut said_yes = 0u64;
        for probe in 0..10_000 {
            if bloom.may_contain(&format!("cpu,host=stranger{probe}")) {
                said_yes += 1;
            }
        }
        assert!(
            said_yes <= 10,
            "10 000 terms that went into no filter and 16 that did: {said_yes} of the strangers \
             answered `may`, and the size this build writes bounds this at about one in seventy \
             thousand"
        );
        for member in &members {
            assert!(
                bloom.may_contain(member),
                "and every term that did go in still answers `may`: {member}"
            );
        }
    }

    #[test]
    fn a_segment_index_states_the_names_and_the_pairs_but_not_the_metric() {
        let records = vec![
            record("cpu,host=h0", 0, 1.0),
            record("cpu,host=h0", 60, 2.0),
        ];
        let index = SegmentIndex::of(&records);
        let from = EPOCH;
        let to = EPOCH + 24 * HOUR;
        assert!(
            index.may_answer(from, to, &["host=h0"]),
            "the pair the segment holds answers `may`"
        );
        assert!(
            !index.may_answer(from, to, &["host=h9"]),
            "and a pair it does not hold is what the index prunes with"
        );
        assert!(
            index.may_answer(from, to, &[]),
            "a question that named no pair is not asked about the vocabulary at all: with nothing to \
             ask, the filter is not consulted"
        );
        assert_eq!(
            terms_of("cpu,host=h0,dc=east"),
            ["cpu,host=h0,dc=east", "host=h0", "dc=east"],
            "the terms are the whole name and its pairs, and the metric is deliberately not one of \
             them"
        );

        // A question about a metric the segment never held is still answered from its records: the
        // filter is not asked about a metric, so a store that never held `mem` is refused there rather
        // than answered empty here.
        let other = SegmentIndex::of(&[record("mem,host=h0", 0, 1.0)]);
        assert!(
            other.may_answer(from, to, &[]),
            "a question that names no pair keeps a segment holding another metric"
        );
        assert!(
            !other.may_answer(from, to, &["host=h9"]),
            "while one that names a pair the segment does not carry prunes it"
        );
        assert!(
            !index.may_answer(to, to + HOUR, &["host=h0"]),
            "and a range after the last record does not meet it, whatever pair was named: both ends \
             of the rule are load-bearing in opposite directions"
        );
    }

    #[test]
    fn pruning_a_question_that_covers_everything_prunes_nothing() {
        let blocks = workload();
        let indexes = indexes(&blocks);
        assert_eq!(
            blocks_pruned(&indexes, 0, u64::MAX, &[]),
            0,
            "a range that covers everything and names no pair prunes nothing: zero is what a store \
             with no usable index answers, and it is also the right answer here"
        );
        assert_eq!(
            blocks_pruned(&indexes, 0, u64::MAX, &["host=h0"]),
            11,
            "while the same range with a pair named prunes every segment but the one that carries it"
        );

        // Both ends of the half-open rule, on the block whose records stand at minute 60..65.
        let second = &blocks[1];
        let first = second.records[0].ts_millis;
        let last = second.records[second.records.len() - 1].ts_millis;
        assert!(
            !second.index.may_answer(first, first, &[]),
            "a range that ends where the first record stands does not meet the segment: that record \
             belongs to the next question"
        );
        assert!(
            second.index.may_answer(first - 100, first + 1, &[]),
            "and it meets one that reaches one millisecond past that record"
        );
        assert!(
            second.index.may_answer(last, last + 1, &[]),
            "a range that starts where the *last* record stands does meet it"
        );
        assert!(
            !second.index.may_answer(last + 1, last + 2, &[]),
            "and one that starts a millisecond later does not"
        );
    }

    #[test]
    fn a_map_that_disagrees_with_the_data_changes_the_number_and_not_the_answer() {
        let blocks = workload();
        let honest = indexes(&blocks);
        let from = EPOCH + 30 * MINUTE;
        let to = EPOCH + 8 * HOUR;
        let pairs = ["host=h5"];

        assert_eq!(
            answers_read(&blocks, &honest, from, to, &pairs),
            answers_unpruned(&blocks, from, to, &pairs),
            "the answer is the records the question asks for, whatever the index did: the index is \
             derived state, and a map that disagrees with the data costs speed and never correctness"
        );
        assert!(
            blocks_pruned(&honest, from, to, &pairs) > 0,
            "the honest map prunes: eleven of the twelve segments hold another `host`"
        );
        assert_eq!(
            answers_read(&blocks, &honest, from, to, &pairs).len(),
            6,
            "and what it leaves is one segment of six records"
        );

        // A store whose index directory was deleted: nothing to prune with, and the same answer.
        let missing: Vec<SegmentIndex> = blocks.iter().map(|_| SegmentIndex::unstated()).collect();
        assert_eq!(
            blocks_pruned(&missing, from, to, &pairs),
            0,
            "a missing index prunes nothing — which is the whole of what it costs"
        );
        assert_eq!(
            answers_read(&blocks, &missing, from, to, &pairs),
            answers_unpruned(&blocks, from, to, &pairs),
            "and it changes no answer: an index that is not there is counted, not raised"
        );

        // An index built from the whole store rather than from one segment is *coarser* than the
        // honest one: it may say `may` where the honest map said no, and it may never say no where
        // the honest map said `may`. That is the one direction a map is allowed to err in, and the
        // pruning number is what pays for it.
        let every: Vec<Record> = blocks
            .iter()
            .flat_map(|block| block.records.clone())
            .collect();
        let one = SegmentIndex::of(&every);
        let coarse: Vec<SegmentIndex> = blocks.iter().map(|_| one.clone()).collect();
        assert!(
            blocks_pruned(&coarse, from, to, &pairs) <= blocks_pruned(&honest, from, to, &pairs),
            "a coarser map prunes less and never more: the other direction is a map that claims to \
             know something the data does not say"
        );
        assert_eq!(
            answers_read(&blocks, &coarse, from, to, &pairs),
            answers_unpruned(&blocks, from, to, &pairs),
            "and it changes no answer either"
        );
    }

    #[test]
    fn the_index_never_rules_out_a_segment_that_holds_the_answer() {
        let blocks = workload();
        let indexes = indexes(&blocks);
        let questions: [(u64, u64, &[&str]); 7] = [
            (EPOCH, EPOCH + HOUR, &[]),
            (EPOCH + 30 * MINUTE, EPOCH + 8 * HOUR, &["host=h5"]),
            (EPOCH + 5 * HOUR, EPOCH + 6 * HOUR, &["host=h5", "dc=east"]),
            (0, u64::MAX, &["host=h0"]),
            (EPOCH + 24 * HOUR, EPOCH + 48 * HOUR, &[]),
            (EPOCH, EPOCH + 12 * HOUR, &["host=none"]),
            // Ten seconds between two of the second segment's records: a zone is bounds, so it
            // answers `may` about a range that holds nothing at all.
            (
                EPOCH + 62 * MINUTE + 30_000,
                EPOCH + 62 * MINUTE + 40_000,
                &[],
            ),
        ];
        let mut pruned_something = false;
        let mut allowed_a_false_positive = false;

        for (from, to, pairs) in questions {
            for (at, block) in blocks.iter().enumerate() {
                let index = &indexes[at];
                assert_eq!(
                    answers_read(&blocks, &indexes, from, to, pairs),
                    answers_unpruned(&blocks, from, to, pairs),
                    "the index prunes {from}..{to} with {pairs:?} into a different answer: a segment \
                     it ruled out held something the question asked for, and a record nobody answered \
                     with is a hole no count in this store would report"
                );
                if index.may_answer(from, to, pairs) {
                    if !holds(block, from, to, pairs) {
                        allowed_a_false_positive = true;
                    }
                } else {
                    assert!(
                        !holds(block, from, to, pairs),
                        "the index ruled out segment {at}, which holds a record the question asks \
                         for: a false positive costs one file read, and a false negative costs the \
                         record"
                    );
                    pruned_something = true;
                }
            }
        }

        assert!(
            pruned_something,
            "the index pruned something over these questions, or this case is evidence about nothing"
        );
        assert!(
            allowed_a_false_positive,
            "and it answered `may` about a segment that holds nothing — the false positive a zone is \
             allowed, because `may` is only an admission that it cannot tell"
        );
    }
}
