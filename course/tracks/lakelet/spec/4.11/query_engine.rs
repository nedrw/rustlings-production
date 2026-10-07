//! Spec for `lakelet/4.11-query-engine`: the grammar a query is written in, the plan it makes, and
//! the refusals that matter as much as the answers.
//!
//! `kiln run` copies this file into the project as `tests/spec_query_engine.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Eighteen cases, and the shape of them is the point:
//!
//! * **a series name is read, never validated.** The ingest protocol accepts arbitrary bytes, so
//!   `Series::of` is total: the metric is what is before the first comma, a `<key>=<value>` with a
//!   key is a tag, anything else is not a tag and never matches, and matching is exact — case
//!   included. A name no client would have chosen is still a name in the store.
//! * **the scanner reads four kinds of token and refuses by position.** Keywords keep their case
//!   and are compared without it; a `'…'` value may contain `=`, `,` and spaces; an unterminated
//!   value is refused at the quote that opened it, and the offset is the one the reader can count
//!   to.
//! * **every refusal is typed and carries its class.** One case per shape, each asserting the
//!   *variant* — not the sentence — and that `class()` is the exit code the binary really leaves
//!   behind: `2` for the text the caller wrote, `3` for a metric the store has never held.
//! * **the time predicate is mandatory and half-open.** A query without `ts BETWEEN` is refused; a
//!   record whose timestamp is exactly `from` is in the answer and one whose timestamp is exactly
//!   `to` is not, asserted by moving the range's end onto that record and watching it appear.
//! * **`OR` and a limit without `GROUP BY …, series` are refused**, because the first is the clause
//!   whose absence changes what a plan may prune and the second bounds nothing.
//! * **the plan is made from the manifest alone.** A store whose segments were sealed in known
//!   windows gives an exact `segments_opened` / `segments_pruned`, and `EXPLAIN` answers a store
//!   whose segment files are unreadable — the strongest form of "it opened nothing", because the
//!   files it did not open are *gone*: a query that prunes one such segment away still answers, and
//!   one that opens it exits `4`.
//! * **execution reuses the window pass.** The buckets an answer holds are the buckets
//!   `lakelet/4.10-downsampling`'s `downsample` computes for the same records — compared, not
//!   assumed — and the groups come out in series-name order, up to `LIMIT`.
//! * **a metric the store never held is not a quiet fleet.** The same range and the same store are
//!   asked about a metric whose records are all outside the range (an empty answer, `0`) and about
//!   a metric this store has never seen (`3`).
//! * **a range below the floor is refused in a sentence that names the floor**, while the flag form
//!   `lakelet/4.10-downsampling` shipped still answers what the store keeps: the grammar is
//!   stricter, and the earlier contract is intact.
//! * **the two input forms answer the same question the same way.** The flag form and the `--sql`
//!   form are compared byte for byte on one question, every failing call is run twice — with and
//!   without `--json` — and the text form's shapes (`<series> <start> <count> <value>`, the counts
//!   beside the buckets, one object of `EXPLAIN`'s keys) are asserted rather than described.
//! * **the boundary holds.** `src/engine/**` still may not name the service layer or the command
//!   line, and the page a reader of `--help` opens (`docs/query.md`) exists and every link in it
//!   resolves.
//!
//! Nothing here sleeps, nothing here reads a clock, and nothing here measures a duration: every
//! range, window and `now` is a number the case passes in.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use lakelet::cli::{self, MAX_WINDOWS, USAGE};
use lakelet::engine::compaction::{Segments, Snapshot, file_of};
use lakelet::engine::query::downsample::{self, Aggregate, Bucket, Range, Window, agree};
use lakelet::engine::query::exec::{Outcome, execute};
use lakelet::engine::query::lex::{Kind, Refusal, Token, lex};
use lakelet::engine::query::parse::{Query, Statement, parse, unit_millis};
use lakelet::engine::query::plan::{COLUMNS, Plan};
use lakelet::engine::query::series::Series;
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
/// `1_699_999_200_000` is `472_222 * HOUR` exactly: every window, segment span and range a case
/// names starts on a number a reader can check by hand, which is the whole reason the grid is
/// absolute.
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

/// The starts of a bucket list, in the order it holds them.
fn starts(buckets: &[Bucket]) -> Vec<u64> {
    buckets.iter().map(Bucket::start).collect()
}

/// The counts of a bucket list.
fn counts(buckets: &[Bucket]) -> Vec<u64> {
    buckets.iter().map(Bucket::count).collect()
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
            "lakelet-query-engine-spec-{label}-{}",
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

    /// Overwrite segment `index`'s file with bytes of the same length.
    ///
    /// The length matters and is the whole trick: a store whose segment is *shorter* than the index
    /// states refuses to open (`lakelet/4.9-retention`), so a file that is the right length and the
    /// wrong format is the only way to have a store that opens, plans and cannot be read — which is
    /// what tells "the plan reads no segment" apart from "the plan happened to like this one".
    fn spoil(&self, index: u64) {
        let path = file_of(&self.path, index);
        let length = std::fs::metadata(&path)
            .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()))
            .len();
        let bytes = vec![0u8; usize::try_from(length).unwrap_or(0)];
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

/// The query one text parses to, for the cases that ask the engine rather than the binary.
fn parsed(text: &str) -> Query {
    match parse(text) {
        Ok(Statement::Query(query)) | Ok(Statement::Explain(query)) => query,
        Err(refusal) => {
            panic!("`{text}` is a query this grammar has, and it was refused: {refusal}")
        }
    }
}

/// The refusal one text is refused with, for the cases that assert the shape rather than a run.
fn refusal_of(text: &str) -> Refusal {
    match parse(text) {
        Ok(_) => panic!("`{text}` is not a query this grammar has, and it was accepted"),
        Err(refusal) => refusal,
    }
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

/// The command line that asks `text` about the store at `store`.
fn sql<'a>(store: &'a str, text: &'a str) -> Vec<&'a str> {
    vec!["query", "--store", store, "--sql", text]
}

/// The query one case asks, in both input forms: the flags `lakelet/4.10-downsampling` shipped.
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

/// The answer one text gets from one snapshot, for the cases about what an answer holds.
fn asked(snapshot: &Snapshot, text: &str) -> Outcome {
    execute(snapshot, &parsed(text)).unwrap_or_else(|refusal| {
        panic!("`{text}` is answerable against this store, and it was refused: {refusal}")
    })
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

#[test]
fn a_window_is_a_number_and_one_of_five_units() {
    for (window, millis) in [
        ("1500ms", 1_500u64),
        ("90s", 90_000),
        ("5m", 300_000),
        ("2h", 7_200_000),
        ("1d", 86_400_000),
    ] {
        let text = format!(
            "SELECT count(value) FROM cpu WHERE ts BETWEEN 0 AND {millis} GROUP BY time({window})"
        );
        assert_eq!(
            parsed(&text).window().width_millis(),
            millis,
            "`time({window})` is {millis} milliseconds"
        );
    }

    // The table is the *grammar's*, and it is public because the command line asks it instead of
    // carrying a second copy: `--window 5m` and `time(5m)` are one rule, and one place to answer
    // what `90x` means.
    for (unit, millis) in [
        ("ms", 1u64),
        ("s", 1_000),
        ("m", 60_000),
        ("h", 3_600_000),
        ("d", 86_400_000),
    ] {
        assert_eq!(unit_millis(unit), Some(millis), "`{unit}` is a unit");
    }
    assert_eq!(
        unit_millis("x"),
        None,
        "and a word that is not one is not one"
    );
    assert_eq!(unit_millis("M"), None, "a unit is exact: `M` is not `m`");

    // Two windows are refused before a store is opened: one with no bucket in it, and one wider
    // than the millisecond count this store states.
    match refusal_of("SELECT count(value) FROM cpu WHERE ts BETWEEN 0 AND 1000 GROUP BY time(0ms)")
    {
        Refusal::Syntax { detail, .. } => {
            assert!(
                detail.contains("no bucket"),
                "the sentence says it: `{detail}`"
            );
        }
        other => panic!("a window of zero is a syntax refusal, and this is {other:?}"),
    }
    match refusal_of(
        "SELECT count(value) FROM cpu WHERE ts BETWEEN 0 AND 1000 \
         GROUP BY time(18446744073709551615d)",
    ) {
        Refusal::Syntax { detail, .. } => assert!(
            detail.contains("wider"),
            "the sentence says what is wrong with it: `{detail}`"
        ),
        other => panic!("a window wider than a u64 of milliseconds is refused: {other:?}"),
    }
}

/// The series names a grouped answer covers, in the order it holds them.
fn named(outcome: &Outcome) -> Vec<&str> {
    outcome
        .answers()
        .iter()
        .map(|answer| {
            answer.series().unwrap_or_else(|| {
                panic!("this answer is from a query that groups by series, and it names none")
            })
        })
        .collect()
}

#[test]
fn a_series_name_is_a_metric_and_the_tags_it_carries() {
    // The shape clients write, read as the two things a query names it by.
    let named = Series::of("cpu,host=h0,dc=east");
    assert_eq!(
        named.metric(),
        "cpu",
        "the metric is everything before the first comma"
    );
    assert_eq!(
        named.tags(),
        &[("host", "h0"), ("dc", "east")],
        "and the rest are the tag pairs, in the order they were written"
    );
    assert!(
        named.matches("host", "h0"),
        "a tag the series carries matches"
    );
    assert!(named.matches("dc", "east"));
    assert!(
        !named.matches("host", "h1"),
        "one it does not carry does not"
    );
    assert!(
        !named.matches("Host", "h0"),
        "and matching is exact — a filter that folded case would put one host's samples behind \
         another's name (`lakelet/4.10-downsampling` argues the same rule)"
    );

    // No comma: a metric and nothing else.
    let plain = Series::of("cpu");
    assert_eq!(plain.metric(), "cpu");
    assert!(plain.tags().is_empty(), "a name with no comma has no tags");
    assert!(
        !plain.matches("cpu", ""),
        "and a query for an empty tag does not select it"
    );

    // A pair splits at its *first* `=`, so a value may contain one.
    let odd = Series::of("cpu,host=h0,x=y=z");
    assert_eq!(odd.tags(), &[("host", "h0"), ("x", "y=z")]);

    // A malformed pair is not a tag, and never matches anything: `host` and `=h0` are bytes a
    // client may well have written, and a query may not read a tag into them.
    let malformed = Series::of("cpu,host,,=h0,east");
    assert_eq!(malformed.metric(), "cpu");
    assert!(
        malformed.tags().is_empty(),
        "a pair with no `=`, a pair with no key and a bare word are not tags: `{:?}`",
        malformed.tags()
    );
    assert!(!malformed.matches("host", ""));
    assert!(!malformed.matches("", "h0"));
    assert!(!malformed.matches("east", ""));

    // An empty value *is* a value, and the same key may appear twice.
    assert!(Series::of("cpu,host=").matches("host", ""));
    let twice = Series::of("cpu,host=h0,host=h1");
    assert_eq!(twice.tags().len(), 2, "the same key twice is two tags");
    assert!(twice.matches("host", "h0") && twice.matches("host", "h1"));
    assert!(!twice.matches("host", "h2"));

    // Total, including the names nobody meant to write.
    assert_eq!(Series::of("").metric(), "");
    assert!(Series::of("").tags().is_empty());
    assert_eq!(Series::of(",host=h0").metric(), "");
    assert!(Series::of(",host=h0").matches("host", "h0"));
}

#[test]
fn the_scanner_reads_words_numbers_values_and_the_four_symbols() {
    // A word, and the case it keeps: the grammar folds case where it *compares*, not where it
    // reads, so a metric named `CPU` and the keyword `SELECT` are both still the bytes written.
    let one = lex("select").unwrap_or_else(|error| panic!("one word is lexable: {error}"));
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].at(), 0, "a token remembers the byte it started at");
    assert_eq!(one[0].kind(), &Kind::Word("select".to_owned()));
    assert!(
        one[0].spells("SELECT"),
        "and a keyword is compared without case"
    );
    assert!(one[0].spells("Select"));
    assert!(!one[0].spells("selec"));
    assert!(
        !one[0].spells("ts"),
        "a word that is not this keyword is not this keyword"
    );

    // The four symbols, the numbers of the range, and a value that contains the characters the
    // grammar would otherwise read.
    let text = "MEAN(value) host = 'a=b,c d' 1700000000000";
    let tokens = lex(text).unwrap_or_else(|error| panic!("`{text}` is lexable: {error}"));
    let kinds: Vec<&Kind> = tokens.iter().map(Token::kind).collect();
    assert_eq!(
        kinds,
        [
            &Kind::Word("MEAN".to_owned()),
            &Kind::Symbol('('),
            &Kind::Word("value".to_owned()),
            &Kind::Symbol(')'),
            &Kind::Word("host".to_owned()),
            &Kind::Symbol('='),
            &Kind::Text("a=b,c d".to_owned()),
            &Kind::Number(1_700_000_000_000),
        ],
        "in the order they were written, with the quotes taken off the value"
    );
    assert_eq!(tokens[1].at(), 4, "`(` is the fifth byte");
    let quote = text
        .find('\'')
        .unwrap_or_else(|| panic!("this case's text holds a quote"));
    assert_eq!(
        tokens[6].at(),
        quote,
        "and the value starts at the quote a reader can count to"
    );
    assert!(
        tokens[6].kind() == &Kind::Text("a=b,c d".to_owned()),
        "a value may hold `=`, `,` and spaces: `{text}`"
    );

    // Whitespace separates and is otherwise nothing; a name may hold the characters a metric name
    // is usually made of, because the store holds whatever clients wrote.
    let spaced = lex("a  b\n\tcpu.load-1").unwrap_or_else(|error| panic!("lexable: {error}"));
    let words: Vec<&str> = spaced
        .iter()
        .map(|token| match token.kind() {
            Kind::Word(word) => word.as_str(),
            other => panic!("words, and this is {other:?}"),
        })
        .collect();
    assert_eq!(words, ["a", "b", "cpu.load-1"]);

    // A window is a number and the unit right after it.
    let window = lex("5m").unwrap_or_else(|error| panic!("lexable: {error}"));
    assert_eq!(window[0].kind(), &Kind::Number(5));
    assert_eq!(window[1].kind(), &Kind::Word("m".to_owned()));

    // And a run of digits this store cannot state is refused rather than wrapped.
    let Err(too_large) = lex("99999999999999999999999") else {
        panic!("a number larger than a u64 is not a millisecond count this store states")
    };
    assert!(matches!(too_large, Refusal::Syntax { .. }));
    assert_eq!(too_large.class(), 2);
}

#[test]
fn an_unterminated_value_is_refused_at_the_quote_that_opened_it() {
    let text = "SELECT count(value) FROM cpu WHERE host = 'h0 GROUP BY time(1m)";
    let Err(refusal) = lex(text) else {
        panic!("a value that never closes is not a query this grammar reads")
    };
    let quote = text
        .find('\'')
        .unwrap_or_else(|| panic!("this case's text holds a quote"));
    match &refusal {
        Refusal::Syntax { at, detail } => {
            assert_eq!(
                *at, quote,
                "the refusal names the quote that opened the value — byte {quote} — rather than the \
                 end of the text a reader would have to search"
            );
            assert!(
                detail.contains("never closed"),
                "and says what is wrong with it: `{detail}`"
            );
        }
        other => panic!("an unterminated value is a syntax refusal, and this is {other:?}"),
    }
    assert_eq!(refusal.class(), 2, "the caller's class");
    let sentence = refusal.to_string();
    assert!(
        sentence.contains(&quote.to_string()),
        "the sentence a caller reads names the byte too: `{sentence}`"
    );
}

#[test]
fn every_refusal_is_a_variant_and_the_class_is_what_the_binary_exits_with() {
    let (dir, _store) = sealed("refusals", &[series("cpu,host=h0", 0, STEP, 4, |_| 1.0)]);
    let store = dir.argument();
    let head = "SELECT count(value) FROM cpu";

    // One case per shape of "this is not a query this store can be asked", with the variant it
    // arrives as. Every one of these is the *caller's* class (`2`); the refusals that are about
    // the store rather than about the text are the two cases that follow this one.
    let cases: Vec<(String, &str, &str)> = vec![
        (
            "UPDATE cpu SET value = 1".to_owned(),
            "a word that is not this statement",
            "syntax",
        ),
        (
            format!("{head} WHERE host = 'h0' GROUP BY time(1m)"),
            "no time predicate at all",
            "syntax",
        ),
        (
            format!("{head} WHERE ts BETWEEN 0 AND 1"),
            "no `GROUP BY time(<window>)`",
            "syntax",
        ),
        (
            format!("{head} WHERE ts BETWEEN 0 AND 1 GROUP BY time(1m) GROUP BY time(1m)"),
            "`GROUP BY` twice",
            "syntax",
        ),
        (
            format!("{head} WHERE ts BETWEEN 0 AND 1 GROUP BY time(1m) WHERE host = 'a'"),
            "a clause after the last one",
            "syntax",
        ),
        (
            "SELECT average(value) FROM cpu WHERE ts BETWEEN 0 AND 1 GROUP BY time(1m)".to_owned(),
            "an aggregate nobody has",
            "aggregate",
        ),
        (
            format!("{head} WHERE ts BETWEEN 5 AND 5 GROUP BY time(1m)"),
            "a range that ends where it starts",
            "syntax",
        ),
        (
            format!("{head} WHERE ts BETWEEN 0 AND 1 GROUP BY time(0ms)"),
            "a window with no bucket in it",
            "syntax",
        ),
        (
            format!("{head} WHERE ts BETWEEN 0 AND 1 GROUP BY time(90x)"),
            "a unit nobody has",
            "syntax",
        ),
        (
            format!("{head} WHERE ts BETWEEN 0 AND 1 GROUP BY time(1m) LIMIT 3"),
            "a limit without `series`",
            "limit",
        ),
        (
            format!("{head} WHERE ts BETWEEN 0 AND 1 GROUP BY time(1m), series LIMIT 0"),
            "a limit of zero",
            "limit",
        ),
        (
            format!("{head} WHERE ts BETWEEN 0 AND 1 OR ts BETWEEN 2 AND 3 GROUP BY time(1m)"),
            "an `OR` between two time ranges",
            "disjunction",
        ),
        (
            format!(
                "{head} WHERE host = 'a' OR host = 'b' AND ts BETWEEN 0 AND 1 GROUP BY time(1m)"
            ),
            "an `OR` between two tag predicates",
            "disjunction",
        ),
        (
            format!("{head} WHERE ts = '5' GROUP BY time(1m)"),
            "a predicate on `ts`",
            "syntax",
        ),
        (
            format!("{head} WHERE host = h0 AND ts BETWEEN 0 AND 1 GROUP BY time(1m)"),
            "a value that is not quoted",
            "syntax",
        ),
        (
            format!("{head} WHERE ts BETWEEN 0 AND 1 GROUP BY time(1m), series LIMIT many"),
            "a limit that is not a number",
            "syntax",
        ),
        (
            format!("{head} WHERE ts BETWEEN 0 AND 1 GROUP BY time(1m) FROM cpu"),
            "the metric named twice",
            "syntax",
        ),
    ];

    for (text, what, variant) in &cases {
        let refusal = refusal_of(text);
        let found = match &refusal {
            Refusal::Syntax { .. } => "syntax",
            Refusal::Aggregate { .. } => "aggregate",
            Refusal::Disjunction { .. } => "disjunction",
            Refusal::Limit { .. } => "limit",
            Refusal::Forgot { .. } => "forgot",
            Refusal::Unknown { .. } => "unknown",
            Refusal::Store { .. } => "store",
        };
        assert_eq!(
            found, *variant,
            "`{text}` is a `{variant}` refusal ({what}), and it is {refusal}"
        );
        assert_eq!(
            refusal.class(),
            2,
            "`{text}` ({what}) is the caller's class: {refusal}"
        );

        // The same text through the real binary, in both shapes: the code and the diagnosis are
        // the same either way, and an exit code on its own is what a script branches on.
        let args = sql(&store, text);
        let as_text = lakeletq(&args);
        refused(&as_text, 2, what);
        let mut with_json = args.clone();
        with_json.push("--json");
        let as_json = lakeletq(&with_json);
        refused(&as_json, 2, what);
        assert_eq!(
            as_json.stderr.trim(),
            as_text.stderr.trim(),
            "`--json` is a format and not a second command: `{text}` says the same thing either way"
        );
    }

    // One sentence says what `ts` *is*, because a predicate on it is a mistake about the shape of
    // the grammar rather than a word the reader will find by staring at it.
    match refusal_of(&format!("{head} WHERE ts = '5' GROUP BY time(1m)")) {
        Refusal::Syntax { detail, .. } => assert!(
            detail.contains("time axis"),
            "the sentence says what `ts` is: `{detail}`"
        ),
        other => panic!("a predicate on `ts` is a syntax refusal, and this is {other:?}"),
    }
    // And an `OR` is refused where a reader writes one, at the byte it stands on.
    match refusal_of(&format!(
        "{head} WHERE host = 'a' OR host = 'b' AND ts BETWEEN 0 AND 1 GROUP BY time(1m)"
    )) {
        Refusal::Disjunction { at } => assert!(at > 0, "the `OR` is not the first byte"),
        other => panic!("an `OR` is a disjunction refusal, and this is {other:?}"),
    }
}

#[test]
fn the_time_predicate_is_mandatory_and_its_upper_end_is_not_in_the_answer() {
    // Four samples half a minute apart: one segment, spanning `[EPOCH, EPOCH + 90s]`.
    let (_dir, store) = sealed(
        "half-open",
        &[series("cpu,host=h0", 0, 30_000, 4, |index| index as f64)],
    );
    let snapshot = store.snapshot();

    // The range is not optional, and the sentence says which clause is missing.
    let missing = refusal_of("SELECT count(value) FROM cpu GROUP BY time(1m)");
    assert!(matches!(missing, Refusal::Syntax { .. }));
    assert!(
        missing.to_string().contains("ts BETWEEN"),
        "the refusal names the clause the query has to have: {missing}"
    );

    // Half-open, asserted from both sides rather than declared: a range whose start is on a sample
    // answers it, and a range whose *end* is on one does not — until the range is moved past it,
    // at which point the very same record appears.
    let from = EPOCH + 30_000;
    let to = EPOCH + 90_000;
    let narrow = asked(
        &snapshot,
        &format!(
            "SELECT count(value) FROM cpu WHERE ts BETWEEN {from} AND {to} GROUP BY time(30s)"
        ),
    );
    assert_eq!(
        starts(&mixed(&narrow)),
        vec![EPOCH + 30_000, EPOCH + 60_000],
        "the windows the range touches, and not the one the record at `to` would fall in"
    );
    assert_eq!(
        counts(&mixed(&narrow)),
        vec![1, 1],
        "the record at `from` is in the answer and the one at `to` is not"
    );
    assert_eq!(
        mixed(&narrow).first().map(Bucket::start),
        Some(from),
        "a bucket that starts at `from` holds the record at `from`: the grid's edges and the \
         range's are the same numbers here, and a window owns its first millisecond"
    );

    let wider = asked(
        &snapshot,
        &format!(
            "SELECT count(value) FROM cpu WHERE ts BETWEEN {from} AND {} GROUP BY time(30s)",
            EPOCH + 120_000
        ),
    );
    assert_eq!(
        starts(&mixed(&wider)),
        vec![EPOCH + 30_000, EPOCH + 60_000, EPOCH + 90_000],
        "moving the end past the record that was at `to` answers it"
    );
    assert_eq!(counts(&mixed(&wider)), vec![1, 1, 1]);
}

#[test]
fn a_tag_predicate_selects_exactly_the_records_whose_series_carries_that_pair() {
    // Five series in *one* segment, so the case is about the filter rather than about pruning:
    // every record of the segment is read, and only the ones whose series carries the pair asked
    // about land in an answer.
    let mut batch: Vec<Record> = Vec::new();
    for (name, value) in [
        ("cpu,host=h0", 1.0),
        ("cpu,host=h0,dc=east", 2.0),
        ("cpu,host=h1", 3.0),
        ("cpu", 4.0),
        ("mem,host=h0", 5.0),
    ] {
        batch.extend(series(name, 0, MINUTE, 3, |_| value));
    }
    let (_dir, store) = sealed("selects", &[batch]);
    let from = EPOCH;
    let to = EPOCH + 3 * MINUTE;
    let text = format!(
        "SELECT sum(value) FROM cpu WHERE host = 'h0' AND ts BETWEEN {from} AND {to} \
         GROUP BY time(1m), series"
    );
    let query = parsed(&text);

    assert_eq!(
        query.metric(),
        "cpu",
        "`FROM` names the metric and not a series"
    );
    assert_eq!(
        query.predicates(),
        &[("host".to_owned(), "h0".to_owned())],
        "and the predicates are the pairs `WHERE` named"
    );
    assert_eq!(
        query.series_name(),
        "cpu,host=h0",
        "the name the grammar can spell out of them"
    );
    assert!(query.selects(&Series::of("cpu,host=h0")));
    assert!(
        query.selects(&Series::of("cpu,host=h0,dc=east")),
        "a series that carries the metric and *more* tags than the predicates named is selected: \
         the predicates are conditions, not a complete tag set"
    );
    assert!(!query.selects(&Series::of("cpu,host=h1")));
    assert!(
        !query.selects(&Series::of("cpu")),
        "a series that carries no such tag is not selected"
    );
    assert!(
        !query.selects(&Series::of("mem,host=h0")),
        "and a tag is not a metric: the same pair on another metric selects nothing"
    );
    assert!(
        !query.selects(&Series::of("cpu,host=H0")),
        "the value is exact, case included"
    );

    let outcome = asked(&store.snapshot(), &text);
    assert_eq!(
        named(&outcome),
        ["cpu,host=h0", "cpu,host=h0,dc=east"],
        "one answer per matching series, in name order"
    );
    assert_eq!(outcome.series_matched(), 2, "two series matched");
    assert_eq!(outcome.series_answered(), 2, "and both were answered");
    assert_eq!(
        outcome.segments_opened(),
        1,
        "one segment holds all five series"
    );
    assert_eq!(outcome.segments_pruned(), 0);
    assert_eq!(
        outcome.rows_scanned(),
        15,
        "every record of the segment was read: pruning is by segment, not by series"
    );
    assert_eq!(
        outcome.rows_answered(),
        6,
        "and only the two matching series' records landed in a bucket"
    );
    for (answer, sum) in outcome.answers().iter().zip([1.0, 2.0]) {
        assert_eq!(
            counts(answer.buckets()),
            vec![1, 1, 1],
            "one record a minute, and each series carries three"
        );
        assert!(
            agree(answer.buckets()[0].value(), sum),
            "and the sum is that series' own records: {} against {sum}",
            answer.buckets()[0].value()
        );
    }
}

#[test]
fn the_five_aggregates_are_the_window_passs_and_the_grammar_folds_their_names() {
    for (name, aggregate) in [
        ("count", Aggregate::Count),
        ("sum", Aggregate::Sum),
        ("mean", Aggregate::Mean),
        ("min", Aggregate::Min),
        ("max", Aggregate::Max),
    ] {
        let text =
            format!("SELECT {name}(value) FROM cpu WHERE ts BETWEEN 1 AND 2 GROUP BY time(1m)");
        assert_eq!(
            parsed(&text).aggregate(),
            aggregate,
            "`{name}` is an aggregate"
        );
        // Keywords are case-insensitive, aggregates are keywords: nothing about a query text is a
        // name that has to be spelled the way the code spells it.
        let shouted = format!(
            "SELECT {}(VALUE) FROM cpu WHERE TS BETWEEN 1 AND 2 GROUP BY TIME(1m)",
            name.to_ascii_uppercase()
        );
        assert_eq!(
            parsed(&shouted).aggregate(),
            aggregate,
            "`{shouted}` is the same query"
        );
    }

    // `lakelet/4.10-downsampling`'s own lookup stays exact: the folding belongs to the grammar,
    // which is where a caller's text is read, and the earlier contract is unchanged.
    assert_eq!(Aggregate::parse("mean"), Some(Aggregate::Mean));
    assert_eq!(
        Aggregate::parse("Mean"),
        None,
        "the value type's answer is exact, and the grammar folds before it asks"
    );

    // A near miss is refused *by name*, so the reader is sent to the word they typed.
    match refusal_of("SELECT avg(value) FROM cpu WHERE ts BETWEEN 1 AND 2 GROUP BY time(1m)") {
        Refusal::Aggregate { name, .. } => assert_eq!(name, "avg"),
        other => panic!("a name nobody has is an aggregate refusal, and this is {other:?}"),
    }
}

#[test]
fn the_plan_prunes_from_the_manifest_and_explain_opens_nothing() {
    // Four hours, ten samples a minute each, sealed an hour to a segment: every segment states the
    // span its records cover, which is the only fact a plan has.
    let batches: Vec<Vec<Record>> = (0..4u64)
        .map(|hour| series("cpu,host=h0", hour * HOUR, MINUTE, 10, |_| 1.0))
        .collect();
    let (dir, store) = sealed("plan", &batches);
    let store_path = dir.argument();
    let from = EPOCH + HOUR;
    let to = EPOCH + 2 * HOUR;
    let text =
        format!("SELECT count(value) FROM cpu WHERE ts BETWEEN {from} AND {to} GROUP BY time(10m)");
    let snapshot = store.snapshot();
    let plan = Plan::of(&snapshot, &parsed(&text))
        .unwrap_or_else(|refusal| panic!("a range this store keeps is a plan: {refusal}"));

    assert_eq!(
        plan.opened()
            .iter()
            .map(|info| info.index)
            .collect::<Vec<u64>>(),
        vec![1],
        "the one segment whose stated span meets an hour-long range"
    );
    assert_eq!(
        plan.pruned()
            .iter()
            .map(|info| info.index)
            .collect::<Vec<u64>>(),
        vec![0, 2, 3],
        "and the three that do not: the hour that ends before `from`, and the two that begin at or \
         after `to` — a segment whose *first* record stands at `to` holds nothing this question \
         asked about, which is the half-open rule at the plan's own level"
    );
    assert_eq!(
        plan.records(),
        10,
        "how many records those segments hold, from the manifest"
    );
    assert_eq!(
        plan.windows(),
        6,
        "an hour of ten-minute windows touches six"
    );
    assert_eq!(plan.limit(), None);
    assert_eq!(plan.metric(), "cpu");
    assert_eq!(plan.series(), "cpu", "no predicate named a tag");

    // `EXPLAIN` is a statement form, and what it answers is that plan written down.
    let explained = lakeletq(&sql(&store_path, &format!("EXPLAIN {text}")));
    assert_eq!(explained.code, 0, "a plan is an answer: {explained:?}");
    assert_eq!(explained.stderr, "", "and not a diagnosis");
    for expected in [
        "metric = cpu",
        "series = cpu",
        "aggregate = count",
        &format!("from = {from}"),
        &format!("to = {to}"),
        "window = 600000",
        "windows = 6",
        "columns = series,ts_millis,value",
        "segments_opened = 1",
        "segments_pruned = 3",
        "records = 10",
        "limit = none",
    ] {
        assert!(
            explained.stdout.contains(expected),
            "the plan states `{expected}`:\n{}",
            explained.stdout
        );
    }
    let keys: Vec<&str> = explained
        .stdout
        .lines()
        .map(|line| {
            line.split_once(" = ")
                .map(|(key, _)| key)
                .unwrap_or_else(|| panic!("a plan line is `key = value`: `{line}`"))
        })
        .collect();
    // Twelve facts when this step shipped, and a floor rather than an equality on purpose:
    // `lakelet/4.14-index-pruning` grows this plan a second pruning decision (`blocks_pruned`, the
    // segments its index ruled out) beside `segments_pruned`, which is the pruning this step has.
    // The line below is the one assertion in this case that a later step may widen — the twelve
    // keys themselves keep their names, their order and their meanings — and the count stays exact
    // about what this step answers on its own tree, which is what the case is for.
    assert!(keys.len() >= 12, "one line per fact: {keys:?}");

    // The same keys as one object, which is what `--json` is: a format, and not a second plan.
    let documented_text = format!("EXPLAIN {text}");
    let mut documented_args = sql(&store_path, &documented_text);
    documented_args.push("--json");
    let documented = lakeletq(&documented_args);
    assert_eq!(documented.code, 0, "{documented:?}");
    let document: Value = serde_json::from_str(&documented.stdout)
        .unwrap_or_else(|error| panic!("`EXPLAIN --json` prints one document: {error}"));
    for key in &keys {
        assert!(
            document.get(*key).is_some(),
            "the document carries `{key}` beside the lines: {document}"
        );
    }
    assert_eq!(document["segments_opened"], Value::from(1));
    assert_eq!(document["segments_pruned"], Value::from(3));
    assert_eq!(document["records"], Value::from(10));
    assert_eq!(
        document["limit"],
        Value::Null,
        "no limit is `null` and not a zero, which would be a query that asks for no answer"
    );

    // What the same question costs when it is answered rather than planned, and the identity that
    // makes `rows_scanned` worth printing: this query read a quarter of the store.
    let outcome = asked(&snapshot, &text);
    assert_eq!(outcome.segments_opened(), 1);
    assert_eq!(outcome.segments_pruned(), 3);
    assert_eq!(outcome.rows_scanned(), 10);
    assert_eq!(outcome.rows_answered(), 10);
    assert!(
        outcome.rows_scanned() < store.manifest().records(),
        "the store holds {} record(s) and this query read {}: pruning is what makes that number \
         smaller than the store",
        store.manifest().records(),
        outcome.rows_scanned()
    );

    // **The plan opens nothing**, and the strongest way to say it is a segment that cannot be read
    // at all: the one the plan *pruned* is overwritten with bytes of the same length — so the store
    // still opens and the reader still refuses it — and neither the plan nor the answer changes,
    // because nothing ever opened that file.
    let before = lakeletq(&sql(&store_path, &text));
    assert_eq!(before.code, 0, "{before:?}");
    assert!(
        !before.stdout.is_empty(),
        "this question has an answer for the comparison to mean something"
    );
    dir.spoil(2);
    let explained_anyway = lakeletq(&sql(&store_path, &format!("EXPLAIN {text}")));
    assert_eq!(
        explained_anyway.code, 0,
        "a plan answers about a store whose segments cannot be read: {explained_anyway:?}"
    );
    assert_eq!(
        explained_anyway.stdout, explained.stdout,
        "and answers the same plan, byte for byte"
    );
    let after = lakeletq(&sql(&store_path, &text));
    assert_eq!(
        after.code, 0,
        "and a query whose range pruned that segment away never opens it: {after:?}"
    );
    assert_eq!(
        after.stdout, before.stdout,
        "so its answer is the same bytes"
    );

    // The segment the query *does* open is a different story: the plan still answers — it reads the
    // manifest and no segment — while the answer cannot be computed at all.
    dir.spoil(1);
    let planned_again = lakeletq(&sql(&store_path, &format!("EXPLAIN {text}")));
    assert_eq!(
        planned_again.code, 0,
        "a plan reads no segment: {planned_again:?}"
    );
    refused(
        &lakeletq(&sql(&store_path, &text)),
        4,
        "a segment this build cannot read",
    );
}

#[test]
fn a_metric_the_store_never_held_is_not_a_quiet_range() {
    // Two hours of one series, and the second hour's records stand at two minutes of it only: a
    // range *inside* that segment that holds neither of them.
    let batches = vec![
        series("cpu,host=h0", 0, MINUTE, 3, |_| 1.0),
        vec![
            record("cpu,host=h0", HOUR, 1.0),
            record("cpu,host=h0", HOUR + 9 * MINUTE, 1.0),
        ],
    ];
    let (dir, store) = sealed("quiet", &batches);
    let store_path = dir.argument();
    let from = EPOCH + HOUR + 3 * MINUTE;
    let to = EPOCH + HOUR + 6 * MINUTE;

    // The metric the store *does* hold, over a range its records are not in: an answer, and an
    // empty one.
    let quiet = format!(
        "SELECT count(value) FROM cpu WHERE ts BETWEEN {from} AND {to} GROUP BY time(1m), series"
    );
    let answered = lakeletq(&sql(&store_path, &quiet));
    assert_eq!(
        answered.code, 0,
        "a range the store holds records around but none in is an answer: {answered:?}"
    );
    assert_eq!(answered.stdout, "", "and the answer is no line at all");
    assert_eq!(answered.stderr, "");
    let mut quiet_json = sql(&store_path, &quiet);
    quiet_json.push("--json");
    let document: Value = serde_json::from_str(&lakeletq(&quiet_json).stdout)
        .unwrap_or_else(|error| panic!("`--json` prints one document: {error}"));
    assert_eq!(
        document["segments_opened"],
        Value::from(1),
        "the segment that spans the range was opened: pruning is by the range's span, not by \
         whether the range happens to hold a record"
    );
    assert_eq!(
        document["rows_scanned"],
        Value::from(2),
        "and its two records were read — that is what makes the difference between this answer and \
         the refusal below possible at all"
    );
    assert_eq!(
        document["series_matched"],
        Value::from(1),
        "the store holds this series, so the answer is about it"
    );
    assert_eq!(document["rows_answered"], Value::from(0));
    assert_eq!(
        document["buckets"].as_array().map(Vec::len),
        Some(0),
        "and it holds nothing: an empty answer, not an absent one"
    );

    // The metric the store has **never held**, over the same range and in the same store: the
    // thing asked about is not there, and a script is told with `3` rather than with a quiet
    // answer it would believe.
    let unknown =
        format!("SELECT count(value) FROM mem WHERE ts BETWEEN {from} AND {to} GROUP BY time(1m)");
    let refused_unknown = lakeletq(&sql(&store_path, &unknown));
    refused(&refused_unknown, 3, "a metric the store has never held");
    assert!(
        refused_unknown.stderr.contains("mem"),
        "and the sentence names it, so a typo is visible: {}",
        refused_unknown.stderr
    );
    let mut unknown_json = sql(&store_path, &unknown);
    unknown_json.push("--json");
    let as_json = lakeletq(&unknown_json);
    refused(
        &as_json,
        3,
        "a metric the store has never held, with `--json`",
    );
    assert_eq!(
        as_json.stderr.trim(),
        refused_unknown.stderr.trim(),
        "`--json` is a format and not a second command"
    );

    // In process, the same two are a variant each: the refusal is typed, and the case asserts the
    // type rather than the sentence.
    let snapshot = store.snapshot();
    let refusal = execute(&snapshot, &parsed(&unknown))
        .expect_err("a metric this store has never held is not an answer");
    assert!(matches!(refusal, Refusal::Unknown { .. }), "{refusal}");
    assert_eq!(refusal.class(), 3);
    assert!(
        execute(&snapshot, &parsed(&quiet)).is_ok(),
        "and the range with no records in it is an answer"
    );
}

#[test]
fn a_range_below_the_floor_is_refused_in_a_sentence_that_names_it() {
    // Four hours of one series, a sample a minute, sealed an hour to a segment.
    let batches: Vec<Vec<Record>> = (0..4u64)
        .map(|hour| series("cpu,host=h0", hour * HOUR, MINUTE, 60, |_| 1.0))
        .collect();
    let (dir, mut store) = sealed("below-floor", &batches);
    let store_path = dir.argument();

    // A retention pass that keeps one hour, at a `now` this case passes in (`lakelet/4.9-retention`):
    // the store forgets three hours and writes the floor down.
    store
        .expire(&Policy::new(HOUR, HOUR), EPOCH + 4 * HOUR)
        .unwrap_or_else(|error| {
            panic!("a store whose segments state a range carries a pass out: {error}")
        });
    let floor = store.floor();
    assert_eq!(floor, EPOCH + 3 * HOUR, "the floor is the cutoff's window");

    let from = EPOCH;
    let to = EPOCH + 4 * HOUR;
    let text =
        format!("SELECT count(value) FROM cpu WHERE ts BETWEEN {from} AND {to} GROUP BY time(1h)");
    let refused_floor = lakeletq(&sql(&store_path, &text));
    refused(
        &refused_floor,
        2,
        "a range that reaches below the store's floor",
    );
    assert!(
        refused_floor.stderr.contains(&floor.to_string()),
        "the sentence names the floor ({floor}), so a reader knows what the store still answers: {}",
        refused_floor.stderr
    );
    let mut json_args = sql(&store_path, &text);
    json_args.push("--json");
    let as_json = lakeletq(&json_args);
    refused(&as_json, 2, "the same range with `--json`");
    assert_eq!(as_json.stderr.trim(), refused_floor.stderr.trim());

    // In the engine it is a variant that carries both numbers, which is what makes the sentence
    // possible without the store being asked twice.
    match Plan::of(&store.snapshot(), &parsed(&text)) {
        Err(Refusal::Forgot {
            from: asked,
            floor: forgotten,
        }) => {
            assert_eq!(asked, EPOCH, "the range's own start");
            assert_eq!(
                forgotten, floor,
                "and the floor of the store's own manifest"
            );
        }
        other => panic!("a range below the floor is refused, and this is {other:?}"),
    }

    // The flag form is `lakelet/4.10-downsampling`'s and still answers what the store keeps: this
    // step's grammar is stricter than that step's tool, and the earlier contract is intact
    // (docs/handoff-m3.md §1 rule 10).
    let answered = lakeletq(&query_arguments(
        &store_path,
        &from.to_string(),
        &to.to_string(),
        "1h",
        "count",
    ));
    assert_eq!(
        answered.code, 0,
        "the flag form answers the part of the range the store still keeps: {answered:?}"
    );
    let lines: Vec<&str> = answered.stdout.lines().collect();
    assert_eq!(lines.len(), 1, "one hour is left: `{}`", answered.stdout);
    assert!(
        lines[0].starts_with(&floor.to_string()),
        "and the answer begins at the floor: `{}`",
        lines[0]
    );
}

#[test]
fn the_two_forms_answer_the_same_question_the_same_way() {
    let batches = vec![
        series("cpu,host=h0", 0, STEP, 4 * 4, |index| index as f64),
        series("cpu,host=h0", 4 * MINUTE, STEP, 4 * 4, |index| {
            100.0 + index as f64
        }),
    ];
    let (dir, store) = sealed("forms", &batches);
    let store_path = dir.argument();
    let from = EPOCH + MINUTE + 30_000;
    let to = EPOCH + 8 * MINUTE + 30_000;
    let from_text = from.to_string();
    let to_text = to.to_string();

    // One question, written both ways: the flags `lakelet/4.10-downsampling` shipped, and the text
    // this step adds. The answers are the same bytes, which is the whole of "the earlier contract
    // is intact" that a spec can check.
    let by_flags = lakeletq(&query_arguments(
        &store_path,
        &from_text,
        &to_text,
        "1m",
        "mean",
    ));
    let text =
        format!("SELECT mean(value) FROM cpu WHERE ts BETWEEN {from} AND {to} GROUP BY time(1m)");
    let by_text = lakeletq(&sql(&store_path, &text));
    assert_eq!(by_flags.code, 0, "the flag form answers: {by_flags:?}");
    assert_eq!(by_text.code, 0, "the text form answers: {by_text:?}");
    assert!(
        !by_flags.stdout.is_empty(),
        "this question has an answer, or the comparison below is two empty strings"
    );
    assert_eq!(
        by_text.stdout, by_flags.stdout,
        "the same question asked two ways is the same answer, byte for byte"
    );
    assert_eq!(by_text.stderr, "");

    // And the buckets are the ones `lakelet/4.10-downsampling`'s pass computes for the same
    // records, compared rather than assumed: the executor adds pruning, ordering and counting, and
    // no arithmetic at all.
    let expected = downsample::downsample(
        &batches.concat(),
        None,
        Range::new(from, to),
        Window::new(MINUTE),
        Aggregate::Mean,
    );
    let outcome = asked(&store.snapshot(), &text);
    assert_eq!(
        mixed(&outcome),
        expected.buckets(),
        "the grammar's buckets are the window pass's buckets: one definition of a `mean` in this \
         project, and the answer is built out of it"
    );

    // `--json` is a format in both, and the shapes are the two the contract names: 4.10's array,
    // and one object whose counts stand beside the buckets the query cost.
    let mut flag_json = query_arguments(&store_path, &from_text, &to_text, "1m", "mean");
    flag_json.push("--json");
    let array = lakeletq(&flag_json);
    assert_eq!(array.code, 0, "{array:?}");
    let array_document: Value = serde_json::from_str(&array.stdout)
        .unwrap_or_else(|error| panic!("the flag form's `--json` is one document: {error}"));
    let shown = array_document
        .as_array()
        .unwrap_or_else(|| panic!("and it is the array it was in 4.10: `{}`", array.stdout));

    let mut text_json = sql(&store_path, &text);
    text_json.push("--json");
    let object = lakeletq(&text_json);
    assert_eq!(object.code, 0, "{object:?}");
    let document: Value = serde_json::from_str(&object.stdout)
        .unwrap_or_else(|error| panic!("the text form's `--json` is one document: {error}"));
    for key in [
        "segments_opened",
        "segments_pruned",
        "rows_scanned",
        "rows_answered",
        "series_matched",
        "series_answered",
        "buckets",
    ] {
        assert!(
            document.get(key).is_some(),
            "the text form's document states `{key}`: {document}"
        );
    }
    let buckets = document["buckets"]
        .as_array()
        .unwrap_or_else(|| panic!("beside the counts, the buckets: {document}"));
    assert_eq!(
        buckets.len(),
        shown.len(),
        "one object per bucket, the same buckets as the array"
    );
    for (ours, theirs) in buckets.iter().zip(shown) {
        assert_eq!(ours["start"], theirs["start"], "the same windows");
        assert_eq!(ours["count"], theirs["count"], "the same counts");
        let left = ours["value"].as_f64().unwrap_or(f64::NAN);
        let right = theirs["value"].as_f64().unwrap_or(f64::NAN);
        assert!(
            agree(left, right),
            "and the same values: {left} against {right}"
        );
        assert!(
            ours.get("series").is_none(),
            "an answer that groups by nothing does not carry a series field: {ours}"
        );
    }

    // `GROUP BY …, series`: the series name is the first field of a line, and the first field of an
    // object, because an answer that covers several series has to say which is which.
    let grouped = format!(
        "SELECT count(value) FROM cpu WHERE host = 'h0' AND ts BETWEEN {from} AND {to} \
         GROUP BY time(5m), series"
    );
    let lines = lakeletq(&sql(&store_path, &grouped));
    assert_eq!(lines.code, 0, "{lines:?}");
    let first = lines
        .stdout
        .lines()
        .next()
        .unwrap_or_else(|| panic!("this question has an answer: {lines:?}"));
    let fields: Vec<&str> = first.split(' ').collect();
    assert_eq!(
        fields.len(),
        4,
        "a line is `<series> <start> <count> <value>`: `{first}`"
    );
    assert_eq!(
        fields[0], "cpu,host=h0",
        "and the series name is first: `{first}`"
    );
    assert!(
        fields[1].parse::<u64>().is_ok() && fields[2].parse::<u64>().is_ok(),
        "then the window's start and the count: `{first}`"
    );
    assert!(fields[3].parse::<f64>().is_ok(), "and the value: `{first}`");

    let mut grouped_json = sql(&store_path, &grouped);
    grouped_json.push("--json");
    let grouped_ran = lakeletq(&grouped_json);
    assert_eq!(grouped_ran.code, 0, "{grouped_ran:?}");
    let grouped_document: Value = serde_json::from_str(&grouped_ran.stdout)
        .unwrap_or_else(|error| panic!("one document: {error}"));
    let grouped_buckets = grouped_document["buckets"]
        .as_array()
        .unwrap_or_else(|| panic!("buckets: {grouped_document}"));
    assert!(!grouped_buckets.is_empty(), "this question has an answer");
    for bucket in grouped_buckets {
        assert_eq!(
            bucket["series"],
            Value::from("cpu,host=h0"),
            "every bucket says which series it is about: {bucket}"
        );
    }
    let series_at = grouped_ran.stdout.find("\"series\"").unwrap_or(0);
    let start_at = grouped_ran.stdout.find("\"start\"").unwrap_or(0);
    let count_at = grouped_ran.stdout.find("\"count\"").unwrap_or(0);
    let value_at = grouped_ran.stdout.find("\"value\"").unwrap_or(0);
    assert!(
        series_at < start_at && start_at < count_at && count_at < value_at,
        "the fields are written in the order the contract names them: {}",
        grouped_ran.stdout
    );
}

#[test]
fn a_limit_bounds_the_series_an_answer_covers_and_not_the_buckets_it_holds() {
    let batches = vec![
        series("cpu,host=h0", 0, MINUTE, 3, |_| 1.0),
        series("cpu,host=h1", 0, MINUTE, 3, |_| 2.0),
        series("cpu,host=h2", 0, MINUTE, 3, |_| 3.0),
    ];
    let (dir, store) = sealed("limit", &batches);
    let store_path = dir.argument();
    let from = EPOCH;
    let to = EPOCH + 3 * MINUTE;
    let all = format!(
        "SELECT count(value) FROM cpu WHERE ts BETWEEN {from} AND {to} GROUP BY time(1m), series"
    );
    let limited = format!(
        "SELECT count(value) FROM cpu WHERE ts BETWEEN {from} AND {to} GROUP BY time(1m), series \
         LIMIT 2"
    );
    let snapshot = store.snapshot();

    let everything = asked(&snapshot, &all);
    assert_eq!(everything.series_matched(), 3);
    assert_eq!(everything.series_answered(), 3);
    let bounded = asked(&snapshot, &limited);
    assert_eq!(
        bounded.series_matched(),
        3,
        "the store still holds three series the query selects"
    );
    assert_eq!(bounded.series_answered(), 2, "and the answer covers two");
    assert_eq!(
        named(&bounded),
        ["cpu,host=h0", "cpu,host=h1"],
        "the first two by name, which is the order a grouped answer is written in"
    );

    // A limit bounds *series*, not lines: the same question through the binary answers two series
    // of three minutes each.
    let ran = lakeletq(&sql(&store_path, &limited));
    assert_eq!(ran.code, 0, "{ran:?}");
    assert_eq!(
        ran.stdout.lines().count(),
        6,
        "two series, three buckets each: `{}`",
        ran.stdout
    );
    for line in ran.stdout.lines() {
        assert!(
            line.starts_with("cpu,host=h0 ") || line.starts_with("cpu,host=h1 "),
            "and never the third: `{line}`"
        );
    }

    // Without `GROUP BY …, series` there is one answer however many series went into it, so a limit
    // there names nothing to bound — which is a refusal rather than a silent no-op.
    let without = format!(
        "SELECT count(value) FROM cpu WHERE ts BETWEEN {from} AND {to} GROUP BY time(1m) LIMIT 2"
    );
    assert!(matches!(refusal_of(&without), Refusal::Limit { .. }));
}

#[test]
fn the_outcome_states_what_it_read_and_what_it_answered() {
    let batches: Vec<Vec<Record>> = (0..3u64)
        .map(|hour| series("cpu,host=h0", hour * HOUR, MINUTE, 60, |_| 1.0))
        .collect();
    let (_dir, store) = sealed("identities", &batches);
    let from = EPOCH + HOUR + 30 * MINUTE;
    let to = EPOCH + 2 * HOUR + 30 * MINUTE;
    let text =
        format!("SELECT count(value) FROM cpu WHERE ts BETWEEN {from} AND {to} GROUP BY time(10m)");
    let snapshot = store.snapshot();
    let plan = Plan::of(&snapshot, &parsed(&text)).unwrap_or_else(|refusal| panic!("{refusal}"));
    let outcome = asked(&snapshot, &text);

    // The plan's numbers and the executed ones are different questions, and the two sets that must
    // agree are the ones about *work*: what was read is what the plan said those segments hold.
    assert_eq!(outcome.segments_opened(), 2, "two hours of three");
    assert_eq!(outcome.segments_pruned(), 1);
    assert_eq!(
        outcome.segments_opened() + outcome.segments_pruned(),
        3,
        "every segment is opened or pruned, and no segment is both"
    );
    assert_eq!(
        outcome.segments_opened() as usize,
        plan.opened().len(),
        "and the outcome agrees with the plan about how many were opened"
    );
    assert_eq!(
        outcome.rows_scanned(),
        plan.records(),
        "the records read are the records the opened segments state they hold"
    );
    assert_eq!(outcome.rows_scanned(), 120);
    assert_eq!(plan.windows(), 6, "an hour of ten-minute windows");

    // `rows_scanned <= records`, where `records` is what the store holds, and `rows_answered <=
    // rows_scanned`: the two identities a budget acts on, and the reason they are asserted rather
    // than assumed is that both are the kind of number that goes wrong quietly.
    assert!(
        outcome.rows_scanned() <= store.manifest().records(),
        "what was read is at most what the store holds: {} against {}",
        outcome.rows_scanned(),
        store.manifest().records()
    );
    assert!(
        outcome.rows_answered() <= outcome.rows_scanned(),
        "and what was answered is at most what was read: {} against {}",
        outcome.rows_answered(),
        outcome.rows_scanned()
    );
    assert_eq!(
        outcome.rows_answered(),
        60,
        "an hour of a minute-apart samples"
    );
    let summed: u64 = outcome
        .answers()
        .iter()
        .flat_map(|answer| answer.buckets())
        .map(Bucket::count)
        .sum();
    assert_eq!(
        outcome.rows_answered(),
        summed,
        "and it is what the answer's own buckets add up to, which is the check a caller makes when \
         an answer looks wrong"
    );
}

#[test]
fn a_store_that_is_not_there_a_store_that_refused_and_a_broken_deployment() {
    let (dir, _store) = sealed("classes", &[series("cpu,host=h0", 0, STEP, 4, |_| 1.0)]);
    let store = dir.argument();
    let from = EPOCH.to_string();
    let to = (EPOCH + HOUR).to_string();
    let text =
        format!("SELECT count(value) FROM cpu WHERE ts BETWEEN {from} AND {to} GROUP BY time(1m)");

    // `3`: the thing asked about is not there. A read does not create a store, so a typo in
    // `--store` is told apart from a store with nothing in it.
    let missing = lakeletq(&sql("/nowhere/lakelet", &text));
    refused(&missing, 3, "a store directory that is not there");

    // `4`: the store refused — a manifest that is not this build's is the fast half of that class,
    // and a segment that cannot be read is the other.
    let damaged = Dir::new("damaged");
    damaged.damage(
        "segments.manifest",
        b"not a manifest, not this build's, not a store",
    );
    let refused_manifest = lakeletq(&sql(&damaged.argument(), &text));
    refused(&refused_manifest, 4, "a manifest this build cannot read");

    // `1`: this deployment is broken rather than the call. A file where a store should be is
    // neither a typo in a path nor something a store can refuse.
    let file = Dir::new("not-a-store");
    file.damage("a-file", b"this is not a directory");
    let path = file.path().join("a-file").display().to_string();
    let broken = lakeletq(&sql(&path, &text));
    refused(&broken, 1, "a path that exists and is not a directory");

    // Each of the three is also checked with `--json`, because a format must not move a code — and
    // the three stay three, because one exit code per failure class is the whole of the table a
    // script branches on.
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
        (&broken, path, "a deployment that is broken"),
    ] {
        let mut args = sql(&store, &text);
        args.push("--json");
        let as_json = lakeletq(&args);
        assert_eq!(
            as_json.code, ran.code,
            "`--json` does not change the class of {what}: {as_json:?}"
        );
        assert_eq!(as_json.stdout, "", "and prints nothing on stdout");
        assert_eq!(as_json.stderr.trim(), ran.stderr.trim());
    }
    assert_eq!(
        [missing.code, refused_manifest.code, broken.code],
        [3, 4, 1],
        "a store that is not there, a store that refused and a deployment that is broken are three \
         different answers to a script"
    );

    // A question has a size, and the size is `lakelet/4.10-downsampling`'s `MAX_WINDOWS` — the same
    // ceiling, checked before a store is opened, in both input forms. The boundary itself is
    // answered rather than guessed about.
    let widest = MAX_WINDOWS * MINUTE;
    let accepted = lakeletq(&sql(
        &store,
        &format!("SELECT count(value) FROM cpu WHERE ts BETWEEN 0 AND {widest} GROUP BY time(1m)"),
    ));
    assert_eq!(
        accepted.code, 0,
        "exactly as many buckets as this tool answers is a question it answers: {accepted:?}"
    );
    let one_past = widest + 1;
    let refused_text = lakeletq(&sql(
        &store,
        &format!(
            "SELECT count(value) FROM cpu WHERE ts BETWEEN 0 AND {one_past} GROUP BY time(1m)"
        ),
    ));
    refused(&refused_text, 2, "one bucket past what the tool answers");
    assert!(
        refused_text.stderr.contains(&(MAX_WINDOWS + 1).to_string())
            && refused_text.stderr.contains(&MAX_WINDOWS.to_string()),
        "and the sentence names both numbers — what was asked for and what is answered — so the \
         reader can find the typo: {}",
        refused_text.stderr
    );
    let refused_flags = lakeletq(&query_arguments(
        &store,
        "0",
        &one_past.to_string(),
        "1m",
        "count",
    ));
    assert_eq!(
        refused_flags.code, 2,
        "the flag form refuses the same typo: {refused_flags:?}"
    );
    assert!(
        refused_flags
            .stderr
            .contains(&(MAX_WINDOWS + 1).to_string())
            && refused_flags.stderr.contains(&MAX_WINDOWS.to_string()),
        "with the same two numbers in it, because there is one ceiling and one count: {}",
        refused_flags.stderr
    );
}

#[test]
fn a_call_that_gives_both_forms_is_the_callers_class() {
    let (dir, _store) = sealed("mix", &[series("cpu,host=h0", 0, STEP, 4, |_| 1.0)]);
    let store = dir.argument();
    let text = "SELECT count(value) FROM cpu WHERE ts BETWEEN 0 AND 60000 GROUP BY time(1m)";

    // `--sql` *is* the question, so a flag that means the same thing beside it is a caller saying
    // it twice — and the flag is named, because "you cannot mix these" sends a reader back to
    // their script to guess which one did it.
    for flag in ["--from", "--to", "--window", "--agg", "--series"] {
        let mut args = sql(&store, text);
        args.push(flag);
        args.push("0");
        let mixed = lakeletq(&args);
        refused(&mixed, 2, "a flag given beside `--sql`");
        assert!(
            mixed.stderr.contains(flag),
            "the diagnosis names `{flag}`: {}",
            mixed.stderr
        );
        args.push("--json");
        let as_json = lakeletq(&args);
        refused(&as_json, 2, "the same mixture with `--json`");
        assert_eq!(as_json.stderr.trim(), mixed.stderr.trim());
    }

    // `--store` is not a mixture: the text cannot name a directory, so the store is a flag in both
    // forms, and a call with both of *those* is the normal way to ask.
    let asked_both_forms = lakeletq(&sql(&store, text));
    assert_eq!(
        asked_both_forms.code, 0,
        "`--store` is the store in both forms: {asked_both_forms:?}"
    );

    // And the usage text is where a reader finds the form, so it is part of the contract rather
    // than a nicety: `--help` is an answer, on stdout, with exit `0`.
    assert!(
        USAGE.contains("--sql"),
        "the usage text names the form the tool answers"
    );
    let help = lakeletq(&["--help"]);
    assert_eq!(help.code, 0);
    assert_eq!(
        help.stdout, USAGE,
        "and the binary writes the library's text"
    );
    assert_eq!(help.stderr, "");
}

#[test]
fn the_flag_form_the_earlier_step_shipped_still_answers() {
    let (dir, _store) = sealed(
        "regression",
        &[series("cpu,host=h0", 0, STEP, 8, |index| index as f64)],
    );
    let store = dir.argument();

    // Help and version are answers, and the library and the process write the same bytes for them.
    let in_process = cli::run(argv(&["--help"]));
    assert_eq!(in_process.code(), 0);
    assert_eq!(in_process.stdout(), USAGE);
    let process = lakeletq(&["--help"]);
    assert_eq!(process.stdout, in_process.stdout());
    assert_eq!(process.code, i32::from(in_process.code()));
    assert!(lakeletq(&["--version"]).stdout.starts_with("lakeletq "));

    // A real question, asked both ways: this is what makes "the flag form is unchanged" a fact
    // rather than a comment.
    let from = EPOCH.to_string();
    let to = (EPOCH + 2 * MINUTE + 30_000).to_string();
    let args = query_arguments(&store, &from, &to, "1m", "count");
    let asked = cli::run(argv(&args));
    let ran = lakeletq(&args);
    assert!(
        !asked.stdout().is_empty(),
        "this question has an answer, or the comparison below is two empty strings"
    );
    assert_eq!(
        asked.stdout(),
        ran.stdout,
        "the library's answer and the process's are the same bytes"
    );
    assert_eq!(i32::from(asked.code()), ran.code);

    // The caller's class, twice — with and without `--json` — including the two answers this step
    // moved into the grammar: the ceiling on how many buckets a question may ask for, and the
    // table of window units, which is now the grammar's rather than a second copy in the flags.
    let cases: Vec<Vec<&str>> = vec![
        vec!["query"],
        vec![
            "query",
            "--store",
            store.as_str(),
            "--from",
            "0",
            "--to",
            "60000",
            "--agg",
            "mean",
        ],
        vec![
            "query",
            "--store",
            store.as_str(),
            "--from",
            "0",
            "--to",
            "60000",
            "--window",
            "90x",
            "--agg",
            "mean",
        ],
        vec![
            "query",
            "--store",
            store.as_str(),
            "--from",
            "0",
            "--to",
            "60000",
            "--window",
            "1s",
            "--agg",
            "average",
        ],
        vec![
            "query",
            "--store",
            store.as_str(),
            "--from",
            "5",
            "--to",
            "5",
            "--window",
            "1s",
            "--agg",
            "mean",
        ],
        vec![
            "query",
            "--store",
            store.as_str(),
            "--from",
            "60000",
            "--to",
            "5",
            "--window",
            "1s",
            "--agg",
            "mean",
        ],
    ];
    for args in &cases {
        let as_text = lakeletq(args);
        refused(&as_text, 2, "a caller mistake in the flags");
        let mut with_json = args.clone();
        with_json.push("--json");
        let as_json = lakeletq(&with_json);
        refused(&as_json, 2, "the same mistake with `--json`");
        assert_eq!(
            as_json.stderr.trim(),
            as_text.stderr.trim(),
            "`--json` is a format and not a second command: `{args:?}`"
        );
    }

    // One table of window units, asked by both forms: a word neither has is refused by both, and
    // neither can drift from the other because there is nothing to drift from.
    let by_flags = lakeletq(&[
        "query",
        "--store",
        store.as_str(),
        "--from",
        "0",
        "--to",
        "600000",
        "--window",
        "90x",
        "--agg",
        "count",
    ]);
    let by_text = lakeletq(&sql(
        &store,
        "SELECT count(value) FROM cpu WHERE ts BETWEEN 0 AND 600000 GROUP BY time(90x)",
    ));
    refused(&by_flags, 2, "`--window 90x`");
    refused(&by_text, 2, "`time(90x)`");
    for ran in [&by_flags, &by_text] {
        assert!(
            ran.stderr.contains("90x"),
            "and both name the unit that was written: {}",
            ran.stderr
        );
    }
}

#[test]
fn the_query_sources_do_not_reach_into_the_service_and_the_page_is_there() {
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
                "{} names `{forbidden}` — the daemon uses the engine, never the other way, and the \
                 files this step adds are where a new violation would appear \
                 (`lakelet/4.1-ingest-protocol` reads the same sources for it)",
                file.display()
            );
        }
    }

    // The query module is the window pass of `lakelet/4.10-downsampling` and the five files this
    // step adds beside it.
    let query = PathBuf::from("src/engine/query");
    let mut files: Vec<PathBuf> = Vec::new();
    collect_rust(&query, &mut files).expect("the query module is part of the project");
    assert!(
        files.len() >= 6,
        "{} holds {} Rust file(s): the window pass and the grammar, its scanner, its parser, its \
         plan and its executor",
        query.display(),
        files.len()
    );
    let module = std::fs::read_to_string("src/engine/query/mod.rs").unwrap_or_else(|error| {
        panic!(
            "src/engine/query/mod.rs cannot be read: {error} — it is the page a reader of the \
                query engine opens first"
        )
    });
    for name in ["downsample", "exec", "lex", "parse", "plan", "series"] {
        assert!(
            module.contains(name),
            "the module does not name `{name}`, and this is the page the rest of the engine is read \
             from"
        );
    }

    // The binary on top of it is still a shell: it asks `lakelet::cli` and writes what it answered.
    let shell = std::fs::read_to_string("src/bin/lakeletq.rs")
        .unwrap_or_else(|error| panic!("src/bin/lakeletq.rs cannot be read: {error}"));
    assert!(
        shell.contains("cli::run"),
        "the binary calls the library's grammar: `{shell}`"
    );
    for absent in ["Segments", "downsample", "snapshot"] {
        assert!(
            !shell.contains(absent),
            "src/bin/lakeletq.rs names `{absent}`: the shell decides about this process and nothing \
             about the answer"
        );
    }

    // And the page a reader of the refusal table opens. The `links` gate judges every markdown file
    // in this project, so this is the same contract from inside the spec: the page is there, it
    // says what the grammar is, and every link in it resolves.
    let page = std::fs::read_to_string("docs/query.md").unwrap_or_else(|error| {
        panic!(
            "docs/query.md cannot be read: {error} — it is the page `lakeletq --help` points a \
             reader of the grammar at, and `kiln check` fails the step without it"
        )
    });
    assert!(
        page.len() > 1000,
        "the page says something about the grammar rather than holding a heading and a TODO: {} \
         byte(s)",
        page.len()
    );
    for name in [
        "SELECT",
        "ts BETWEEN",
        "EXPLAIN",
        "segments_pruned",
        COLUMNS,
        "class",
    ] {
        assert!(
            page.contains(name),
            "the page names `{name}`, which is what a reader of the grammar or of a refusal number \
             is looking for"
        );
    }
    let mut links = 0;
    for (index, line) in page.lines().enumerate() {
        for target in link_targets(line) {
            if target.starts_with("http://") || target.starts_with("https://") {
                continue;
            }
            links += 1;
            assert!(
                !target.starts_with('/'),
                "docs/query.md:{} links `{target}`: an absolute path leaves the project the gate \
                 reads",
                index + 1
            );
            let relative = target.split('#').next().unwrap_or(&target);
            if relative.is_empty() {
                continue;
            }
            let resolved = Path::new("docs").join(relative);
            assert!(
                resolved.exists(),
                "docs/query.md:{} links `{target}`, and {} is not there",
                index + 1,
                resolved.display()
            );
        }
    }
    assert!(
        links > 0,
        "the page points at the code it describes, which is what a reader follows next"
    );
}

/// Every markdown link target in one line, without the optional title after it.
fn link_targets(line: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find("](") {
        let after = &rest[open + 2..];
        let Some(close) = after.find(')') else {
            break;
        };
        let inside = &after[..close];
        if let Some(target) = inside.split_whitespace().next()
            && !target.is_empty()
        {
            targets.push(target.to_owned());
        }
        rest = &after[close + 1..];
    }
    targets
}
