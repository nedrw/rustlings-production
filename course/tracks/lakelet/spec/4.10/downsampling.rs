//! Spec for `lakelet/4.10-downsampling`: what a question about the store is, and what its answer
//! may and may not say.
//!
//! `kiln run` copies this file into the project as `tests/spec_downsampling.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Seventeen cases, and the shape of them is the point:
//!
//! * **a window is a bucket of the absolute time axis.** `Window::partition` and
//!   `Window::partitions` are `Partition`'s arithmetic — the same `floor(ts / width) * width`, the
//!   same half-open `[start, end)` `lakelet/4.9-retention` snaps its cutoff with — so a boundary
//!   timestamp belongs to the window that starts on it, a range touches exactly the windows it
//!   overlaps, and an empty range touches none;
//! * **a bucket counts what is in the range, not what is in the window.** A range that starts and
//!   ends mid-window leaves the first and last bucket **partial**, and `Bucket::count()` is the
//!   honest number beside `value()`: there is no filling a partial window from outside the range,
//!   because an answer that read outside the question would be a different answer;
//! * **an empty window is absent, and a measured zero is not.** The two are asserted as a pair,
//!   because they are the two halves of one rule: a window with no records in the range does not
//!   appear at all, while a window whose records all measured `0.0` appears with the count that
//!   says so;
//! * **`series` is an exact name, and `None` is every series.** A near miss matches nothing, and
//!   the three counts an answer carries say what happened: what was scanned, what survived the
//!   series filter, and what landed in a bucket;
//! * **the tolerance is a number the code states.** The interesting comparison of this step — the
//!   mean of the buckets against the raw mean of the same records — goes through `agree`, and
//!   the case asserts both that the two differ and that they still agree, plus where `agree`
//!   starts disagreeing and why its scale has a floor;
//! * **the command line is the contract `jobq/3.15-cli-contract` established.** Help, in any of its
//!   spellings, is an answer on stdout with exit `0`, one exit code per failure class, `--json` as a
//!   format rather than a second command, and every failing call run **twice** — with and without
//!   `--json` — so the code and the diagnosis are compared rather than assumed;
//! * **the boundary holds.** `src/engine/**` may not name the service layer, and the query module
//!   is no exception; the binary on top of the library is a shell that decides about this process
//!   and nothing about the answer.
//!
//! Nothing here sleeps, nothing here reads a clock, and nothing here measures a duration: the
//! windows, the ranges and the `now` a retention pass is given are numbers every case passes in.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use lakelet::cli::{self, MAX_WINDOWS, USAGE};
use lakelet::engine::compaction::{Segments, file_of};
use lakelet::engine::query::downsample::{
    self, Aggregate, Answer, Bucket, Range, TOLERANCE, Window, agree,
};
use lakelet::engine::retention::Policy;
use lakelet::protocol::batch::Record;
use serde_json::Value;

/// Milliseconds in a minute: the window every case counts in, unless it builds one of its own.
const MINUTE: u64 = 60_000;

/// Milliseconds in an hour.
const HOUR: u64 = 3_600_000;

/// Milliseconds between two samples of a dense series: four a minute.
const STEP: u64 = 15_000;

/// The first timestamp of every case's workload, on an hour boundary.
///
/// `1_699_999_200_000` is `472_222 * HOUR` exactly: every window a case asks for therefore starts
/// on a number a reader can check by hand, which is the whole reason the grid is absolute.
const EPOCH: u64 = 472_222 * HOUR;

/// One record of `series`, `offset` milliseconds after [`EPOCH`].
fn record(series: &str, offset: u64, value: f64) -> Record {
    Record {
        series: series.to_owned(),
        ts_millis: (EPOCH + offset) as i64,
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

/// A store directory of this case's own, removed when it is dropped.
struct Dir {
    /// Where the store lives.
    path: PathBuf,
}

impl Dir {
    /// Claim a directory for `label`.
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lakelet-downsampling-spec-{label}-{}",
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

    /// Write `bytes` over the file `relative`, for the cases that damage a store on purpose.
    fn damage(&self, relative: &str, bytes: &[u8]) {
        let path = self.path.join(relative);
        std::fs::write(&path, bytes)
            .unwrap_or_else(|error| panic!("{} cannot be written: {error}", path.display()));
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
        "{what} must say what went wrong on stderr: an exit code on its own sends the reader to the \
         source"
    );
}

/// A command line for the library, out of the literals a case writes.
fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|argument| (*argument).to_owned()).collect()
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

#[test]
fn a_window_is_a_bucket_of_the_absolute_time_axis() {
    let hour = Window::new(HOUR);
    assert_eq!(
        hour.width_millis(),
        HOUR,
        "a window is the width it was made with"
    );

    // The boundary belongs to the window that starts on it, which is `Partition`'s half-open rule
    // and not a second one: this step reuses the arithmetic `lakelet/4.9-retention` snaps its
    // cutoff with, so a retention boundary and a query's bucket are the same number.
    let window = hour.partition(EPOCH as i64 + HOUR as i64);
    assert_eq!(
        window.start(),
        EPOCH + HOUR,
        "the boundary starts a window of its own"
    );
    assert_eq!(
        window.end(),
        EPOCH + 2 * HOUR,
        "and the next one ends one width later"
    );
    assert!(
        window.contains(EPOCH + HOUR),
        "a boundary timestamp is in it"
    );
    assert!(
        !window.contains(EPOCH + HOUR - 1),
        "and the millisecond before it is in the window before"
    );
    assert_eq!(
        hour.partition(EPOCH as i64 + HOUR as i64 - 1).start(),
        EPOCH,
        "which is the same window as the first millisecond of the hour"
    );

    // The grid is the absolute one: every window starts on a multiple of the width counted from
    // the epoch, whatever the data does. A window anchored at the first record would answer a
    // different `start` for the same timestamp depending on what else was in the range.
    for offset in [0, 1, HOUR - 1, HOUR, 7 * HOUR + 13] {
        let start = hour.partition(EPOCH as i64 + offset as i64).start();
        assert_eq!(
            start % HOUR,
            0,
            "a window starts on the absolute grid, and {start} is not a multiple of an hour"
        );
    }

    // A width that does not divide the timestamp: the window starts *below* it, as `floor` does.
    // The timestamps here are counted from the epoch itself, because `EPOCH` is not a multiple of
    // seven minutes and a case built on it would be asserting where this workload's grid happens to
    // fall instead of what a window does.
    let odd = Window::new(7 * MINUTE);
    assert_eq!(
        odd.partition(0).start(),
        0,
        "the epoch is a boundary of every width"
    );
    assert_eq!(
        odd.partition((3 * MINUTE) as i64).start(),
        0,
        "three minutes into a seven-minute window is still that window"
    );
    assert_eq!(
        odd.partition((7 * MINUTE - 1) as i64).start(),
        0,
        "and so is the millisecond before the next one starts"
    );
    assert_eq!(
        odd.partition((7 * MINUTE) as i64).start(),
        7 * MINUTE,
        "which is the first millisecond of the next window"
    );

    // Before the epoch there is no window: a record's timestamp is an `i64` this store converts,
    // and the conversion is the index's own — anything below 1970 falls in the first bucket
    // rather than in no bucket at all.
    assert_eq!(
        hour.partition(-1).start(),
        0,
        "a clock that was wrong lands in the first bucket"
    );
}

#[test]
fn the_windows_a_half_open_range_touches_are_the_ones_it_touches() {
    let window = Window::new(MINUTE);

    // A range that starts and ends mid-window touches every window between them, in order, and
    // nothing outside: this is the grid a gap can be counted against.
    let from = 2 * MINUTE + 3;
    let to = 5 * MINUTE + 7;
    let touched: Vec<u64> = window
        .partitions(from, to)
        .iter()
        .map(|partition| partition.start())
        .collect();
    assert_eq!(
        touched,
        vec![2 * MINUTE, 3 * MINUTE, 4 * MINUTE, 5 * MINUTE],
        "the range starts inside the third window and ends inside the sixth, and touches exactly \
         those four"
    );

    // Half-open at the far end: the millisecond at `to` belongs to the next question, so a range
    // that ends on a boundary does not touch the window that starts there.
    assert_eq!(
        window.partitions(0, MINUTE).len(),
        1,
        "[0, 1min) is one window"
    );
    assert_eq!(
        window.partitions(0, MINUTE + 1).len(),
        2,
        "and one millisecond past it reaches into the next"
    );
    assert_eq!(
        window.partitions(from, from + 1).len(),
        1,
        "a range inside one window"
    );

    // An empty range touches no window at all, rather than one window for the instant. The command
    // line refuses a range that ends before it starts, and the arithmetic answers it with nothing.
    assert!(
        window.partitions(5, 5).is_empty(),
        "a range with no milliseconds in it"
    );
    assert!(
        window.partitions(5 * MINUTE, 3).is_empty(),
        "and one that ends before it starts"
    );

    // A width of zero is not refused here, it is turned into the narrowest window there is — one
    // millisecond — which is also what stops `partitions` from walking for ever: the step is the
    // width the buckets were actually made with, not the number a caller typed (`Window::new`).
    let zero = Window::new(0);
    assert_eq!(
        zero.width_millis(),
        0,
        "the width is reported as it was given"
    );
    assert_eq!(
        zero.partition(EPOCH as i64).start(),
        EPOCH,
        "and a zero-width window is the narrowest real one, not a hole"
    );
    assert_eq!(
        zero.partitions(EPOCH, EPOCH + 3).len(),
        3,
        "three milliseconds are three windows, and the walk ends"
    );

    // A width that does not divide the range is the same arithmetic: no window is dropped and none
    // is invented at the edges.
    // The range here is twelve minutes and the window is seven: the first window starts below the
    // range and the last one ends above it, and neither edge invents a window the range does not
    // touch.
    let odd = Window::new(7 * MINUTE);
    let touched = odd.partitions(3 * MINUTE, 15 * MINUTE);
    assert_eq!(
        touched.len(),
        3,
        "twelve minutes of range, windows seven minutes wide"
    );
    assert_eq!(touched.first().map(|first| first.start()), Some(0));
    assert_eq!(touched.last().map(|last| last.start()), Some(14 * MINUTE));
}

#[test]
fn an_aggregate_is_one_of_five_names_and_a_near_miss_is_none() {
    assert_eq!(Aggregate::ALL.len(), 5, "five ways to summarize a window");
    assert_eq!(
        Aggregate::ALL.map(|aggregate| aggregate.name()),
        ["count", "sum", "mean", "min", "max"],
        "`ALL` is the five in the order the command line's `--agg` names them"
    );
    for aggregate in Aggregate::ALL {
        assert_eq!(
            Aggregate::parse(aggregate.name()),
            Some(aggregate),
            "a name this tool prints has to be a name it reads: `{}`",
            aggregate.name()
        );
    }

    // An exact, lower-case match: a near miss is a caller's typo, and a typo that answered
    // something else would be a plausible number for a question nobody asked.
    for unknown in ["Mean", "MEAN", "avg", "average", "counts", "count ", ""] {
        assert_eq!(
            Aggregate::parse(unknown),
            None,
            "`{unknown}` is not one of the five, and answering it anyway is how a dashboard gets \
             the wrong statistic"
        );
    }
}

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
    let window = Window::new(MINUTE);
    let answer = |aggregate| downsample::downsample(&records, None, range, window, aggregate);

    let counted = answer(Aggregate::Count);
    assert_eq!(
        counts(&counted),
        vec![4],
        "a count is how many records the window held"
    );
    assert_eq!(
        values(&counted),
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
    // same number and completely different facts, which is what `Bucket::count` is for.
    let one = downsample::downsample(
        &records,
        None,
        Range::new(EPOCH + 2 * STEP, EPOCH + 3 * STEP),
        window,
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
}

#[test]
fn a_range_that_starts_and_ends_mid_window_leaves_partial_buckets() {
    // Eight minutes of four samples a minute, so a window is not a whole number of anything the
    // range's edges can be trusted to agree with.
    let records = series("cpu,host=h0", 0, STEP, 8 * 4, |index| index as f64);
    let from = 2 * MINUTE + 30_000;
    let to = 6 * MINUTE + 30_000;
    let answer = downsample::downsample(
        &records,
        None,
        Range::new(EPOCH + from, EPOCH + to),
        Window::new(MINUTE),
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
        "the first bucket starts on the grid and not at the range's edge: a window is a bucket of \
         the time axis"
    );
    assert_eq!(
        counts(&answer),
        vec![2, 4, 4, 4, 2],
        "the first and last windows are partial, and the bucket counts what is in the range: two \
         samples in the first, four in each whole one, two in the last"
    );
    assert_eq!(
        answer.in_range(),
        16,
        "and the pass says how many records it bucketed, which is the buckets' own counts added up"
    );
    assert_eq!(
        answer.in_range(),
        counts(&answer).iter().sum::<u64>(),
        "a partial window is not filled from outside the range: an answer that reached past either \
         edge would be an answer to a different question"
    );
}

#[test]
fn an_empty_window_is_absent_and_a_measured_zero_is_present() {
    // Four minutes of dense samples, one minute of them missing, and one minute that measured
    // zero. The two are the two halves of one rule and they are asserted together, because the
    // bug is printing the same thing for both.
    let mut records = series("cpu,host=h0", 0, STEP, 4, |_| 5.0);
    records.extend(series("cpu,host=h0", MINUTE, STEP, 4, |_| 0.0));
    records.extend(series("cpu,host=h0", 3 * MINUTE, STEP, 4, |_| 7.0));

    let window = Window::new(MINUTE);
    let from = EPOCH;
    let to = EPOCH + 4 * MINUTE;
    let answer = downsample::downsample(
        &records,
        None,
        Range::new(from, to),
        window,
        Aggregate::Mean,
    );

    assert_eq!(
        starts(&answer),
        vec![EPOCH, EPOCH + MINUTE, EPOCH + 3 * MINUTE],
        "the second minute held only zeros and the third held nothing at all"
    );
    assert_eq!(
        values(&answer)[1],
        0.0,
        "a window whose records all measured zero is an answer: it is present, and its value is \
         zero"
    );
    assert_eq!(
        counts(&answer)[1],
        4,
        "with the count that says four records were looked at and they were all zero"
    );

    let touched = window.partitions(from, to).len();
    assert_eq!(touched, 4, "the range touches four windows");
    assert_eq!(
        touched - answer.buckets().len(),
        1,
        "and the one held nothing: an empty window is absent rather than present with a zero in \
         it, and the gap is what the range's windows minus the answer's buckets counts"
    );
    assert!(
        !starts(&answer).contains(&(EPOCH + 2 * MINUTE)),
        "so nothing in the answer claims anything about the minute nothing was reported in"
    );

    // The other end of the same rule: a range that held nothing at all is an empty answer rather
    // than an answer full of zeros.
    let nothing = downsample::downsample(
        &records,
        None,
        Range::new(EPOCH + 10 * MINUTE, EPOCH + 11 * MINUTE),
        window,
        Aggregate::Mean,
    );
    assert!(
        nothing.is_empty(),
        "there is nothing to draw, and nothing is what it says"
    );
    assert_eq!(nothing.in_range(), 0);
}

#[test]
fn a_series_filter_is_an_exact_name_and_none_is_every_series() {
    let mut records = series("cpu,host=h0", 0, STEP, 16, |index| index as f64);
    records.extend(series("cpu,host=h1", 0, STEP, 16, |index| {
        100.0 + index as f64
    }));
    let range = Range::new(EPOCH + MINUTE, EPOCH + 3 * MINUTE);
    let window = Window::new(MINUTE);

    let asked = downsample::downsample(
        &records,
        Some("cpu,host=h1"),
        range,
        window,
        Aggregate::Count,
    );
    assert_eq!(
        asked.scanned(),
        32,
        "every record is looked at: there is no index to skip one with until \
         `lakelet/4.14-index-pruning`"
    );
    assert_eq!(
        asked.series_matched(),
        16,
        "and the filter keeps the series that was asked for"
    );
    assert_eq!(asked.in_range(), 8, "of which eight are in the range");
    assert_eq!(
        counts(&asked),
        vec![4, 4],
        "four samples in each of two windows"
    );

    // `None` is every series, and then the three counts collapse into one number.
    let every = downsample::downsample(&records, None, range, window, Aggregate::Count);
    assert_eq!(
        every.scanned(),
        every.series_matched(),
        "with no filter nothing is dropped"
    );
    assert_eq!(
        counts(&every),
        vec![8, 8],
        "and each window holds both series' samples"
    );

    // A name that is *nearly* the one asked for matches nothing: folding one host's samples into
    // another's is the bug a loose filter hides, and an empty answer is at least an honest one.
    let near = downsample::downsample(
        &records,
        Some("cpu,host=h"),
        range,
        window,
        Aggregate::Count,
    );
    assert!(
        near.is_empty(),
        "`cpu,host=h` is not a series in this store"
    );
    assert_eq!(
        near.series_matched(),
        0,
        "and it matched no record, not nearly all of them"
    );
    assert_eq!(near.in_range(), 0);
}

#[test]
fn the_mean_of_the_buckets_agrees_with_the_raw_mean_of_the_range() {
    // Ten minutes of dense samples on a gentle slope, asked about from the middle of one window to
    // the *start* of another: the first bucket is partial and the last is whole, which is what
    // makes the two ways of answering different numbers rather than two sides of a symmetric case
    // cancelling out.
    let records = series("cpu,host=h0", 0, STEP, 10 * 4, |index| {
        10.0 + 0.1 * index as f64
    });
    let from = EPOCH + MINUTE + 30_000;
    let to = EPOCH + 8 * MINUTE;
    let window = Window::new(MINUTE);
    let answer = downsample::downsample(
        &records,
        None,
        Range::new(from, to),
        window,
        Aggregate::Mean,
    );

    let in_range: Vec<f64> = records
        .iter()
        .filter(|record| {
            let at = u64::try_from(record.ts_millis).unwrap_or(0);
            from <= at && at < to
        })
        .map(|record| record.value)
        .collect();
    let raw_mean = in_range.iter().sum::<f64>() / in_range.len() as f64;
    let bucket_mean = values(&answer).iter().sum::<f64>() / answer.buckets().len() as f64;
    let gap = (bucket_mean - raw_mean).abs();

    assert!(
        gap > 0.0,
        "the two ways of answering are the same number only when every window holds records with \
         the same mean, and this range's first and last window are partial: {bucket_mean} against \
         {raw_mean}"
    );
    assert!(
        gap < TOLERANCE * raw_mean.abs().max(1.0),
        "and the gap is inside the tolerance the code declares, not merely inside floating-point \
         slop: {gap} against TOLERANCE {TOLERANCE}"
    );
    assert!(
        agree(bucket_mean, raw_mean),
        "so `agree` says the downsampled answer is this answer: {bucket_mean} against {raw_mean}"
    );

    // And the comparison is not vacuous in either direction: an answer computed from the wrong
    // records does not fit inside the same tolerance.
    assert!(
        !agree(bucket_mean, raw_mean + raw_mean),
        "twice the raw mean is not the same answer, whatever the windows did"
    );
}

#[test]
fn the_tolerance_is_a_number_the_code_states() {
    // A declaration is only worth having if it is a number strictly between "always agree" and
    // "never agree", and both ends are visible in the comparison rather than only in the constant.
    let declared = TOLERANCE;
    assert!(
        declared > 0.0 && declared < 1.0,
        "a tolerance of zero can never compare two computed numbers, and one of one can never \
         disagree with anything: {declared}"
    );

    // Where it agrees and where it stops: a relative tolerance, so the same *fraction* is the same
    // answer far from zero and a different one next to it.
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
        "and the comparison scales with the magnitude, so a megabyte of samples is compared the \
         way a handful is"
    );

    // The floor: near zero there is nothing to scale by, and a purely relative test would call two
    // values a thousandth apart disagreeing — including the comparison this step makes most often.
    assert!(agree(0.0, 0.0), "two zeroes are the same answer");
    assert!(
        agree(0.0, TOLERANCE / 2.0),
        "and near zero the tolerance is absolute"
    );
    assert!(
        !agree(0.0, 1.0),
        "but zero and one are not the same answer at any scale"
    );

    // A value that is not a number is not an answer this can certify, and quietly certifying it is
    // how a broken aggregation reaches a dashboard looking healthy.
    assert!(
        !agree(f64::NAN, f64::NAN),
        "`NaN` does not even agree with itself"
    );
    assert!(!agree(1.0, f64::NAN), "and it agrees with nothing else");
}

#[test]
fn asking_for_help_is_an_answer_on_stdout_and_names_the_commands() {
    for asked in [vec![], vec!["--help"], vec!["-h"], vec!["help"]] {
        let ran = lakeletq(&asked);
        assert_eq!(
            ran.code, 0,
            "asking `{asked:?}` must not look like a failure: a script checking `$?` around a \
             `--help` is a script that thinks the tool is broken ({ran:?})"
        );
        assert_eq!(
            ran.stderr, "",
            "and nothing is a diagnosis here: the usage text is the answer"
        );
        assert_eq!(
            ran.stdout, USAGE,
            "the usage text is the library's, byte for byte"
        );
        assert!(
            ran.stdout.contains("query"),
            "`--help` must name the command: a command nobody can discover from the help text is a \
             command that exists only in a wiki"
        );
        assert!(
            ran.stdout.contains("--window") && ran.stdout.contains("--agg"),
            "and the flags a query takes: `{}`",
            ran.stdout
        );
    }

    let version = lakeletq(&["--version"]);
    assert_eq!(version.code, 0, "`--version` is an answer too: {version:?}");
    assert!(
        version.stdout.starts_with("lakeletq ")
            && version.stdout.contains(env!("CARGO_PKG_VERSION")),
        "`--version` prints the version this binary was built with: {}",
        version.stdout
    );
    assert!(
        version.stdout != USAGE,
        "and it is not the usage text: a script that greps for a version must not be handed the \
         whole grammar"
    );
}

#[test]
fn a_query_answers_one_line_per_bucket_in_both_shapes() {
    let batches = vec![
        series("cpu,host=h0", 0, STEP, 4 * 4, |index| index as f64),
        series("cpu,host=h0", 4 * MINUTE, STEP, 4 * 4, |index| {
            100.0 + index as f64
        }),
    ];
    let (dir, _store) = sealed("shapes", &batches);
    let store = dir.argument();
    let from = EPOCH + MINUTE + 30_000;
    let to = EPOCH + 8 * MINUTE + 30_000;
    let from_text = from.to_string();
    let to_text = to.to_string();
    let args = [
        "query",
        "--store",
        store.as_str(),
        "--from",
        from_text.as_str(),
        "--to",
        to_text.as_str(),
        "--window",
        "1m",
        "--agg",
        "mean",
    ];

    // What the library answers for the same question, out of the same records: the text shape is
    // that answer written down, and the two are compared rather than one of them being trusted.
    let records: Vec<Record> = batches.concat();
    let expected = downsample::downsample(
        &records,
        None,
        Range::new(from, to),
        Window::new(MINUTE),
        Aggregate::Mean,
    );

    let ran = lakeletq(&args);
    assert_eq!(
        ran.code, 0,
        "a question about a store that is there is answered: {ran:?}"
    );
    assert_eq!(ran.stderr, "", "with nothing on the diagnosis stream");
    let lines: Vec<&str> = ran.stdout.lines().collect();
    assert_eq!(
        lines.len(),
        expected.buckets().len(),
        "one line per bucket and nothing else: `{}`",
        ran.stdout
    );
    for (line, bucket) in lines.iter().zip(expected.buckets()) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(
            fields.len(),
            3,
            "a line is `start count value` and nothing more: `{line}`"
        );
        assert_eq!(
            fields[0],
            bucket.start().to_string(),
            "fields in that order: `{line}`"
        );
        assert_eq!(
            fields[1],
            bucket.count().to_string(),
            "count second: `{line}`"
        );
        let value: f64 = fields[2]
            .parse()
            .unwrap_or_else(|error| panic!("`{}` is not a value: {error}", fields[2]));
        assert!(
            agree(value, bucket.value()),
            "and the value third, printed so that reading it back gives the same number: `{line}` \
             against {}",
            bucket.value()
        );
    }

    // The same question as one JSON document, with the fields in the order a script reads them.
    let mut json_args = args.to_vec();
    json_args.push("--json");
    let document_ran = lakeletq(&json_args);
    assert_eq!(
        document_ran.code, 0,
        "`--json` is a format and not a second command: {document_ran:?}"
    );
    let document: Value = serde_json::from_str(&document_ran.stdout)
        .unwrap_or_else(|error| panic!("`--json` prints one document: {error}"));
    let shown = document
        .as_array()
        .unwrap_or_else(|| panic!("one array of buckets, and `{document}` is not an array"));
    assert_eq!(
        shown.len(),
        expected.buckets().len(),
        "one object per bucket"
    );
    for (object, bucket) in shown.iter().zip(expected.buckets()) {
        assert_eq!(
            object["start"],
            Value::from(bucket.start()),
            "start is the window's start"
        );
        assert_eq!(
            object["count"],
            Value::from(bucket.count()),
            "count is what went into it"
        );
        let value = object["value"]
            .as_f64()
            .unwrap_or_else(|| panic!("a value is a number, and `{object}` has none"));
        assert!(
            agree(value, bucket.value()),
            "and the value is the aggregate"
        );
    }
    let start_at = document_ran
        .stdout
        .find("\"start\"")
        .unwrap_or_else(|| panic!("the shape names `start`: {}", document_ran.stdout));
    let count_at = document_ran
        .stdout
        .find("\"count\"")
        .unwrap_or_else(|| panic!("and `count`: {}", document_ran.stdout));
    let value_at = document_ran
        .stdout
        .find("\"value\"")
        .unwrap_or_else(|| panic!("and `value`: {}", document_ran.stdout));
    assert!(
        start_at < count_at && count_at < value_at,
        "the fields are written in the order the contract names them, in both shapes"
    );
    assert!(
        serde_json::from_str::<Value>(&ran.stdout).is_err(),
        "and the text shape is not a JSON document — a script that asked for a machine-readable \
         answer gets one, and a script that did not gets lines to read: `{}`",
        ran.stdout
    );
}

#[test]
fn an_empty_answer_is_no_line_or_an_empty_document() {
    let (dir, _store) = sealed("empty", &[series("cpu,host=h0", 0, STEP, 4, |_| 1.0)]);
    let store = dir.argument();
    // A range the store has nothing in, which is an answer rather than a failure: this store has a
    // floor of nothing and a query is not refused for asking about a quiet hour.
    let from = (EPOCH + 10 * HOUR).to_string();
    let to = (EPOCH + 11 * HOUR).to_string();
    let args = [
        "query",
        "--store",
        store.as_str(),
        "--from",
        from.as_str(),
        "--to",
        to.as_str(),
        "--window",
        "5m",
        "--agg",
        "max",
    ];

    let text = lakeletq(&args);
    assert_eq!(
        text.code, 0,
        "an hour nothing happened in is an answer: {text:?}"
    );
    assert_eq!(
        text.stdout, "",
        "and in the text shape it is no line at all"
    );
    assert_eq!(text.stderr, "");

    let mut json_args = args.to_vec();
    json_args.insert(1, "--json");
    let json = lakeletq(&json_args);
    assert_eq!(json.code, 0, "{json:?}");
    assert_eq!(
        json.stdout.trim_end(),
        "[]",
        "an empty answer is `[]` rather than nothing at all: a script that has to special-case no \
         output gets it wrong once and stops trusting the tool"
    );
}

/// The arguments of one `query`, in the shape the tool's usage writes them.
fn query_arguments<'a>(
    store: &'a str,
    from: &'a str,
    to: &'a str,
    window: &'a str,
    agg: &'a str,
) -> Vec<&'a str> {
    vec![
        "query", "--store", store, "--from", from, "--to", to, "--window", window, "--agg", agg,
    ]
}

#[test]
fn a_call_the_tool_cannot_understand_is_the_callers_class() {
    let (dir, _store) = sealed("usage", &[series("cpu,host=h0", 0, STEP, 4, |_| 1.0)]);
    let store = dir.argument();
    let store = store.as_str();

    // Every one of these is a caller's mistake, and each is answered before a store is opened:
    // `2` means the script's problem wherever the deployment happens to be. The list is the whole
    // of what a caller can get wrong, one line each, and the two that are about the *window* are
    // the ones this step adds to `jobq/3.15-cli-contract`'s grammar.
    let cases: Vec<(Vec<&str>, &str)> = vec![
        (vec!["frobnicate"], "an unknown command"),
        (vec!["--kiln-exit-code-probe"], "an unknown argument"),
        (
            vec![
                "query", "--from", "0", "--to", "60000", "--window", "1s", "--agg", "mean",
            ],
            "a query that never says which store",
        ),
        (vec!["query", "--store"], "a flag with nothing after it"),
        (
            vec![
                "query", "--store", store, "--to", "60000", "--window", "1s", "--agg", "mean",
            ],
            "a query with no `--from`",
        ),
        (
            vec![
                "query", "--store", store, "--from", "0", "--window", "1s", "--agg", "mean",
            ],
            "a query with no `--to`",
        ),
        (
            vec![
                "query", "--store", store, "--from", "0", "--to", "60000", "--agg", "mean",
            ],
            "a query with no `--window`",
        ),
        (
            vec![
                "query", "--store", store, "--from", "0", "--to", "60000", "--window", "1s",
            ],
            "a query with no `--agg`",
        ),
        (
            vec![
                "query", "--store", store, "--from", "soon", "--to", "60000", "--window", "1s",
                "--agg", "mean",
            ],
            "a `--from` that is not a number of milliseconds",
        ),
        (
            vec![
                "query", "--store", store, "--from", "5", "--to", "5", "--window", "1s", "--agg",
                "mean",
            ],
            "a range that ends where it starts",
        ),
        (
            vec![
                "query", "--store", store, "--from", "60000", "--to", "5", "--window", "1s",
                "--agg", "mean",
            ],
            "the two flags typed the wrong way round",
        ),
        (
            vec![
                "query", "--store", store, "--from", "0", "--to", "60000", "--window", "0",
                "--agg", "mean",
            ],
            "a window of zero, which has no bucket in it",
        ),
        (
            vec![
                "query", "--store", store, "--from", "0", "--to", "60000", "--window", "60",
                "--agg", "mean",
            ],
            "a width with no unit",
        ),
        (
            vec![
                "query", "--store", store, "--from", "0", "--to", "60000", "--window", "90x",
                "--agg", "mean",
            ],
            "a unit nobody has",
        ),
        (
            vec![
                "query", "--store", store, "--from", "0", "--to", "60000", "--window", "1s",
                "--agg", "average",
            ],
            "an aggregate nobody has",
        ),
        (
            vec![
                "query", "--store", store, "--from", "0", "--to", "60000", "--window", "1s",
                "--agg", "mean", "--store", store,
            ],
            "a flag given twice",
        ),
        (
            vec![
                "query",
                "--store",
                store,
                "--from",
                "0",
                "--to",
                "18446744073709551615",
                "--window",
                "1ms",
                "--agg",
                "mean",
            ],
            "a range that asks for more buckets than any tool should answer",
        ),
    ];

    for (args, what) in &cases {
        let text = lakeletq(args);
        refused(&text, 2, what);
        // The same call with `--json`, which is a format and not a second command: the code and
        // the diagnosis have to be identical, and the drift between two code paths is always in
        // the failure handling.
        let mut with_json = args.clone();
        with_json.push("--json");
        let with_json = lakeletq(&with_json);
        refused(&with_json, 2, what);
        assert_eq!(
            with_json.code, text.code,
            "`--json` is a format and not a second command: `{args:?}` exits the same either way"
        );
        assert_eq!(
            with_json.stderr.trim(),
            text.stderr.trim(),
            "and the diagnosis does not depend on how the answer would have been printed"
        );
    }

    // A refusal names the thing it refused, so a reader does not have to guess which of five flags
    // they spelled wrong.
    for (args, named) in [
        (vec!["frobnicate"], "frobnicate"),
        (vec!["--kiln-exit-code-probe"], "--kiln-exit-code-probe"),
        (
            vec![
                "query", "--from", "0", "--to", "1", "--window", "1s", "--agg", "mean",
            ],
            "--store",
        ),
        (
            vec![
                "query", "--store", store, "--from", "0", "--to", "1", "--window", "1s", "--agg",
                "average",
            ],
            "average",
        ),
    ] {
        let ran = lakeletq(&args);
        assert!(
            ran.stderr.contains(named),
            "`{args:?}` has to name `{named}` in its diagnosis: a tool that says `invalid argument` \
             sends the reader back to their script to guess which one, and it said `{}`",
            ran.stderr
        );
    }

    // The last caller mistake is the one a store cannot be judged by: the range and the window
    // between them ask for more buckets than this tool answers, and the count is arithmetic, so it
    // is refused before a store is opened and before a million buckets are allocated. The boundary
    // itself is answered rather than guessed about.
    let widest = MAX_WINDOWS * MINUTE;
    let accepted = lakeletq(&query_arguments(
        store,
        "0",
        &widest.to_string(),
        "1m",
        "count",
    ));
    assert_eq!(
        accepted.code, 0,
        "exactly as many buckets as the tool answers is a question it answers: {accepted:?}"
    );
    let one_past = lakeletq(&query_arguments(
        store,
        "0",
        &(widest + 1).to_string(),
        "1m",
        "count",
    ));
    refused(&one_past, 2, "one bucket past what the tool answers");
}

#[test]
fn a_store_that_is_not_there_a_store_that_refused_and_a_broken_deployment() {
    let from = EPOCH.to_string();
    let to = (EPOCH + HOUR).to_string();

    // `3`: the thing asked about is not there. `Segments::open` would *create* the directory, which
    // is right for a daemon and wrong for a read — a typo in `--store` would leave an empty store
    // behind and answer `0` with no lines — so the tool checks first.
    let missing = lakeletq(&query_arguments(
        "/nowhere/lakelet",
        &from,
        &to,
        "1m",
        "count",
    ));
    refused(&missing, 3, "a store directory that is not there");

    // `4`: the store refused. A manifest that is not this build's is the fast half of that class —
    // the bytes are read, found wanting, and named — and it is the same class a segment that cannot
    // be read answers with.
    let damaged = Dir::new("damaged");
    damaged.damage(
        "segments.manifest",
        b"not a manifest, not this build's, not a store",
    );
    let refused_manifest = lakeletq(&query_arguments(
        &damaged.argument(),
        &from,
        &to,
        "1m",
        "count",
    ));
    refused(&refused_manifest, 4, "a manifest this build cannot read");

    let (truncated, store) = sealed(
        "truncated",
        &[series("cpu,host=h0", 0, MINUTE, 60, |_| 1.0)],
    );
    let index = store
        .manifest()
        .segments()
        .first()
        .map(|info| info.index)
        .unwrap_or_else(|| panic!("a store sealed with sixty records names one segment"));
    let segment = file_of(truncated.path(), index);
    std::fs::write(&segment, b"half a segment")
        .unwrap_or_else(|error| panic!("{} cannot be written: {error}", segment.display()));
    let refused_segment = lakeletq(&query_arguments(
        &truncated.argument(),
        &from,
        &to,
        "1m",
        "count",
    ));
    refused(&refused_segment, 4, "a segment this build cannot read");

    // `1`: this deployment is broken, rather than the call. A file where a store should be is
    // neither a typo in a path nor something a store can refuse: nothing about the call is wrong
    // and nothing can be answered either, and telling that apart from `3` is the whole reason the
    // two numbers are different.
    let file = Dir::new("not-a-store");
    file.damage("a-file", b"this is not a directory");
    let broken = lakeletq(&query_arguments(
        &file.path().join("a-file").display().to_string(),
        &from,
        &to,
        "1m",
        "count",
    ));
    refused(&broken, 1, "a path that exists and is not a directory");

    // Each of the three is also checked with `--json`, because a format must not move a code, and
    // the three stay three: one exit code per failure class is the whole of the table a script
    // branches on.
    for (ran, store, what) in [
        (
            &missing,
            "/nowhere/lakelet".to_owned(),
            "a store that is not there",
        ),
        (
            &refused_manifest,
            damaged.argument(),
            "a store that refused",
        ),
        (
            &broken,
            file.path().join("a-file").display().to_string(),
            "a deployment that is broken",
        ),
    ] {
        let mut args = query_arguments(&store, &from, &to, "1m", "count");
        args.push("--json");
        let with_json = lakeletq(&args);
        assert_eq!(
            with_json.code, ran.code,
            "`--json` does not change the class of {what}: {with_json:?}"
        );
        assert_eq!(with_json.stdout, "", "and prints nothing on stdout");
        assert!(
            !with_json.stderr.trim().is_empty(),
            "and says why on stderr"
        );
    }
    let codes = [missing.code, refused_manifest.code, broken.code];
    assert_eq!(
        codes,
        [3, 4, 1],
        "a store that is not there, a store that refused and a deployment that is broken are three \
         different answers to a script: {codes:?}"
    );
}

#[test]
fn the_grammar_is_the_librarys_and_the_binary_only_writes_it() {
    let (dir, _store) = sealed(
        "shell",
        &[series("cpu,host=h0", 0, STEP, 8, |index| index as f64)],
    );
    let store = dir.argument();

    let in_process = cli::run(argv(&["--help"]));
    assert_eq!(in_process.code(), 0, "help is an answer");
    assert_eq!(
        in_process.stdout(),
        USAGE,
        "the usage text is the library's"
    );
    assert_eq!(in_process.stderr(), "", "and help is not a diagnosis");
    let process = lakeletq(&["--help"]);
    assert_eq!(
        process.stdout,
        in_process.stdout(),
        "the binary writes exactly what the library answered: a second copy of the grammar in the \
         binary is a second grammar"
    );
    assert_eq!(process.code, i32::from(in_process.code()));

    let probe = cli::run(argv(&["--kiln-exit-code-probe"]));
    assert_eq!(probe.code(), 2, "a usage error is the caller's class");
    assert_eq!(probe.stdout(), "", "with nothing on the answer stream");
    assert!(
        probe.stderr().contains("--kiln-exit-code-probe"),
        "and the argument's name in the diagnosis: {}",
        probe.stderr()
    );

    // And a real question, asked both ways. This is what makes "the binary only writes it" a fact
    // rather than a comment: the same bytes come out of a process and out of a function.
    let from = EPOCH.to_string();
    let to = (EPOCH + 2 * MINUTE + 30_000).to_string();
    let args = query_arguments(&store, &from, &to, "1m", "count");
    let asked = cli::run(argv(&args));
    let ran = lakeletq(&args);
    assert!(
        !asked.stdout().is_empty(),
        "this question has an answer, or the comparison below would be two empty strings"
    );
    assert_eq!(
        asked.stdout(),
        ran.stdout,
        "the library's answer and the process's are the same bytes"
    );
    assert_eq!(i32::from(asked.code()), ran.code);
}

#[test]
fn a_store_that_has_forgotten_something_answers_what_it_still_keeps() {
    // Four hours of one series, a sample a minute, sealed an hour to a segment.
    let batches: Vec<Vec<Record>> = (0..4u64)
        .map(|hour| series("cpu,host=h0", hour * HOUR, MINUTE, 60, |_| 1.0))
        .collect();
    let (dir, mut store) = sealed("floor", &batches);
    let store_path = dir.argument();

    // A retention pass that keeps one hour, at a `now` this case passes in
    // (`lakelet/4.9-retention`): the store forgets the first three hours and writes the floor down.
    let policy = Policy::new(HOUR, HOUR);
    store
        .expire(&policy, EPOCH + 4 * HOUR)
        .unwrap_or_else(|error| {
            panic!("a store whose segments all state a time range carries a pass out: {error}")
        });
    let floor = store.floor();
    assert_eq!(
        floor,
        EPOCH + 3 * HOUR,
        "the cutoff is snapped to a window boundary, so the floor is one of this query's own \
         windows"
    );

    // The range asks from the epoch, which the store has forgotten. That is an answer and not a
    // failure: the tool says what the store still keeps, and `lakelet/4.11-query-engine` is where a
    // query *refuses* a range the store cannot answer instead.
    let from = EPOCH.to_string();
    let to = (EPOCH + 4 * HOUR).to_string();
    let ran = lakeletq(&query_arguments(&store_path, &from, &to, "1h", "count"));
    assert_eq!(
        ran.code, 0,
        "an answer that begins later than `--from` is still an answer: {ran:?}"
    );
    assert_eq!(ran.stderr, "", "and nothing about it is a diagnosis");
    let lines: Vec<&str> = ran.stdout.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "one hour is left, so one bucket: `{}`",
        ran.stdout
    );
    let fields: Vec<&str> = lines[0].split_whitespace().collect();
    assert_eq!(
        fields[0],
        floor.to_string(),
        "the answer begins at the floor — the window the store has forgotten up to — rather than at \
         the range's edge: `{}`",
        ran.stdout
    );
    assert!(floor > EPOCH, "and the range asked from the epoch");
    assert_eq!(fields[1], "60", "sixty samples of the hour the store kept");
}

#[test]
fn the_query_sources_do_not_reach_into_the_service() {
    // The module this step adds is engine code: it runs in a test, in a repair tool and in a load
    // generator, none of which has a configuration file or a signal handler.
    let query = PathBuf::from("src/engine/query");
    let mut files: Vec<PathBuf> = Vec::new();
    collect_rust(&query, &mut files).expect("the query module is part of the project");
    assert!(
        files.len() >= 2,
        "{} holds {} Rust file(s): the module tree and the window arithmetic are what this step is \
         made of",
        query.display(),
        files.len()
    );
    for file in &files {
        let source = std::fs::read_to_string(file).expect("a source file the project keeps");
        for forbidden in ["crate::service", "crate::cli"] {
            assert!(
                !source.contains(forbidden),
                "{} names `{forbidden}` — the daemon uses the engine, never the other way, and the \
                 files this step adds are where a new violation would appear \
                 (`lakelet/4.1-ingest-protocol` reads the same sources for it)",
                file.display()
            );
        }
    }

    // The rule is `src/engine/**`'s and this module is no exception: every file below the engine
    // still reads as engine code. The binaries are not read this way — a shell may name whatever it
    // runs — which is why the walk starts at `src/engine` and not at `src`.
    let engine = PathBuf::from("src/engine");
    let mut all: Vec<PathBuf> = Vec::new();
    collect_rust(&engine, &mut all).expect("the engine directory is part of the project");
    assert!(
        all.len() > files.len(),
        "the query module is one part of the engine and not all of it: {} file(s) against {}",
        all.len(),
        files.len()
    );
    for file in &all {
        let source = std::fs::read_to_string(file).expect("a source file the project keeps");
        assert!(
            !source.contains("crate::service"),
            "{} names the service layer: the engine is what runs in a test, a repair tool and a \
             load generator",
            file.display()
        );
    }

    // The module a reader opens first names the things this step is about: a rule, a window and the
    // pass that answers are what a query is made of, and a file that stopped naming them would be a
    // file nobody can find the rule in.
    let module = std::fs::read_to_string("src/engine/query/mod.rs").unwrap_or_else(|error| {
        panic!(
            "src/engine/query/mod.rs cannot be read: {error} — this is the file a reader of the \
             query engine opens first"
        )
    });
    for name in ["downsample", "Window", "records", "store"] {
        assert!(
            module.contains(name),
            "the module does not name `{name}`, and this is the page the rest of the engine is read \
             from"
        );
    }

    // And the binary on top of it is a shell: it asks `lakelet::cli` and writes what it answered.
    // A binary that named the store or the window arithmetic would be a second place where the
    // answer is decided, and a second grammar is the bug `jobq/3.15-cli-contract` is about.
    let shell = std::fs::read_to_string("src/bin/lakeletq.rs").unwrap_or_else(|error| {
        panic!("src/bin/lakeletq.rs cannot be read: {error} — `lakeletq` is what a script runs")
    });
    assert!(
        shell.contains("cli::run"),
        "the binary calls the library's grammar: `{shell}`"
    );
    for absent in ["Segments", "downsample", "snapshot"] {
        assert!(
            !shell.contains(absent),
            "src/bin/lakeletq.rs names `{absent}`: the shell decides about this process — its \
             arguments, its two streams, its exit code — and nothing about the answer"
        );
    }
}
