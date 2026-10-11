//! probe = "num/aggregation-correctness@apply"
//! title = "A sum is not `+`, and a day is not twenty-four hours"
//! hint = "Keep what each addition rounds away and add it back at the end, state the error bound as a number over the magnitudes, and resolve a local day's two midnights against the offset in force at each of them."
//!
//! Step `lakelet/4.12-aggregation-correctness` is about the two numbers every other test in this
//! track is happy to lie about, because the test everybody writes is the one that passes: four
//! samples of 1, 2, 3 and 4 add up to 10 whatever you do to them. A store is not four samples, and
//! a day is not twenty-four hours. Five decisions are the step:
//!
//! * **a sum is not `+`.** Adding a hundred thousand values in the order they arrived loses the
//!   small ones to the large ones: an `f64` carries about sixteen decimal digits, an accumulator
//!   over a store's worth of samples reaches magnitudes where a single sample falls below the last
//!   place, and every one of those additions rounds. [`Sum`] keeps the part of each addition that
//!   the rounding threw away and adds it back at the end — Kahan's rule in Neumaier's form — and
//!   the correction is *skipped* when the addition did not round for a reason an `f64` can round:
//!   an infinity, a `NaN`, or an overflow. `inf - inf` is not a number, and `1.0 + inf` is a sum
//!   that is perfectly well defined and must keep its answer;
//! * **a count is an integer, and the bound is a number in the code.** [`Sum::count`] is a `u64`,
//!   because a count carried in an `f64` is exact only up to two to the fifty-third and a mean is
//!   a sum over a count; [`BOUND`] and [`error_bound`] state the error a compensated sum is
//!   allowed as `BOUND * Σ|v|`, so the promise is a *number a case can ask for* rather than a
//!   comment — and the magnitudes are summed through the accumulator as well, because a bound
//!   computed by the arithmetic it bounds loses the same small terms and promises less than it
//!   means;
//! * **the answer may not depend on the order the store read it in.** Segments are read oldest
//!   first, a merge rewrites them and a crash can change an epoch, so the same records arrive in a
//!   different order on a different day and the same question has to answer the same way. That is
//!   what compensation buys, and it is also the honest reading of *monotonic time*: a bucket is
//!   computed from the records whose timestamp falls in it and never from where a record sat in
//!   the read, because the data is full of clocks that stepped backwards;
//! * **an absent answer is not a number.** An accumulator with nothing in it has no mean, and
//!   [`Sum::mean`] answers `None` rather than `NaN`: a `NaN` propagates through every comparison
//!   that follows it and shows up as a missing point on a dashboard an hour later. [`agrees`] is
//!   the comparison for magnitudes — `|left - right|` against the caller's own bound, with no
//!   floor of one hiding underneath it — and `NaN` agrees with nothing, itself included, which is
//!   exactly what a broken aggregation produces;
//! * **a day is not twenty-four hours.** Every bucket this store has computed sits on the absolute
//!   grid counted from the epoch, and that grid is right for a second, a minute and an hour — an
//!   hour is an hour everywhere — and wrong for a **day**: a zone's days are twenty-three hours
//!   once a year and twenty-five once a year, and a dashboard counting 86 400-second steps labels
//!   two different days with the same date and loses an hour of one of them. [`Zone::offset_at`]
//!   answers the offset in force, and `None` **outside the table it carries** rather than a guess;
//!   [`Zone::local_day`] is the half-open span of the *local* day an instant falls in, found by
//!   resolving each midnight against the offset in force **at that midnight** — the offset at the
//!   caller's own instant is the wrong one on the day a zone changes.
//!
//! ## What you write
//!
//! * [`Sum::add`] — the total, the part the addition dropped, and the count;
//! * [`Sum::mean`] — `None` when nothing was added, and the sum over the count otherwise;
//! * [`error_bound`] — [`BOUND`] times the magnitudes, and the magnitudes through the accumulator
//!   that can state them;
//! * [`Zone::offset_at`] — the offset in force at an instant, or `None` outside the table;
//! * [`Zone::local_day`] — the two instants a local day spans, or `None` outside the table.
//!
//! [`BOUND`], [`Sum::new`], [`Sum::value`], [`Sum::count`], [`naive`], [`running`], [`agrees`],
//! [`Zone::shipped`], [`Zone::fixed`], [`Zone::utc`], [`Zone::name`] and [`Zone::covers`] are
//! given, with [`Change`], the fixture table [`CHANGES`] and the span it covers. The vocabulary
//! *is* the interface here: these are the step's own names, and the table is given because
//! *transcribing* a time-zone database is not the habit being re-checked — reading one is.
//!
//! ## What the tests ask
//!
//! One very large value and a hundred thousand ones after it: a running total answers the large
//! value, the compensated sum answers exactly what the terms add up to, the count is exact and the
//! mean is that sum over that count — and the promise is not vacuous, because the running total is
//! outside the bound for that workload while the compensated sum is far inside it. The bound is
//! `BOUND` times the magnitudes, zero when there are no values, not a number when one of them is
//! not, and infinite when one of them is. The same values in the other order are the same
//! compensated answer and a *different* naive one, which is the case that justifies the
//! accumulator at all. An empty accumulator has no mean. An infinity does not become a `NaN`, and
//! `NaN` stays a `NaN` with the count still exact. The table answers at each transition and one
//! millisecond before it, and answers `None` one millisecond before the span, at the first instant
//! after it, and in 1970 — while a fixed offset answers everywhere. A spring-forward day is
//! twenty-three hours and a fall-back day is twenty-five, every instant of the short day names the
//! same day on both sides of the change, and a fixed offset's days are the absolute grid.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no store, no command line and no time-zone database.** The real
//! step's zone is a *name* the command line takes (`--zone Europe/Berlin`), with two years of
//! transitions transcribed from the zone database, a refusal for a name this build ships no table
//! for, a refusal for a range the table does not cover, and a rule that keeps the whole thing
//! narrow: only a window of *exactly one day* is moved, because an hour is an hour everywhere and a
//! rule that moved a five-minute bucket would be a second grid beside the one retention counts in.
//! None of that is here. [`Zone`] is built from the table it carries, so what this file re-checks
//! is the arithmetic a table is read with and the one refusal that keeps an answer honest: an
//! instant the table does not know answers `None` rather than a confident guess an hour out.
//!
//! The fixture table is two transitions rather than two years, and its instants are the ones 2024's
//! two transitions really happened at — so a reader can check them the way an operator checks a
//! zone, which is what the step's own table is for. The step's gates are not here either: the
//! budget workload, `slow:mutants@80`, the executor threading a zone through both input forms, and
//! the `USAGE` text that has to name the flag are all wiring *around* these five functions, and
//! none of them is a thing a three-minute re-check can be honest about.

/// How many milliseconds a day is when a caller counts days on the absolute grid.
///
/// The grid this step's zone arithmetic departs from, and the width that makes it depart: a window
/// of exactly one day is the one window a time zone may move.
pub const DAY_MILLIS: u64 = 86_400_000;

/// The error a compensated sum is allowed, relative to the sum of the magnitudes it added.
///
/// The promise is relative on purpose, and the base is `Σ|v|` rather than the sum itself: a sum
/// that cancels has a tiny answer and a large error budget, and a bound stated against the answer
/// would call it exact. Four units in the last place of the magnitudes is that promise with room to
/// spare.
pub const BOUND: f64 = 4.0 * f64::EPSILON;

/// A compensated (Kahan–Neumaier) accumulator: what one window's values are summed with.
///
/// The two float fields are the whole of the trick. `sum` is the running total — what `+=` would
/// have left behind — and `compensation` accumulates what each addition rounded away, so the
/// rounding is undone at the end rather than at every step. The count travels beside them as a
/// `u64`, because a mean is a sum over a count and a count carried in an `f64` is the bug a store
/// with a hundred million records finds.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sum {
    /// The running total: what `+=` would have left behind.
    sum: f64,
    /// What the additions rounded away, accumulated instead of lost.
    compensation: f64,
    /// How many values have been added. An integer, and an exact one.
    count: u64,
}

impl Sum {
    /// An accumulator with nothing in it: the sum of no values at all.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sum: 0.0,
            compensation: 0.0,
            count: 0,
        }
    }

    /// Add one value.
    ///
    /// The correction is the difference between the addition that was made and the exact one,
    /// taken the Neumaier way — from whichever of the two addends is the larger, because that is
    /// the one whose low bits the addition dropped — and it is skipped when the addition did not
    /// round for a reason an `f64` can *round*: an infinity, a `NaN`, or an overflow.
    pub fn add(&mut self, value: f64) {
        todo!()
    }

    /// The sum, as well as this accumulator can state it: the total plus what the additions left
    /// behind.
    #[must_use]
    pub const fn value(&self) -> f64 {
        self.sum + self.compensation
    }

    /// How many values have been added, exactly.
    #[must_use]
    pub const fn count(&self) -> u64 {
        self.count
    }

    /// What the values average to, or `None` when there are none.
    ///
    /// An empty accumulator has no mean, and it is `None` rather than `NaN` because a mean of
    /// nothing is not a number a caller can use: `NaN` propagates through every comparison that
    /// follows it and shows up as a missing point on a dashboard an hour later.
    #[must_use]
    pub fn mean(&self) -> Option<f64> {
        todo!()
    }
}

/// The sum everybody writes: add each value to a running total, in the order they arrive.
///
/// Given on purpose, and it is not dead code: it is the half of this subject a reader already has,
/// and the cases below call it to show what it costs. A claim that compensation is necessary is a
/// claim *about this function* and cannot be made without running it.
#[must_use]
pub fn naive(values: &[f64]) -> f64 {
    let mut total = 0.0;
    for value in values {
        total += value;
    }
    total
}

/// The same values through [`Sum`]: what a query computes them with.
#[must_use]
pub fn running(values: &[f64]) -> f64 {
    let mut sum = Sum::new();
    for value in values {
        sum.add(*value);
    }
    sum.value()
}

/// The error [`Sum`] is allowed on `values`: [`BOUND`] times the sum of their magnitudes.
///
/// The magnitudes go through a `Sum` and not through [`naive`], which is the part a reader can get
/// wrong: a bound computed by the arithmetic it is bounding would itself be wrong by orders of
/// magnitude more than the bound it states, and the case it exists to judge would be judged
/// against a number that means nothing. A `NaN` or an infinity among the values makes the bound
/// not a number or infinite, and [`agrees`] then certifies nothing — the honest answer for a
/// workload no bound applies to.
#[must_use]
pub fn error_bound(values: &[f64]) -> f64 {
    todo!()
}

/// `true` when `left` and `right` are the same number, inside the absolute `bound`.
///
/// Unlike `lakelet/4.10-downsampling`'s relative tolerance there is no floor of one underneath it:
/// the scale here is the caller's own bound, which is the promise being checked. `NaN` agrees with
/// nothing, itself included — and `NaN` is exactly what a broken aggregation produces, so the
/// comparison that would certify an answer has to refuse to certify it.
#[must_use]
pub fn agrees(left: f64, right: f64, bound: f64) -> bool {
    (left - right).abs() <= bound
}

/// One step of a zone's table: from `from_millis` on, the zone is `offset_millis` from UTC.
///
/// Both numbers are milliseconds rather than hours, because that is the unit a timestamp is in and
/// the unit a local day is computed in: a conversion per lookup would be a rounding per lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    /// The first millisecond since the Unix epoch the offset applies at.
    pub from_millis: u64,
    /// The offset from UTC, in milliseconds. `3_600_000` is `+01:00`.
    pub offset_millis: i64,
}

/// What a zone knows: one offset that never changes, or a table of transitions.
///
/// The two cases are genuinely different and the type says so: a table is a *history*, with the
/// span it was built with and nothing outside it, while a fixed offset is arithmetic a caller wrote
/// down and is true for every instant there is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Table {
    /// One offset from UTC for every instant the zone covers.
    Fixed(i64),
    /// The transitions this build ships, ascending, each in force from its own instant on.
    Shipped(Vec<Change>),
}

/// A time zone: a name, the span this build's table covers, and what it knows about those instants.
///
/// The span is a different fact from where the transitions are: a table built with two years of
/// them knows those two years and nothing outside, and saying so is what makes
/// [`Zone::offset_at`] able to answer `None` instead of extrapolating a rule it does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    name: String,
    from_millis: u64,
    to_millis: u64,
    table: Table,
}

impl Zone {
    /// A zone this build ships: a name, its transitions in ascending order (the first one is the
    /// offset the span opens on), and the first millisecond *after* the span it covers.
    #[must_use]
    pub fn shipped(name: &str, changes: Vec<Change>, to_millis: u64) -> Self {
        let from_millis = changes.first().map_or(0, |change| change.from_millis);
        Self {
            name: name.to_owned(),
            from_millis,
            to_millis,
            table: Table::Shipped(changes),
        }
    }

    /// An offset that never changes, for every instant there is.
    #[must_use]
    pub fn fixed(name: &str, offset_millis: i64) -> Self {
        Self {
            name: name.to_owned(),
            from_millis: 0,
            to_millis: u64::MAX,
            table: Table::Fixed(offset_millis),
        }
    }

    /// UTC: zero offset, for every instant an `i64` can name.
    #[must_use]
    pub fn utc() -> Self {
        Self::fixed("UTC", 0)
    }

    /// What this zone is called.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The half-open span of instants this build's table covers, as `[from, to)`.
    #[must_use]
    pub const fn covers(&self) -> (u64, u64) {
        (self.from_millis, self.to_millis)
    }

    /// How far `ts_millis` is from UTC in this zone, or `None` when the table does not cover that
    /// instant.
    ///
    /// `None` is the whole of this build's honesty about time zones. A table is a *file*: it names
    /// the transitions it was built with and there is no rule in it for 2019 or for 2029, and a
    /// query answered with a guessed offset would put a bucket an hour from the one the caller
    /// asked about, twice a year, in a way nobody can see. So the offset is stated where the table
    /// has it and refused where it does not, while a fixed offset is stated everywhere. A timestamp
    /// before the Unix epoch is read as the epoch, which is the same choice the window pass makes
    /// for the same clock: a table that starts in 2024 covers neither.
    #[must_use]
    pub fn offset_at(&self, ts_millis: i64) -> Option<i64> {
        todo!()
    }

    /// The half-open span of the *local* day `ts_millis` falls in, as milliseconds since the Unix
    /// epoch in UTC, or `None` when the table does not cover the day.
    ///
    /// The two numbers are the instants the local day begins and ends, which is what a bucket key
    /// has to be: a dashboard asked for a point a day has to print one *instant* per day, and it
    /// has to be the instant the day started — not midnight in the caller's own offset, and not the
    /// start of the absolute-grid window the day happens to overlap.
    ///
    /// The span is the point of the whole module. On the day a zone springs forward the local day
    /// is **twenty-three hours** long and on the day it falls back it is **twenty-five**, and that
    /// is not an artefact to be smoothed away: it is the answer. The boundaries are found by
    /// resolving local midnight against the offset in force *at that midnight*, which is why a day
    /// whose offset changes part-way through still comes out as one span.
    #[must_use]
    pub fn local_day(&self, ts_millis: i64) -> Option<(u64, u64)> {
        todo!()
    }
}

/// The milliseconds in an hour, for reading the table below.
const HOUR_MILLIS: i64 = 3_600_000;

/// The transitions this file's fixture zone ships, and the offset its span opens on.
///
/// **Given, and deliberately two transitions long.** The step's own table is two years of these,
/// transcribed from the zone database, and a real one is a file that goes stale, ships as a data
/// update and is wrong the moment a government changes a rule. What this file re-checks is the
/// arithmetic a table is *read* with, and two transitions is the least a table can hold and still
/// have a day that is twenty-three hours long *and* one that is twenty-five. The instants are the
/// ones 2024's two European transitions really happened at, so a reader can check them by hand.
pub const CHANGES: [Change; 3] = [
    // 2024-01-01T00:00:00+01:00: the offset the span opens on, and the first instant it covers.
    Change {
        from_millis: 1_704_063_600_000,
        offset_millis: HOUR_MILLIS,
    },
    // 2024-03-31T01:00:00Z: 02:00 local becomes 03:00, and that day is twenty-three hours long.
    Change {
        from_millis: 1_711_846_800_000,
        offset_millis: 2 * HOUR_MILLIS,
    },
    // 2024-10-27T01:00:00Z: 03:00 local becomes 02:00, and that day is twenty-five hours long.
    Change {
        from_millis: 1_729_990_800_000,
        offset_millis: HOUR_MILLIS,
    },
];

/// The first millisecond after the span [`CHANGES`] covers: 2025-01-01T00:00:00+01:00.
pub const COVERS_TO: u64 = 1_735_686_000_000;

#[cfg(test)]
mod tests {
    use super::*;

    /// Milliseconds in an hour.
    const HOUR: u64 = 3_600_000;

    /// Milliseconds in a day.
    const DAY: u64 = 86_400_000;

    /// The instant the fixture zone springs forward in 2024: 01:00 UTC, 02:00 local becoming
    /// 03:00.
    const SPRING: u64 = 1_711_846_800_000;

    /// Midnight local on the day that springs forward: 2024-03-30T23:00:00Z, that day's first
    /// instant.
    const SPRING_DAY: u64 = 1_711_839_600_000;

    /// Midnight local after it: 2024-03-31T22:00:00Z, so the day is twenty-three hours long.
    const SPRING_DAY_END: u64 = 1_711_922_400_000;

    /// The instant it falls back in 2024: 01:00 UTC, 03:00 local becoming 02:00.
    const FALL: u64 = 1_729_990_800_000;

    /// Midnight local on the day that falls back: 2024-10-26T22:00:00Z, three hours before the
    /// transition itself, because the day opens on the long offset.
    const FALL_DAY: u64 = 1_729_980_000_000;

    /// Midnight local after it: 2024-10-27T23:00:00Z, so the day is twenty-five hours long.
    const FALL_DAY_END: u64 = 1_730_070_000_000;

    /// One of this file's `u64` instants as the signed millisecond a zone names instants in.
    ///
    /// The two units are the same count and different types, and both are honest: a manifest and a
    /// bucket are `u64` — there is no bucket below the epoch — while a record's timestamp and an
    /// offset are `i64`, because a clock that was wrong can land there.
    fn at(millis: u64) -> i64 {
        i64::try_from(millis).expect("a dated constant of this file is a millisecond count")
    }

    /// The fixture zone: the table above, covering the span it was built with.
    fn fixture() -> Zone {
        Zone::shipped("Fixture/Berlin", CHANGES.to_vec(), COVERS_TO)
    }

    /// One very large value and a hundred thousand ones after it: the case the accumulator exists
    /// for.
    #[test]
    fn a_compensated_sum_keeps_the_terms_a_running_total_loses() {
        let mut values = vec![1e16];
        values.extend(std::iter::repeat_n(1.0, 100_000));
        let exact = 1e16 + 100_000.0;

        assert_eq!(
            naive(&values),
            1e16,
            "a running total adds a hundred thousand ones to a number that cannot hold one, and \
             answers the large value: this is what a query would report, and what this file exists \
             to refuse"
        );
        assert_eq!(
            running(&values),
            exact,
            "the compensated sum keeps the part of each addition that the rounding threw away, so \
             the terms `+=` lost are in the answer"
        );

        // The accumulator is a value, so it says how many values went in as well as what they add
        // to, and a mean is that sum over that count.
        let mut sum = Sum::new();
        for value in &values {
            sum.add(*value);
        }
        assert_eq!(
            sum.count(),
            100_001,
            "a count is exact, and the count is not the sum"
        );
        assert_eq!(sum.mean(), Some(exact / 100_001.0));

        // Both directions of the promise: the running total is outside the bound and the
        // compensated sum is far inside it, which is the only way a bound is worth stating.
        assert!(
            !agrees(naive(&values), exact, error_bound(&values)),
            "the promise is a number and not a hope: the running total is {} away from the answer, \
             and this file's bound for that workload is {}",
            naive(&values) - exact,
            error_bound(&values)
        );
        assert!(
            agrees(running(&values), exact, error_bound(&values)),
            "while the compensated sum is far inside it"
        );
    }

    /// The bound is a number the caller can read, and the magnitudes behind it go through the
    /// accumulator too.
    #[test]
    fn the_bound_is_a_number_the_caller_can_read() {
        assert_eq!(error_bound(&[]), 0.0, "no values, no error");
        assert_eq!(
            error_bound(&[3.0, 4.0]),
            BOUND * 7.0,
            "`BOUND` times the magnitudes, and nothing else"
        );
        assert!(
            agrees(0.0, 0.0, 0.0),
            "two zeroes are the same number inside nothing"
        );

        // A bound computed by the arithmetic it bounds would have lost the same small terms and
        // promised less than it means, which is why the magnitudes are summed with `Sum`.
        let mut values = vec![1e16];
        values.extend(std::iter::repeat_n(1.0, 100_000));
        assert!(
            error_bound(&values) > BOUND * 1e16,
            "the bound is over the magnitudes that were added and not over the ones a running \
             total could hold: {} against {}",
            error_bound(&values),
            BOUND * 1e16
        );

        // A promise with no number behind it certifies nothing, and a value that is not a number is
        // not an answer this can certify.
        assert!(
            error_bound(&[f64::NAN]).is_nan(),
            "a `NaN` among the values makes the bound not one"
        );
        assert_eq!(
            error_bound(&[f64::INFINITY]),
            f64::INFINITY,
            "an infinite magnitude makes the bound infinite, which is the honest answer"
        );
        assert!(
            !agrees(f64::NAN, f64::NAN, 1.0),
            "`NaN` agrees with nothing, itself included"
        );
        assert!(
            !agrees(f64::INFINITY, f64::INFINITY, 1.0),
            "and two infinities are not zero apart: `inf - inf` is not a number"
        );
        assert!(
            !agrees(1.0, 1.0 + BOUND, 0.0),
            "a bound of zero admits no difference at all"
        );
        assert!(
            agrees(1.0, 1.0 + BOUND, BOUND),
            "and `BOUND` admits exactly `BOUND`"
        );
    }

    /// The same values in another order are the same compensated answer, and a different naive
    /// one.
    #[test]
    fn the_same_values_in_another_order_are_the_same_answer() {
        // A segment is read oldest first, a merge rewrites it and a crash can change an epoch, so
        // the order the records arrive in is a property of the read and not of the question.
        let mut values = vec![1e16];
        values.extend(std::iter::repeat_n(1.0, 1_000));
        let shuffled: Vec<f64> = values.iter().rev().copied().collect();

        assert_eq!(running(&values), 1e16 + 1_000.0);
        assert_eq!(
            running(&shuffled),
            1e16 + 1_000.0,
            "the same values in the other order are the same answer, exactly"
        );
        assert!(
            agrees(running(&values), running(&shuffled), error_bound(&values)),
            "and inside the bound whatever the arithmetic does with the last place"
        );

        // And the reason this file is about compensation at all: the implementation everybody
        // writes is a function of the order it was handed.
        assert_eq!(naive(&values), 1e16);
        assert_eq!(naive(&shuffled), 1e16 + 1_000.0);
        assert!(
            !agrees(naive(&values), naive(&shuffled), error_bound(&values)),
            "a naive sum answers {} for one read and {} for the other, and the bound for that \
             workload is {}",
            naive(&values),
            naive(&shuffled),
            error_bound(&values)
        );
    }

    /// An empty accumulator has no mean, and a count is an integer rather than a float.
    #[test]
    fn an_empty_accumulator_has_no_mean_and_a_count_is_an_integer() {
        let empty = Sum::new();
        assert_eq!(empty.value(), 0.0, "the sum of no values is zero");
        assert_eq!(empty.count(), 0, "and nothing was added to it");
        assert_eq!(
            empty.mean(),
            None,
            "a mean of nothing is `None` and not `NaN`: a `NaN` propagates through every \
             comparison that follows it and shows up as a missing point on a dashboard an hour \
             later"
        );

        // A hundred thousand values, and the count that divides the sum is their own number and
        // not a rounded one: the type is the assertion, because `count` is a `u64`.
        let mut sum = Sum::new();
        for index in 0..100_000u64 {
            sum.add(index as f64);
        }
        assert_eq!(sum.count(), 100_000);
        assert_eq!(
            sum.value(),
            4_999_950_000.0,
            "0 + 1 + ... + 99_999, exactly"
        );
        assert_eq!(
            sum.mean(),
            Some(49_999.5),
            "a mean is that sum over that count, and the count of a hundred thousand values is \
             not a number a float could be relied on to state"
        );
    }

    /// A value that is not a number is not invented by the correction, and neither is an
    /// infinity.
    #[test]
    fn a_value_that_is_not_a_number_is_not_invented() {
        // `1.0 + inf` is `inf`, and an accumulator that "corrected" it with `inf - inf` would
        // answer `NaN` for a sum that is perfectly well defined.
        assert_eq!(naive(&[1.0, f64::INFINITY]), f64::INFINITY);
        assert_eq!(
            running(&[1.0, f64::INFINITY]),
            f64::INFINITY,
            "the compensation is for rounding, and an infinity does not round: a sum that has an \
             answer keeps it"
        );
        assert_eq!(running(&[f64::INFINITY, 1.0]), f64::INFINITY);

        // An infinity and its negative are the one sum with no answer at all, and the answer says
        // so rather than inventing a number that looks like one.
        assert!(running(&[f64::INFINITY, f64::NEG_INFINITY]).is_nan());
        assert!(naive(&[f64::INFINITY, f64::NEG_INFINITY]).is_nan());
        assert!(
            running(&[1.0, f64::NAN]).is_nan(),
            "a `NaN` among the values makes the sum a `NaN`"
        );

        // The accumulator still counted exactly, and its mean is a number that is not one.
        let mut sum = Sum::new();
        sum.add(1.0);
        sum.add(f64::NAN);
        assert_eq!(sum.count(), 2, "a `NaN` is a value that was added");
        assert!(
            sum.mean().is_some_and(f64::is_nan),
            "and its mean is not a number"
        );
    }

    /// The table answers inside the span it covers and `None` outside it, while a fixed offset
    /// answers everywhere.
    #[test]
    fn the_table_answers_inside_its_span_and_nothing_outside_it() {
        let zone = fixture();
        assert_eq!(zone.name(), "Fixture/Berlin");
        assert_eq!(
            zone.covers(),
            (CHANGES[0].from_millis, COVERS_TO),
            "a table covers the span it was built with"
        );

        // The two transitions, asserted at the instant and one millisecond before it: an offset
        // table is a set of instants, and a case that checked a summer date and a winter date
        // would pass for a table whose changes were days out.
        assert_eq!(
            zone.offset_at(at(SPRING - 1)),
            Some(HOUR_MILLIS),
            "an hour before the spring forward the zone is still +01:00"
        );
        assert_eq!(
            zone.offset_at(at(SPRING)),
            Some(2 * HOUR_MILLIS),
            "and at it, it is +02:00"
        );
        assert_eq!(
            zone.offset_at(at(FALL - 1)),
            Some(2 * HOUR_MILLIS),
            "the same the other way round: +02:00 one millisecond before the fall back"
        );
        assert_eq!(
            zone.offset_at(at(FALL)),
            Some(HOUR_MILLIS),
            "and +01:00 at it"
        );

        // Outside the span the honest answer is `None`: this build has no rule for 2019 or for
        // 2025, and a guessed offset would put a bucket an hour from the one the caller asked
        // about, twice a year, in a way nobody can see.
        assert_eq!(
            zone.offset_at(at(CHANGES[0].from_millis - 1)),
            None,
            "one millisecond before the first instant the table covers"
        );
        assert_eq!(
            zone.offset_at(at(COVERS_TO)),
            None,
            "and at the first instant after the span"
        );
        assert_eq!(
            zone.offset_at(0),
            None,
            "1970 is far outside a table that starts in 2024"
        );
        assert_eq!(
            zone.local_day(at(CHANGES[0].from_millis - DAY)),
            None,
            "a day the table does not cover is not guessed either"
        );

        // While a fixed offset is arithmetic rather than a table, and covers every instant there
        // is: that is the whole difference between the two provenances of a zone.
        let utc = Zone::utc();
        assert_eq!(utc.offset_at(0), Some(0), "UTC is zero offset");
        assert_eq!(
            utc.offset_at(-1),
            Some(0),
            "and a clock that landed before 1970 is read as the epoch, the choice the window pass \
             makes for the same clock"
        );
        assert_eq!(
            utc.covers(),
            (0, u64::MAX),
            "UTC covers every instant there is"
        );
        assert_eq!(
            Zone::fixed("+02:00", 2 * HOUR_MILLIS).offset_at(0),
            Some(2 * HOUR_MILLIS),
            "and an offset a caller wrote answers everywhere too"
        );
    }

    /// A spring-forward day is twenty-three hours long and a fall-back day is twenty-five.
    #[test]
    fn a_local_day_is_twenty_three_hours_once_a_year_and_twenty_five_the_other() {
        let zone = fixture();

        // These are the two numbers a dashboard counting in fixed 86 400-second steps gets wrong
        // twice a year.
        assert_eq!(
            zone.local_day(at(SPRING)),
            Some((SPRING_DAY, SPRING_DAY_END))
        );
        assert_eq!(
            SPRING_DAY_END - SPRING_DAY,
            23 * HOUR,
            "a spring-forward day is twenty-three hours long"
        );
        assert_eq!(zone.local_day(at(FALL)), Some((FALL_DAY, FALL_DAY_END)));
        assert_eq!(
            FALL_DAY_END - FALL_DAY,
            25 * HOUR,
            "and a fall-back day is twenty-five"
        );

        // Every instant of the short day names the same day, on both sides of the change, which is
        // the whole of "a bucket is a function of the time it covers".
        for instant in [
            SPRING_DAY,
            SPRING - 1,
            SPRING,
            SPRING + HOUR,
            SPRING_DAY_END - 1,
        ] {
            assert_eq!(
                zone.local_day(at(instant)),
                Some((SPRING_DAY, SPRING_DAY_END)),
                "the local day of {instant} is one day and not two"
            );
        }
        assert_eq!(
            zone.local_day(at(SPRING_DAY - 1)),
            Some((SPRING_DAY - DAY, SPRING_DAY)),
            "and the day before ends exactly where the short one begins"
        );
        assert_eq!(
            zone.local_day(at(FALL_DAY - 1)),
            Some((FALL_DAY - DAY, FALL_DAY)),
            "as does the one before the day that falls back, which opens on the long offset"
        );
    }

    /// A fixed offset's days are the absolute grid, and a local day is not the grid window that
    /// happens to cover it.
    #[test]
    fn utc_days_are_the_absolute_grid_and_a_local_day_is_not_a_grid_window() {
        let utc = Zone::utc();
        let grid = SPRING / DAY * DAY;

        // UTC's local days *are* the grid, which is why it is the default and not a mode.
        assert_eq!(utc.local_day(at(SPRING)), Some((grid, grid + DAY)));
        assert_eq!(
            utc.local_day(at(grid)),
            Some((grid, grid + DAY)),
            "midnight opens the day it starts"
        );
        assert_eq!(
            utc.local_day(at(grid + 1)),
            Some((grid, grid + DAY)),
            "and an instant a millisecond later is in the same day"
        );
        assert_eq!(
            utc.local_day(at(grid)),
            utc.local_day(at(grid + DAY - 1)),
            "the last millisecond of the day is in the day that ends there"
        );

        // The local day of a zone that moves is not the window that covers it: midnight local is
        // an hour before the grid's, and the short day is an hour short of a grid window while the
        // long one is an hour over it.
        let zone = fixture();
        assert_ne!(
            zone.local_day(at(SPRING)),
            utc.local_day(at(SPRING)),
            "a local day is not the window that happens to cover it"
        );
        assert_eq!(
            SPRING_DAY_END - SPRING_DAY,
            DAY - HOUR,
            "which is exactly the hour a local day is short of a grid window"
        );
        assert_eq!(
            FALL_DAY_END - FALL_DAY,
            DAY + HOUR,
            "and the day that falls back is an hour longer than one"
        );
    }
}
