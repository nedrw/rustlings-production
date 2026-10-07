//! Spec for `lakelet/4.14-index-pruning`: a sidecar index that may cost a read and never a record.
//!
//! `kiln run` copies this file into the project as `tests/spec_index_pruning.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Twenty-two cases, and the shape of them is the point:
//!
//! * **a zone answers "may" and never "does".** The bounds of a segment's timestamps and values are a
//!   *value*, and every rule about it leans one way: a false positive costs a file read, a false
//!   negative would cost a record. A bound nobody stated answers `true` to everything, and so does a
//!   zone built from no records at all;
//! * **a bloom filter has no false negatives.** Every term that went into a filter answers
//!   `may_contain`, ten thousand terms in a thousand bits make it answer `yes` to everything (the
//!   documented price of a fixed size, and not a defect), and the rate for a vocabulary a segment
//!   really has is a number this file measures rather than assumes;
//! * **the index is derived state, and that is the central rule.** A store whose index directory is
//!   deleted answers exactly what it answered, having opened more segments; a store whose index files
//!   are scribbled over does too; and the counters say *which* of the two happened
//!   (`index_missing`, `index_unusable`) instead of the answer changing. One scribbled byte is a
//!   damaged hint and not a plausible lie: the seal over every byte above it is what says so;
//! * **the writer writes it in the one order that cannot hurt.** A flush writes the segment's index
//!   after the file and before the manifest names the segment; a merge writes the **union** of its
//!   inputs' indexes without reading a series name; an input whose index is gone makes the merged
//!   index state nothing, because stating nothing prunes nothing and a guess would prune records; and
//!   an expired segment's index file is left behind as garbage, named by an ordinal nothing names any
//!   more and therefore never consulted;
//! * **the reader prunes twice and counts it twice.** The manifest's stated span rules a segment out
//!   of a range (`segments_pruned`, `lakelet/4.11-query-engine`), the index rules one out of the
//!   segments a range *meets* (`blocks_pruned`, this step), `EXPLAIN` prints both beside the twelve
//!   keys it always had, and a selective predicate is what makes the difference visible;
//! * **the boundary holds.** `src/engine/index/**` is engine code (it may not name the service layer,
//!   read out of the sources as text, `jobq/3.1-engine-boundary`), the index lives in a directory of
//!   its own rather than in `segments/` (`lakelet/4.8-compaction` counts the files there and deletes
//!   what the manifest does not name), and the segment file's own layout is untouched — its `VERSION`
//!   is still 1 and the index carries a magic of its own.
//!
//! Nothing here sleeps, nothing here reads a clock, and nothing here measures a duration: every
//! timestamp, range and cutoff is a number the case passes in.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use lakelet::cli::MAX_WINDOWS;
use lakelet::engine::compaction::{self, Policy as MergePolicy, Segments, Snapshot, file_of};
use lakelet::engine::index::bloom::{BITS as BLOOM_BITS, Bloom};
use lakelet::engine::index::zone::{self, Zone};
use lakelet::engine::index::{self, Lookup, SegmentIndex};
use lakelet::engine::query::exec::{self, Outcome};
use lakelet::engine::query::parse::{Query, Statement, parse};
use lakelet::engine::query::plan::Plan;
use lakelet::engine::retention::Policy;
use lakelet::engine::segment;
use lakelet::protocol::batch::Record;
use serde_json::Value;

/// Milliseconds in a minute.
const MINUTE: u64 = 60_000;

/// Milliseconds in an hour: the width of one sealed segment in most of these cases.
const HOUR: u64 = 3_600_000;

/// Milliseconds between two samples of a series.
const STEP: u64 = 60_000;

/// The first timestamp of every case's workload, on an hour boundary.
///
/// `1_699_999_200_000` is `472_222 * HOUR` exactly, so every range, span and cutoff below starts on a
/// number a reader can check by hand.
const EPOCH: u64 = 472_222 * HOUR;

/// How many segments the pruning cases build: one hour each, one series each.
const SEGMENTS: u64 = 12;

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

/// The records of one segment of the pruning workload: its own host, one sample a minute.
fn hour(segment: u64) -> Vec<Record> {
    series(
        &format!("cpu,host=h{segment}"),
        segment * HOUR,
        STEP,
        30,
        |index| index as f64,
    )
}

/// The whole pruning workload: [`SEGMENTS`] hours of one series each, `h0` first.
fn workload() -> Vec<Vec<Record>> {
    (0..SEGMENTS).map(hour).collect()
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
            "lakelet-index-pruning-spec-{label}-{}",
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

    /// The names of the files in one of the store's directories, sorted.
    fn listing(&self, directory: &str) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.path.join(directory))
            .unwrap_or_else(|error| panic!("{} cannot be listed: {error}", self.path.display()))
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// The names of the files in the store's segment directory.
    fn segment_files(&self) -> Vec<String> {
        self.listing(compaction::DIRECTORY)
    }

    /// The names of the files in the store's index directory.
    fn index_files(&self) -> Vec<String> {
        self.listing(index::DIRECTORY)
    }

    /// Overwrite one of the store's index files with `bytes`, for the cases that damage one on
    /// purpose.
    fn scribble(&self, ordinal: u64, bytes: &[u8]) {
        let path = index::file_of(&self.path, ordinal);
        std::fs::write(&path, bytes)
            .unwrap_or_else(|error| panic!("{} cannot be written: {error}", path.display()));
    }

    /// The bytes of one of the store's index files, as they are on the disk.
    fn index_bytes(&self, ordinal: u64) -> Vec<u8> {
        let path = index::file_of(&self.path, ordinal);
        std::fs::read(&path)
            .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()))
    }

    /// Delete the whole index directory, which is the loss a store recovers from by reading more.
    fn lose_the_index(&self) {
        std::fs::remove_dir_all(self.path.join(index::DIRECTORY)).unwrap_or_else(|error| {
            panic!(
                "{} cannot be removed: {error} — this case is about a store that lost its index",
                self.path.display()
            )
        });
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A store in a directory of its own, sealed from `batches` in order.
fn sealed(label: &str, batches: &[Vec<Record>]) -> (Dir, Segments) {
    sealed_with(label, MergePolicy::DEFAULT, batches)
}

/// The same, with the merge policy the case wants to plan with.
fn sealed_with(label: &str, policy: MergePolicy, batches: &[Vec<Record>]) -> (Dir, Segments) {
    let dir = Dir::new(label);
    let mut store = Segments::with_policy(dir.path(), policy)
        .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
    for batch in batches {
        store
            .seal(batch)
            .unwrap_or_else(|error| panic!("a segment of a case's own records is closed: {error}"));
    }
    (dir, store)
}

/// The query one text parses to.
fn parsed(text: &str) -> Query {
    match parse(text) {
        Ok(Statement::Query(query)) | Ok(Statement::Explain(query)) => query,
        Err(refusal) => {
            panic!("`{text}` is a query this grammar has, and it was refused: {refusal}")
        }
    }
}

/// The plan one text makes against one snapshot.
fn planned(snapshot: &Snapshot, text: &str) -> Plan {
    Plan::of(snapshot, &parsed(text))
        .unwrap_or_else(|refusal| panic!("`{text}` is a plan against this store: {refusal}"))
}

/// What one query text answered, in process.
fn asked(snapshot: &Snapshot, text: &str) -> Outcome {
    exec::execute(snapshot, &parsed(text))
        .unwrap_or_else(|refusal| panic!("`{text}` is answerable against this store: {refusal}"))
}

/// The answers of an outcome as lines a case can compare: series, window, count and value, one per
/// bucket — the shape the binary prints, and nothing about what the query cost.
fn answers_of(outcome: &Outcome) -> String {
    let mut text = String::new();
    for answer in outcome.answers() {
        for bucket in answer.buckets() {
            let series = answer.series().unwrap_or("mixed");
            text.push_str(&format!(
                "{series} {} {} {}\n",
                bucket.start(),
                bucket.count(),
                bucket.value()
            ));
        }
    }
    text
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

/// The command line that asks `text` about the store at `store`.
fn sql<'a>(store: &'a str, text: &'a str) -> Vec<&'a str> {
    vec!["query", "--store", store, "--sql", text]
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

/// One source file of the project, as text.
fn source(relative: &str) -> String {
    std::fs::read_to_string(relative).unwrap_or_else(|error| {
        panic!("{relative} cannot be read: {error} — this case is about it")
    })
}

#[test]
fn a_zone_states_the_bounds_of_every_column_it_maps() {
    // Five records: timestamps a second apart, values that go up and come back.
    let records = vec![
        record("cpu,host=h0", 0, 5.0),
        record("cpu,host=h0", 1_000, -2.5),
        record("cpu,host=h0", 2_000, 17.0),
        record("cpu,host=h0", 3_000, 0.0),
        record("cpu,host=h0", 4_000, 3.0),
    ];
    let zone = Zone::of(&records);
    assert_eq!(
        zone.first_millis(),
        Some(EPOCH),
        "the earliest timestamp of the records this zone was built from"
    );
    assert_eq!(zone.last_millis(), Some(EPOCH + 4_000));
    assert_eq!(
        zone.min_value(),
        Some(-2.5),
        "and the smallest and largest value, which is the column a manifest states nothing about"
    );
    assert_eq!(zone.max_value(), Some(17.0));
    assert_eq!(
        zone.to_string(),
        format!("ts=[{},{}] value=[-2.5,17]", EPOCH, EPOCH + 4_000),
        "the one line a log or a plan prints: both columns, both bounds, in the order the layout \
         writes them"
    );

    // One record is a span of no width, and a set whose values are all equal has the same shape: the
    // bounds are the record itself, which is what makes `may_contain` exact at both ends.
    let single = Zone::of(&[record("cpu,host=h0", 7, 2.0)]);
    assert_eq!(single.first_millis(), single.last_millis());
    assert_eq!(single.min_value(), single.max_value());
    assert_eq!(
        single.to_string(),
        format!("ts=[{},{}] value=[2,2]", EPOCH + 7, EPOCH + 7)
    );

    // No records at all: nothing is stated, and the zone says so rather than claiming a span.
    let empty = Zone::of(&[]);
    assert_eq!(empty.first_millis(), None);
    assert_eq!(empty.last_millis(), None);
    assert_eq!(empty.min_value(), None);
    assert_eq!(empty.max_value(), None);
    assert_eq!(
        empty.to_string(),
        "ts=unstated value=unstated",
        "a zone built from nothing states nothing, which is a fact about the index and not a claim \
         about the records"
    );
}

#[test]
fn a_zone_answers_may_and_never_does() {
    let records = vec![
        record("cpu,host=h0", 1_000, 10.0),
        record("cpu,host=h0", 2_000, 20.0),
        record("cpu,host=h0", 3_000, 30.0),
    ];
    let zone = Zone::of(&records);

    // Both ends of both intervals are *in*: a millisecond and a value are points and not half-open
    // spans, and a bound that answered `false` about itself would drop the record standing on it.
    assert!(zone.may_contain_ts(EPOCH + 1_000));
    assert!(zone.may_contain_ts(EPOCH + 3_000));
    assert!(zone.may_contain_ts(EPOCH + 2_000), "and between them");
    assert!(!zone.may_contain_ts(EPOCH + 999), "one millisecond below");
    assert!(!zone.may_contain_ts(EPOCH + 3_001), "and one above");
    assert!(zone.may_contain_value(10.0));
    assert!(zone.may_contain_value(30.0));
    assert!(
        zone.may_contain_value(15.5),
        "a value no record holds, and it may: a zone is bounds"
    );
    assert!(!zone.may_contain_value(9.99));
    assert!(!zone.may_contain_value(30.01));

    // A bound nobody stated is not a bound: the answer has to be `may`, because a `false` here is a
    // segment a query would not open — the one failure this whole value is shaped to avoid.
    let unstated = Zone::of(&[]);
    assert!(unstated.may_contain_ts(0));
    assert!(unstated.may_contain_ts(u64::MAX / 2));
    assert!(unstated.may_contain_value(f64::MIN));
    assert!(unstated.may_contain_value(f64::MAX));
    assert!(
        unstated.may_contain_value(f64::NAN),
        "even a NaN, which is not a number any interval contains"
    );

    // And a NaN *in the records* is the same absence from the other side: a value that is above and
    // below nothing has no place on an interval, so the pair stops being stated rather than quietly
    // leaving it out.
    let with_nan = Zone::of(&[
        record("cpu,host=h0", 1_000, 1.0),
        record("cpu,host=h0", 2_000, f64::NAN),
    ]);
    assert_eq!(
        with_nan.min_value(),
        None,
        "a set holding a NaN states no bounds: bounds that excluded it would be bounds claiming a \
         record is not there"
    );
    assert_eq!(with_nan.max_value(), None);
    assert!(
        with_nan.may_contain_value(f64::NAN),
        "and it may contain anything, including the value that made it unstated"
    );
    assert_eq!(
        with_nan.first_millis(),
        Some(EPOCH + 1_000),
        "the timestamps are a different column and are still stated"
    );
}

#[test]
fn the_zone_meets_a_range_by_the_half_open_rule_the_manifest_uses() {
    // One span: `[EPOCH + 1s, EPOCH + 3s]`.
    let zone = Zone::of(&[
        record("cpu,host=h0", 1_000, 1.0),
        record("cpu,host=h0", 3_000, 1.0),
    ]);
    let from = EPOCH + 1_000;
    let to = EPOCH + 3_000;

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
        "a range starting on the *last* record meets it: that record is inside the range"
    );
    assert!(
        !zone.may_meet(to + 1, to + 100),
        "a range that begins after the last record does not"
    );
    assert!(
        !zone.may_meet(from - 100, from),
        "and neither does one that ends where the first record stands: the range is half-open, so a \
         record whose timestamp is exactly `to` belongs to the next question"
    );
    assert!(
        zone.may_meet(from - 100, from + 1),
        "and it meets one that reaches one past it"
    );
    assert!(zone.may_meet(from + 1, to + 1), "a range inside the span");
    assert!(
        zone.may_meet(from - HOUR, to + HOUR),
        "one that swallows it"
    );
    assert!(
        Zone::of(&[]).may_meet(u64::MAX - 1, u64::MAX),
        "and a zone that states nothing meets every range, which is the only answer that cannot \
         lose a record"
    );
}

#[test]
fn a_union_grows_a_zone_and_an_unstated_one_absorbs_it() {
    let left = Zone::of(&[
        record("cpu,host=h0", 100, -5.0),
        record("cpu,host=h0", 300, 4.0),
    ]);
    let right = Zone::of(&[
        record("mem,host=h1", 200, 1.0),
        record("mem,host=h1", 700, 9.0),
    ]);
    let union = left.union(&right);
    assert_eq!(
        union.first_millis(),
        Some(EPOCH + 100),
        "the union covers both segments: the earliest of the two firsts"
    );
    assert_eq!(union.last_millis(), Some(EPOCH + 700));
    assert_eq!(
        union.min_value(),
        Some(-5.0),
        "and the smallest of the two values"
    );
    assert_eq!(union.max_value(), Some(9.0));
    assert_eq!(
        union,
        right.union(&left),
        "a union of two intervals does not depend on the order they are handed over in"
    );

    // An unstated side absorbs: one of the two has records whose bounds nobody wrote down, so the
    // pair has no bounds anybody can defend — the same rule a merged segment's time range follows.
    let unstated = Zone::of(&[]);
    assert_eq!(left.union(&unstated), unstated);
    assert_eq!(unstated.union(&left), unstated);
    assert_eq!(
        left.union(&unstated).first_millis(),
        None,
        "and the merged zone is not the stated side: that would be a bound claimed for records \
         nobody measured"
    );
}

#[test]
fn a_zone_round_trips_and_bytes_that_are_not_one_are_refused() {
    let zone = Zone::of(&[
        record("cpu,host=h0", 1_000, -7.5),
        record("cpu,host=h0", 9_000, 22.25),
    ]);
    let bytes = zone.encode();
    assert_eq!(
        bytes.len(),
        zone::BYTES,
        "a zone is a fixed number of bytes"
    );
    assert_eq!(
        Zone::decode(&bytes)
            .unwrap_or_else(|error| panic!("a zone this build wrote reads back: {error}")),
        zone
    );
    let unstated = Zone::of(&[]);
    assert_eq!(
        Zone::decode(&unstated.encode()).unwrap_or_else(|error| panic!("{error}")),
        unstated,
        "an unstated zone round trips too, which is what makes it a state and not an absence"
    );

    // Three kinds of bytes that are not one zone: cut short, longer than a zone, and a flags byte
    // with a bit this build does not write.
    let mut body = bytes.clone();
    let mut unknown_flag = bytes.clone();
    unknown_flag[0] |= 0b1000_0000;
    let mut long = bytes.clone();
    long.push(0);
    for damaged in [
        {
            body.pop();
            body
        },
        long,
        unknown_flag,
    ] {
        assert!(
            Zone::decode(&damaged).is_err(),
            "a zone a reader cannot trust is refused rather than read as a shorter span"
        );
    }
    let refusal = Zone::decode(
        &[0b0000_0100]
            .iter()
            .chain(bytes.iter().skip(1))
            .copied()
            .collect::<Vec<u8>>(),
    )
    .err()
    .map(|error| error.to_string());
    assert!(
        refusal
            .as_deref()
            .is_some_and(|detail| detail.contains("flags")),
        "and the refusal about a flags byte names the byte nobody wrote: {refusal:?}"
    );
}

#[test]
fn every_term_that_went_into_a_bloom_may_be_here() {
    // The elements are *terms*: a series name, and each `key=value` pair a name carries. Which terms
    // a store's series contribute is the store's rule (`SegmentIndex::of`, the case after this one);
    // this case is the filter, so it hands over the terms themselves.
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
            "`{term}` went in and answers `may` — a bloom filter has no false negatives, and that is \
             the whole reason it is the right shape for an index that may never lose a record"
        );
    }
    let mut grown = bloom.clone();
    grown.insert("cpu,host=h9");
    grown.insert("nothing=at-all");
    assert!(grown.may_contain("cpu,host=h9"));
    assert!(
        grown.may_contain("nothing=at-all"),
        "a term inserted on its own is a term like any other"
    );
    assert!(
        !bloom.may_contain("nothing=at-all"),
        "and the filter it was not inserted into does not hold it"
    );
}

#[test]
fn a_bloom_answers_may_for_the_rate_its_size_promises_and_for_everything_once_it_is_full() {
    // Eight series of one segment — sixteen terms, their names and their pairs — in a thousand and
    // twenty-four bits: the module's table bounds this at about one in seventy-four thousand, and the
    // case asserts a bound two orders of magnitude looser than that. What it really catches is a
    // filter that answers `yes` without looking at its bits at all, which is what a checksum used as
    // a hash does: before the mixing step in that module the measured rate here was one in thirty.
    let members: Vec<String> = (0..8)
        .map(|index| format!("cpu,host=member{index}"))
        .collect();
    let records: Vec<Record> = members
        .iter()
        .enumerate()
        .map(|(at, name)| record(name, at as u64 * 1_000, 1.0))
        .collect();
    let bloom = SegmentIndex::of(&records);
    let bloom = bloom.series();
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
        "10 000 terms that went into no filter and 16 terms that did: {said_yes} of the strangers \
         answered `may`, and the table in the module docs bounds this at one in seventy-four thousand"
    );
    for member in &members {
        assert!(
            bloom.may_contain(member),
            "and every term that did go in still answers `may`: {member}"
        );
    }

    // A vocabulary past what the size can hold makes the filter answer `yes` to everything, and that
    // is the documented price of a fixed geometry rather than a defect: `yes` is what "I cannot tell
    // you" has to look like in a filter that may never answer `no` by mistake. What it costs is the
    // reads the index was going to save; what it can never cost is a record.
    let everything: Vec<String> = (0..10_000)
        .map(|index| format!("series,id={index}"))
        .collect();
    let full = Bloom::of(everything.iter().map(String::as_str));
    for probe in 0..100 {
        assert!(
            full.may_contain(&format!("series,id=stranger{probe}")),
            "a filter holding ten thousand terms in a thousand bits holds every term as far as a \
             reader can tell"
        );
    }
    assert!(
        Bloom::everything().may_contain("anything,at=all"),
        "and the filter that states nothing is this one: every bit set, so it prunes nothing"
    );
}

#[test]
fn a_union_of_two_blooms_may_contain_what_either_may() {
    let left = Bloom::of(["cpu,host=h0", "host=h0"]);
    let right = Bloom::of(["cpu,host=h1", "host=h1"]);
    let union = left.union(&right);
    assert_eq!(
        union.bits(),
        BLOOM_BITS,
        "a union is a filter of the same shape"
    );
    for term in ["cpu,host=h0", "host=h0", "cpu,host=h1", "host=h1"] {
        assert!(
            union.may_contain(term),
            "`{term}` went into one of the two, so the union may hold it — which is what makes a \
             merge's index a union of its inputs' indexes rather than a second read of every record"
        );
    }
    assert!(
        union.union(&Bloom::everything()).may_contain("cpu,host=h9"),
        "and a filter that states nothing absorbs the union, which is the safe direction"
    );
}

#[test]
fn a_bloom_round_trips_and_another_shape_is_refused() {
    let bloom = Bloom::of(["cpu,host=h0", "mem,host=h1,dc=east"]);
    let bytes = bloom.encode();
    assert_eq!(
        Bloom::decode(&bytes)
            .unwrap_or_else(|error| panic!("a filter this build wrote reads: {error}")),
        bloom
    );

    // A filter that states another size is refused: two filters can only be merged when their bits
    // mean the same places, and merging is how a merge's index is written.
    let foreign: Vec<u8> = [0x00u8, 0x02]
        .iter()
        .chain(bytes.iter().skip(2))
        .copied()
        .collect();
    assert!(
        Bloom::decode(&foreign).is_err(),
        "512 bits is not this build's shape"
    );
    let mut cut = bytes.clone();
    cut.pop();
    let mut longer = bytes.clone();
    longer.push(0);
    for damaged in [
        cut,
        longer,
        bytes.iter().take(1).copied().collect::<Vec<u8>>(),
    ] {
        assert!(
            Bloom::decode(&damaged).is_err(),
            "a filter that is cut short, longer than it says, or missing its size is refused"
        );
    }
}

#[test]
fn a_segment_index_states_its_records_and_a_union_knows_both() {
    let first = vec![
        record("cpu,host=h0", 0, 1.0),
        record("cpu,host=h0", MINUTE, 2.0),
    ];
    let second = vec![
        record("cpu,host=h1,dc=east", HOUR, 3.0),
        record("cpu,host=h1,dc=east", HOUR + MINUTE, 4.0),
    ];
    let one = SegmentIndex::of(&first);
    let two = SegmentIndex::of(&second);

    assert_eq!(
        one.zone(),
        &Zone::of(&first),
        "the zone of an index is the zone of the records it was built from and not a second opinion"
    );
    assert!(one.may_hold("host=h0"));
    assert!(
        !one.may_hold("host=h1"),
        "the term a segment does not hold is what a query's predicate is asked about"
    );
    assert!(
        one.may_hold("cpu,host=h0"),
        "the whole name is a term as well, because a query can spell one out in full"
    );
    assert!(
        one.may_meet(EPOCH, EPOCH + MINUTE + 1),
        "the index answers the same range question its zone does"
    );
    assert!(!one.may_meet(EPOCH + HOUR, EPOCH + HOUR + MINUTE));

    let union = one.union(&two);
    for term in [
        "host=h0",
        "cpu,host=h0",
        "host=h1",
        "dc=east",
        "cpu,host=h1,dc=east",
    ] {
        assert!(
            union.may_hold(term),
            "the union holds what either side holds: {term}"
        );
    }
    assert_eq!(
        union.zone().first_millis(),
        Some(EPOCH),
        "and it meets every range either side met"
    );
    assert_eq!(union.zone().last_millis(), Some(EPOCH + HOUR + MINUTE));
    assert!(
        one.series().union(two.series()) == *union.series(),
        "the vocabulary half is the filters' union, which is the property a merge is built on"
    );

    // The index that states nothing: the safe answer for a segment whose vocabulary nobody wrote
    // down. It prunes nothing, which costs reads, and it cannot cost a record.
    let unstated = SegmentIndex::unstated();
    for term in ["host=h0", "nothing=at-all", "cpu,host=h0"] {
        assert!(unstated.may_hold(term));
    }
    assert!(unstated.may_meet(u64::MAX - 1, u64::MAX));
    assert_eq!(unstated.zone().first_millis(), None);
}

#[test]
fn a_segment_index_round_trips_and_a_flipped_byte_is_refused() {
    let index = SegmentIndex::of(&[
        record("cpu,host=h0", 0, -1.0),
        record("cpu,host=h0,dc=east", HOUR, 8.0),
    ]);
    let bytes = index.encode();
    assert_eq!(
        SegmentIndex::decode(&bytes)
            .unwrap_or_else(|error| panic!("an index this build wrote reads back: {error}")),
        index,
        "the value round trips: the zone map, the filter and the seal over both"
    );

    // A flipped bit anywhere in the file is a damaged hint, and the seal is what makes it one: an
    // index is derived state, and a wrong hint read as a right one is a wrong *answer*, not a
    // slower one.
    let mut flipped = bytes.clone();
    let middle = bytes.len() / 2;
    flipped[middle] ^= 0b0000_0001;
    let refusal = SegmentIndex::decode(&flipped)
        .err()
        .map(|error| error.to_string());
    assert!(
        refusal
            .as_deref()
            .is_some_and(|detail| detail.contains("seal")),
        "a flipped byte fails the seal and the refusal says so: {refusal:?}"
    );

    let mut cut = bytes.clone();
    cut.pop();
    let mut longer = bytes.clone();
    longer.push(0);
    for damaged in [
        cut,
        bytes.iter().take(4).copied().collect::<Vec<u8>>(),
        longer,
        vec![0u8; bytes.len()],
    ] {
        assert!(
            SegmentIndex::decode(&damaged).is_err(),
            "a cut, appended or foreign file is refused rather than read as an index"
        );
    }
    let foreign: Vec<u8> = b"LKLS"
        .iter()
        .chain(bytes.iter().skip(4))
        .copied()
        .collect();
    let named = SegmentIndex::decode(&foreign)
        .err()
        .map(|error| error.to_string());
    assert!(
        named
            .as_deref()
            .is_some_and(|detail| detail.contains("not this store's index")),
        "and a segment's magic on an index's bytes is refused as somebody else's file: {named:?}"
    );
}

#[test]
fn seal_writes_an_index_beside_the_segment_and_nothing_into_the_segment_directory() {
    let batches = workload();
    let (dir, store) = sealed("seal", &batches);

    assert_eq!(
        dir.segment_files().len() as u64,
        SEGMENTS,
        "one file per sealed segment, and nothing else in the directory the manifest's rule about \
         orphans applies to: {:?}",
        dir.segment_files()
    );
    assert!(
        dir.segment_files()
            .iter()
            .all(|name| name.ends_with(".lks")),
        "the segment directory holds segment files: {:?}",
        dir.segment_files()
    );
    assert_eq!(
        dir.index_files().len() as u64,
        SEGMENTS,
        "and the store wrote one index per segment, in a directory of its own beside them: {:?}",
        dir.index_files()
    );
    assert!(
        dir.index_files().iter().all(|name| name.ends_with(".lki")),
        "named by the ordinal they belong to, with an extension of their own: {:?}",
        dir.index_files()
    );

    for (ordinal, batch) in batches.iter().enumerate() {
        let ordinal = ordinal as u64;
        assert_eq!(
            index::load(dir.path(), ordinal),
            Lookup::Found(SegmentIndex::of(batch)),
            "the index of segment {ordinal} states exactly what its records are, which is what a \
             reader decides from without opening the segment"
        );
        assert!(
            index::load(dir.path(), ordinal)
                .into_index()
                .is_some_and(|index| index.may_hold(&format!("host=h{ordinal}"))),
            "and it holds the term this segment's series spells"
        );
    }
    assert_eq!(
        index::load(dir.path(), SEGMENTS + 7),
        Lookup::Missing,
        "an ordinal the store never sealed has no index, and that is `Missing` rather than an error"
    );

    // The store is still the manifest: an index is written before the manifest names the segment, and
    // what the store *is* does not depend on any index file existing.
    assert_eq!(store.manifest().segments().len() as u64, SEGMENTS);
    assert_eq!(
        store.manifest().segments().first().map(|info| info.index),
        Some(0),
        "and the manifest lists the ordinals the indexes are named by"
    );
}

#[test]
fn a_merge_writes_the_union_of_its_inputs_indexes_and_prunes_nothing_when_one_is_missing() {
    // Four segments, comparable in size, and a policy that takes all four.
    let batches: Vec<Vec<Record>> = (0..4u64)
        .map(|index| {
            series(
                &format!("cpu,host=h{index}"),
                index * HOUR,
                STEP,
                30,
                |_| 1.0,
            )
        })
        .collect();
    let (dir, mut store) = sealed_with("merge", MergePolicy::new(4, u64::MAX), &batches);
    let before: Vec<SegmentIndex> = (0..4)
        .map(|ordinal| {
            index::load(dir.path(), ordinal)
                .into_index()
                .unwrap_or_else(|| panic!("segment {ordinal} has an index: this store sealed it"))
        })
        .collect();
    let plan = store.plan();
    assert_eq!(
        plan.segments(),
        [0, 1, 2, 3],
        "the policy takes the whole store"
    );
    let ran = store
        .merge(&plan)
        .unwrap_or_else(|error| panic!("four comparable segments merge: {error}"));
    let merged = ran.index();
    store
        .install(ran)
        .unwrap_or_else(|error| panic!("a merge this store wrote is installed: {error}"));

    let union = before
        .iter()
        .cloned()
        .reduce(|so_far, theirs| so_far.union(&theirs))
        .unwrap_or_else(|| panic!("four indexes make a union"));
    assert_eq!(
        index::load(dir.path(), merged),
        Lookup::Found(union),
        "the merged segment's index is the union of its inputs' indexes: no series name was read to \
         write it, which is the trick the merged entry's time range already uses"
    );
    let merged_index = index::load(dir.path(), merged)
        .into_index()
        .unwrap_or_else(|| panic!("the merge wrote an index"));
    for term in ["host=h0", "host=h1", "host=h2", "host=h3"] {
        assert!(
            merged_index.may_hold(term),
            "and every input's vocabulary is in it: {term}"
        );
    }
    assert_eq!(merged_index.zone().first_millis(), Some(EPOCH));

    // The same store with one input's index file gone. The merged index cannot claim a vocabulary
    // nobody wrote down, so it claims nothing — which prunes nothing and can never hide a record.
    let (dir, mut store) = sealed_with("merge-missing", MergePolicy::new(4, u64::MAX), &batches);
    std::fs::remove_file(index::file_of(dir.path(), 2))
        .unwrap_or_else(|error| panic!("a file this case put there can be removed: {error}"));
    let plan = store.plan();
    let ran = store
        .merge(&plan)
        .unwrap_or_else(|error| panic!("a merge that reads four segments runs: {error}"));
    let merged = ran.index();
    store.install(ran).unwrap_or_else(|error| panic!("{error}"));
    let merged_index = index::load(dir.path(), merged)
        .into_index()
        .unwrap_or_else(|| panic!("the merge wrote an index even with an input missing"));
    assert_eq!(
        merged_index.zone().first_millis(),
        None,
        "an input whose index is gone makes the merged index state no bounds: a union with what \
         nobody wrote down is nothing anybody can defend"
    );
    for term in ["host=h0", "host=h9", "everything=at-once"] {
        assert!(
            merged_index.may_hold(term),
            "and no vocabulary either: `{term}` may be in a file whose inputs nobody described"
        );
    }

    // Which is the property that matters: the records are all still answered.
    let outcome = asked(
        &store.snapshot(),
        &format!(
            "SELECT count(value) FROM cpu WHERE ts BETWEEN {} AND {} GROUP BY time(1h)",
            EPOCH,
            EPOCH + 4 * HOUR
        ),
    );
    assert_eq!(outcome.rows_scanned(), 120, "four hours of thirty samples");
    assert_eq!(outcome.rows_answered(), 120);
    assert_eq!(
        outcome.blocks_pruned(),
        0,
        "and an index that states nothing prunes nothing, which is what keeps a damaged store a \
         slow store rather than a wrong one"
    );
}

#[test]
fn an_expired_ordinals_index_file_stays_behind_and_is_never_consulted() {
    let batches = workload();
    let (dir, mut store) = sealed("expire", &batches);
    let kept = 4u64;
    let keep = kept * HOUR;
    let now = EPOCH + SEGMENTS * HOUR;
    let policy = Policy::new(keep, HOUR);
    let before_epoch = store.manifest().epoch();
    let report = store.expire(&policy, now).unwrap_or_else(|error| {
        panic!("a store whose segments state a range carries a pass out: {error}")
    });
    let expired: Vec<u64> = (0..SEGMENTS - kept).collect();
    assert_eq!(report.expired().len() as u64, expired.len() as u64);
    // The store moved to the epoch the report names, and it moved *forwards*: the epoch is the number
    // a snapshot carries, so a pass that bumped it backwards would be a store whose two different
    // shapes answer to the same number.
    assert_eq!(
        store.manifest().epoch(),
        before_epoch + 1,
        "the pass moved the store on by one epoch"
    );
    assert_eq!(
        report.epoch(),
        store.manifest().epoch(),
        "and the report names the epoch the store is, which is what makes it worth printing"
    );

    for ordinal in &expired {
        assert!(
            !file_of(dir.path(), *ordinal).exists(),
            "segment {ordinal} left the store with the whole file"
        );
        assert!(
            index::file_of(dir.path(), *ordinal).exists(),
            "and its index file stayed behind: nothing names that ordinal any more, so nothing can \
             consult it — the garbage a later step turns into a capacity number"
        );
    }
    assert_eq!(
        dir.index_files().len() as u64,
        SEGMENTS,
        "every index the store ever wrote is still on the disk: {:?}",
        dir.index_files()
    );

    // Never consulted, and the way to say that is to take the garbage away and compare what a query
    // did: a reader that had consulted a stale zone map would answer differently in the two stores.
    // The range the pass left answers: from the floor it wrote, which is `now` less the window the
    // policy keeps, snapped down to a bucket boundary.
    let text = format!(
        "SELECT count(value) FROM cpu WHERE ts BETWEEN {} AND {} GROUP BY time(1h), series",
        EPOCH + (SEGMENTS - kept) * HOUR,
        now
    );
    let before = planned(&store.snapshot(), &text);
    for ordinal in &expired {
        std::fs::remove_file(index::file_of(dir.path(), *ordinal))
            .unwrap_or_else(|error| panic!("a file this case put there can be removed: {error}"));
    }
    let after = planned(&store.snapshot(), &text);
    assert_eq!(
        (
            before.opened().len(),
            before.blocks_pruned(),
            before.index_missing(),
            before.index_unusable(),
        ),
        (
            after.opened().len(),
            after.blocks_pruned(),
            after.index_missing(),
            after.index_unusable(),
        ),
        "a stale index is not consulted: deleting the files the manifest does not name changes \
         nothing about what a query opens"
    );
    assert_eq!(
        before.opened().len() as u64,
        kept,
        "the four segments the pass kept, and none of the garbage"
    );
}

#[test]
fn a_selective_query_opens_one_segment_of_many_and_an_unselective_one_opens_them_all() {
    let batches = workload();
    let (dir, store) = sealed("prune", &batches);
    let store_path = dir.argument();
    let snapshot = store.snapshot();
    let whole = format!(
        "SELECT count(value) FROM cpu WHERE ts BETWEEN {} AND {} GROUP BY time(1m)",
        EPOCH,
        EPOCH + SEGMENTS * HOUR
    );

    // No predicate: nothing but the manifest's spans decides, and they decide nothing here — every
    // segment's span meets a range that covers the store. The index is asked nothing, because a
    // query with no tag predicate has no vocabulary question to ask.
    let all = planned(&snapshot, &whole);
    assert_eq!(all.opened().len() as u64, SEGMENTS);
    assert_eq!(all.pruned().len(), 0);
    assert_eq!(
        all.blocks_pruned(),
        0,
        "an unselective query prunes nothing by the index"
    );
    assert_eq!(
        all.index_missing(),
        0,
        "and every index it could have consulted was there"
    );
    assert_eq!(all.index_unusable(), 0);

    // One tag predicate: the same range, the same store, and the index leaves one segment standing.
    let selective = format!(
        "SELECT count(value) FROM cpu WHERE host = 'h7' AND ts BETWEEN {} AND {} GROUP BY time(1m), \
         series",
        EPOCH,
        EPOCH + SEGMENTS * HOUR
    );
    let one = planned(&snapshot, &selective);
    assert_eq!(
        one.opened()
            .iter()
            .map(|info| info.index)
            .collect::<Vec<u64>>(),
        vec![7],
        "the one segment whose records carry the pair — counted, not described"
    );
    assert_eq!(
        one.pruned().len(),
        0,
        "and the manifest pruned nothing: the range meets every segment it names"
    );
    assert_eq!(
        one.blocks_pruned(),
        SEGMENTS - 1,
        "the index's share of the decision, which is the pruning this step adds"
    );
    assert_eq!(
        one.bytes_read(),
        one.opened().first().map_or(0, |info| info.bytes),
        "what the query has to open is one segment's bytes"
    );
    assert_eq!(
        one.bytes_read() + one.bytes_avoided(),
        store.manifest().bytes(),
        "and every byte of the store is in exactly one of the two counts: the segments it would \
         open, and the ones the index ruled out"
    );
    assert!(
        one.bytes_avoided() > 0,
        "the pruning is worth bytes, not only segments"
    );

    // The answer is the segment's records and nothing else, which is the half a count of pruned
    // segments does not prove by itself.
    let outcome = asked(&snapshot, &selective);
    assert_eq!(
        outcome.rows_scanned(),
        30,
        "thirty samples of the one segment opened"
    );
    assert_eq!(outcome.rows_answered(), 30);
    assert_eq!(outcome.blocks_pruned(), SEGMENTS - 1);
    let answers = answers_of(&outcome);
    assert_eq!(
        answers.lines().count(),
        30,
        "one window per minute the segment holds records in: `{answers}`"
    );
    assert!(
        answers.lines().all(|line| line.starts_with("cpu,host=h7 ")),
        "and every line is about the series the predicate named: `{answers}`"
    );
    let ran = lakeletq(&sql(&store_path, &selective));
    assert_eq!(ran.code, 0, "the binary answers it too: {ran:?}");
    assert_eq!(
        ran.stderr, "",
        "and it is an answer rather than a diagnosis"
    );
    assert_eq!(ran.stdout, answers, "with the same lines");

    // A range that meets one segment is pruned by the *manifest*, and the index is never reached:
    // the two prunings are counted apart because they are different facts about a store.
    let narrow = format!(
        "SELECT count(value) FROM cpu WHERE ts BETWEEN {} AND {} GROUP BY time(1m)",
        EPOCH + 7 * HOUR,
        EPOCH + 8 * HOUR
    );
    let by_time = planned(&snapshot, &narrow);
    assert_eq!(by_time.opened().len(), 1);
    assert_eq!(by_time.pruned().len() as u64, SEGMENTS - 1);
    assert_eq!(
        by_time.blocks_pruned(),
        0,
        "the index is asked about the segments the range meets and no others"
    );
    assert_eq!(
        by_time.opened().len() as u64 + by_time.pruned().len() as u64 + by_time.blocks_pruned(),
        SEGMENTS,
        "and every segment is in exactly one of the three: opened, pruned by the manifest, or pruned \
         by the index"
    );
}

#[test]
fn a_store_without_its_index_answers_the_same_and_says_which_counter_moved() {
    let batches = workload();
    let (dir, store) = sealed("gone", &batches);
    let selective = format!(
        "SELECT count(value) FROM cpu WHERE host = 'h7' AND ts BETWEEN {} AND {} GROUP BY time(1m), \
         series",
        EPOCH,
        EPOCH + SEGMENTS * HOUR
    );
    let answered = answers_of(&asked(&store.snapshot(), &selective));
    assert!(
        answered.starts_with("cpu,host=h7 "),
        "the case has an answer to compare: {answered}"
    );
    let with_index = planned(&store.snapshot(), &selective);
    assert_eq!(with_index.index_missing(), 0);
    assert_eq!(with_index.blocks_pruned(), SEGMENTS - 1);

    // The index directory goes, which is a loss this store recovers from by reading more.
    dir.lose_the_index();
    let snapshot = store.snapshot();
    let outcome = asked(&snapshot, &selective);
    assert_eq!(
        answers_of(&outcome),
        answered,
        "a store that lost its index answers exactly what it answered before: this is the rule the \
         whole step is shaped around, and the answer is what it protects"
    );
    assert_eq!(outcome.rows_answered(), 30);
    let after = planned(&snapshot, &selective);
    assert_eq!(
        after.blocks_pruned(),
        0,
        "there is nothing left to prune with"
    );
    assert_eq!(
        after.index_missing(),
        SEGMENTS,
        "and the counter says why: every segment the range met had no index to consult"
    );
    assert_eq!(
        after.index_unusable(),
        0,
        "nothing was damaged — the files are not there, which is the other fact"
    );
    assert_eq!(
        after.opened().len() as u64 + after.pruned().len() as u64 + after.blocks_pruned(),
        SEGMENTS,
        "every segment is opened, pruned by the range, or open because the index is gone"
    );
    assert_eq!(
        after.bytes_avoided(),
        0,
        "and the bytes the index was saving are being read again, which is what a counter is for"
    );
    assert_eq!(
        after.bytes_read(),
        store.manifest().bytes(),
        "all of it, because the range meets every segment"
    );

    // The index is asked about the segments the *range* meets, so the counter follows the range: the
    // same store with one hour asked for is one missing index and not twelve.
    let narrow = format!(
        "SELECT count(value) FROM cpu WHERE host = 'h7' AND ts BETWEEN {} AND {} GROUP BY time(1m)",
        EPOCH + 7 * HOUR,
        EPOCH + 8 * HOUR
    );
    assert_eq!(
        planned(&store.snapshot(), &narrow).index_missing(),
        1,
        "the eleven segments the range rules out are never asked about their index"
    );
}

#[test]
fn a_scribbled_index_changes_the_answer_no_more_than_a_missing_one() {
    let batches = workload();
    let (dir, store) = sealed("scribbled", &batches);
    let selective = format!(
        "SELECT count(value) FROM cpu WHERE host = 'h7' AND ts BETWEEN {} AND {} GROUP BY time(1m), \
         series",
        EPOCH,
        EPOCH + SEGMENTS * HOUR
    );
    let answered = answers_of(&asked(&store.snapshot(), &selective));

    // Four ways of being not-an-index, on four different segments: an empty file, one cut short, one
    // with a flipped byte, and one that is not this store's file at all. Each of them is a *hint
    // that is wrong*, and the seal is what makes "damaged" a refusal instead of a plausible lie.
    let good = dir.index_bytes(7);
    dir.scribble(0, &[]);
    dir.scribble(1, &good[..good.len() / 2]);
    let mut flipped = good.clone();
    let at = good.len() - 3;
    flipped[at] ^= 0b0000_1000;
    dir.scribble(2, &flipped);
    dir.scribble(
        3,
        b"not an index at all, and long enough to be read as one if anybody tried",
    );

    for ordinal in 0..4 {
        assert_eq!(
            index::load(dir.path(), ordinal),
            Lookup::Damaged,
            "segment {ordinal}'s index is damaged, and the reader is told which of the two states \
             this is"
        );
    }
    assert_eq!(
        index::load(dir.path(), 7),
        Lookup::Found(SegmentIndex::of(&batches[7])),
        "and the ones nobody touched are still indexes"
    );

    let snapshot = store.snapshot();
    let outcome = asked(&snapshot, &selective);
    assert_eq!(
        answers_of(&outcome),
        answered,
        "a scribbled index changes the answer no more than a missing one does"
    );
    assert_eq!(outcome.rows_answered(), 30);
    let after = planned(&snapshot, &selective);
    assert_eq!(
        after.index_unusable(),
        4,
        "four segments had an index that could not be read"
    );
    assert_eq!(
        after.index_missing(),
        0,
        "and none of them had no index at all: the two counters are two different facts, which is \
         why there are two of them"
    );
    assert_eq!(
        after.blocks_pruned(),
        SEGMENTS - 1 - 4,
        "so the four damaged segments are opened and the pruning is done with the rest"
    );
    assert_eq!(
        after
            .opened()
            .iter()
            .map(|info| info.index)
            .collect::<Vec<u64>>(),
        vec![0, 1, 2, 3, 7],
        "which is exactly the segments that could still be decided about"
    );
}

#[test]
fn explain_states_blocks_pruned_beside_the_twelve_keys_it_always_had() {
    let batches = workload();
    let (dir, store) = sealed("explain", &batches);
    let store_path = dir.argument();
    let text = format!(
        "SELECT count(value) FROM cpu WHERE host = 'h7' AND ts BETWEEN {} AND {} GROUP BY time(1m)",
        EPOCH,
        EPOCH + SEGMENTS * HOUR
    );
    let plan = planned(&store.snapshot(), &text);
    let explained = plan.explain();
    let keys: Vec<&str> = explained
        .lines()
        .map(|line| {
            line.split_once(" = ")
                .map(|(key, _)| key)
                .unwrap_or_else(|| panic!("a plan line is `key = value`: `{line}`"))
        })
        .collect();
    assert_eq!(
        keys,
        vec![
            "metric",
            "series",
            "aggregate",
            "from",
            "to",
            "window",
            "windows",
            "columns",
            "segments_opened",
            "segments_pruned",
            "blocks_pruned",
            "records",
            "limit",
        ],
        "the twelve keys `lakelet/4.11-query-engine` shipped, in their order, with the one this step \
         adds beside the pruning it belongs to"
    );
    for expected in [
        "metric = cpu",
        "series = cpu,host=h7",
        "aggregate = count",
        &format!("from = {EPOCH}"),
        &format!("to = {}", EPOCH + SEGMENTS * HOUR),
        "window = 60000",
        "windows = 720",
        "columns = series,ts_millis,value",
        "segments_opened = 1",
        "segments_pruned = 0",
        "blocks_pruned = 11",
        "records = 30",
        "limit = none",
    ] {
        assert!(
            explained.contains(expected),
            "the plan states `{expected}`:\n{explained}"
        );
    }

    // The binary, both ways: `EXPLAIN` as lines and as one object, and the answer's own document
    // carrying the index's share of what the query cost.
    let explained_text = format!("EXPLAIN {text}");
    let ran = lakeletq(&sql(&store_path, &explained_text));
    assert_eq!(ran.code, 0, "a plan is an answer: {ran:?}");
    assert_eq!(ran.stderr, "", "and not a diagnosis");
    assert_eq!(ran.stdout, explained, "the same plan the library printed");
    let mut json = sql(&store_path, &explained_text);
    json.push("--json");
    let document: Value = serde_json::from_str(&lakeletq(&json).stdout)
        .unwrap_or_else(|error| panic!("`EXPLAIN --json` prints one document: {error}"));
    assert_eq!(
        document["blocks_pruned"],
        Value::from(11),
        "the document carries the key beside the lines: {document}"
    );
    assert_eq!(document["segments_opened"], Value::from(1));

    let mut answered = sql(&store_path, &text);
    answered.push("--json");
    let answer: Value = serde_json::from_str(&lakeletq(&answered).stdout)
        .unwrap_or_else(|error| panic!("`--json` prints one document: {error}"));
    assert_eq!(
        answer["blocks_pruned"],
        Value::from(11),
        "and a script that ran the query reads what the pruning was worth: {answer}"
    );
    assert_eq!(answer["index_missing"], Value::from(0));
    assert_eq!(answer["index_unusable"], Value::from(0));
    assert!(
        answer["bytes_read"].is_number() && answer["bytes_avoided"].is_number(),
        "in the units a store is measured in: {answer}"
    );
}

#[test]
fn the_two_forms_of_one_question_agree_at_the_bound_the_tool_answers_to() {
    // `lakelet/4.10-downsampling` bounds a query by how many windows it would touch (`MAX_WINDOWS`),
    // and this step grows what the tool says about a query it did answer. The two meet at the
    // boundary, and the boundary is where a surface that just grew is easiest to get wrong: the
    // widest grid the tool answers is answered by both input forms, one bucket past it is refused by
    // both, and the document the text form prints is where the *index's* counters are read.
    let batches = workload();
    let (dir, _store) = sealed("widest", &batches);
    let store_path = dir.argument();
    let from = EPOCH.to_string();
    let widest = (EPOCH + MAX_WINDOWS * HOUR).to_string();
    let more = (EPOCH + (MAX_WINDOWS + 1) * HOUR).to_string();

    let by_flags = lakeletq(&[
        "query",
        "--store",
        &store_path,
        "--from",
        &from,
        "--to",
        &widest,
        "--window",
        "1h",
        "--agg",
        "count",
    ]);
    assert_eq!(
        by_flags.code, 0,
        "the widest grid the tool answers is answered: {by_flags:?}"
    );
    assert_eq!(
        by_flags.stderr, "",
        "and it is an answer rather than a diagnosis"
    );
    assert_eq!(
        by_flags.stdout.lines().count() as u64,
        SEGMENTS,
        "one line per window the store holds records in: `{}`",
        by_flags.stdout
    );
    let past_flags = lakeletq(&[
        "query",
        "--store",
        &store_path,
        "--from",
        &from,
        "--to",
        &more,
        "--window",
        "1h",
        "--agg",
        "count",
    ]);
    assert_eq!(
        past_flags.code, 2,
        "one bucket past it is refused, and a refusal is a caller mistake: {past_flags:?}"
    );
    assert_eq!(
        past_flags.stdout, "",
        "with nothing on standard output to parse"
    );

    // The same question in the text form: the same answer, and the one surface this step adds to it.
    let text = format!(
        "SELECT count(value) FROM cpu WHERE ts BETWEEN {from} AND {widest} GROUP BY time(1h)"
    );
    let mut by_text = sql(&store_path, &text);
    by_text.push("--json");
    let answered = lakeletq(&by_text);
    assert_eq!(
        answered.code, 0,
        "the text form answers the same grid: {answered:?}"
    );
    let document: Value = serde_json::from_str(&answered.stdout)
        .unwrap_or_else(|error| panic!("`--json` prints one document: {error}"));
    for key in [
        "blocks_pruned",
        "bytes_read",
        "bytes_avoided",
        "index_missing",
        "index_unusable",
    ] {
        assert!(
            document.get(key).is_some(),
            "the document of the widest grid states `{key}`, which is what the index's work is read \
             from: {document}"
        );
    }
    assert_eq!(
        document["segments_opened"],
        Value::from(SEGMENTS),
        "this question asks about the whole store, so the index it can prune nothing from is asked \
         about every segment: {document}"
    );
    let past_text = format!(
        "SELECT count(value) FROM cpu WHERE ts BETWEEN {from} AND {more} GROUP BY time(1h)"
    );
    let refused = lakeletq(&sql(&store_path, &past_text));
    assert_eq!(
        refused.code, 2,
        "and the refusal is the same in both forms: {refused:?}"
    );
    assert!(
        !refused.stderr.trim().is_empty(),
        "with a sentence on standard error, as every refusal in this tool has"
    );
}

#[test]
fn the_index_is_engine_code_and_it_lives_beside_the_segments() {
    // The module this step adds is engine code, and the engine is what runs in a test, in a repair
    // tool and in a load generator — none of which has a configuration file or a signal handler. The
    // check reads the sources as text, which is how `lakelet/4.9-retention`'s boundary case is
    // written as well.
    let engine = PathBuf::from("src/engine");
    let mut all: Vec<PathBuf> = Vec::new();
    collect_rust(&engine, &mut all).expect("the engine directory is part of the project");
    assert!(
        all.iter()
            .any(|path| path.ends_with("src/engine/index/mod.rs")),
        "the index module is engine code and this step put it there"
    );
    for file in &all {
        let text = source(&file.to_string_lossy());
        for forbidden in ["crate::service", "crate::cli"] {
            assert!(
                !text.contains(forbidden),
                "{} names `{forbidden}` — the daemon uses the engine, never the other way, and the \
                 files this step adds are where a new violation would appear",
                file.display()
            );
        }
    }
    let module = source("src/engine/index/mod.rs");
    for declaration in ["pub mod bloom;", "pub mod zone;"] {
        assert!(
            module.contains(declaration),
            "the index module does not declare `{declaration}`, which is the surface this spec is \
             written against"
        );
    }
    assert!(
        source("src/engine/mod.rs").contains("pub mod index;"),
        "the engine does not declare the index module, so nothing in the store could reach it"
    );

    // It is a *sidecar*: its own directory, its own extension, and nothing of it inside the segment
    // directory that a manifest's orphan rule cleans up and that two earlier steps count the files
    // of.
    assert_ne!(
        index::DIRECTORY,
        compaction::DIRECTORY,
        "the index does not live in the segment directory"
    );
    assert_eq!(index::DIRECTORY, "index");
    assert_eq!(index::EXTENSION, "lki");
    assert!(
        index::file_of(Path::new("/store"), 7).ends_with(Path::new("index/0000000007.lki")),
        "an index file is named by its ordinal, the way a segment file is: {:?}",
        index::file_of(Path::new("/store"), 7)
    );

    // And the segment file's own layout is untouched by this step: the version `4.7`'s spec pins is
    // still 1, and an index file does not start with a segment's magic.
    assert_eq!(
        segment::VERSION,
        1,
        "the segment layout did not move in this step — the index is a separate file with a version \
         of its own, which is what keeps `lakelet/4.7-columnar-segments`' bytes the same"
    );
    let (dir, _store) = sealed("magic", &workload());
    assert_ne!(
        dir.index_bytes(0).get(..4).map(<[u8]>::to_vec),
        Some(segment::MAGIC.to_vec()),
        "and the two formats have magics of their own, so neither is read as the other"
    );

    // The page `lakelet/4.11-query-engine` shipped promised this step's key on its own surface.
    let page = source("docs/query.md");
    assert!(
        page.contains("blocks_pruned"),
        "docs/query.md does not name `blocks_pruned`, and that page is where a reader of `EXPLAIN` \
         looks for what the keys mean"
    );
    assert!(
        page.contains("segments_pruned"),
        "and it still describes the pruning it always did"
    );
}

/// A merge whose inputs all state a range writes a segment that states one too.
///
/// `lakelet/4.8-compaction` pinned the *union* of the ranges; this case is about the flag behind
/// it. An input that states nothing is meant to make the merged range unstated (`SegmentInfo`), and
/// a merge that answered "nothing stated" for inputs that stated everything would write a segment
/// no retention pass may ever drop — for ever, and silently.
#[test]
fn a_merge_of_stated_ranges_states_the_range_it_wrote() {
    let batches = vec![hour(0), hour(1)];
    let (dir, mut store) = sealed_with("merge-states", MergePolicy::new(2, u64::MAX), &batches);
    let inputs: Vec<(u64, u64)> = store
        .manifest()
        .segments()
        .iter()
        .map(|info| {
            (
                info.first_millis()
                    .unwrap_or_else(|| panic!("a sealed segment states its range")),
                info.last_millis()
                    .unwrap_or_else(|| panic!("a sealed segment states its range")),
            )
        })
        .collect();
    assert_eq!(inputs.len(), 2, "the case sealed two segments: {inputs:?}");

    let ran = store
        .compact()
        .unwrap_or_else(|error| panic!("two comparable segments merge: {error}"))
        .unwrap_or_else(|| panic!("the policy has a plan for two comparable segments"));
    let merged = store
        .manifest()
        .segments()
        .first()
        .copied()
        .unwrap_or_else(|| panic!("the merge left one segment"));
    assert_eq!(
        merged.index,
        ran.wrote(),
        "the merged segment is the one it wrote"
    );
    let first = merged
        .first_millis()
        .unwrap_or_else(|| panic!("a merge of stated ranges states the range it wrote"));
    let last = merged
        .last_millis()
        .unwrap_or_else(|| panic!("a merge of stated ranges states the range it wrote"));
    assert_eq!(
        (first, last),
        (
            inputs
                .iter()
                .map(|(first, _)| *first)
                .min()
                .expect("two inputs"),
            inputs
                .iter()
                .map(|(_, last)| *last)
                .max()
                .expect("two inputs")
        ),
        "the range the merge wrote is the union of the ranges it took"
    );
    drop(dir);
}

/// An index shorter than the header, a zone map, a filter and a seal is refused, not decoded.
///
/// The length is the first of the four checks a reader makes, and it is the one that keeps the
/// arithmetic below it in range: a file of four bytes is not a small index, it is not an index.
#[test]
fn an_index_shorter_than_its_own_arithmetic_is_refused() {
    for length in [0usize, 1, 4, 8, 16] {
        let bytes = vec![0u8; length];
        let refused = SegmentIndex::decode(&bytes);
        assert!(
            refused.is_err(),
            "{length} byte(s) is not an index, and reading it as one would be guessing at where it \
             ends"
        );
    }
}
