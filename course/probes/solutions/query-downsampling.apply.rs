//! probe = "query/downsampling@apply"
//! title = "A window is a bucket of the absolute grid"
//! hint = "Align every bucket to the window width rather than to the first record, answer only the range you were asked about, and leave an empty window out of the answer instead of printing a zero."
//!
//! Step `lakelet/4.10-downsampling` is where the store stops being a filing cabinet and becomes an
//! instrument: a dashboard wants a point a minute, an alert a point an hour and a capacity plan a
//! point a day, and all three are the same question about a **window** of time. What that step paid
//! for is the three ways a downsampled answer is wrong while looking completely right:
//!
//! * **a window is a bucket of the absolute time axis.** [`Bucket::of`] is `floor(ts / width) *
//!   width`, counted from the Unix epoch and half-open, so the bucket a timestamp lands in does not
//!   depend on who asked. Anchor the first window at the first record a query happens to see and
//!   one extra sample at the front shifts every boundary: the same hour answers two different
//!   `start`s, and a retention cutoff that counts in whole buckets (`lakelet/4.9-retention`) can no
//!   longer be compared with a query's buckets at all;
//! * **a bucket counts what is in the range, not what is in the window.** The first and last window
//!   of an answer are usually partial: they hold the records between the range's edge and the
//!   window's edge, and nothing else. The tempting repair — fill the partial window from whatever
//!   is nearby — reads records the caller did not ask about, which is why [`Bucket::count`] is the
//!   honest number beside [`Bucket::value`]: a mean of one sample and a mean of four are the same
//!   number and completely different facts;
//! * **an empty window is absent.** A window in which nothing was *reported* does not appear in
//!   the answer at all — not with a zero, not with a count of zero. Printing one destroys the only
//!   difference an operator has, a collector that is down against a machine that is idle, and the
//!   gap stays countable as [`Answer::gaps`]. The other direction of the same rule is
//!   [`Answer::invented_windows`]: a bucket for a window the range does not touch is a window
//!   nobody asked about;
//! * **the answer states its own size.** Every record handed to the pass is looked at
//!   ([`Answer::scanned`]), and the records that landed in a bucket are [`Answer::in_range`], which
//!   is the buckets' counts added up. A number that did not come from the records it claims is a
//!   number nobody can check — an index that skips a record is `lakelet/4.14-index-pruning`'s
//!   subject, and it may not change the answer;
//! * **the tolerance is a number in the code.** [`TOLERANCE`] and [`agree`] are this track's first
//!   declared comparison, and every comparison of two computed numbers goes through them: two sums
//!   over the same records in a different order are different `f64`s, so a case written with `==`
//!   is a flaky test in an afternoon and a mystery in a year. The comparison is relative, with a
//!   floor of one at the bottom, so two zeroes are the same answer and two values near zero are
//!   not compared by a ratio that does not exist.
//!
//! ## What you write
//!
//! * [`Bucket::of`] — the window a timestamp falls in, on the absolute grid;
//! * [`downsample`] — one pass: a bucket for every window of the range that held records, oldest
//!   first, and the counts the pass read;
//! * [`Answer::gaps`] — how many windows the range touches that the answer says nothing about;
//! * [`Answer::invented_windows`] — how many buckets the answer holds for windows the range does
//!   not touch; an honest answer's is zero;
//! * [`agree`] — the comparison two computed numbers go through.
//!
//! [`Record`], [`Range`], [`Aggregate`], [`Bucket`], [`Answer`] and [`TOLERANCE`] are given, with
//! the accessors that read them: the vocabulary *is* the interface. The command line prints
//! `start count value`, the budget lane counts `gaps` and `invented_windows_max`, and
//! `lakelet/4.11-query-engine`'s grammar asks `Aggregate::parse` for its five names — a file that
//! let you invent those would be checking a different step.
//!
//! ## What the tests ask
//!
//! A timestamp exactly on a window edge belongs to the window that starts on it and the
//! millisecond before it belongs to the window before; a clock that was wrong and landed before
//! 1970 lands in the first bucket, because there is no bucket below the epoch; and a zero-width
//! window is the narrowest window there is rather than a hole. A record before `from` and a record
//! at `to` are dropped rather than clamped into an edge bucket, `in_range` is the buckets' counts
//! added up, and every record handed in is scanned whether it was bucketed or not. A range that
//! starts and ends mid-window leaves partial buckets: five windows, counts `[2, 4, 4, 4, 2]`, and
//! sixteen records bucketed out of the thirty-two scanned. A window whose records all measured
//! zero is present with its count; the window nothing was reported in is absent and is a gap; and
//! no bucket is invented for a window the range does not touch. A range that ends where it starts
//! answers nothing at all rather than a zero, and a range whose windows held nothing is an empty
//! answer with those windows counted as gaps. One window answers five ways: `count` 4, `sum` 11,
//! `mean` 2.75, `min` -1 and `max` 7. And the tolerance's two ends: one tolerance away agrees, two
//! do not, and `NaN` agrees with nothing.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no store and no command line.** The records are a `Vec` here and a
//! question is a function call. The real step reads a store's snapshot in `src/cli/mod.rs`, sits
//! behind `lakeletq query --store … --from … --to … --window 5m --agg mean` with five exit codes
//! and a `MAX_WINDOWS` ceiling decided before a store is opened, and measures itself against a
//! budget harness that counts 400 records scanned, 88 in range, 28 windows touched, 23 buckets and
//! 5 gaps over a workload of its own. None of that is here, and none of it is what a probe is for:
//! what is here is the grid and the honesty about missing data, which is exactly what somebody who
//! passed this step a year ago should still be able to write from an empty file in a few minutes.
//!
//! Two things are missing on purpose, and both are named rather than hidden. The **series filter**
//! is not in the signature: the step's pass takes `Option<&str>` where `None` is every series and a
//! name is an exact, case-carrying match, and one series per question is what keeps this file about
//! the grid — `lakelet/4.11-query-engine`'s `FROM` is that rule in the query's own words. And there
//! is **no zone**: a window here is milliseconds counted from the epoch on one absolute grid,
//! while a *local* day — 23 hours once a year in a zone that springs forward, 25 the day it falls
//! back — is a table of offsets, which is `lakelet/4.12-aggregation-correctness`'s second half.
//! That one is not an omission but the point of the alignment rule: an hour is an hour everywhere,
//! and what a store forgets has to be a number that does not depend on who asked.

/// One record of a series, as the store hands it to a pass: the series it belongs to, when it was
/// measured, and what it measured.
///
/// The same three fields the window pass reads off the protocol's record, with the timestamp kept
/// signed: a clock that was wrong can land before 1970, so the grid has to have an answer for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// The series it belongs to: the metric and its tags, exactly as they were ingested.
    pub series: String,
    /// When it was measured, in milliseconds since the Unix epoch.
    pub ts_millis: i64,
    /// What it measured.
    pub value: f64,
}

/// The half-open span of time a question is about: `[from, to)`.
///
/// Given in full, because it is the shape this track already counts in: a record at `from` is in
/// the answer and a record at `to` is the first record of the *next* question, and there is one
/// rule about that rather than one per module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    from_millis: u64,
    to_millis: u64,
}

impl Range {
    /// The range `[from_millis, to_millis)`.
    #[must_use]
    pub const fn new(from_millis: u64, to_millis: u64) -> Self {
        Self {
            from_millis,
            to_millis,
        }
    }

    /// The first millisecond the range is about.
    #[must_use]
    pub const fn from(&self) -> u64 {
        self.from_millis
    }

    /// The first millisecond the range is not about.
    #[must_use]
    pub const fn to(&self) -> u64 {
        self.to_millis
    }

    /// `true` when `ts_millis` is the range's: at or after its start and before its end.
    #[must_use]
    pub const fn contains(&self, ts_millis: u64) -> bool {
        self.from_millis <= ts_millis && ts_millis < self.to_millis
    }

    /// `true` when the range holds no millisecond at all — `to` at or below `from`.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.to_millis <= self.from_millis
    }
}

/// How one window's records become one number.
///
/// Given, with its names and its exact-match parse, because the five are the command line's `--agg`
/// values and the grammar's `<agg>`: a near miss is a caller's typo, and a typo that answered
/// something else would be a plausible number for a question nobody asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggregate {
    /// How many records the window held.
    Count,
    /// What they add up to.
    Sum,
    /// What they average to.
    Mean,
    /// The smallest of them.
    Min,
    /// The largest of them.
    Max,
}

impl Aggregate {
    /// The five, in the order the command line's `--agg` names them.
    pub const ALL: [Aggregate; 5] = [
        Aggregate::Count,
        Aggregate::Sum,
        Aggregate::Mean,
        Aggregate::Min,
        Aggregate::Max,
    ];

    /// The name this aggregate is asked for and printed by.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Aggregate::Count => "count",
            Aggregate::Sum => "sum",
            Aggregate::Mean => "mean",
            Aggregate::Min => "min",
            Aggregate::Max => "max",
        }
    }

    /// The aggregate a name names, exactly and in lower case, or `None`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Aggregate::ALL
            .into_iter()
            .find(|aggregate| aggregate.name() == name)
    }
}

/// The tolerance this step declares: a twentieth, relative, with a floor of one underneath it.
///
/// A declaration and not a derivation: the difference between a downsampled answer and the raw one
/// is a property of how the range falls across its partial windows, which is not something a
/// tolerance can be computed from, only stated.
pub const TOLERANCE: f64 = 0.05;

/// One window's answer: where it starts on the grid, how many records of the range went into it,
/// and what they come to.
///
/// The three fields are the whole answer a caller reads, and the command line prints them in this
/// order — so the shape is given and the arithmetic over it is the exercise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    start_millis: u64,
    count: u64,
    value: f64,
}

impl Bucket {
    /// The bucket `ts_millis` falls in at a window `width_millis` wide, with nothing in it yet.
    ///
    /// This is the alignment rule and nothing else, and a pass uses it for every record it
    /// buckets: the bucket a timestamp lands in is `floor(ts / width) * width`, half-open, counted
    /// from the Unix epoch. A width of zero is read as the narrowest window there is — one
    /// millisecond — because a value type that answers a hole is worse than one that answers a
    /// small bucket (the command line refuses `--window 0`, where a caller's typo is). A timestamp
    /// before 1970 is read as the epoch: a clock that was wrong lands in the first bucket rather
    /// than in no bucket at all.
    #[must_use]
    pub fn of(ts_millis: i64, width_millis: u64) -> Self {
        let at = u64::try_from(ts_millis).unwrap_or(0);
        let width = width_millis.max(1);
        Self {
            start_millis: at / width * width,
            count: 0,
            value: 0.0,
        }
    }

    /// The millisecond this window starts at, on the absolute grid.
    #[must_use]
    pub const fn start(&self) -> u64 {
        self.start_millis
    }

    /// How many records of the range this bucket was computed from.
    #[must_use]
    pub const fn count(&self) -> u64 {
        self.count
    }

    /// The aggregate over them.
    #[must_use]
    pub const fn value(&self) -> f64 {
        self.value
    }
}

/// What one pass answered: the buckets, oldest window first, and the counts the pass read on the
/// way.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    range: Range,
    width_millis: u64,
    buckets: Vec<Bucket>,
    scanned: u64,
    in_range: u64,
}

impl Answer {
    /// The range this answer is about.
    #[must_use]
    pub const fn range(&self) -> Range {
        self.range
    }

    /// The width the buckets were asked for, exactly as it was given.
    #[must_use]
    pub const fn width_millis(&self) -> u64 {
        self.width_millis
    }

    /// The buckets, oldest window first. Windows with no records in the range are not here at all.
    #[must_use]
    pub fn buckets(&self) -> &[Bucket] {
        &self.buckets
    }

    /// How many records the pass looked at: every record it was handed.
    #[must_use]
    pub const fn scanned(&self) -> u64 {
        self.scanned
    }

    /// How many records were inside the range: the records the buckets were computed from.
    #[must_use]
    pub const fn in_range(&self) -> u64 {
        self.in_range
    }

    /// `true` when the range held no record at all, so there is nothing to draw.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }

    /// How many windows the range touches that the answer says nothing about.
    ///
    /// An empty window is absent from [`Answer::buckets`] rather than present with a zero in it,
    /// and this is the count that keeps it visible: the windows the range touches, minus the
    /// buckets the answer holds.
    #[must_use]
    pub fn gaps(&self) -> u64 {
        if self.range.is_empty() {
            return 0;
        }
        let width = self.width_millis.max(1);
        let first = self.range.from() / width;
        // The last window the range touches is the one `to - 1` falls in: the millisecond at `to`
        // belongs to the next question.
        let last = (self.range.to() - 1) / width;
        let touched = last.saturating_sub(first).saturating_add(1);
        touched.saturating_sub(self.buckets.len() as u64)
    }

    /// How many buckets the answer holds for windows the range does not touch.
    ///
    /// An honest answer's is zero: a bucket outside the range would be a window nobody asked
    /// about, filled at an edge from records the range does not hold. The budget lane counts it as
    /// `invented_windows_max`, and the number it may not exceed is zero.
    #[must_use]
    pub fn invented_windows(&self) -> u64 {
        if self.range.is_empty() {
            // No window is the range's, so every bucket here is one the answer invented.
            return self.buckets.len() as u64;
        }
        let width = self.width_millis.max(1);
        let first = self.range.from() / width * width;
        let last = (self.range.to() - 1) / width * width;
        self.buckets
            .iter()
            .filter(|bucket| bucket.start() < first || bucket.start() > last)
            .count() as u64
    }
}

/// Answer one question about `records`: a bucket for every window of `range` that held records,
/// oldest first, and the counts the pass read.
///
/// One walk, one bucketing: a record outside the range is dropped rather than clamped into an edge
/// bucket, and a window with no records in the range is absent rather than present with a zero.
#[must_use]
pub fn downsample(
    records: &[Record],
    range: Range,
    width_millis: u64,
    aggregate: Aggregate,
) -> Answer {
    use std::collections::BTreeMap;

    let mut scanned = 0u64;
    let mut in_range = 0u64;
    // One entry per window the range held records in, and four numbers rather than the records: a
    // window of a day's samples is not something to hold in memory to answer a mean, and these
    // four are exactly what the five aggregates need. The map is the answer's order for free, and
    // a window the pass never saw is a window the answer does not have.
    let mut windows: BTreeMap<u64, (u64, f64, f64, f64)> = BTreeMap::new();
    for record in records {
        scanned += 1;
        let at = u64::try_from(record.ts_millis).unwrap_or(0);
        if !range.contains(at) {
            continue;
        }
        in_range += 1;
        let start = Bucket::of(record.ts_millis, width_millis).start();
        // `(count, sum, min, max)`, with `f64::min`/`f64::max` rather than a comparison: a value
        // that is not a number is not an extreme, and a comparison would let one poison the whole
        // window's answer.
        let entry = windows
            .entry(start)
            .or_insert((0, 0.0, record.value, record.value));
        entry.0 += 1;
        entry.1 += record.value;
        entry.2 = entry.2.min(record.value);
        entry.3 = entry.3.max(record.value);
    }
    let buckets = windows
        .into_iter()
        .map(|(start, (count, sum, min, max))| Bucket {
            start_millis: start,
            count,
            value: match aggregate {
                Aggregate::Count => count as f64,
                Aggregate::Sum => sum,
                Aggregate::Mean => sum / count as f64,
                Aggregate::Min => min,
                Aggregate::Max => max,
            },
        })
        .collect();
    Answer {
        range,
        width_millis,
        buckets,
        scanned,
        in_range,
    }
}

/// `true` when `left` and `right` are the same answer, inside [`TOLERANCE`].
///
/// Relative, with a floor of one at the bottom: a megabyte of samples is compared the way a handful
/// is, and two values near zero are not compared by a ratio that does not exist. `NaN` agrees with
/// nothing, itself included — quietly certifying one is how a broken aggregation reaches a
/// dashboard looking healthy.
#[must_use]
pub fn agree(left: f64, right: f64) -> bool {
    let magnitude = left.abs().max(right.abs()).max(1.0);
    (left - right).abs() <= TOLERANCE * magnitude
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Milliseconds in a minute: the width every case counts in, unless it builds one of its own.
    const MINUTE: u64 = 60_000;

    /// Milliseconds in an hour.
    const HOUR: u64 = 3_600_000;

    /// Milliseconds between two samples of a dense series: four a minute.
    const STEP: u64 = 15_000;

    /// The first timestamp of every case, on an hour boundary.
    ///
    /// `1_699_999_200_000` is `472_222 * HOUR` exactly, so every window a case asks for starts on
    /// a number a reader can check by hand — which is the whole reason the grid is absolute.
    const EPOCH: u64 = 1_699_999_200_000;

    /// One record of `series`, `offset` milliseconds after [`EPOCH`].
    fn record(name: &str, offset: u64, value: f64) -> Record {
        Record {
            series: name.to_owned(),
            ts_millis: (EPOCH + offset) as i64,
            value,
        }
    }

    /// `count` records of `name`, one every `step` milliseconds from `offset` after [`EPOCH`].
    fn series(
        name: &str,
        offset: u64,
        step: u64,
        count: usize,
        value: impl Fn(usize) -> f64,
    ) -> Vec<Record> {
        (0..count)
            .map(|index| record(name, offset + index as u64 * step, value(index)))
            .collect()
    }

    /// The starts of an answer's buckets, in the order the answer holds them.
    fn starts(answer: &Answer) -> Vec<u64> {
        answer.buckets().iter().map(Bucket::start).collect()
    }

    /// The counts of an answer's buckets.
    fn counts(answer: &Answer) -> Vec<u64> {
        answer.buckets().iter().map(Bucket::count).collect()
    }

    /// The values of an answer's buckets.
    fn values(answer: &Answer) -> Vec<f64> {
        answer.buckets().iter().map(Bucket::value).collect()
    }

    /// A timestamp on a window edge starts a bucket of its own, and the grid it starts on is the
    /// absolute one.
    #[test]
    fn a_window_is_a_bucket_of_the_absolute_grid() {
        assert_eq!(
            Bucket::of((EPOCH + HOUR) as i64, HOUR).start(),
            EPOCH + HOUR,
            "a timestamp on a window edge starts a bucket of its own"
        );
        assert_eq!(
            Bucket::of((EPOCH + HOUR - 1) as i64, HOUR).start(),
            EPOCH,
            "and the millisecond before it is in the bucket that began an hour earlier"
        );

        // The grid is the absolute one: a bucket starts on a multiple of the width counted from
        // the epoch, whatever the data does. A bucket anchored at the first record a query sees
        // answers a different `start` for the same timestamp depending on what else is in range.
        for offset in [0, 1, HOUR - 1, HOUR, 7 * HOUR + 13] {
            let start = Bucket::of((EPOCH + offset) as i64, HOUR).start();
            assert_eq!(
                start % HOUR,
                0,
                "a bucket starts on the absolute grid, and {start} is not a multiple of an hour"
            );
        }

        // A width that does not divide the timestamp: the bucket starts *below* it, as `floor`
        // does. These timestamps are counted from the epoch itself, because `EPOCH` is not a
        // multiple of seven minutes and a case built on it would assert where this workload's grid
        // happens to fall instead of what a bucket does.
        let odd = 7 * MINUTE;
        assert_eq!(
            Bucket::of(0, odd).start(),
            0,
            "the epoch is a boundary of every width"
        );
        assert_eq!(
            Bucket::of((3 * MINUTE) as i64, odd).start(),
            0,
            "three minutes into a seven-minute window is still that window"
        );
        assert_eq!(
            Bucket::of((7 * MINUTE - 1) as i64, odd).start(),
            0,
            "and so is the millisecond before the next one starts"
        );
        assert_eq!(
            Bucket::of((7 * MINUTE) as i64, odd).start(),
            7 * MINUTE,
            "which is the first millisecond of the next window"
        );

        // There is no bucket below the epoch: a clock that was wrong lands in the first bucket
        // rather than in no bucket at all, which is the same choice the store's index makes.
        assert_eq!(
            Bucket::of(-1, HOUR).start(),
            0,
            "a clock that was wrong lands in the first bucket"
        );

        // A width of zero is the narrowest window there is rather than a hole: a value type that
        // answers a hole is worse than one that answers a small bucket.
        assert_eq!(
            Bucket::of(EPOCH as i64, 0).start(),
            EPOCH,
            "a zero-width window is one millisecond wide, not a hole"
        );
        assert_eq!(Bucket::of((EPOCH + 1) as i64, 0).start(), EPOCH + 1);
    }

    /// A record outside the range is dropped rather than clamped into an edge bucket, and the
    /// counts say so.
    #[test]
    fn a_record_outside_the_range_is_dropped_rather_than_clamped() {
        let records = vec![
            record("cpu,host=h0", 0, 1.0),
            record("cpu,host=h0", MINUTE, 2.0),
            record("cpu,host=h0", 2 * MINUTE - 1, 3.0),
            record("cpu,host=h0", 2 * MINUTE, 4.0),
        ];
        let answer = downsample(
            &records,
            Range::new(EPOCH + MINUTE, EPOCH + 2 * MINUTE),
            MINUTE,
            Aggregate::Sum,
        );
        assert_eq!(
            counts(&answer),
            vec![2],
            "the two records inside `[from, to)`, and neither of the two the range does not hold"
        );
        assert_eq!(
            values(&answer),
            vec![5.0],
            "2 + 3: the record one millisecond before `from` and the one exactly at `to` are in a \
             different answer"
        );
        assert_eq!(
            answer.scanned(),
            4,
            "every record handed to the pass is looked at: there is no index to skip one with \
             until `lakelet/4.14-index-pruning`"
        );
        assert_eq!(
            answer.in_range(),
            2,
            "and the pass says how many records it bucketed"
        );
        assert_eq!(
            answer.in_range(),
            counts(&answer).iter().sum::<u64>(),
            "which is the buckets' own counts added up, not a second number that could disagree"
        );
        assert_eq!(
            answer.gaps(),
            0,
            "the one window the range touches held records"
        );
        assert_eq!(
            answer.invented_windows(),
            0,
            "and no bucket was invented at either edge of the range"
        );
    }

    /// A range that starts and ends mid-window leaves partial buckets, and every record is still
    /// scanned.
    #[test]
    fn a_range_that_starts_and_ends_mid_window_leaves_partial_buckets() {
        // Eight minutes of four samples a minute, asked about from the middle of one window to the
        // middle of another, so neither edge can be a whole window by accident.
        let records = series("cpu,host=h0", 0, STEP, 8 * 4, |index| index as f64);
        let answer = downsample(
            &records,
            Range::new(EPOCH + 2 * MINUTE + 30_000, EPOCH + 6 * MINUTE + 30_000),
            MINUTE,
            Aggregate::Count,
        );
        assert_eq!(
            starts(&answer),
            vec![
                EPOCH + 2 * MINUTE,
                EPOCH + 3 * MINUTE,
                EPOCH + 4 * MINUTE,
                EPOCH + 5 * MINUTE,
                EPOCH + 6 * MINUTE
            ],
            "the first bucket starts on the grid and not at the range's edge: a bucket is a bucket \
             of the time axis"
        );
        assert_eq!(
            counts(&answer),
            vec![2, 4, 4, 4, 2],
            "the first and last windows are partial, and a bucket counts what is in the range: two \
             samples in the first, four in each whole one, two in the last"
        );
        assert_eq!(
            answer.in_range(),
            16,
            "and the pass says how many records it bucketed"
        );
        assert_eq!(
            answer.scanned(),
            32,
            "out of every record it was handed, which is what a question costs before there is an \
             index"
        );
        assert_eq!(
            answer.gaps(),
            0,
            "every window the range touches held records"
        );
        assert_eq!(
            answer.invented_windows(),
            0,
            "and nothing was filled in from outside the range: that would be an answer to a \
             different question"
        );
    }

    /// An empty window is absent and a measured zero is present: the two halves of one rule, which
    /// is why they are asserted together.
    #[test]
    fn an_empty_window_is_absent_and_a_measured_zero_is_present() {
        let mut records = series("cpu,host=h0", 0, STEP, 4, |_| 5.0);
        records.extend(series("cpu,host=h0", MINUTE, STEP, 4, |_| 0.0));
        records.extend(series("cpu,host=h0", 3 * MINUTE, STEP, 4, |_| 7.0));
        let answer = downsample(
            &records,
            Range::new(EPOCH, EPOCH + 4 * MINUTE),
            MINUTE,
            Aggregate::Mean,
        );
        assert_eq!(
            starts(&answer),
            vec![EPOCH, EPOCH + MINUTE, EPOCH + 3 * MINUTE],
            "the second minute held only zeroes and the third held nothing at all"
        );
        assert_eq!(
            values(&answer)[1],
            0.0,
            "a window whose records all measured zero is an answer: it is present, and its value \
             is zero"
        );
        assert_eq!(
            counts(&answer)[1],
            4,
            "with the count that says four records were looked at and they were all zero"
        );
        assert!(
            !starts(&answer).contains(&(EPOCH + 2 * MINUTE)),
            "so nothing in the answer claims anything about the minute nothing was reported in"
        );
        assert_eq!(
            answer.gaps(),
            1,
            "and that minute is a gap: the four windows the range touches, minus the three \
             buckets the answer holds"
        );
        assert_eq!(
            answer.invented_windows(),
            0,
            "an absent window is absent: the answer does not fill it in to make the drawing \
             easier"
        );
    }

    /// A range with no millisecond in it answers nothing at all, and so does a range whose windows
    /// held nothing — with the windows it touched still countable.
    #[test]
    fn an_empty_range_answers_nothing_rather_than_a_zero() {
        let records = series("cpu,host=h0", 0, STEP, 4, |_| 5.0);
        let empty = downsample(
            &records,
            Range::new(EPOCH + MINUTE, EPOCH + MINUTE),
            MINUTE,
            Aggregate::Count,
        );
        assert!(
            empty.is_empty(),
            "a range that ends where it starts holds no millisecond: there is nothing to draw"
        );
        assert_eq!(empty.buckets().len(), 0, "and not one bucket of zeroes");
        assert_eq!(empty.in_range(), 0);
        assert_eq!(empty.gaps(), 0, "a range that touches no window has no gap");
        assert_eq!(empty.invented_windows(), 0);

        // The other empty answer: a range with windows in it that held nothing. It is empty rather
        // than a row of zeroes, and what it did not answer is still countable.
        let elsewhere = downsample(
            &records,
            Range::new(EPOCH + 10 * MINUTE, EPOCH + 12 * MINUTE),
            MINUTE,
            Aggregate::Count,
        );
        assert!(
            elsewhere.is_empty(),
            "the records are all outside the range"
        );
        assert_eq!(
            elsewhere.scanned(),
            4,
            "which the pass still looked at, all four of them"
        );
        assert_eq!(
            elsewhere.gaps(),
            2,
            "and two windows went unanswered, which is what a gap is"
        );
    }

    /// The five aggregates answer from the same window, and a count travels beside the value it
    /// describes.
    #[test]
    fn the_five_aggregates_answer_from_the_same_window() {
        // One window, four records: three, minus one, seven and two.
        let records = vec![
            record("cpu,host=h0", 0, 3.0),
            record("cpu,host=h0", STEP, -1.0),
            record("cpu,host=h0", 2 * STEP, 7.0),
            record("cpu,host=h0", 3 * STEP, 2.0),
        ];
        let range = Range::new(EPOCH, EPOCH + MINUTE);
        let answer = |aggregate| downsample(&records, range, MINUTE, aggregate);

        assert_eq!(
            counts(&answer(Aggregate::Count)),
            vec![4],
            "a count is how many records the window held"
        );
        assert_eq!(
            values(&answer(Aggregate::Count)),
            vec![4.0],
            "and its own number, as an `f64`"
        );
        assert_eq!(
            values(&answer(Aggregate::Sum)),
            vec![11.0],
            "3 - 1 + 7 + 2, and a sum is exact while its terms are"
        );
        assert_eq!(
            values(&answer(Aggregate::Mean)),
            vec![2.75],
            "11 / 4, in that order"
        );
        assert_eq!(
            values(&answer(Aggregate::Min)),
            vec![-1.0],
            "the smallest value in it"
        );
        assert_eq!(
            values(&answer(Aggregate::Max)),
            vec![7.0],
            "and the largest"
        );

        // The count and the value travel together: a mean of one record and a mean of four are the
        // same number and completely different facts.
        let one = downsample(
            &records,
            Range::new(EPOCH + 2 * STEP, EPOCH + 3 * STEP),
            MINUTE,
            Aggregate::Mean,
        );
        assert_eq!(
            counts(&one),
            vec![1],
            "one record of the four is in this range"
        );
        assert_eq!(
            values(&one),
            vec![7.0],
            "and the mean of one record is that record"
        );

        // The five names are the command line's, in its order, and a near miss is nobody's.
        assert_eq!(
            Aggregate::ALL.map(Aggregate::name),
            ["count", "sum", "mean", "min", "max"],
            "`ALL` is the five in the order `--agg` names them"
        );
        assert_eq!(Aggregate::parse("mean"), Some(Aggregate::Mean));
        assert_eq!(
            Aggregate::parse("Mean"),
            None,
            "a name is an exact, lower-case match: `Mean` is a typo, and answering a typo is how a \
             dashboard gets the wrong statistic"
        );
    }

    /// The tolerance is a number the code states, and it has to disagree with something.
    #[test]
    fn the_tolerance_is_a_number_the_code_states() {
        assert!(
            TOLERANCE > 0.0 && TOLERANCE < 1.0,
            "a tolerance of zero can never compare two computed numbers, and one of one can never \
             disagree with anything: {TOLERANCE}"
        );
        assert!(agree(1.0, 1.0), "a number is its own answer");
        assert!(
            agree(1.0, 1.0 + TOLERANCE),
            "one tolerance away still agrees"
        );
        assert!(
            !agree(1.0, 1.0 + 2.0 * TOLERANCE),
            "two tolerances away does not"
        );
        assert!(
            agree(1_000_000.0, 1_000_000.0 * (1.0 + TOLERANCE)),
            "the comparison scales with the magnitude, so a megabyte of samples is compared the \
             way a handful is"
        );

        // The floor: near zero there is nothing to scale by, and a purely relative test would call
        // two values a thousandth apart disagreeing.
        assert!(agree(0.0, 0.0), "two zeroes are the same answer");
        assert!(
            agree(0.0, TOLERANCE / 2.0),
            "and near zero the tolerance is absolute"
        );
        assert!(
            !agree(0.0, 1.0),
            "but zero and one are not the same answer at any scale"
        );

        // A value that is not a number is not an answer this can certify, and quietly certifying
        // it is how a broken aggregation reaches a dashboard looking healthy.
        assert!(
            !agree(f64::NAN, f64::NAN),
            "`NaN` does not even agree with itself"
        );
        assert!(!agree(1.0, f64::NAN), "and it agrees with nothing else");
    }

    /// The same instant answers the same `start` however the range falls: a bucket is a bucket of
    /// the absolute grid and not of whatever else the query happened to see.
    #[test]
    fn the_same_instant_answers_the_same_bucket_however_the_range_falls() {
        // One record, late in its window. A pass that anchored its first window at the first
        // record it saw would answer a `start` that depends on what else was in range.
        let late = vec![record("cpu,host=h0", 3 * MINUTE + 12_000, 1.0)];
        let only = downsample(
            &late,
            Range::new(EPOCH, EPOCH + 4 * MINUTE),
            MINUTE,
            Aggregate::Count,
        );
        assert_eq!(
            starts(&only),
            vec![EPOCH + 3 * MINUTE],
            "the record's bucket is the grid's, not the record's own start"
        );
        assert_eq!(
            only.gaps(),
            3,
            "and the three windows the range touches that held nothing are gaps rather than \
             buckets"
        );

        // The same record in a range with a sample in every other window: the bucket it is in is
        // the same number, and the edges did not borrow anything.
        let mut more = late.clone();
        more.extend(series("cpu,host=h0", 0, MINUTE, 4, |_| 2.0));
        let wide = downsample(
            &more,
            Range::new(EPOCH, EPOCH + 4 * MINUTE),
            MINUTE,
            Aggregate::Count,
        );
        assert_eq!(
            starts(&wide),
            vec![
                EPOCH,
                EPOCH + MINUTE,
                EPOCH + 2 * MINUTE,
                EPOCH + 3 * MINUTE
            ],
            "the four windows the range touches, oldest first"
        );
        assert_eq!(
            counts(&wide),
            vec![1, 1, 1, 2],
            "and the late record is one of the two in the last of them"
        );
        assert_eq!(
            only.buckets()[0].start(),
            wide.buckets()[3].start(),
            "the same instant answers the same `start` whoever asks"
        );
        assert_eq!(wide.gaps(), 0, "every window held at least one record");
        assert_eq!(
            wide.invented_windows(),
            0,
            "and a bucket appears for no window the range does not touch"
        );
    }
}
