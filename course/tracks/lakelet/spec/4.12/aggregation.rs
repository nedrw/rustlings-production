//! Spec for `lakelet/4.12-aggregation-correctness`: the two numbers a query is wrong about while
//! every other test passes — how a sum rounds, and what a day is.
//!
//! `kiln run` copies this file into the project as `tests/spec_aggregation.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Seventeen cases, and the shape of them is the point:
//!
//! * **a sum is not `+`, and the case says so by running both.** One large value and a hundred
//!   thousand ones after it: a running total answers the large value and the compensated sum
//!   answers the sum, and the difference is a hundred times the error this project promises. A
//!   step that shipped only the correct implementation would have no way to show that the one
//!   everybody writes is wrong.
//! * **the promise is a number, and it is a function of the magnitudes.** `error_bound` is
//!   `BOUND * Σ|v|`, the magnitudes are summed through the accumulator too — a bound computed by
//!   the arithmetic it bounds promises less than it means — and the cases assert both halves: the
//!   running sum inside the bound, and `naive` outside it.
//! * **the answer may not depend on the order the store reads it in.** One workload, two reads of
//!   it, one answer — at the level of the slice and at the level of a window — while a running
//!   total answers differently for the two and fails the bound, which is the case that justifies
//!   the module existing at all.
//! * **a count is an integer, and a mean of nothing is not a number.** `Sum::count` is a `u64`
//!   with no rounding in it, an empty accumulator's `mean` is `None` rather than `NaN`, and a
//!   `NaN` or an infinity among the values is behaviour rather than an accident: the compensation
//!   must not turn a well-defined sum (`1 + inf`) into `NaN`, and `agrees` must certify nothing
//!   about a `NaN`.
//! * **the window pass is the accumulator.** A window's answer from
//!   `lakelet/4.10-downsampling`'s `downsample` is the same number `Sum` states for the same
//!   records, the counts an answer carries are exact over a hundred thousand records, and the
//!   pass places a record by its timestamp and never by its position in the read.
//! * **a zone is a name and a table this build ships.** `Zone::parse` takes the shipped names and
//!   one `±HH:MM` shape and refuses everything else as the caller's class, `offset_at` answers
//!   `None` outside the table rather than guessing, and `local_day` gives the span of the *local*
//!   day — twenty-three hours on the day a zone springs forward and twenty-five on the day it
//!   falls back, with the boundary instants asserted one millisecond either side.
//! * **the command line counts a one-day window in local days and nothing else in anything.**
//!   Two zones give different boundaries and different counts for the same query, a window
//!   narrower than a day gives byte-identical output with and without a zone, `--zone UTC` is the
//!   answer a caller who says nothing gets, a range outside the table is refused in a sentence
//!   naming what the table covers, and the two input forms agree.
//! * **the boundary holds.** `src/engine/**` still may not name the service layer or the command
//!   line, `src/engine/query` names the module this step adds, and the usage text names `--zone`:
//!   it is a constant a scaffold cannot patch, so it is the learner's edit, and this pins it.
//!
//! Nothing here sleeps, nothing here reads a clock and nothing here measures a duration: every
//! range, window and instant is a number the case passes in, and the dated constants are instants
//! a reader can look up in any time-zone database of the same vintage.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use lakelet::cli::USAGE;
use lakelet::engine::compaction::Segments;
use lakelet::engine::query::agg::{BOUND, Sum, Zone, agrees, error_bound, naive, running};
use lakelet::engine::query::downsample::{self, Aggregate, Bucket, Range, Window, agree};
use lakelet::engine::query::exec::{Outcome, execute};
use lakelet::engine::query::lex::Refusal;
use lakelet::engine::query::parse::{Query, Statement, parse};
use lakelet::protocol::batch::Record;
use serde_json::Value;

/// Milliseconds in a minute: the window every undated case counts in, unless it builds its own.
const MINUTE: u64 = 60_000;

/// Milliseconds in an hour.
const HOUR: u64 = 3_600_000;

/// Milliseconds in a day: the one window a time zone may move.
const DAY: u64 = 86_400_000;

/// Milliseconds between two samples of a dense series: four a minute.
const STEP: u64 = 15_000;

/// The first timestamp of the cases about windows and arithmetic, on an hour boundary.
///
/// `1_699_999_200_000` is `472_222 * HOUR` exactly, so every window a case asks for starts on a
/// number a reader can check by hand — which is the whole reason the grid is absolute.
const EPOCH: u64 = 472_222 * HOUR;

/// The first millisecond `Europe/Berlin`'s table covers: 2024-01-01T00:00:00+01:00.
const BERLIN_FROM: u64 = 1_704_063_600_000;

/// The first millisecond after the last one it covers: 2026-01-01T00:00:00+01:00.
const BERLIN_TO: u64 = 1_767_222_000_000;

/// The instant `Europe/Berlin` springs forward in 2024: 01:00 UTC, 02:00 local becoming 03:00.
const SPRING: u64 = 1_711_846_800_000;

/// The instant it falls back in 2024: 01:00 UTC, 03:00 local becoming 02:00.
const FALL: u64 = 1_729_990_800_000;

/// Midnight local on the day that springs forward: 2024-03-30T23:00:00Z, that day's first instant.
const SPRING_DAY: u64 = 1_711_839_600_000;

/// Midnight local after it: 2024-03-31T22:00:00Z, so the day is twenty-three hours long.
const SPRING_DAY_END: u64 = 1_711_922_400_000;

/// Midnight local on the day that falls back: 2024-10-26T22:00:00Z, three hours before the
/// transition itself, because the day opens on the long offset.
const FALL_DAY: u64 = 1_729_980_000_000;

/// Midnight local after it: 2024-10-27T23:00:00Z, so the day is twenty-five hours long.
const FALL_DAY_END: u64 = 1_730_070_000_000;

/// The first timestamp of the command-line cases: 2024-03-29T00:00:00Z, two days before the
/// spring-forward day, so the answer has a whole day either side of the one that is short.
const SPRING_RANGE_FROM: u64 = 1_711_670_400_000;

/// The first millisecond after them: 2024-04-01T00:00:00Z.
const SPRING_RANGE_TO: u64 = 1_711_929_600_000;

/// How many hourly samples those three days hold.
const SPRING_SAMPLES: usize = 72;

/// One of this file's `u64` instants as the signed millisecond a [`Zone`] names instants in.
///
/// The two units are the same count and different types, and they are both honest: a manifest, a
/// `Range` and a bucket are `u64` (there is no bucket below the epoch), while a record's timestamp
/// and an offset are `i64` (a clock that was wrong can land there).
fn at(millis: u64) -> i64 {
    i64::try_from(millis).expect("a dated constant of this file is a millisecond count")
}

/// One record of `series`, `offset` milliseconds after [`EPOCH`].
fn record(series: &str, offset: u64, value: f64) -> Record {
    stamped(series, EPOCH + offset, value)
}

/// One record of `series` at the absolute instant `ts_millis`.
fn stamped(series: &str, ts_millis: u64, value: f64) -> Record {
    Record {
        series: series.to_owned(),
        ts_millis: ts_millis as i64,
        value,
    }
}

/// `count` records of `series`, one every `step` milliseconds from `offset` after [`EPOCH`].
fn series(
    series: &str,
    offset: u64,
    step: u64,
    count: usize,
    value: impl Fn(usize) -> f64,
) -> Vec<Record> {
    (0..count)
        .map(|index| record(series, offset + index as u64 * step, value(index)))
        .collect()
}

/// `count` hourly records of `series` from the absolute instant `from`.
fn hourly(series: &str, from: u64, count: usize) -> Vec<Record> {
    (0..count)
        .map(|index| stamped(series, from + index as u64 * HOUR, index as f64))
        .collect()
}

/// The starts of an answer's buckets, in the order the answer holds them.
fn starts(answer: &downsample::Answer) -> Vec<u64> {
    answer.buckets().iter().map(Bucket::start).collect()
}

/// The counts of an answer's buckets.
fn counts(answer: &downsample::Answer) -> Vec<u64> {
    answer.buckets().iter().map(Bucket::count).collect()
}

/// The values of an answer's buckets.
fn values(answer: &downsample::Answer) -> Vec<f64> {
    answer.buckets().iter().map(Bucket::value).collect()
}

/// A store directory of this case's own, removed when it is dropped.
struct Dir {
    /// Where the store lives.
    path: PathBuf,
}

impl Dir {
    /// Claim a directory for `label`.
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lakelet-aggregation-spec-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("{} cannot be created: {error}", path.display()));
        Self { path }
    }

    /// Where the store lives.
    fn path(&self) -> &Path {
        &self.path
    }

    /// The path a command line names, as the tool will read it.
    fn argument(&self) -> String {
        self.path
            .to_str()
            .unwrap_or_else(|| panic!("{} is not UTF-8", self.path.display()))
            .to_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A store in a directory of its own, sealed from `batches` in order.
fn sealed(label: &str, batches: &[Vec<Record>]) -> (Dir, Segments) {
    let dir = Dir::new(label);
    let mut store = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
    for batch in batches {
        store
            .seal(batch)
            .unwrap_or_else(|error| panic!("a segment of a case's own records is closed: {error}"));
    }
    (dir, store)
}

/// One `lakeletq` run: what a script would have seen.
#[derive(Debug)]
struct Ran {
    /// The exit code, or a panic's worth of explanation when the process was killed.
    code: i32,
    /// What standard output received.
    stdout: String,
    /// What standard error received.
    stderr: String,
}

/// Run the real binary, with no environment and `/dev/null` on standard input.
///
/// `env_clear` is the point: a case that inherited the developer's variables would be a case about
/// this machine rather than about the tool. Standard input is `null` rather than a pipe so that a
/// command which wrongly waited for a person fails here instead of hanging the suite.
fn lakeletq(args: &[&str]) -> Ran {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lakeletq"));
    command
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("`lakeletq {args:?}` could not be started: {error}"));
    Ran {
        code: output.status.code().unwrap_or_else(|| {
            panic!("`lakeletq {args:?}` was killed by a signal: it must exit with a code")
        }),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Run the real binary on a command line a case built up.
fn run(arguments: &[String]) -> Ran {
    let borrowed: Vec<&str> = arguments.iter().map(String::as_str).collect();
    lakeletq(&borrowed)
}

/// The failure rule, asserted once: nothing on stdout, a sentence on stderr, and this code.
fn refused(ran: &Ran, code: i32, what: &str) {
    assert_eq!(
        ran.code, code,
        "{what} must exit {code} — a script branches on this number, and {ran:?}"
    );
    assert_eq!(
        ran.stdout, "",
        "{what} must print nothing on stdout: a script that parses the answer must not be handed \
         half an answer, and it got `{}`",
        ran.stdout
    );
    assert!(
        !ran.stderr.trim().is_empty(),
        "{what} must say what went wrong on stderr: an exit code on its own sends the reader \
         to the source"
    );
}

/// A `query` in the flag form, which is `lakelet/4.10-downsampling`'s shape plus this step's flag.
fn query_arguments(
    store: &str,
    from: u64,
    to: u64,
    window: &str,
    aggregate: &str,
    zone: Option<&str>,
) -> Vec<String> {
    let mut arguments: Vec<String> = vec![
        "query".to_owned(),
        "--store".to_owned(),
        store.to_owned(),
        "--from".to_owned(),
        from.to_string(),
        "--to".to_owned(),
        to.to_string(),
        "--window".to_owned(),
        window.to_owned(),
        "--agg".to_owned(),
        aggregate.to_owned(),
    ];
    if let Some(zone) = zone {
        arguments.push("--zone".to_owned());
        arguments.push(zone.to_owned());
    }
    arguments
}

/// A `query` in the text form, which is `lakelet/4.11-query-engine`'s shape plus this step's flag.
fn text_arguments(store: &str, text: &str, zone: Option<&str>) -> Vec<String> {
    let mut arguments: Vec<String> = vec![
        "query".to_owned(),
        "--store".to_owned(),
        store.to_owned(),
        "--sql".to_owned(),
        text.to_owned(),
    ];
    if let Some(zone) = zone {
        arguments.push("--zone".to_owned());
        arguments.push(zone.to_owned());
    }
    arguments
}

/// The query one text parses to, for the cases that ask the engine rather than the binary.
fn parsed(text: &str) -> Query {
    match parse(text) {
        Ok(Statement::Query(query)) | Ok(Statement::Explain(query)) => query,
        Err(refusal) => {
            panic!("`{text}` is a query this grammar has, and it was refused: {refusal}")
        }
    }
}

/// The buckets of the one answer a query without `GROUP BY …, series` gets.
fn mixed(outcome: &Outcome) -> Vec<Bucket> {
    assert_eq!(
        outcome.answers().len(),
        1,
        "a query that groups by nothing has one answer, whatever the store holds"
    );
    assert!(
        outcome.answers()[0].series().is_none(),
        "and that answer is about every series at once"
    );
    outcome.answers()[0].buckets().to_vec()
}

/// Every `.rs` file below `directory`, for the checks that read the sources as text.
fn collect_rust(directory: &Path, found: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rust(&path, found)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
    Ok(())
}

/// The store the command-line cases read: three days of hourly samples around the spring-forward
/// day, and the path a command line names it by.
fn spring_store(label: &str) -> (Dir, String) {
    let dir = Dir::new(label);
    let mut store = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
    let records = hourly("cpu,host=h0", SPRING_RANGE_FROM, SPRING_SAMPLES);
    store
        .seal(&records)
        .unwrap_or_else(|error| panic!("a segment of a case's own records is closed: {error}"));
    let path = dir.argument();
    (dir, path)
}

#[test]
fn a_compensated_sum_keeps_the_terms_a_running_total_loses() {
    // One very large value and a hundred thousand ones after it. Every one of those additions is
    // below the last place of the total, so a running total drops each of them on the floor and a
    // compensated one does not.
    let mut values = vec![1e16];
    values.extend(std::iter::repeat_n(1.0, 100_000));
    let exact = 1e16 + 100_000.0;

    assert_eq!(
        naive(&values),
        1e16,
        "a running total adds a hundred thousand ones to a number that cannot hold one, and \
         answers the large value: this is what a query would report, and what this step exists \
         to refuse"
    );
    assert_eq!(
        running(&values),
        exact,
        "the compensated sum keeps the part of each addition that the rounding threw away, so \
         the terms `+=` lost are in the answer"
    );
    assert!(
        !agrees(naive(&values), exact, error_bound(&values)),
        "and the promise is a number and not a hope: the running total is {} away from the \
         answer and this project's bound for that workload is {}",
        naive(&values) - exact,
        error_bound(&values)
    );
    assert!(
        agrees(running(&values), exact, error_bound(&values)),
        "while the compensated sum is far inside it"
    );

    // The accumulator is a value, so it says how many values went in as well as what they add to.
    let mut sum = Sum::new();
    for value in &values {
        sum.add(*value);
    }
    assert_eq!(
        sum.count(),
        100_001,
        "a count is exact, and the count is not the sum"
    );
    assert_eq!(sum.value(), exact);
    assert_eq!(sum.mean(), Some(exact / 100_001.0));
}

#[test]
fn the_bound_is_the_promise_it_states_and_the_magnitudes_are_summed_with_it() {
    // The bound is proportional to the magnitudes that went in, and an accumulator with nothing
    // in it promises nothing at all.
    assert_eq!(error_bound(&[]), 0.0, "no values, no error");
    assert_eq!(
        error_bound(&[3.0, 4.0]),
        BOUND * 7.0,
        "`BOUND` times the magnitudes"
    );
    assert!(
        agrees(0.0, 0.0, 0.0),
        "two zeros are the same number inside nothing"
    );

    // The magnitudes go through the compensated accumulator too, and this is why: a bound computed
    // by the arithmetic it bounds would have lost the same small terms and promised less than it
    // means.
    let mut values = vec![1e16];
    values.extend(std::iter::repeat_n(1.0, 100_000));
    assert!(
        error_bound(&values) > BOUND * 1e16,
        "the bound is over the magnitudes that were added and not over the ones a running total \
         could hold: {} against {}",
        error_bound(&values),
        BOUND * 1e16
    );

    // A promise with no number behind it certifies nothing, and a value that is not a number is
    // not an answer this can certify.
    assert!(
        error_bound(&[f64::NAN]).is_nan(),
        "a `NaN` in the values makes the bound not one"
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

#[test]
fn the_same_values_shuffled_are_the_same_answer() {
    // One workload, two reads of it. A segment is read oldest first, a merge rewrites it and a
    // crash can change an epoch, so the order the records arrive in is a property of the read and
    // not of the question.
    let mut values = vec![1e16];
    values.extend(std::iter::repeat_n(1.0, 1_000));
    let shuffled: Vec<f64> = values.iter().rev().copied().collect();
    let bound = error_bound(&values);

    assert_eq!(running(&values), 1e16 + 1_000.0);
    assert_eq!(
        running(&shuffled),
        1e16 + 1_000.0,
        "the same values in the other order are the same answer, exactly"
    );
    assert!(
        agrees(running(&values), running(&shuffled), bound),
        "and inside the bound whatever the arithmetic does with the last place"
    );

    // And the reason this module exists: the implementation everybody writes is a function of the
    // order it was handed, and it misses the promise by two orders of magnitude.
    assert_eq!(naive(&values), 1e16);
    assert_eq!(naive(&shuffled), 1e16 + 1_000.0);
    assert!(
        !agrees(naive(&values), naive(&shuffled), bound),
        "a naive sum answers {} for one read and {} for the other, and the bound for that \
         workload is {bound}",
        naive(&values),
        naive(&shuffled)
    );
}

#[test]
fn an_empty_accumulator_has_no_mean_and_a_count_is_an_integer() {
    let empty = Sum::new();
    assert_eq!(empty.value(), 0.0, "the sum of no values is zero");
    assert_eq!(empty.count(), 0, "and nothing was added to it");
    assert_eq!(
        empty.mean(),
        None,
        "a mean of nothing is `None` and not `NaN`: a `NaN` propagates through every comparison \
         that follows it and shows up as a missing point on a dashboard an hour later"
    );

    // A hundred thousand values, and the count that divides the sum is their own number and not a
    // rounded one. The type is the assertion: `count` is a `u64`, and so is `Bucket::count`.
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
        "a mean is that sum over that count, and the count of a hundred thousand values is not a \
         number a float could be relied on to state"
    );
}

#[test]
fn a_value_that_is_not_a_number_is_not_invented() {
    // `1.0 + inf` is `inf`, and an accumulator that "corrected" it with `inf - inf` would answer
    // `NaN` for a sum that is perfectly well defined.
    assert_eq!(naive(&[1.0, f64::INFINITY]), f64::INFINITY);
    assert_eq!(
        running(&[1.0, f64::INFINITY]),
        f64::INFINITY,
        "the compensation is for rounding, and an infinity does not round: a sum that has an \
         answer keeps it"
    );
    assert_eq!(running(&[f64::INFINITY, 1.0]), f64::INFINITY);

    // An infinity and its negative are the one sum with no answer at all, and the answer says so.
    assert!(running(&[f64::INFINITY, f64::NEG_INFINITY]).is_nan());
    assert!(naive(&[f64::INFINITY, f64::NEG_INFINITY]).is_nan());

    // A `NaN` among the values makes the sum a `NaN`, and never a number that looks like an answer.
    assert!(running(&[1.0, f64::NAN]).is_nan());

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

#[test]
fn a_window_answer_is_the_accumulators_answer() {
    // One window, one large value and a thousand ones after it: the pass that answers a query is
    // the accumulator and not a running total beside it.
    let mut records = vec![record("cpu,host=h0", 0, 1e16)];
    records.extend(series("cpu,host=h0", STEP, 1, 1_000, |_| 1.0));
    let range = Range::new(EPOCH, EPOCH + MINUTE);
    let window = Window::new(MINUTE);

    let summed = downsample::downsample(&records, None, range, window, Aggregate::Sum);
    assert_eq!(
        counts(&summed),
        vec![1_001],
        "one window held every record of the range"
    );
    assert_eq!(
        values(&summed),
        vec![1e16 + 1_000.0],
        "the window's sum is the accumulator's sum: a running total over the same records \
         answers 1e16"
    );
    assert!(
        agree(values(&summed)[0], 1e16 + 1_000.0),
        "which is the answer `lakelet/4.10-downsampling`'s tolerance certifies"
    );

    // And a mean is that sum over that count, exactly as `Sum` states it — the same number, and
    // not a number inside a tolerance of it.
    let mean = downsample::downsample(&records, None, range, window, Aggregate::Mean);
    let mut sum = Sum::new();
    for record in &records {
        sum.add(record.value);
    }
    assert_eq!(
        values(&mean),
        vec![
            sum.mean()
                .expect("a window that held a thousand and one records has a mean")
        ],
        "the mean of a window is `Sum::mean` of the records that landed in it"
    );
    assert_eq!(counts(&mean), counts(&summed));
}

#[test]
fn a_window_does_not_care_what_order_the_store_reads_in() {
    // The same records as the case above, read in the other order — which is what a merge, a crash
    // and a clock that stepped backwards all look like from the pass's side.
    let mut forward = vec![record("cpu,host=h0", 0, 1e16)];
    forward.extend(series("cpu,host=h0", STEP, 1, 1_000, |_| 1.0));
    let mut backward = forward.clone();
    backward.reverse();

    let range = Range::new(EPOCH, EPOCH + MINUTE);
    let window = Window::new(MINUTE);
    let summed =
        |records: &[Record]| downsample::downsample(records, None, range, window, Aggregate::Sum);
    let one = summed(&forward);
    let other = summed(&backward);

    assert_eq!(
        starts(&one),
        starts(&other),
        "the grid is a property of the time axis"
    );
    assert_eq!(
        counts(&one),
        counts(&other),
        "and the grouping is a property of the timestamps"
    );
    assert_eq!(
        values(&one),
        values(&other),
        "and one window is one set of records, however they were read"
    );
    assert_eq!(values(&one), vec![1e16 + 1_000.0]);

    // A running total over the same window is a function of the read, and it misses the bound this
    // project states for that window by two orders of magnitude.
    let naive_of = |records: &[Record]| {
        let mut total = 0.0;
        for record in records {
            if range.contains(record.ts_millis as u64) {
                total += record.value;
            }
        }
        total
    };
    let magnitudes: Vec<f64> = forward.iter().map(|record| record.value).collect();
    assert_eq!(naive_of(&forward), 1e16);
    assert_eq!(naive_of(&backward), 1e16 + 1_000.0);
    assert!(
        !agrees(
            naive_of(&forward),
            naive_of(&backward),
            error_bound(&magnitudes)
        ),
        "a naive window holds {} for one read and {} for the other, and this project's bound \
         for that window is {}",
        naive_of(&forward),
        naive_of(&backward),
        error_bound(&magnitudes)
    );
}

#[test]
fn a_query_over_a_hundred_thousand_records_counts_every_one_it_read() {
    // A hundred thousand records in one segment: what a query says it read and what its answer
    // says went into it are the same number, and the counts are integers.
    let records = series("cpu,host=h0", 0, 1, 100_000, |index| index as f64);
    let (_dir, store) = sealed("counters", &[records]);
    let snapshot = store.snapshot();
    let (from, to) = (EPOCH, EPOCH + 100_000);
    let text = |aggregate: &str| {
        format!(
            "SELECT {aggregate}(value) FROM cpu WHERE ts BETWEEN {from} AND {to} \
             GROUP BY time(1d)"
        )
    };

    let counted = execute(&snapshot, &parsed(&text("count")))
        .unwrap_or_else(|refusal| panic!("a query over this store is answerable: {refusal}"));
    assert_eq!(
        counted.rows_scanned(),
        100_000,
        "the query read every record the segment holds"
    );
    assert_eq!(
        counted.rows_answered(),
        100_000,
        "and every one of them landed in a bucket"
    );
    let buckets = mixed(&counted);
    assert_eq!(buckets.len(), 1, "a hundred seconds is one day-wide window");
    assert_eq!(
        buckets[0].count(),
        100_000,
        "a count is an integer, and it is exact"
    );
    assert_eq!(
        buckets[0].value(),
        100_000.0,
        "reported as the number the aggregate is"
    );

    let summed = execute(&snapshot, &parsed(&text("sum")))
        .unwrap_or_else(|refusal| panic!("a query over this store is answerable: {refusal}"));
    let sums: Vec<f64> = mixed(&summed).iter().map(Bucket::value).collect();
    assert_eq!(
        sums,
        vec![4_999_950_000.0],
        "0 + 1 + ... + 99_999 exactly, which is what a hundred thousand additions have to be worth"
    );

    let averaged = execute(&snapshot, &parsed(&text("mean")))
        .unwrap_or_else(|refusal| panic!("a query over this store is answerable: {refusal}"));
    assert_eq!(
        mixed(&averaged)[0].value(),
        49_999.5,
        "and the mean of them"
    );
}

#[test]
fn a_zone_is_a_name_this_build_ships_or_a_fixed_offset() {
    let utc = Zone::parse("UTC").expect("`UTC` is a name this build ships");
    assert_eq!(utc.name(), "UTC");
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

    let berlin = Zone::parse("Europe/Berlin").expect("`Europe/Berlin` is a name this build ships");
    assert_eq!(berlin.name(), "Europe/Berlin");
    assert_eq!(
        berlin.covers(),
        (BERLIN_FROM, BERLIN_TO),
        "a shipped name is a table, and a table covers the span it was built with"
    );

    let plus = Zone::parse("+02:00").expect("a fixed offset is arithmetic a caller may write");
    assert_eq!(
        plus.name(),
        "+02:00",
        "and it is named the way the caller spelled it"
    );
    assert_eq!(plus.offset_at(at(BERLIN_FROM)), Some(2 * HOUR as i64));
    assert_eq!(
        plus.covers(),
        (0, u64::MAX),
        "a fixed offset is not a table and does not go stale"
    );

    let minus = Zone::parse("-05:30").expect("a fixed offset may have minutes in it");
    assert_eq!(
        minus.offset_at(at(BERLIN_FROM)),
        Some(-(5 * HOUR as i64 + 30 * MINUTE as i64)),
        "and it is the offset the caller wrote, sign and minutes included"
    );

    // The list of names is the whole of what a caller may write in place of an offset, and every
    // name on it answers as itself: a name added to the list and forgotten in the parser would be
    // a zone this build claims to know and answers UTC for.
    assert_eq!(
        Zone::SHIPPED.len(),
        2,
        "the names this build has tables for"
    );
    for name in Zone::SHIPPED {
        let zone = Zone::parse(name)
            .unwrap_or_else(|refusal| panic!("`{name}` is shipped, and it was refused: {refusal}"));
        assert_eq!(zone.name(), name, "a shipped name answers as itself");
    }
}

#[test]
fn a_name_nobody_shipped_is_the_callers_class() {
    // Every one of these is a value a caller typed, so every one of them is exit code `2` rather
    // than a guess: a `--zone Europe/Berlinn` that quietly answered UTC would be a dashboard a day
    // out that nobody can explain.
    for name in ["Europe/Berlinn", "utc", "Mars/Base", ""] {
        let refusal = Zone::parse(name).unwrap_err();
        assert_eq!(
            refusal.class(),
            2,
            "`{name}` is the caller's class: {refusal}"
        );
    }
    let unknown = Zone::parse("Mars/Base").unwrap_err().to_string();
    assert!(
        unknown.contains("UTC") && unknown.contains("Europe/Berlin"),
        "the sentence names the zones this build knows, which is what the caller has to choose \
         from: {unknown}"
    );

    // A fixed offset is `±HH:MM` and nothing else, and the refusal points at the character that is
    // wrong rather than at the value: `+2:00` and `+02:0` are different mistakes.
    for (name, at) in [
        ("+2:00", 2usize),
        ("02:00", 0),
        ("+0200", 3),
        ("+02:0", 5),
        ("+02:00:00", 6),
    ] {
        match Zone::parse(name) {
            Err(Refusal::Syntax { at: found, detail }) => assert_eq!(
                found, at,
                "`{name}` stops being an offset at character {at}, and the refusal has to say \
                 which one: {detail}"
            ),
            other => panic!("`{name}` is not a fixed offset, and it was accepted: {other:?}"),
        }
    }

    // The offsets the shape allows and the world does not: fourteen hours is as far as any zone has
    // ever been from UTC, and the minutes of an offset do not reach sixty.
    for name in ["+15:00", "-99:00", "+02:60"] {
        let refusal = Zone::parse(name).unwrap_err();
        assert_eq!(
            refusal.class(),
            2,
            "`{name}` is the caller's class: {refusal}"
        );
        assert!(
            refusal.to_string().contains("14:00"),
            "and the sentence names the bound it is over: {refusal}"
        );
    }
}

#[test]
fn the_offset_table_has_a_spring_forward_and_a_fall_back() {
    let berlin = Zone::parse("Europe/Berlin").expect("`Europe/Berlin` is a name this build ships");

    // The two transitions of 2024, asserted at the instant and one millisecond before it: an
    // offset table is a set of instants, and a case that only checked a summer date and a winter
    // date would pass for a table whose changes were days out.
    assert_eq!(
        berlin.offset_at(at(SPRING - 1)),
        Some(HOUR as i64),
        "an hour before the spring forward the zone is still +01:00"
    );
    assert_eq!(
        berlin.offset_at(at(SPRING)),
        Some(2 * HOUR as i64),
        "and at it, it is +02:00"
    );
    assert_eq!(
        berlin.offset_at(at(FALL - 1)),
        Some(2 * HOUR as i64),
        "the same the other way round: +02:00 one millisecond before the fall back"
    );
    assert_eq!(
        berlin.offset_at(at(FALL)),
        Some(HOUR as i64),
        "and +01:00 at it"
    );

    // A whole year of the table, so the case does not pass on a build that ships one transition.
    assert_eq!(
        berlin.offset_at(at(SPRING_DAY)),
        Some(HOUR as i64),
        "midnight local, +01:00"
    );
    assert_eq!(
        berlin.offset_at(at(SPRING + DAY)),
        Some(2 * HOUR as i64),
        "a day later, +02:00"
    );
}

#[test]
fn a_local_day_is_twenty_three_hours_once_a_year_and_twenty_five_the_other() {
    let berlin = Zone::parse("Europe/Berlin").expect("`Europe/Berlin` is a name this build ships");

    // The day a zone springs forward is short and the day it falls back is long: these are the two
    // numbers a dashboard counting in fixed 86 400-second steps gets wrong twice a year.
    assert_eq!(
        berlin.local_day(at(SPRING)),
        Some((SPRING_DAY, SPRING_DAY_END))
    );
    assert_eq!(
        SPRING_DAY_END - SPRING_DAY,
        23 * HOUR,
        "a spring-forward day is twenty-three hours long"
    );
    assert_eq!(berlin.local_day(at(FALL)), Some((FALL_DAY, FALL_DAY_END)));
    assert_eq!(
        FALL_DAY_END - FALL_DAY,
        25 * HOUR,
        "and a fall-back day is twenty-five"
    );

    // Every instant of the short day names the same day, on both sides of the change — which is
    // the whole of "a bucket is a function of the time it covers".
    for instant in [
        SPRING_DAY,
        SPRING - 1,
        SPRING,
        SPRING + HOUR,
        SPRING_DAY_END - 1,
    ] {
        assert_eq!(
            berlin.local_day(at(instant)),
            Some((SPRING_DAY, SPRING_DAY_END)),
            "the local day of {instant} is one day and not two"
        );
    }
    assert_eq!(
        berlin.local_day(at(SPRING_DAY - 1)),
        Some((SPRING_DAY - DAY, SPRING_DAY)),
        "and the day before ends exactly where the short one begins"
    );

    // UTC's local days are the absolute grid, which is why `--zone UTC` is the default and not a
    // mode; the hour a local day is short of a grid window is the hour the zone moved.
    let utc = Zone::parse("UTC").expect("`UTC` is a name this build ships");
    let day = Window::new(DAY).partition(SPRING as i64).start();
    assert_eq!(utc.local_day(at(SPRING)), Some((day, day + DAY)));
    assert_eq!(
        SPRING_DAY_END - SPRING_DAY,
        DAY - HOUR,
        "which is exactly the hour a local day is short of a grid window"
    );
    assert_ne!(
        day, SPRING_DAY,
        "a local day is not the window that happens to cover it: Berlin's midnight is an hour \
         before the grid's"
    );
}

#[test]
fn an_instant_the_table_does_not_cover_is_not_guessed() {
    let berlin = Zone::parse("Europe/Berlin").expect("`Europe/Berlin` is a name this build ships");

    // The table is a file that was built with a span, and outside it this build says so rather than
    // extrapolating a rule it does not have.
    assert_eq!(
        berlin.offset_at(at(BERLIN_FROM - 1)),
        None,
        "one millisecond before the span"
    );
    assert_eq!(
        berlin.offset_at(at(BERLIN_TO)),
        None,
        "and at the first instant after it"
    );
    assert_eq!(
        berlin.offset_at(0),
        None,
        "and 1970 is far outside a table that starts in 2024"
    );
    assert_eq!(berlin.local_day(at(BERLIN_FROM - DAY)), None);
    assert!(
        Zone::parse("+02:00")
            .expect("a fixed offset is a name this build accepts")
            .offset_at(0)
            .is_some(),
        "while a fixed offset is arithmetic and covers every instant"
    );

    // And the command line refuses such a range rather than answering it approximately: the class
    // is the caller's and the sentence says what the table does cover, because "this build does not
    // know that instant" is not something a caller can act on.
    let (dir, _store) = sealed("outside", &[hourly("cpu,host=h0", SPRING_RANGE_FROM, 1)]);
    let store = dir.argument();
    let outside = run(&query_arguments(
        &store,
        1_685_577_600_000,
        1_685_664_000_000,
        "1d",
        "count",
        Some("Europe/Berlin"),
    ));
    refused(&outside, 2, "a range the zone's table does not cover");
    assert!(
        outside.stderr.contains(&BERLIN_FROM.to_string())
            && outside.stderr.contains(&BERLIN_TO.to_string()),
        "the diagnosis names the span the table covers: {}",
        outside.stderr
    );
}

#[test]
fn the_command_line_counts_a_one_day_window_in_local_days() {
    // Three days of hourly samples around the spring-forward day, asked for once a day.
    let (_dir, store) = spring_store("local-days");
    let plain = run(&query_arguments(
        &store,
        SPRING_RANGE_FROM,
        SPRING_RANGE_TO,
        "1d",
        "count",
        None,
    ));
    assert_eq!(plain.code, 0, "{plain:?}");
    assert_eq!(plain.stderr, "", "an answer is not a diagnosis");
    assert_eq!(
        plain.stdout, "1711670400000 24 24\n1711756800000 24 24\n1711843200000 24 24\n",
        "on the absolute grid, three days of twenty-four hourly samples each"
    );

    // The same question in `Europe/Berlin`. The boundaries move, the counts move, and the day the
    // zone springs forward holds twenty-three samples — which is the whole reason `--zone` is not
    // a display setting.
    let berlin = run(&query_arguments(
        &store,
        SPRING_RANGE_FROM,
        SPRING_RANGE_TO,
        "1d",
        "count",
        Some("Europe/Berlin"),
    ));
    assert_eq!(berlin.code, 0, "{berlin:?}");
    assert_eq!(
        berlin.stdout,
        format!(
            "{} 23 23\n{} 24 24\n{} 23 23\n{} 2 2\n",
            SPRING_RANGE_FROM - HOUR,
            SPRING_RANGE_FROM + DAY - HOUR,
            SPRING_DAY,
            SPRING_DAY_END
        ),
        "four local days, the first and last partial because the range's edges are on the grid: \
         the day that springs forward holds twenty-three samples and the day it falls back holds \
         twenty-five"
    );

    // `--zone UTC` is the answer a caller who said nothing gets, which is what makes UTC a default
    // rather than a special case anywhere in the pass.
    let utc = run(&query_arguments(
        &store,
        SPRING_RANGE_FROM,
        SPRING_RANGE_TO,
        "1d",
        "count",
        Some("UTC"),
    ));
    assert_eq!(
        utc.stdout, plain.stdout,
        "`--zone UTC` and no zone are the same question"
    );

    // `--json` is a format and not a second answer: the same numbers, as one document.
    let mut documented = query_arguments(
        &store,
        SPRING_RANGE_FROM,
        SPRING_RANGE_TO,
        "1d",
        "count",
        Some("Europe/Berlin"),
    );
    documented.push("--json".to_owned());
    let as_json = run(&documented);
    assert_eq!(as_json.code, 0, "{as_json:?}");
    let document: Value = serde_json::from_str(&as_json.stdout)
        .unwrap_or_else(|error| panic!("`--json` prints one document: {error}"));
    let buckets = document
        .as_array()
        .unwrap_or_else(|| panic!("the flag form's `--json` is one array: {document}"));
    assert_eq!(buckets.len(), 4, "one object per local day");
    assert_eq!(buckets[0]["start"], Value::from(SPRING_RANGE_FROM - HOUR));
    assert_eq!(buckets[0]["count"], Value::from(23));
    assert_eq!(buckets[3]["start"], Value::from(SPRING_DAY_END));
    assert_eq!(buckets[3]["count"], Value::from(2));
}

#[test]
fn a_window_narrower_than_a_day_ignores_the_zone() {
    // An hour is an hour in every zone, and a rule that moved a shorter bucket would be a second
    // grid beside the one `lakelet/4.10-downsampling` argued for and `lakelet/4.9-retention`
    // counts in.
    let (_dir, store) = spring_store("narrow");
    let plain = run(&query_arguments(
        &store,
        SPRING_RANGE_FROM,
        SPRING_RANGE_TO,
        "1h",
        "mean",
        None,
    ));
    assert_eq!(plain.code, 0, "{plain:?}");
    assert!(
        !plain.stdout.is_empty(),
        "the query answers something, or the comparison below means nothing"
    );
    for zone in ["Europe/Berlin", "+02:00", "UTC"] {
        let zoned = run(&query_arguments(
            &store,
            SPRING_RANGE_FROM,
            SPRING_RANGE_TO,
            "1h",
            "mean",
            Some(zone),
        ));
        assert_eq!(
            zoned.stdout, plain.stdout,
            "`--zone {zone}` with a one-hour window is the same answer byte for byte"
        );
    }
}

#[test]
fn a_zone_is_a_flag_beside_the_store_and_the_two_forms_agree() {
    // `--zone` is not part of the question text, the way `--store` is not: `GROUP BY time(1d)`
    // says how wide a bucket is and not which midnight it starts at. So it is a flag in both forms,
    // and the two forms have to answer one local-day question the same way.
    let (_dir, store) = spring_store("both-forms");
    let text = format!(
        "SELECT count(value) FROM cpu WHERE ts BETWEEN {SPRING_RANGE_FROM} AND {SPRING_RANGE_TO} \
         GROUP BY time(1d)"
    );
    let flags = run(&query_arguments(
        &store,
        SPRING_RANGE_FROM,
        SPRING_RANGE_TO,
        "1d",
        "count",
        Some("Europe/Berlin"),
    ));
    let as_text = run(&text_arguments(&store, &text, Some("Europe/Berlin")));
    assert_eq!(as_text.code, 0, "{as_text:?}");
    assert_eq!(
        as_text.stdout, flags.stdout,
        "the two forms answer the same local-day question the same way, byte for byte"
    );

    // And with no zone named, in both forms: the default is UTC and it is on the absolute grid.
    let plain_flags = run(&query_arguments(
        &store,
        SPRING_RANGE_FROM,
        SPRING_RANGE_TO,
        "1d",
        "count",
        None,
    ));
    let plain_text = run(&text_arguments(&store, &text, None));
    assert_eq!(plain_text.stdout, plain_flags.stdout);

    // A range the named table does not cover is the caller's class in the text form too, and the
    // sentence is the engine's rather than the tool's: this file does not keep a second table of
    // what a wrong query exits with.
    let ancient = "SELECT count(value) FROM cpu WHERE ts BETWEEN 1685577600000 AND 1685664000000 \
                   GROUP BY time(1d)";
    refused(
        &run(&text_arguments(&store, ancient, Some("Europe/Berlin"))),
        2,
        "a range the zone's table does not cover, as a text query",
    );
}

#[test]
fn the_aggregation_sources_do_not_reach_into_the_service_and_the_usage_text_names_the_zone() {
    // The module this step adds is engine code, and the engine is what runs in a test, a repair
    // tool and a load generator — none of which has a configuration file or a signal handler.
    let engine = PathBuf::from("src/engine");
    let mut all: Vec<PathBuf> = Vec::new();
    collect_rust(&engine, &mut all).expect("the engine directory is part of the project");
    for file in &all {
        let source = std::fs::read_to_string(file).expect("a source file the project keeps");
        for forbidden in ["crate::service", "crate::cli"] {
            assert!(
                !source.contains(forbidden),
                "{} names `{forbidden}` — the daemon uses the engine, never the other way, and \
                 the file this step adds is where a new violation would appear \
                 (`lakelet/4.1-ingest-protocol` reads the same sources for it)",
                file.display()
            );
        }
    }

    // The module is on the query page, and the file this step is about is the contract the spec
    // compiles against: the declarations have to be there for the scaffold to be a scaffold.
    let query = PathBuf::from("src/engine/query");
    let mut files: Vec<PathBuf> = Vec::new();
    collect_rust(&query, &mut files).expect("the query module is part of the project");
    assert!(
        files.len() >= 8,
        "{} holds {} Rust file(s): the window pass, the grammar, its scanner, its parser, its \
         plan, its executor, and the floating-point and calendar arithmetic beside them",
        query.display(),
        files.len()
    );
    let module = std::fs::read_to_string("src/engine/query/mod.rs").unwrap_or_else(|error| {
        panic!(
            "src/engine/query/mod.rs cannot be read: {error} — it is the page a reader of the \
             query engine opens first"
        )
    });
    for name in [
        "agg",
        "downsample",
        "exec",
        "lex",
        "parse",
        "plan",
        "series",
    ] {
        assert!(
            module.contains(name),
            "the module does not name `{name}`, and this is the page the rest of the engine is \
             read from"
        );
    }
    let source = std::fs::read_to_string("src/engine/query/agg.rs").unwrap_or_else(|error| {
        panic!(
            "src/engine/query/agg.rs cannot be read: {error} — it is the file this step is about"
        )
    });
    assert!(
        source.len() > 2_000,
        "the arithmetic says something rather than holding a heading and a `todo!()`: {} byte(s)",
        source.len()
    );
    for declaration in [
        "pub struct Sum",
        "pub struct Zone",
        "pub const BOUND",
        "pub fn error_bound",
        "pub fn local_day",
    ] {
        assert!(
            source.contains(declaration),
            "the module states `{declaration}`, which is the surface this step's spec is written \
             against"
        );
    }

    // The usage text is a constant and a scaffold may only add files or insert lines, so the
    // `--zone` half of it is the learner's edit, and this is what pins it. `--help` is an answer
    // on stdout with exit `0`, as `jobq/3.15-cli-contract` established.
    assert!(
        USAGE.contains("--zone"),
        "the usage text names the flag this step adds, because a reader of `--help` is where the \
         calendar is discovered"
    );
    assert!(
        USAGE.contains("--sql"),
        "and it still names the form the step before it added"
    );
    let help = lakeletq(&["--help"]);
    assert_eq!(help.code, 0, "{help:?}");
    assert_eq!(
        help.stdout, USAGE,
        "and the binary writes the library's text"
    );
    assert_eq!(help.stderr, "");
}
