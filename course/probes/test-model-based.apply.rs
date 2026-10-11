//! probe = "test/model-based@apply"
//! title = "A walk, a stupid model, and a failure small enough to read"
//! hint = "Draw every operation out of the seed, tell the model only what a flush closed, compare after each one, and shrink the sequence that disagrees until no single operation can be taken out of it."
//!
//! Step `lakelet/4.13-model-based-tests` is where a suite of hand-written cases stops being enough.
//! The bug that costs a week is the one nobody thought of — a flush that closes a segment in the
//! middle of a merge, a retention cutoff that lands inside a segment, a clock that steps backwards
//! between two appends — and those cannot be enumerated, because the ones you would enumerate are the
//! ones you already understand. The technique that finds them is not a library: write a second,
//! *stupid* implementation of what a **reader** of the store sees, drive both with a random sequence
//! of operations, and compare after every operation.
//!
//! Four decisions, and each one is the difference between a property test and a flaky test:
//!
//! * **the model knows nothing about the implementation.** It is the list of records the store was
//!   told and one number, the floor. A model with a segment in it would have the same bug as the
//!   store, and a walk comparing two copies of one mistake proves nothing about either;
//! * **the model is told at the flush, not at the append.** A reader here is handed segments, so a
//!   record the log holds and no segment holds is not an answer: accepting a record changes nothing
//!   anybody can read, and a model told at the append fails on the first one while the store is
//!   right;
//! * **a seed is the whole transcript.** Which operation, which series, how many records, which
//!   window and which cutoff all come out of the seed and nothing else — no clock, no path, no
//!   address, no hash seed — so the same seed replays the same transcript and a failure is a seed
//!   plus a bug rather than a rumour;
//! * **a failure is shrunk before it is reported.** Sixty-four operations is a rumour; the same
//!   failure with the operations nobody needs deleted from it is a case somebody can act on.
//!
//! ## What you write
//!
//! * [`agree`] — the oracle: what a reader can see, and the floor;
//! * [`step`] — one operation played on both halves, and the comparison after it;
//! * [`shrink`] — the smallest subsequence of a failing walk that still disagrees.
//!
//! [`Rng`], [`Record`], [`Op`], [`Effect`], [`Store`], [`Model`] and [`Walk`] are given, and so are
//! the drawing ([`plan`]), the two walks the tests use ([`walk`], [`failing`]) and the digest. The
//! generator is the track's own arithmetic and the store is the implementation under test: this probe
//! re-checks the *walk*, not the stream and not a store's file layout, so both halves arrive written,
//! with one deliberate defect in one of them.
//!
//! ## What the tests ask
//!
//! The oracle holds for two halves that were told nothing, and it fails in both directions: the store
//! answers a record the model was never told, or the model's floor is not the store's, or the same
//! two records arrive in the other order. A walk of appends and nothing else agrees and the model is
//! still holding nothing — the sharpest edge in the file, and the one a walk that compared only at
//! the end would never see. The build with the one defect is caught and the correct build over the
//! same seed is not, so the walk is not vacuous. The same seed replays the same transcript (the
//! digest is pinned) and seed 7 is a different one — three identical walks would mean the seed
//! reached nothing. A merge changes what the store is and not what a reader sees. A cutoff at or
//! below the floor moves nothing. And the shrinker: a failing walk shrinks to a sequence that still
//! disagrees, that is a subsequence of what was played in the same order, and that has no operation
//! which can be taken out of it; a walk that agreed has nothing to shrink.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no store on a disk**: the real walk seals real segments, writes a
//! real manifest, replays a real log and reads the files back, and the claim of
//! `lakelet/4.13-model-based-tests` — a flush, a merge, a reopen and a retention pass are invisible
//! to a reader — is a claim about *those files*. Here a store is a `Vec` of segment lists, so a
//! reopen is the effect [`Effect::Nothing`] and an inode, an address and a page cache never get a
//! chance to make the two answers differ. What is re-checked is the walk's own shape: the alphabet,
//! the oracle, the seed and the shrinking.
//!
//! Also missing on purpose: no query and no buckets. The real walk asks every question twice — once
//! through `lakelet/4.11-query-engine` over the manifest, once through `lakelet/4.10-downsampling`
//! over the model's records — and the grid, the partial buckets and the five aggregates are that
//! step's subject, so a probe that re-derived them would be a second implementation of them rather
//! than a re-check of this one. A series is a `&'static str` where the track's `Record` owns a
//! `String`, because the alphabet's series are fixed and a record that owns a string would make every
//! comparison a question about allocation. And the defect is deliberately one line: a build that
//! answers what the floor forgot is the defect a soak and a retention pass both report, and a walk is
//! only ever as good as the build it is pointed at.

/// One minute, in milliseconds: the step between two records of one batch.
const MINUTE: i64 = 60_000;

/// One window of simulated time: how far a walk's clock steps, and the width every cutoff names.
const WINDOW: i64 = 3_600_000;

/// The instant this file's records start at, so a case can name a window and a cutoff.
const EPOCH: i64 = 1_700_000_000_000;

/// How many operations the walks in the tests play.
const OPS: usize = 64;

/// Where a zero seed is nudged to, once, at construction: all three shifts of zero are zero.
const AWAY_FROM_ZERO: u64 = 0x9E37_79B9_7F4A_7C15;

/// The star: an odd constant, and therefore invertible, so a nonzero state never answers zero.
const MULTIPLIER: u64 = 0x2545_F491_4F6C_DD1D;

/// The generator a walk is seeded with: xorshift64\*.
///
/// Given rather than written, because it is the track's own arithmetic
/// (`lakelet/4.13-model-based-tests`' `src/engine/rng.rs`) and this probe re-checks the walk that
/// stands on it. What matters here is the property the walk rests on: one seed in, one sequence out,
/// on every machine, for ever — which is why `rand` is the wrong crate for a walk even though it is a
/// good one, and why the track's own spec pins the first four numbers of seed 1.
#[derive(Debug, Clone)]
pub struct Rng {
    /// The whole state.
    state: u64,
}

impl Rng {
    /// A generator seeded with `seed`. **Zero is a legal seed**: the one state xorshift cannot leave
    /// is nudged off zero once, here, where the correction is visible.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { AWAY_FROM_ZERO } else { seed },
        }
    }

    /// The next 64 bits: the state moves on, and the answer is the state mixed with itself.
    pub fn next_u64(&mut self) -> u64 {
        let mut state = self.state;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.state = state;
        state.wrapping_mul(MULTIPLIER)
    }

    /// A number in `0..bound`, and `0` when there is nothing to draw from: an alphabet with no
    /// letters in it hands no letter out and the stream does not move.
    pub fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            return 0;
        }
        self.next_u64() % bound
    }

    /// A number in `low..=high`: **both ends are inside**, and a span that names nothing answers
    /// `low` without drawing.
    pub fn between(&mut self, low: i64, high: i64) -> i64 {
        let span = high.saturating_sub(low);
        if span <= 0 {
            return low;
        }
        low + self.below(span as u64 + 1) as i64
    }

    /// One of `alphabet`, drawn uniformly.
    ///
    /// # Panics
    ///
    /// An empty alphabet has no element to answer with, and inventing one would be a walk playing an
    /// operation nobody wrote.
    #[must_use]
    pub fn pick<'a, T>(&mut self, alphabet: &'a [T]) -> &'a T {
        assert!(
            !alphabet.is_empty(),
            "`pick` has no element to answer with: an empty alphabet is not a draw"
        );
        let index = self.below(alphabet.len() as u64) as usize;
        match alphabet.get(index) {
            Some(item) => item,
            None => unreachable!("`below(len)` is in `0..len`, so one element matches"),
        }
    }

    /// `true` about `percent` times in a hundred, and `always` for anything above a hundred: a
    /// probability written above a hundred is an emphatic yes rather than a setting to validate.
    pub fn chance(&mut self, percent: u64) -> bool {
        percent >= 100 || self.below(100) < percent
    }
}

/// One record a reader can see: a series, an instant, and a value.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// The series it belongs to. Compared exactly: `cpu,host=h1` is not `cpu`.
    pub series: &'static str,
    /// When it was written, in milliseconds.
    pub ts_millis: i64,
    /// The value. Compared by *bits*, so `-0.0` and `NaN` are the values they are.
    pub value: f64,
}

/// `true` when two records are the same record, the value compared by bits.
#[must_use]
pub fn same(left: &Record, right: &Record) -> bool {
    left.series == right.series
        && left.ts_millis == right.ts_millis
        && left.value.to_bits() == right.value.to_bits()
}

/// `true` when two lists are the same records, in the same order — one list, not a set: a reader is
/// handed the records in the order they were appended, whatever they are of.
#[must_use]
pub fn same_all(left: &[Record], right: &[Record]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| same(left, right))
}

/// One operation of the alphabet: the five things a store's life is made of.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// A batch the log accepts.
    Append {
        /// The series the batch is for.
        series: &'static str,
        /// The instant the first record of it is written at.
        from: i64,
        /// How many records, a minute apart.
        count: usize,
    },
    /// Close what the log holds into a segment.
    Flush,
    /// Rewrite the segments into one.
    Merge,
    /// Forget everything below `at`: the cutoff a retention pass writes into the manifest.
    Expire {
        /// The millisecond the floor moves to.
        at: i64,
    },
    /// Open the store from its files again.
    Reopen,
}

/// What one operation did, which is how a walk knows what the model is owed.
///
/// The effect is the interface between the two halves, and the whole edge of this file is in it: a
/// reader is handed segments, so [`Effect::Accepted`] is *not* something the model hears about.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// The log took these records and no segment holds them: nobody can read them yet, and the model
    /// is told **nothing** here.
    Accepted {
        /// What the log now holds.
        records: Vec<Record>,
    },
    /// A flush closed these records into a segment: this is what the model is owed.
    Sealed {
        /// What a reader can now see.
        records: Vec<Record>,
    },
    /// The floor moved to `at`.
    Floor {
        /// The millisecond the store has forgotten up to.
        at: i64,
    },
    /// A merge, a reopen, or a flush that closed no records: nothing a reader can see changed.
    Nothing,
}

/// The implementation under test: what a reader of a store sees, kept as plainly as it can be.
///
/// Given, because this probe is not about writing a store — it is about the walk that has to disagree
/// with one — and [`Store::wrong`] is the build with the one defect the walk exists to catch.
#[derive(Debug, Clone, Default)]
pub struct Store {
    /// What the log holds and no flush has closed.
    log: Vec<Record>,
    /// What the flushes closed, oldest first.
    segments: Vec<Vec<Record>>,
    /// The millisecond retention has forgotten up to.
    floor: i64,
    /// Whether this build answers what the disk holds instead of what the floor allows.
    ignores_floor: bool,
}

impl Store {
    /// The build that is right.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The build that is wrong in one way: it keeps answering the records the floor forgot.
    ///
    /// The floor is a note in the manifest, and a store whose reader can still see what a pass
    /// promised was gone is the defect `lakelet/4.9-retention` is about. A walk that cannot catch
    /// this build is a walk that proves nothing, which is the property the walk's own case pins.
    #[must_use]
    pub fn wrong() -> Self {
        Self {
            ignores_floor: true,
            ..Self::default()
        }
    }

    /// Play `op` and answer what it did.
    pub fn apply(&mut self, op: &Op) -> Effect {
        match op {
            Op::Append {
                series,
                from,
                count,
            } => {
                let records: Vec<Record> = (0..*count)
                    .map(|step| Record {
                        series: *series,
                        ts_millis: *from + step as i64 * MINUTE,
                        value: step as f64 / 4.0,
                    })
                    .collect();
                self.log.extend(records.iter().cloned());
                Effect::Accepted { records }
            }
            Op::Flush => {
                if self.log.is_empty() {
                    return Effect::Nothing;
                }
                let closed = std::mem::take(&mut self.log);
                self.segments.push(closed.clone());
                Effect::Sealed { records: closed }
            }
            Op::Merge => {
                if self.segments.len() < 2 {
                    return Effect::Nothing;
                }
                let merged: Vec<Record> = self.segments.concat();
                self.segments = vec![merged];
                Effect::Nothing
            }
            Op::Expire { at } => {
                if *at > self.floor {
                    self.floor = *at;
                    Effect::Floor { at: *at }
                } else {
                    Effect::Nothing
                }
            }
            // The store is opened from its own files: the same segments in the same order and a log
            // that is replayed rather than lost. A reopen is not an event in what a store was told.
            Op::Reopen => Effect::Nothing,
        }
    }

    /// What a reader can see: the segments in order, everything below the floor left out.
    ///
    /// The floor does not delete anything — a segment that straddles the cutoff keeps its old
    /// records on the disk and stops answering with them — which is why the model has a floor too
    /// rather than a shorter list.
    #[must_use]
    pub fn answerable(&self) -> Vec<Record> {
        self.segments
            .concat()
            .into_iter()
            .filter(|record| self.ignores_floor || record.ts_millis >= self.floor)
            .collect()
    }

    /// The millisecond retention has forgotten up to.
    #[must_use]
    pub fn floor(&self) -> i64 {
        self.floor
    }

    /// How many segments the store is made of right now.
    #[must_use]
    pub fn segments(&self) -> usize {
        self.segments.len()
    }
}

/// The reference model: what the store was *told*, and one number.
///
/// Given, and meant to be stupid. A list in the order the batches were closed and the floor; it knows
/// nothing about a log, a segment, a manifest or a merge, because a model that knew would be a second
/// store and a walk comparing the two would prove nothing about the first.
#[derive(Debug, Clone, Default)]
pub struct Model {
    /// Every record a flush closed, in the order it was told about it.
    accepted: Vec<Record>,
    /// The millisecond this model has forgotten up to.
    floor: i64,
}

impl Model {
    /// A model of a store that has been told nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record what a flush closed, in the order within the batch.
    pub fn append(&mut self, records: &[Record]) {
        self.accepted.extend_from_slice(records);
    }

    /// Move the floor to `at`, and never backwards: a promise that data is gone cannot be taken back,
    /// so a policy whose window grew and a clock that stepped backwards both move nothing.
    pub fn expire(&mut self, at: i64) {
        self.floor = self.floor.max(at);
    }

    /// The millisecond this model has forgotten up to.
    #[must_use]
    pub fn floor(&self) -> i64 {
        self.floor
    }

    /// What a read of the whole store must answer: everything at or after the floor, in order.
    #[must_use]
    pub fn answerable(&self) -> Vec<Record> {
        self.accepted
            .iter()
            .filter(|record| record.ts_millis >= self.floor)
            .cloned()
            .collect()
    }

    /// How many records this model heard — **including the ones the floor forgot**, because what a
    /// store holds and what it answers are two questions.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.accepted.len() as u64
    }

    /// `true` when this model was never told anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.accepted.is_empty()
    }
}

/// One walk: the store under test, the model beside it, and what has happened so far.
///
/// [`step`] is the only thing that moves it, and the transcript it keeps is the operations it played.
/// That is what makes a failure replayable from the seed alone, and what a shrinker gets handed.
#[derive(Debug, Clone, Default)]
pub struct Walk {
    /// The implementation under test.
    store: Store,
    /// The reference model beside it.
    model: Model,
    /// Every operation played, in order.
    played: Vec<Op>,
    /// The step at which the two halves first disagreed, counting from zero.
    mismatch: Option<usize>,
}

impl Walk {
    /// A walk over the correct build.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A walk over the build with the one defect.
    #[must_use]
    pub fn wrong() -> Self {
        Self {
            store: Store::wrong(),
            ..Self::default()
        }
    }

    /// An empty walk over the same build as this one: the same implementation, with none of what
    /// this walk has done to it. A shrinker replays its candidates in one of these, so the store it
    /// hands them to answers from nothing rather than from the walk's last operation.
    #[must_use]
    pub fn like(&self) -> Self {
        Self {
            store: if self.store.ignores_floor {
                Store::wrong()
            } else {
                Store::new()
            },
            ..Self::default()
        }
    }

    /// Play `ops` in order, comparing after every one of them.
    pub fn play(&mut self, ops: &[Op]) {
        for op in ops {
            step(self, op);
        }
    }

    /// `true` when every step agreed.
    #[must_use]
    pub fn agreed(&self) -> bool {
        self.mismatch.is_none()
    }

    /// The step at which the two halves first disagreed, counting from zero.
    #[must_use]
    pub fn mismatch(&self) -> Option<usize> {
        self.mismatch
    }

    /// Every operation played, in order: the transcript.
    #[must_use]
    pub fn played(&self) -> &[Op] {
        &self.played
    }

    /// The store under test.
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The model beside it.
    #[must_use]
    pub fn model(&self) -> &Model {
        &self.model
    }

    /// `true` when playing `ops` on this walk's build disagrees somewhere: the predicate a shrinker
    /// asks, and the reason it does not need the seed again.
    #[must_use]
    pub fn still_disagrees(&self, ops: &[Op]) -> bool {
        let mut candidate = self.like();
        candidate.play(ops);
        !candidate.agreed()
    }

    /// A digest of the transcript: the one number two runs of one seed are compared by.
    ///
    /// Counts, names and a digest, never a path, an address or a clock reading — which is not a
    /// logging convenience but the contract `slow:sim@3` compares two runs of a seed by.
    #[must_use]
    pub fn digest(&self) -> String {
        digest(&self.played.iter().map(describe).collect::<Vec<String>>())
    }
}

/// The digest of a transcript, as sixteen lower-case hex characters.
///
/// FNV-1a 64, given for the reason `release/artifacts@apply` gives the same one: a hand-rolled
/// SHA-256 in a course file teaches nothing while being wrong in ways nobody notices, and this
/// digest's job is to make two runs comparable rather than to resist an attacker.
#[must_use]
pub fn digest(lines: &[String]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in lines.join("\n").as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// One operation as the transcript spells it: counts and names, and a cutoff as the instant it is.
#[must_use]
pub fn describe(op: &Op) -> String {
    match op {
        Op::Append {
            series,
            from,
            count,
        } => format!("append {count} of {series} at {from}"),
        Op::Flush => "flush".to_owned(),
        Op::Merge => "merge".to_owned(),
        Op::Expire { at } => format!("expire {at}"),
        Op::Reopen => "reopen".to_owned(),
    }
}

/// The operations `seed` draws: one number in, a whole sequence out.
///
/// Given, and it is the half of a walk that must not depend on anything else. Which operation, which
/// series, how many records, which window and which cutoff all come out of the seed and the
/// generator, so the same seed replays the same transcript and a failure is a seed plus a bug. The
/// clock steps backwards about a quarter of the time and never leaves four windows, so a walk's whole
/// history is one span of simulated time.
#[must_use]
pub fn plan(seed: u64, count: usize) -> Vec<Op> {
    /// The series the alphabet writes into: two a segment carries alone, one every segment carries.
    const SERIES: [&str; 3] = ["cpu,host=h0", "cpu,host=h1", "cpu,host=fleet"];

    let mut rng = Rng::new(seed);
    let mut ops = Vec::with_capacity(count);
    let mut window = 0_i64;
    for _ in 0..count {
        window = match rng.chance(25) {
            true if window > 0 => window - 1,
            _ if window < 4 => window + 1,
            _ => window,
        };
        let at = EPOCH + window * WINDOW;
        ops.push(match rng.below(6) {
            0 | 1 => Op::Append {
                series: *rng.pick(&SERIES),
                from: at,
                count: rng.between(1, 3) as usize,
            },
            2 | 3 => Op::Flush,
            4 => Op::Expire {
                at: EPOCH + rng.below(5) as i64 * WINDOW,
            },
            _ => match rng.chance(50) {
                true => Op::Merge,
                false => Op::Reopen,
            },
        });
    }
    ops
}

/// A walk of the correct build from `seed`: `count` operations drawn and compared.
#[must_use]
pub fn walk(seed: u64, count: usize) -> Walk {
    let mut walk = Walk::new();
    walk.play(&plan(seed, count));
    walk
}

/// The same walk over the build with the one defect: what a shrinker has something to do with.
#[must_use]
pub fn failing(seed: u64, count: usize) -> Walk {
    let mut walk = Walk::wrong();
    walk.play(&plan(seed, count));
    walk
}

/// Whether what a reader of the store can see is what the model says was seen.
///
/// Two rules, and a walk is only worth its seed if both are in here: the records — the same list, in
/// the same order, every field compared, the value by *bits* — and the floor. Nothing about how the
/// store keeps its records is asked, because that is the property this whole step turns on.
#[must_use]
pub fn agree(fast: &Store, model: &Model) -> bool {
    todo!()
}

/// One step of a walk: play `op`, tell the model what it is owed, and compare.
///
/// The comparison happens **after every operation** — a walk that compared the final state would
/// report "sixty-four operations and the answers differ" — and the step it first disagrees at is what
/// [`Walk::mismatch`] answers.
pub fn step(walk: &mut Walk, op: &Op) {
    todo!()
}

/// The smallest subsequence of `failing`'s transcript that still disagrees.
///
/// The contract is three sentences, and this file asserts all three: the answer still disagrees, its
/// operations are the ones the walk played, in the same order, and no single operation can be taken
/// out of it and still leave a disagreement — deliberately "nothing obvious left to remove" rather
/// than "the smallest", because what a person reads in a bug report is the sequence where every
/// remaining operation is load-bearing. A walk that agreed has no counter-example, so there is
/// nothing to make small and the answer is empty.
#[must_use]
pub fn shrink(failing: &Walk) -> Vec<Op> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A batch of `count` records of `series`, a minute apart, from `at`: the same records
    /// [`Store::apply`] builds for an [`Op::Append`].
    fn batch(series: &'static str, at: i64, count: usize) -> Vec<Record> {
        (0..count)
            .map(|step| Record {
                series,
                ts_millis: at + step as i64 * MINUTE,
                value: step as f64 / 4.0,
            })
            .collect()
    }

    /// The word an operation is spelled with: the first word of its transcript line.
    fn kind(op: &Op) -> &'static str {
        match op {
            Op::Append { .. } => "append",
            Op::Flush => "flush",
            Op::Merge => "merge",
            Op::Expire { .. } => "expire",
            Op::Reopen => "reopen",
        }
    }

    /// The oracle, both ways: what a reader can see, and the floor.
    #[test]
    fn the_oracle_is_what_a_reader_can_see_and_the_floor() {
        let mut fast = Store::new();
        let mut model = Model::new();
        assert!(
            agree(&fast, &model),
            "two halves that were told nothing agree about nothing"
        );

        let append = Op::Append {
            series: "cpu,host=h0",
            from: EPOCH,
            count: 2,
        };
        let Effect::Accepted { records } = fast.apply(&append) else {
            panic!("an append is accepted by the log, and a log is not a segment");
        };
        assert_eq!(
            records,
            batch("cpu,host=h0", EPOCH, 2),
            "a batch is `count` records a minute apart, all of the series it names"
        );
        assert!(
            agree(&fast, &model),
            "an accepted record is not an answer: the model is owed these two records at the flush, \
             not here, and the store answers nothing yet"
        );

        let Effect::Sealed { records: closed } = fast.apply(&Op::Flush) else {
            panic!("a flush closes what the log holds into a segment");
        };
        assert_eq!(
            closed, records,
            "a flush seals the records the log took, in the order it took them"
        );
        assert!(
            !agree(&fast, &model),
            "the store now answers {} record(s) and the model was never told: a walk that compared \
             only at the end would blame its last operation for this",
            fast.answerable().len()
        );

        model.append(&closed);
        assert!(
            agree(&fast, &model),
            "and the model catches up the moment a flush closes them"
        );

        // The floor is the other half of the oracle, and the same rule one level up: the store stops
        // answering what a pass forgot, and nothing is deleted.
        fast.apply(&Op::Expire { at: EPOCH + WINDOW });
        assert!(
            !agree(&fast, &model),
            "the store has forgotten the first window and the model has not"
        );
        model.expire(EPOCH + WINDOW);
        assert!(agree(&fast, &model));
        assert!(
            fast.answerable().is_empty() && model.answerable().is_empty(),
            "both halves answer nothing above the floor, and both still hold what they were told"
        );

        // Order is part of the answer: the same two records the other way round are not the same
        // record for record, because a reader is handed one list.
        let mut backwards = closed;
        backwards.reverse();
        let mut unordered = Model::new();
        unordered.append(&backwards);
        assert!(
            !agree(&fast, &unordered),
            "the model was told the same two records in the other order, and `same_all` is not a \
             set comparison"
        );
    }

    /// The sharpest edge in the file: a batch the log took is invisible until a flush closes it.
    #[test]
    fn a_walk_of_appends_alone_leaves_the_model_holding_nothing() {
        let mut walk = Walk::new();
        walk.play(&[
            Op::Append {
                series: "cpu,host=h0",
                from: EPOCH,
                count: 3,
            },
            Op::Append {
                series: "cpu,host=h1",
                from: EPOCH + WINDOW,
                count: 2,
            },
        ]);
        assert_eq!(
            walk.played().len(),
            2,
            "every operation is played, and recorded"
        );
        assert!(
            walk.agreed(),
            "a model told at the flush agrees with a store that has opened no segment: {} \
             disagreed at step {:?}",
            describe(&walk.played()[walk.mismatch().unwrap_or(1)]),
            walk.mismatch()
        );
        assert!(
            walk.model().is_empty(),
            "the model is owed what a flush closed, so an accepted batch leaves it holding nothing \
             — a model told at the append holds {} records the store cannot answer with",
            walk.model().len()
        );
    }

    /// The build with the one defect is caught, and the build without it is not: a walk that caught
    /// nothing would pass every other case in this file.
    #[test]
    fn the_walk_catches_the_build_that_ignores_the_floor() {
        let right = walk(1, OPS);
        assert!(
            right.agreed(),
            "the correct build agrees after every one of {OPS} operations, and this walk disagreed \
             at step {:?}",
            right.mismatch()
        );

        let wrong = failing(1, OPS);
        assert!(
            !wrong.agreed(),
            "the build that keeps answering what the floor forgot is the defect the walk exists to \
             catch, and this seed of {OPS} operations never caught it"
        );
        let forgotten = wrong
            .store()
            .answerable()
            .iter()
            .filter(|record| record.ts_millis < wrong.model().floor())
            .count();
        assert!(
            forgotten > 0,
            "the wrong build answers {forgotten} record(s) below the floor of {}: it is answering \
             what the disk holds rather than what a pass kept",
            wrong.model().floor()
        );
        let at = wrong.mismatch().unwrap_or(OPS);
        assert!(
            at + 1 < OPS,
            "the walk compared after every operation rather than at the end: it first disagreed at \
             step {at} of {OPS}"
        );
    }

    /// A seed is the whole transcript: the same seed replays it, and two seeds are two walks.
    #[test]
    fn a_seed_replays_its_transcript_and_two_seeds_are_two_walks() {
        let first = walk(1, OPS);
        let again = walk(1, OPS);
        assert_eq!(
            first.played(),
            again.played(),
            "a seed decides every operation, so two runs of one seed play one sequence"
        );
        assert_eq!(
            first.digest(),
            again.digest(),
            "and the digest two runs are compared by is the same digest"
        );
        assert_eq!(
            first.digest(),
            "f44257ff37c6f533",
            "the transcript of seed 1 is the sequence this build walks: a change to the drawing \
             moves it, and a failure nobody can replay is a rumour. The first operation is {}.",
            describe(&first.played()[0])
        );

        let other = walk(7, OPS);
        assert!(other.agreed(), "seed 7 walks a correct store too");
        assert_ne!(
            other.digest(),
            first.digest(),
            "seed 1 and seed 7 played the same {OPS} operations: the seed reaches nothing, which is \
             the property `slow:sim@3` refuses three identical transcripts for"
        );
        assert_eq!(
            plan(1, OPS),
            plan(1, OPS),
            "the drawing is a function of the seed and the count, and of nothing else"
        );
    }

    /// A merge is an event in what the store is and not in what a reader sees.
    #[test]
    fn a_merge_changes_the_store_and_not_what_a_reader_sees() {
        let mut store = Store::new();
        store.apply(&Op::Append {
            series: "cpu,host=h0",
            from: EPOCH,
            count: 2,
        });
        store.apply(&Op::Flush);
        store.apply(&Op::Append {
            series: "cpu,host=h1",
            from: EPOCH + WINDOW,
            count: 3,
        });
        store.apply(&Op::Flush);
        let before = store.answerable();
        assert_eq!(store.segments(), 2, "two flushes, two segments");

        assert_eq!(
            store.apply(&Op::Merge),
            Effect::Nothing,
            "a merge is not an event in what a store was told, so the model hears nothing about it"
        );
        assert_eq!(
            store.segments(),
            1,
            "and it is an event in what the store is"
        );
        assert!(
            same_all(&store.answerable(), &before),
            "the records a reader sees are the ones it saw before, in the same order: {} before, {} \
             after",
            before.len(),
            store.answerable().len()
        );
        assert_eq!(
            store.apply(&Op::Merge),
            Effect::Nothing,
            "a merge of one segment has nothing to merge, and nothing to report"
        );
        assert_eq!(
            store.segments(),
            1,
            "and it leaves the store as it found it"
        );
    }

    /// A cutoff at or below the floor moves nothing, on either side of the comparison.
    #[test]
    fn a_cutoff_below_the_floor_moves_nothing() {
        let high = EPOCH + 2 * WINDOW;
        let mut store = Store::new();
        assert_eq!(
            store.apply(&Op::Expire { at: high }),
            Effect::Floor { at: high },
            "a cutoff above the floor moves it, and that is the only case that does"
        );
        assert_eq!(
            store.apply(&Op::Expire { at: EPOCH + WINDOW }),
            Effect::Nothing,
            "a policy whose window grew and a clock that stepped backwards are both unable to make \
             the store answer again"
        );
        assert_eq!(
            store.apply(&Op::Expire { at: high }),
            Effect::Nothing,
            "and a cutoff that is exactly the floor moves nothing: the floor is already there"
        );
        assert_eq!(
            store.floor(),
            high,
            "the floor is the highest cutoff written"
        );

        let mut walk = Walk::new();
        walk.play(&[Op::Expire { at: high }, Op::Expire { at: EPOCH + WINDOW }]);
        assert!(
            walk.agreed(),
            "the model's floor moves by the same rule, so the two halves agree: {:?}",
            walk.mismatch()
        );
        assert_eq!(
            walk.model().floor(),
            high,
            "and the model is told the number a pass writes down, not one it works out for itself"
        );
    }

    /// The shrinker: the sequence in the bug report, and the contract on it.
    #[test]
    fn a_failing_walk_shrinks_to_the_operations_it_needs() {
        let played = failing(7, OPS);
        assert!(
            !played.agreed(),
            "this seed's walk is the failure being shrunk"
        );
        let minimal = shrink(&played);
        assert!(
            minimal.len() < OPS,
            "the shrunk sequence is {} operation(s) of {OPS}: a transcript that long is a rumour \
             rather than a case",
            minimal.len()
        );
        assert!(
            played.still_disagrees(&minimal),
            "and it still disagrees: {:?}",
            minimal.iter().map(kind).collect::<Vec<&str>>()
        );

        // Its operations are the ones the walk played, in the same order, with some left out.
        let mut at = 0;
        for op in &minimal {
            let found = played.played()[at..]
                .iter()
                .position(|played| played == op)
                .unwrap_or_else(|| {
                    panic!(
                        "`{}` is in the shrunk sequence and not after step {at} of the walk it was \
                         shrunk from",
                        describe(op)
                    )
                });
            at += found + 1;
        }

        // And no single operation can be taken out of it and still leave a disagreement.
        for index in 0..minimal.len() {
            let mut candidate = minimal.clone();
            let removed = candidate.remove(index);
            assert!(
                !played.still_disagrees(&candidate),
                "`{}` can be taken out of the shrunk sequence and the failure is still there, so the \
                 sequence a person reads has {} operation(s) too many: {:?}",
                describe(&removed),
                minimal.len(),
                minimal.iter().map(kind).collect::<Vec<&str>>()
            );
        }

        assert_eq!(
            minimal.iter().map(kind).collect::<Vec<&str>>(),
            vec!["append", "flush", "expire"],
            "and it is the two operations a failure of this kind needs plus the cutoff that moved \
             the floor: {minimal:?}"
        );
    }

    /// A walk that agreed has no counter-example, so there is nothing to make small.
    #[test]
    fn a_walk_that_agreed_has_nothing_to_shrink() {
        let agreed = walk(1, OPS);
        assert!(agreed.agreed(), "the correct build agrees");
        assert!(
            shrink(&agreed).is_empty(),
            "a walk that disagreed nowhere has no sequence to report, and shrinking it anyway would \
             hand a reader {} operation(s) of nothing: {:?}",
            shrink(&agreed).len(),
            shrink(&agreed).iter().map(kind).collect::<Vec<&str>>()
        );
    }
}
