//! Spec for `lakelet/4.15-budget-regression`: a count per phase, and the identities that make the
//! counts a measurement.
//!
//! `kiln run` copies this file into the project as `tests/spec_budget_regression.rs`; it is the
//! course's file, not the learner's (plan §3 D2). Fifteen cases, and the shape of them is the point:
//!
//! * **a count is not a clock.** A trace is seven `u64`s and nothing else — no instant, no name of a
//!   machine — so the same question asked of the same store answers with the same profile, in this
//!   process, in another one, on another machine (`jobq/3.22-bench-budget`'s rule, applied to the
//!   work *inside* a run);
//! * **the phases are a pipeline, and the identities are what make them a measurement.** Every
//!   decoded record is offered to the predicates, every kept record is offered to the window pass,
//!   a bucket holds at least one value, a block is read before it is inflated, and the phase that
//!   decides about the store is the phase that examined *all* of it. A counter no case can falsify
//!   is worse than no counter, so the cases below are written to move one phase at a time;
//! * **a phase is charged where the work happened.** The plan's decision, the read, the decode, the
//!   predicate, the window pass and the answer: a query that opened nothing still paid for the plan
//!   (`headers` and nothing else), and a query that read a segment and kept none of it paid for the
//!   decode and for no bucket;
//! * **the pruning of `lakelet/4.14-index-pruning` is visible as a phase.** The same query over the
//!   same store with the index directory gone reads six segments where it read one: `blocks`,
//!   `decompress` and `decode` move, and `filter`, `aggregate`, `render` and the answer do not —
//!   the index saved *reads*, and the profile is what says which part of the run was paying;
//! * **a window that held nothing is not a bucket.** The grid a range touches and the buckets an
//!   answer has are two different counts (`lakelet/4.10-downsampling`), and `render` is the second;
//! * **the run's profile travels with the answer, and only where it belongs.** The text form's
//!   `--json` document carries one number per phase under `trace`, in the order the phases ran; the
//!   flag form's array is the shape `lakelet/4.10-downsampling` shipped, untouched;
//! * **the boundary holds.** `src/engine/trace/**` and the executor are engine code — they may not
//!   name the service layer or the command line, read out of the sources as text
//!   (`jobq/3.1-engine-boundary`) — and nothing in them measures the machine.
//!
//! Nothing here sleeps, reads a clock or reads the environment: every timestamp, range and window is
//! a number the case passes in.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use lakelet::engine::compaction::Segments;
use lakelet::engine::index;
use lakelet::engine::query::exec::{self, Outcome};
use lakelet::engine::query::parse::{Query, Statement, parse};
use lakelet::engine::query::plan::Plan;
use lakelet::engine::trace::{Phase, Trace};
use lakelet::protocol::batch::Record;
use serde_json::Value;

/// Milliseconds in a minute.
const MINUTE: u64 = 60_000;

/// Milliseconds in an hour: the width of one sealed segment, and of one window.
const HOUR: u64 = 3_600_000;

/// Milliseconds between two samples of a series: two minutes, so one hour is thirty samples.
const STEP: u64 = 120_000;

/// The first timestamp of every case's store, on an hour boundary.
///
/// `1_699_999_200_000` is `472_222 * HOUR` exactly, so every range, span and profile below starts on
/// a number a reader can check by hand.
const EPOCH: u64 = 472_222 * HOUR;

/// How many segments the stores are built from: one hour each, two series each.
const SEGMENTS: u64 = 12;

/// The series every segment carries, so that a segment's vocabulary is never one term and a query
/// for it can be pruned by nothing.
const FLEET: &str = "fleet";

/// The host the selective cases ask about: one series of one segment, in the second half of the
/// store.
const SELECTIVE: u64 = 7;

/// The page this step delivers, relative to the project.
const PAGE: &str = "docs/perf.md";

/// One record of `series`, `offset` milliseconds after [`EPOCH`].
fn record(series: &str, offset: u64, value: f64) -> Record {
    Record {
        series: series.to_owned(),
        ts_millis: (EPOCH + offset) as i64,
        value,
    }
}

/// The records one sealed segment holds: thirty of its own host, then thirty of the fleet's.
fn segment_records(segment: u64) -> Vec<Record> {
    let start = segment * HOUR;
    let mut records: Vec<Record> = (0..30)
        .map(|offset| {
            record(
                &format!("cpu,host=h{segment}"),
                start + offset as u64 * STEP,
                offset as f64,
            )
        })
        .collect();
    records.extend((0..30).map(|offset| {
        record(
            &format!("cpu,host={FLEET}"),
            start + offset as u64 * STEP,
            offset as f64,
        )
    }));
    records
}

/// The whole store: [`SEGMENTS`] hours, one series of its own each and the fleet's.
fn workload() -> Vec<Vec<Record>> {
    (0..SEGMENTS).map(segment_records).collect()
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
            "lakelet-budget-regression-spec-{label}-{}",
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

    /// Delete the whole index directory, which is the loss a store recovers from by reading more
    /// (`lakelet/4.14-index-pruning`).
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

/// The question a case asks: one host, one window an hour, one answer per series.
fn selecting(host: &str, from_hour: u64, to_hour: u64) -> String {
    format!(
        "SELECT count(value) FROM cpu WHERE host = '{host}' AND ts BETWEEN {} AND {} \
         GROUP BY time(1h), series",
        EPOCH + from_hour * HOUR,
        EPOCH + to_hour * HOUR
    )
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

/// What one query text answered, in process.
fn asked(snapshot: &lakelet::engine::compaction::Snapshot, text: &str) -> Outcome {
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

/// One source file of the project, as text.
fn source(relative: &str) -> String {
    std::fs::read_to_string(relative).unwrap_or_else(|error| {
        panic!("{relative} cannot be read: {error} — this case is about it")
    })
}

/// The page this case reads, from the project it is part of.
fn page() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(PAGE);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("{PAGE} is the page this step delivers, and it cannot be read: {error}")
    })
}

#[test]
fn a_trace_counts_each_phase_on_its_own() {
    let mut trace = Trace::new();
    for phase in Phase::ALL {
        assert_eq!(
            trace.count(phase),
            0,
            "a run that has done nothing has done no {phase:?}"
        );
    }
    assert_eq!(trace.total(), 0, "and the whole of it is nothing");
    assert_eq!(
        trace,
        Trace::default(),
        "a trace that has counted nothing is the default one: a caller starts from it"
    );
    assert_eq!(
        trace.runs(),
        1,
        "a trace a query is about to charge is the trace of one run"
    );

    trace.charge(Phase::Decode, 3);
    trace.charge(Phase::Filter, 1);
    trace.charge(Phase::Decode, 2);
    assert_eq!(trace.count(Phase::Decode), 5, "charges to one phase add up");
    assert_eq!(
        trace.count(Phase::Filter),
        1,
        "and do not touch another phase"
    );
    assert_eq!(
        trace.count(Phase::Render),
        0,
        "a phase nothing charged stays at zero rather than at a guess"
    );
    assert_eq!(
        trace.total(),
        6,
        "the whole of what a run did is every phase of it, added up"
    );

    trace.charge(Phase::Blocks, 0);
    assert_eq!(
        trace.total(),
        6,
        "counting nothing is nothing to charge, which is not the same as a phase that ran"
    );
}

#[test]
fn a_profile_is_whole_percent_of_the_work_that_ran() {
    assert!(
        Trace::new().shares().is_empty(),
        "a run that did nothing has no share of anything: whole percent of zero is not a number"
    );

    let mut trace = Trace::new();
    trace.charge(Phase::Decode, 3);
    trace.charge(Phase::Filter, 1);
    assert_eq!(
        trace.shares(),
        vec![(Phase::Decode, 75), (Phase::Filter, 25)],
        "the phases that ran, largest first, as whole percent of the run"
    );
    assert_eq!(
        trace.shares().len(),
        2,
        "and only those: a phase nothing charged is not a row of the profile"
    );

    // Whole percent of a whole run does not add up on its own — the floors leave a point or two
    // behind — so the phases with the largest remainders take it and the profile adds to 100.
    let mut lopsided = Trace::new();
    lopsided.charge(Phase::Headers, 1);
    lopsided.charge(Phase::Decode, 1_000);
    lopsided.charge(Phase::Render, 1);
    let shares = lopsided.shares();
    assert_eq!(shares.len(), 3);
    let given: u64 = shares.iter().map(|(_, percent)| *percent).sum();
    assert_eq!(
        given, 100,
        "a profile that added up to 99 would be a profile somebody has to explain: {shares:?}"
    );
    assert_eq!(
        shares.first().map(|(phase, _)| *phase),
        Some(Phase::Decode),
        "the largest phase is the first row: {shares:?}"
    );
    for pair in shares.windows(2) {
        assert!(
            pair[0].1 >= pair[1].1,
            "the rows are largest first, and a share of a hundred is not a phase that did four \
             times the work of the one above it: {shares:?}"
        );
    }
    assert!(
        shares.iter().any(|(phase, _)| *phase == Phase::Headers)
            && shares.iter().any(|(phase, _)| *phase == Phase::Render),
        "a phase that ran is a row even when its share rounds to nothing: {shares:?}"
    );
}

#[test]
fn a_merge_is_two_runs_in_one_report() {
    let mut left = Trace::new();
    left.charge(Phase::Decode, 10);
    left.charge(Phase::Render, 1);
    let mut right = Trace::new();
    right.charge(Phase::Decode, 5);
    right.charge(Phase::Filter, 4);

    let mut merged = left.clone();
    merged.merge(&right);
    assert_eq!(
        merged.count(Phase::Decode),
        15,
        "the two runs' decode together"
    );
    assert_eq!(merged.count(Phase::Filter), 4, "and each phase on its own");
    assert_eq!(merged.count(Phase::Render), 1);
    assert_eq!(
        merged.total(),
        left.total() + right.total(),
        "the pair's work is the two runs added up, which is what a merged report has to say"
    );
    assert_eq!(
        merged.runs(),
        2,
        "a report of two runs says it covers two: a total nobody can check against anything is not \
         a report"
    );

    // The order the runs are merged in is not part of the report, and the counts themselves are
    // untouched by merging: this is one account of the pair, not a third run.
    let mut other = right.clone();
    other.merge(&left);
    assert_eq!(
        other, merged,
        "two runs, one report: which one was merged into which is not a fact about the work"
    );
    assert_eq!(
        left.count(Phase::Decode),
        10,
        "and merging does not spend a run"
    );
    assert_eq!(right.count(Phase::Decode), 5);
}

#[test]
fn every_phase_has_a_name_and_parses_back() {
    assert_eq!(
        Phase::ALL.map(Phase::name),
        [
            "headers",
            "blocks",
            "decompress",
            "decode",
            "filter",
            "aggregate",
            "render",
        ],
        "the names the course's baseline is written in and the harness prints, in the order a query \
         walks the phases: the harness and the baseline read this list rather than spelling it"
    );
    let mut names: Vec<&str> = Phase::ALL.iter().map(|phase| phase.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names.len(),
        Phase::ALL.len(),
        "seven phases, seven names: {names:?}"
    );

    for phase in Phase::ALL {
        assert_eq!(
            Phase::parse(phase.name()),
            Some(phase),
            "a name is a name this module answers with, so a reader of a profile can go back: \
             `{}`",
            phase.name()
        );
        // And a phase is charged where it is reported: one counter per phase, no two phases
        // sharing one.
        let mut trace = Trace::new();
        trace.charge(phase, 1);
        assert_eq!(trace.count(phase), 1, "the {phase:?} phase's own counter");
        assert_eq!(trace.total(), 1);
        for other in Phase::ALL.iter().filter(|other| **other != phase) {
            assert_eq!(
                trace.count(*other),
                0,
                "charging {phase:?} charged {other:?} as well"
            );
        }
    }

    assert_eq!(
        Phase::parse("Decode"),
        None,
        "a name is the lowercase word a metric line spells"
    );
    assert_eq!(Phase::parse(""), None);
    assert_eq!(
        Phase::parse("bytes_read"),
        None,
        "a counter of the outcome is not a phase: the two lists are different lists, and a reader \
         that has one has to be able to tell whether it is the other"
    );
}

#[test]
fn a_plan_that_prunes_everything_away_charges_headers_and_nothing_else() {
    let (_dir, store) = sealed("pruned", &workload());
    let snapshot = store.snapshot();
    // A range every segment of the store is before: the manifest's stated spans rule all of them
    // out, so nothing is opened and nothing is read.
    let outcome = asked(&snapshot, &selecting(&format!("h{SELECTIVE}"), 100, 101));
    let trace = outcome.trace();

    assert_eq!(outcome.segments_opened(), 0, "no segment meets the range");
    assert_eq!(
        trace.count(Phase::Headers),
        SEGMENTS,
        "and the plan still examined every segment of the store: pruning is cheap and it is not \
         free, which is the one phase a query pays for whether or not it reads"
    );
    assert_eq!(
        trace.total(),
        SEGMENTS,
        "the whole of that run is the plan's decision"
    );
    for phase in [
        Phase::Blocks,
        Phase::Decompress,
        Phase::Decode,
        Phase::Filter,
        Phase::Aggregate,
        Phase::Render,
    ] {
        assert_eq!(
            trace.count(phase),
            0,
            "{phase:?} was charged for a query that read nothing"
        );
    }
    assert!(
        outcome.answers().is_empty(),
        "a range the store is entirely before answers nothing at all: {}",
        answers_of(&outcome)
    );
    assert_eq!(
        trace.count(Phase::Headers),
        outcome.segments_pruned(),
        "and every segment the plan examined is a segment the range ruled out"
    );
}

#[test]
fn a_run_charges_the_phases_it_walked_and_the_identities_hold() {
    let (_dir, store) = sealed("identities", &workload());
    let snapshot = store.snapshot();
    // Half the store, one host, one answer per hour: the manifest rules half of it out by the range
    // and the index eleven segments more, so one segment is opened.
    let outcome = asked(&snapshot, &selecting(&format!("h{SELECTIVE}"), 6, 12));
    let trace = outcome.trace();

    // The plan's phase, tied to the counters `lakelet/4.14-index-pruning` reports: every segment the
    // plan examined is opened, pruned by the range or pruned by the index — never two of them, and
    // never none.
    assert_eq!(
        trace.count(Phase::Headers),
        outcome.segments_opened() + outcome.segments_pruned() + outcome.blocks_pruned(),
        "the phase that decided about the store examined all of it"
    );
    assert_eq!(trace.count(Phase::Headers), SEGMENTS);

    // Reading: a block is read and then inflated, so the pair moves together — and the read is what
    // the plan kept, not what the store holds.
    assert_eq!(
        trace.count(Phase::Blocks),
        outcome.segments_opened(),
        "one block per segment file that was read, and a segment file holds one block"
    );
    assert!(
        trace.count(Phase::Blocks) >= trace.count(Phase::Decompress),
        "every block that was read was inflated, unless the seal refused it: blocks {} against \
         decompress {}",
        trace.count(Phase::Blocks),
        trace.count(Phase::Decompress)
    );
    assert_eq!(
        trace.count(Phase::Blocks),
        1,
        "one segment of twelve was worth reading"
    );

    // Decoding: one per record taken out of a block that was read, before any predicate is asked.
    assert_eq!(
        trace.count(Phase::Decode),
        outcome.rows_scanned(),
        "the outcome's own count of what was read is the decode phase"
    );
    assert_eq!(
        trace.count(Phase::Decode),
        60,
        "the segment the plan kept holds sixty records"
    );

    // The pipeline's identities, on the same run: the predicate keeps a subset of what was decoded,
    // the window pass folds in a subset of what the predicate kept, and a bucket holds at least one
    // value.
    let decode = trace.count(Phase::Decode);
    let filter = trace.count(Phase::Filter);
    let aggregate = trace.count(Phase::Aggregate);
    let render = trace.count(Phase::Render);
    assert!(
        decode >= filter && filter >= aggregate && aggregate >= render,
        "a record has to be decoded to be offered to a predicate, kept to be offered to the window \
         pass, and folded in to make a bucket: decode {decode}, filter {filter}, aggregate \
         {aggregate}, render {render}"
    );
    assert_eq!(
        filter, 30,
        "the thirty records of the host the query asked for"
    );
    assert_eq!(
        aggregate, filter,
        "every value the predicate kept was inside the range this time, so the two phases agree"
    );
    assert_eq!(
        aggregate,
        outcome.rows_answered(),
        "and every value that was folded in landed in a bucket"
    );
    assert_eq!(render, 1, "one hour of records is one bucket");
    assert_eq!(
        filter,
        outcome.series_matched() * 30,
        "one series, thirty records of it"
    );
}

#[test]
fn the_phases_move_when_the_index_is_taken_out_of_the_plan() {
    let (dir, store) = sealed("without-index", &workload());
    let snapshot = store.snapshot();
    let text = selecting(&format!("h{SELECTIVE}"), 6, 12);

    let kept = asked(&snapshot, &text);
    dir.lose_the_index();
    let lost = asked(&store.snapshot(), &text);

    // An index is derived state (`lakelet/4.14-index-pruning`), so the answer is the same answer.
    assert_eq!(
        answers_of(&lost),
        answers_of(&kept),
        "a store that lost its index answers what it answered"
    );
    assert_eq!(
        kept.blocks_pruned(),
        5,
        "five of the six segments it met held no `h7`"
    );
    assert_eq!(
        lost.blocks_pruned(),
        0,
        "with the index directory gone there is nothing to prune by"
    );

    // And this is what the pruning was worth, in the phases that paid for it: six segments read
    // where one was enough.
    let (kept_trace, lost_trace) = (kept.trace(), lost.trace());
    assert_eq!(
        kept_trace.count(Phase::Headers),
        lost_trace.count(Phase::Headers),
        "the plan examined the same store both times"
    );
    for phase in [Phase::Blocks, Phase::Decompress, Phase::Decode] {
        assert!(
            lost_trace.count(phase) > kept_trace.count(phase),
            "{phase:?} did not move when the index was taken out: {} against {} — the phase the \
             index saved work in is the phase that says so",
            lost_trace.count(phase),
            kept_trace.count(phase)
        );
    }
    assert_eq!(
        kept_trace.count(Phase::Blocks),
        1,
        "the index kept five of the six segments the range met closed"
    );
    assert_eq!(
        lost_trace.count(Phase::Blocks),
        6,
        "and now all six are read"
    );
    assert_eq!(
        lost_trace.count(Phase::Decode),
        6 * 60,
        "six segments of sixty records, where one segment was enough"
    );

    // What did *not* move is the other half of the story: the index saved reads, not records, so
    // every phase after the decode is the same work — and the profile is what says which part of the
    // run was paying.
    for phase in [Phase::Filter, Phase::Aggregate, Phase::Render] {
        assert_eq!(
            lost_trace.count(phase),
            kept_trace.count(phase),
            "{phase:?} changed with the index, so the work it counts is not what the index saved"
        );
    }
    assert_eq!(
        lost_trace.shares(),
        lost_trace.shares(),
        "the same run answers the same profile twice"
    );
}

#[test]
fn a_record_the_range_kept_out_is_filtered_and_not_aggregated() {
    let (_dir, store) = sealed("half-hour", &[segment_records(SELECTIVE)]);
    let snapshot = store.snapshot();
    // One segment of thirty samples every two minutes, and a range that starts halfway through its
    // hour: every record passes the predicate, half of them are inside the range.
    let text = format!(
        "SELECT count(value) FROM cpu WHERE host = 'h{SELECTIVE}' AND ts BETWEEN {} AND {} \
         GROUP BY time(1h), series",
        EPOCH + SELECTIVE * HOUR + 30 * MINUTE,
        EPOCH + (SELECTIVE + 1) * HOUR
    );
    let outcome = asked(&snapshot, &text);
    let trace = outcome.trace();

    assert_eq!(
        trace.count(Phase::Decode),
        60,
        "the segment holds sixty records and both series were decoded"
    );
    assert_eq!(
        trace.count(Phase::Filter),
        30,
        "the predicates kept the segment's own host, wherever in the hour it sits: filtering is the \
         predicate's question"
    );
    assert_eq!(
        trace.count(Phase::Aggregate),
        15,
        "and the window pass kept the fifteen samples the range reaches"
    );
    assert!(
        trace.count(Phase::Filter) > trace.count(Phase::Aggregate),
        "nothing is aggregated that was not kept, and a kept record can still be outside the range: \
         filter {} against aggregate {}",
        trace.count(Phase::Filter),
        trace.count(Phase::Aggregate)
    );
    assert_eq!(
        trace.count(Phase::Aggregate),
        outcome.rows_answered(),
        "what was folded in is what the answer says landed in a bucket"
    );
    assert_eq!(trace.count(Phase::Render), 1, "one window held a record");
}

#[test]
fn the_same_question_costs_the_same_work_twice() {
    let (_first_dir, first) = sealed("same-work", &workload());
    // The same store, sealed again in another directory: a profile is a property of the question and
    // the store, and of nothing about the process that asked it.
    let (_second_dir, second) = sealed("same-work-again", &workload());
    let text = selecting(&format!("h{SELECTIVE}"), 6, 12);

    let one = asked(&first.snapshot(), &text);
    let twice = asked(&first.snapshot(), &text);
    assert_eq!(
        one.trace(),
        twice.trace(),
        "the same question over the same store is the same work: nothing in a trace reads a clock, a \
         machine or a counter of this process"
    );
    assert_eq!(one.trace(), asked(&second.snapshot(), &text).trace());
    assert_eq!(one.trace().shares(), twice.trace().shares());

    // And the same question over *another* store costs what this one did: a count is a number two
    // machines agree about, which is the whole reason a budget is written in counts
    // (`jobq/3.22-bench-budget`).
    assert_eq!(one.trace().total(), twice.trace().total());
}

#[test]
fn a_query_that_answers_nothing_still_worked() {
    let (dir, store) = sealed("answered-nothing", &workload());
    // A host no segment carries, of the metric every segment does. The index would rule every
    // segment out — nothing may hold `h99` — so the store loses it, and the query reads the six
    // segments its range meets and keeps nothing at all.
    dir.lose_the_index();
    let outcome = asked(
        &store.snapshot(),
        &selecting(&format!("h{}", SEGMENTS + 1), 6, 12),
    );
    let trace = outcome.trace();

    assert_eq!(trace.count(Phase::Blocks), 6, "the range met six segments");
    assert_eq!(
        trace.count(Phase::Decode),
        6 * 60,
        "and every record of them was decoded, before any predicate was asked"
    );
    assert_eq!(
        trace.count(Phase::Filter),
        0,
        "nothing the query read carried the pair it asked for"
    );
    assert_eq!(trace.count(Phase::Aggregate), 0);
    assert_eq!(
        trace.count(Phase::Render),
        0,
        "an answer with no bucket in it wrote no bucket, whatever it cost to find that out"
    );
    assert!(
        outcome.answers().is_empty(),
        "and the answer is empty: {}",
        answers_of(&outcome)
    );
    assert_eq!(
        outcome.series_matched(),
        0,
        "no series the record carried was the one the query named"
    );
}

#[test]
fn a_window_that_held_nothing_is_not_a_bucket() {
    let (_dir, store) = sealed("windows", &workload());
    let snapshot = store.snapshot();
    // A three-hour range that meets one segment: three windows of the grid, one of them holding
    // records.
    let text = selecting(&format!("h{SELECTIVE}"), SELECTIVE, SELECTIVE + 3);
    let query = parsed(&text);
    let plan = Plan::of(&snapshot, &query)
        .unwrap_or_else(|refusal| panic!("`{text}` is a plan against this store: {refusal}"));
    assert_eq!(
        plan.windows(),
        3,
        "the range touches three windows of an hour"
    );
    assert_eq!(
        plan.opened().len(),
        1,
        "and one segment of the store holds records in it"
    );

    let outcome = asked(&snapshot, &text);
    let trace = outcome.trace();
    assert_eq!(
        trace.count(Phase::Aggregate),
        30,
        "the values that were folded into a window"
    );
    assert_eq!(
        trace.count(Phase::Render),
        1,
        "one bucket: the windows a plan says a range touches and the buckets an answer has are two \
         different counts (`lakelet/4.10-downsampling`), and rendering is the second"
    );
    assert_eq!(
        outcome
            .answers()
            .iter()
            .map(|answer| answer.buckets().len())
            .sum::<usize>(),
        1,
        "and the answer is where that is visible"
    );
}

#[test]
fn the_command_line_prints_one_number_per_phase() {
    let (dir, store) = sealed("json", &workload());
    let text = selecting(&format!("h{SELECTIVE}"), 6, 12);
    let store_path = dir.argument();
    let mut arguments = sql(&store_path, &text);
    arguments.push("--json");
    let ran = lakeletq(&arguments);
    assert_eq!(ran.code, 0, "a query is an answer: {ran:?}");
    assert_eq!(ran.stderr, "", "and not a diagnosis");
    let document: Value = serde_json::from_str(&ran.stdout)
        .unwrap_or_else(|error| panic!("`--json` prints one document: {error}"));
    let profile = document
        .get("trace")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("the document carries the run's profile: {document}"));

    let outcome = asked(&store.snapshot(), &text);
    for phase in Phase::ALL {
        let value = profile
            .get(phase.name())
            .unwrap_or_else(|| panic!("the profile states `{}`: {document}", phase.name()));
        assert!(
            value.is_number(),
            "every phase is a number and not a word: {} is {value}",
            phase.name()
        );
        assert_eq!(
            value,
            &Value::from(outcome.trace().count(phase)),
            "and the number is the library's own count of `{}`: a script that reads a profile reads \
             the run that happened",
            phase.name()
        );
    }
    assert_eq!(
        profile.len(),
        Phase::ALL.len(),
        "seven phases and nothing else: {document}"
    );

    // The keys are written in the order the phases ran, which is the order `Phase::ALL` states and
    // the order the harness prints its lines in.
    let mut at = 0usize;
    for phase in Phase::ALL {
        let found = ran
            .stdout
            .find(&format!("\"{}\"", phase.name()))
            .unwrap_or_else(|| panic!("the document spells `{}`: {}", phase.name(), ran.stdout));
        assert!(
            found > at,
            "a profile is written in the order the phases ran, and `{}` is out of it: {}",
            phase.name(),
            ran.stdout
        );
        at = found;
    }
    assert!(
        ran.stdout.find("\"trace\"").unwrap_or(usize::MAX)
            < ran.stdout.find("\"buckets\"").unwrap_or(0),
        "the profile sits beside the answer rather than inside it: {}",
        ran.stdout
    );
}

#[test]
fn the_answer_document_grew_and_the_flag_forms_array_did_not() {
    let (dir, _store) = sealed("shapes", &workload());
    let store_path = dir.argument();

    // The text form's document is where a run's own account of itself goes, beside the counters
    // `lakelet/4.11-query-engine` and `lakelet/4.14-index-pruning` put there.
    let text = selecting(&format!("h{SELECTIVE}"), 6, 12);
    let mut documented = sql(&store_path, &text);
    documented.push("--json");
    let text_form = lakeletq(&documented);
    assert_eq!(text_form.code, 0, "{text_form:?}");
    let document: Value = serde_json::from_str(&text_form.stdout)
        .unwrap_or_else(|error| panic!("the text form's `--json` is one document: {error}"));
    assert!(
        document.get("trace").is_some() && document.get("buckets").is_some(),
        "one document with the profile and the buckets in it: {document}"
    );

    // The flag form is `lakelet/4.10-downsampling`'s shape: an array of bucket objects, and this
    // step does not put a profile in it. The flag form answers with buckets and says nothing about
    // what the run cost, and a key added there would be a second answer to a question that form does
    // not ask.
    let from = (EPOCH + 6 * HOUR).to_string();
    let to = (EPOCH + 12 * HOUR).to_string();
    let flag_form = lakeletq(&[
        "query",
        "--store",
        &store_path,
        "--from",
        &from,
        "--to",
        &to,
        "--window",
        "1h",
        "--agg",
        "count",
        "--json",
    ]);
    assert_eq!(flag_form.code, 0, "{flag_form:?}");
    let shown: Value = serde_json::from_str(&flag_form.stdout)
        .unwrap_or_else(|error| panic!("the flag form's `--json` is one document: {error}"));
    let buckets = shown
        .as_array()
        .unwrap_or_else(|| panic!("the flag form's answer is the array it was in 4.10: {shown}"));
    assert!(!buckets.is_empty(), "and this question has an answer");
    for bucket in buckets {
        assert!(
            bucket.get("trace").is_none() && bucket.get("start").is_some(),
            "a bucket is one window's answer and not a report about the run: {bucket}"
        );
    }
}

#[test]
fn the_page_states_the_profile_the_lane_prints() {
    let page = page();
    let low = page.to_ascii_lowercase();
    for placeholder in ["todo", "tbd", "fixme"] {
        assert!(
            !low.contains(placeholder),
            "{PAGE} still says `{placeholder}`: a profile page with a blank in it is a page nobody \
             has written down"
        );
    }
    for phase in Phase::ALL {
        assert!(
            page.contains(phase.name()),
            "{PAGE} does not state `{}`, which is a number the harness prints and the gate compares",
            phase.name()
        );
    }
    assert!(
        page.contains("cargo test -p lakelet --test attribution -- --nocapture"),
        "{PAGE} does not say how to read its own numbers: the harness's command is the way from the \
         page to the profile"
    );
    assert!(
        page.contains("blocks_pruned"),
        "{PAGE} does not tie `headers` to the pruning counters `lakelet/4.14-index-pruning` reports, \
         which is the identity that makes the plan's phase checkable against the store"
    );
}

#[test]
fn the_trace_is_engine_code_and_counts_work_rather_than_time() {
    // The boundary `jobq/3.1-engine-boundary` established and `lakelet/4.9-retention` reads out of
    // the sources: the engine may not name the service layer or the command line, and a phase
    // counter is engine code.
    for relative in [
        "src/engine/trace/mod.rs",
        "src/engine/trace/counters.rs",
        "src/engine/query/exec.rs",
    ] {
        let text = source(relative);
        for forbidden in ["crate::service", "crate::cli"] {
            assert!(
                !text.contains(forbidden),
                "{relative} names `{forbidden}`, so the counting has reached out of the engine"
            );
        }
        // And it counts work rather than time: a profile that measured the machine would be a
        // profile two machines disagree about, which is the one thing a budget in this project may
        // not be (`jobq/3.22-bench-budget`).
        for timed in ["Instant", "SystemTime", "std::time", "elapsed"] {
            assert!(
                !text.contains(timed),
                "{relative} names `{timed}`: the trace counts work and never time"
            );
        }
    }
}
