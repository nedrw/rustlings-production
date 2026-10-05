//! Spec for `jobq/3.17-error-budget-alerts`: what the process says about itself, and what a budget
//! of failures means for this queue.
//!
//! `kiln run` copies this file into the project as `tests/spec_alerts.rs`; it is the course's file,
//! not the learner's (plan §3 D2). Two halves, judged from two directions:
//!
//! * **the page and the code are one table** (`docs/alerts.md` × `jobq::service::signals`). Every
//!   rule names a signal this process exports, a threshold, a window and the drill that is supposed
//!   to trigger it; every family the process exports is named by a rule. That is what makes the
//!   leftmost column of the page a name rather than a wish, and it is why this file has to read
//!   both sides: a page that agrees with itself is a page nobody can check;
//! * **the lines are the second half of the same promise.** A run reports every queue it drains, one
//!   field per family, at zero before anything has happened; every line of a run carries the run
//!   itself; and the line about one attempt carries the fields a query needs to get from "this queue
//!   is burning" to "these jobs, for these reasons". The first two are judged on a *process*
//!   (`JOBQ__STORE__URL=memory`, so no container is needed to watch a run report itself); the third
//!   is judged in this process, with a subscriber whose writer is a buffer, because a line about an
//!   attempt exists only when a worker runs one and the fields are what a collector reads.
//!
//! The budget arithmetic is judged last and cheapest: a counter is cumulative, so the window a rule
//! is written over is the *difference* between two readings, and the three numbers that come out of
//! it (what ended, what ended badly, and what a queue's floor allows) are what decides whether
//! anybody is woken up.

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::Value;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::util::SubscriberInitExt;

use jobq::engine::{
    BoxFuture, Engine, Error, Job, MemoryStore, Queue, Store, SubmitRequest, Usage, WorkerId,
};
use jobq::service::signals::{self, Budget, SignalKind};
use jobq::service::worker::{Handler, Shutdown, Worker, WorkerSettings};
use support::service::Service;
use support::var;

/// The page this step delivers, relative to the project.
const PAGE: &str = "docs/alerts.md";
/// The four columns a rule has to have, whatever else the table carries.
const COLUMNS: [&str; 4] = ["signal", "threshold", "window", "drill"];
/// The drill every rule names: the command a responder replays to see the rule fire, which is this
/// file's test target.
const DRILL: &str = "spec_alerts";
/// How long a case waits for a condition before it gives up and says what it saw.
const DEADLINE: Duration = Duration::from_secs(30);
/// How often a waiting case looks again. A wait is a condition with a deadline, never a sleep
/// (authoring.md §6.4).
const POLL: Duration = Duration::from_millis(20);

/// The page, read from the project it is part of.
fn page() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(PAGE);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} is the page this step delivers, and it cannot be read: {error}",
            path.display()
        )
    })
}

/// The queue every case in this file uses.
fn queue() -> Queue {
    Queue::parse("emails").expect("`emails` is a name this file uses everywhere")
}

/// The cells of one markdown table row, or `None` when the line is not a row at all.
///
/// A cell is trimmed and its backticks taken off, because the page writes the signal and the window
/// the way a person writes code in a sentence: `jobq_failed_total`, in backticks.
fn cells(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') {
        return None;
    }
    Some(
        trimmed
            .trim_matches('|')
            .split('|')
            .map(|cell| cell.trim().trim_matches('`').trim().to_owned())
            .collect(),
    )
}

/// `true` for the row of dashes under a table's header.
fn is_rule(cells: &[String]) -> bool {
    cells.iter().all(|cell| {
        !cell.is_empty()
            && cell
                .chars()
                .all(|character| character == '-' || character == ':')
    })
}

/// The page's rules, one map of column name to cell per row.
///
/// The table is found by its header rather than by its position: a page is prose with a table in it,
/// and the header is what says which column is which. A column this file needs and the page does not
/// have is a failure with the columns the page *does* have in the message, because dropping one is
/// the mistake a rule table makes when it is written from memory.
fn rules(page: &str) -> Vec<BTreeMap<String, String>> {
    let mut header: Option<Vec<String>> = None;
    let mut rules: Vec<BTreeMap<String, String>> = Vec::new();
    for line in page.lines() {
        let Some(cells) = cells(line) else {
            continue;
        };
        let Some(names) = header.clone() else {
            if cells.iter().any(|cell| cell == "signal") {
                for column in COLUMNS {
                    assert!(
                        cells.iter().any(|cell| cell == column),
                        "the rule table's header has no `{column}` column: a rule that does not name \
                         it cannot be checked, and the columns this table has are {cells:?}"
                    );
                }
                header = Some(cells);
            }
            continue;
        };
        if is_rule(&cells) {
            continue;
        }
        let rule: BTreeMap<String, String> = names
            .iter()
            .zip(cells.iter())
            .map(|(name, cell)| (name.clone(), cell.clone()))
            .collect();
        rules.push(rule);
    }
    assert!(
        header.is_some(),
        "{PAGE} has no rule table: a table whose header names {COLUMNS:?} is what a rule is written \
         in, and a page of prose cannot be checked for the one thing that matters — that each rule \
         names its signal, its threshold, its window and the drill that triggers it"
    );
    assert!(!rules.is_empty(), "{PAGE}'s rule table has no rules in it");
    rules
}

/// `true` when `value` is a window a rule can be written over: a count and a unit.
fn is_window(value: &str) -> bool {
    let trimmed = value.trim();
    let Some((digits, unit)) = trimmed
        .len()
        .checked_sub(1)
        .and_then(|at| trimmed.split_at_checked(at))
    else {
        return false;
    };
    !digits.is_empty()
        && digits.chars().all(|character| character.is_ascii_digit())
        && matches!(unit, "s" | "m" | "h" | "d")
}

/// The fields of an event, which is where a collector finds everything but the level.
fn fields(event: &Value) -> &serde_json::Map<String, Value> {
    event["fields"]
        .as_object()
        .unwrap_or_else(|| panic!("this event has no `fields` object: {event}"))
}

/// One field of an event, as the collector reads it.
fn field<'a>(event: &'a Value, name: &str) -> &'a Value {
    fields(event)
        .get(name)
        .unwrap_or_else(|| panic!("this event has no field `{name}`: {event}"))
}

/// The `run.id` this event was written inside, taken from the span stack rather than from the
/// event's own fields: the id is context, and a line that names it by hand is a line somebody will
/// forget to name it on.
fn run_id(event: &Value) -> Option<String> {
    let spans = event["spans"].as_array()?;
    spans
        .iter()
        .find(|span| span["name"] == "run")?
        .get("run.id")?
        .as_str()
        .map(str::to_owned)
}

/// The direction a rule is checked in, both ways at once: every rule names a family this process
/// exports, and every family this process exports is named by a rule.
#[test]
fn every_rule_names_a_signal_this_process_exports_and_a_window_and_a_drill() {
    let rules = rules(&page());
    let mut named: BTreeSet<String> = BTreeSet::new();
    for rule in &rules {
        let signal = &rule["signal"];
        let family = signals::signal(signal).unwrap_or_else(|| {
            panic!(
                "a rule wakes somebody up for `{signal}`, which this process does not export: the \
                 families are {:?}. A rule written against a name nobody counts is a rule that never \
                 fires, and it is the mistake this file exists to catch",
                signals::signals()
                    .iter()
                    .map(signals::Signal::name)
                    .collect::<Vec<_>>()
            )
        });
        assert!(
            !family.help().trim().is_empty(),
            "`{signal}` has no help line: the person reading the rule at three in the morning is the \
             person the sentence is for"
        );
        named.insert(signal.clone());
        assert!(
            rule["threshold"].chars().any(|c| c.is_ascii_digit()),
            "`{signal}`'s rule has no number in its threshold ({}): `more than usual` is not a rule, \
             and neither is one nobody can compare",
            rule["threshold"]
        );
        assert!(
            is_window(&rule["window"]),
            "`{signal}`'s rule names `{}` as its window, which is not a window: a counter is \
             cumulative, so a rule without one is a rule about the whole life of the queue",
            rule["window"]
        );
        assert!(
            rule["drill"].contains(DRILL),
            "`{signal}`'s rule names `{}` as the drill that triggers it: a rule nobody can replay is \
             a rule nobody knows the shape of, and the drill for this step is `{DRILL}`",
            rule["drill"]
        );
    }
    for family in signals::signals() {
        assert!(
            named.contains(family.name()),
            "`{}` is exported and no rule names it ({}): a number nobody watches is a number nobody \
             sees — {:?}",
            family.name(),
            family.help(),
            named
        );
    }
}

/// The numbers the page promises and the numbers the drill applies are the same numbers.
///
/// The page is what a person reads; `Budget` is what the drill computes with. A page that says
/// 99.9 % while the drill measures 99 % is a page that has stopped being true, and nothing else in
/// the tree would notice.
#[test]
fn the_page_and_the_drill_agree_on_the_budget() {
    let page = page();
    let budget = Budget::standard();
    let promise = format!("{}%", 100 - budget.allow_bad() / 100);
    assert!(
        page.contains(&promise),
        "the page has to state the budget the drill measures against, which is {promise} of the \
         settled attempts in a window"
    );
    let floor = format!("{} attempts", budget.floor());
    assert!(
        page.contains(&floor),
        "the page has to say how much traffic a window needs before its share is read ({floor}): a \
         rule that pages on one failure in one attempt pages on every quiet hour"
    );
}

/// One real run of the daemon, as a collector sees it: the lines it wrote, parsed.
///
/// The store is the in-process one, so no container is involved: this case is about what a run
/// *says*, and a run with nothing in the store still says all of it.
///
/// The lines are read in a loop rather than once: the sample is the last thing a run writes, so
/// waiting for the process to exit is not waiting for its output to have been read, and a deadline
/// is what turns "the line never came" into a sentence instead of a hang. The fixture hands the
/// lines it read while waiting for the readiness marker back on every call, so what a call has
/// already answered is kept rather than read twice.
fn one_run() -> Vec<Value> {
    let mut service = Service::start(&[
        var("JOBQ__STORE__URL", "memory"),
        var("JOBQ__WORKER__QUEUES", "emails,exports"),
        var("JOBQ__LOG__FORMAT", "json"),
        var("JOBQ__LOG__LEVEL", "info"),
    ]);
    service.stop();
    let code = service.wait_for_exit();
    assert_eq!(
        code, 0,
        "`SIGTERM` is a stop and not a crash (`jobq/3.1-engine-boundary` pinned the exit code)"
    );

    let deadline = Instant::now() + DEADLINE;
    let mut lines: Vec<String> = Vec::new();
    loop {
        for line in service.log_lines() {
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
        let written: Vec<Value> = lines.iter().map(|line| parse(line)).collect();
        if written.iter().any(is_sample) {
            return written;
        }
        assert!(
            Instant::now() < deadline,
            "the run wrote no sample: no line carries every family this process exports ({:?}), and \
             the lines it did write are {lines:?}",
            signals::signals()
                .iter()
                .map(signals::Signal::name)
                .collect::<Vec<_>>()
        );
    }
}

/// `true` when this line is a queue's sample: every family this process exports is a field of it.
fn is_sample(event: &Value) -> bool {
    signals::signals()
        .iter()
        .all(|family| fields(event).contains_key(family.name()))
}

/// One line of `json` mode, parsed: a collector has to be able to do exactly this.
fn parse(line: &str) -> Value {
    serde_json::from_str(line).unwrap_or_else(|error| {
        panic!(
            "`json` mode prints one JSON object per line, and this line is not one ({error}): \
             `{line}`"
        )
    })
}

/// The sample a run reports: one line per queue it drains, one field per family, at zero before
/// anything has happened.
#[test]
fn a_run_reports_every_queue_it_drains_one_field_per_family_at_zero() {
    let events = one_run();
    let sample = |queue: &str| -> Value {
        events
            .iter()
            .find(|event| fields(event).get("queue").and_then(Value::as_str) == Some(queue))
            .unwrap_or_else(|| panic!("the run never reported the queue `{queue}`: {events:?}"))
            .clone()
    };
    for queue in ["emails", "exports"] {
        let event = sample(queue);
        for family in signals::signals() {
            assert_eq!(
                field(&event, family.name()),
                &Value::from(0),
                "`{queue}` had nothing happen to it, and a queue at zero is still a series: a field \
                 that appears only after the first failure answers \"no data\" during the quiet \
                 period ({event})"
            );
        }
        let names: BTreeSet<&str> = fields(&event)
            .keys()
            .map(String::as_str)
            .filter(|name| *name != "message")
            .collect();
        let mut expected: BTreeSet<&str> = signals::signals()
            .iter()
            .map(signals::Signal::name)
            .collect();
        // Every family carries the queue label, and on a line the label is a field of its own.
        for label in signals::signals()
            .iter()
            .flat_map(|family| family.labels())
            .copied()
        {
            expected.insert(label);
        }
        assert_eq!(
            names, expected,
            "the sample's fields have to be the label and the families this process exports, and \
             nothing else: `{event}`"
        );
    }
}

/// Every line of a run is inside the run, whoever wrote it.
#[test]
fn every_line_of_a_run_carries_the_run() {
    let events = one_run();
    assert!(
        events.len() >= 4,
        "a run writes more than one line — what it installed, what it is draining, what the worker \
         did and what the queues now hold — and {events:?} is not a run to judge this on"
    );
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for event in &events {
        let id = run_id(event).unwrap_or_else(|| {
            panic!(
                "this line of a run carries no `run.id`: {} — a field written by hand at each call \
                 site is a field somebody forgets, which is why the run is a *span* and the tasks a \
                 process starts are handed it (`jobq/3.17-error-budget-alerts`)",
                event["fields"]["message"]
            )
        });
        ids.insert(id);
    }
    assert_eq!(
        ids.len(),
        1,
        "one process is one run, and these lines name {} different ones: {ids:?}",
        ids.len()
    );
    // The lines have to come from more than one place, or "every line carries it" is a claim about
    // the one function that has the id in its arguments.
    let targets: BTreeSet<&str> = events
        .iter()
        .filter_map(|event| event["target"].as_str())
        .collect();
    assert!(
        targets.len() > 1,
        "every line of this run was written by one module ({targets:?}), so nothing here shows the \
         run crossing a boundary"
    );
}

/// A buffer a subscriber writes into, so a case can read the lines a worker emitted.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Sink {
    /// Everything written so far, as the lines a collector would have.
    fn lines(&self) -> Vec<Value> {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(parse)
            .collect()
    }
}

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for Sink {
    type Writer = Sink;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// A worker of one's own, running in this process under a subscriber that writes into `sink`.
///
/// The runtime is the current-thread one and the subscriber is the *default* rather than the global
/// one, which is what makes this case possible at all: a default subscriber is scoped to the thread
/// that installed it, and a current-thread runtime polls every task this worker starts on that same
/// thread. The alternative — a container, a real database and a process — measures the same thing a
/// hundred times more slowly.
async fn drain_with<H: Handler + 'static>(handler: H, sink: &Sink) -> (Arc<dyn Store>, Shutdown) {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let engine = Engine::new(Arc::clone(&store));
    let queue = queue();
    engine
        .submit(SubmitRequest::new(
            queue.clone(),
            serde_json::json!({ "to": "a@b" }),
        ))
        .await
        .expect("submitting to an empty memory store cannot fail");
    // The lines this case is about are the ones the worker writes, and it writes none until it has
    // claimed something: what happened before that is another case's evidence.
    sink.0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();

    let shutdown = Shutdown::new();
    let worker = Worker::new(
        Arc::clone(&store),
        handler,
        WorkerSettings::new(WorkerId::new("meter-1"), vec![queue])
            .with_poll_interval(Duration::from_millis(10)),
    );
    let signal = shutdown.clone();
    tokio::spawn(async move {
        let _ = worker.run(signal).await;
    });
    (store, shutdown)
}

/// Wait for a line that satisfies `wanted`, and answer it.
async fn wait_for(sink: &Sink, wanted: impl Fn(&Value) -> bool, what: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(found) = sink.lines().into_iter().find(&wanted) {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "no line {what} within {DEADLINE:?}: {:?}",
            sink.lines()
        );
        tokio::time::sleep(POLL).await;
    }
}

/// The sentence the failing handler fails with.
const FAILURE: &str = "the subscriber said 421";

/// A handler that fails every attempt, with a sentence of its own.
struct Failing;

impl Handler for Failing {
    fn handle<'a>(&'a self, _job: &'a Job) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            Err(Error::Invalid {
                detail: FAILURE.to_owned(),
            })
        })
    }
}

/// The line about one attempt carries what a query needs, and the level is part of it.
#[tokio::test]
async fn an_attempt_line_carries_the_fields_a_query_needs() {
    let sink = Sink::default();
    let guard = tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .with_max_level(LevelFilter::INFO)
        .with_writer(sink.clone())
        .set_default();

    let (_store, shutdown) = drain_with(Failing, &sink).await;
    let event = wait_for(
        &sink,
        |event| fields(event).get("outcome").and_then(Value::as_str) == Some("failed"),
        "about a failed attempt",
    )
    .await;
    shutdown.trigger();
    drop(guard);

    assert_eq!(
        event["level"], "WARN",
        "a failed attempt is a warning: `everything at this level, in this window` is the query an \
         operator has after an alert ({event})"
    );
    assert_eq!(field(&event, "queue"), "emails");
    assert_eq!(
        field(&event, "attempt"),
        1,
        "the claim spent the first attempt"
    );
    assert_eq!(field(&event, "worker"), "meter-1");
    assert_eq!(field(&event, "outcome"), "failed");
    assert_eq!(
        field(&event, "reason"),
        Error::Invalid {
            detail: FAILURE.to_owned()
        }
        .to_string()
        .as_str(),
        "the handler's own words belong on the line, and they are the same words the row keeps as \
         `last_error` (`jobq/3.1-engine-boundary`): a line that paraphrased them would be a line \
         an operator cannot search the row with"
    );
    assert!(
        field(&event, "took_ms").is_u64(),
        "how long the attempt took has to be a number a query can add up, and a formatted duration \
         is a string a dashboard has to parse ({event})"
    );
    assert!(
        field(&event, "job")
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "the line has to name the job, which is the only thing that connects it to the row: {event}"
    );
}

/// A line about a job names the job — and never the payload.
#[tokio::test]
async fn a_line_about_a_job_never_carries_the_payload() {
    let sink = Sink::default();
    let guard = tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .with_max_level(LevelFilter::INFO)
        .with_writer(sink.clone())
        .set_default();

    // The reference handler is the one that talks about the job it took, so it is the one that can
    // be tempted to say too much.
    let (_store, shutdown) = drain_with(jobq::service::handler::LogHandler, &sink).await;
    let event = wait_for(
        &sink,
        |event| fields(event).get("outcome").and_then(Value::as_str) == Some("succeeded"),
        "about a succeeded attempt",
    )
    .await;
    let lines = sink.lines();
    shutdown.trigger();
    drop(guard);

    for line in &lines {
        for name in ["payload", "to"] {
            assert!(
                !fields(line).contains_key(name),
                "a line is data a collector indexes and a payload is the customer's own record: a \
                 field that is unbounded, unqueryable and somebody's address is not a field a queue \
                 writes ({line})"
            );
        }
    }
    assert_eq!(field(&event, "outcome"), "succeeded");
    assert_eq!(
        event["level"], "INFO",
        "an attempt that finished is information, not a warning ({event})"
    );
    assert_eq!(
        field(&event, "reason"),
        "",
        "the field set is the same on every attempt: an empty value is what \"no reason\" looks like \
         to a query, and a field that is only sometimes there is a query that has to handle it \
         ({event})"
    );
}

/// A retry is the budget working; a reclaim is a loss, and no failure counter sees it.
#[tokio::test]
async fn retries_are_the_budget_working_and_a_reclaim_spends_it() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let engine = Engine::new(Arc::clone(&store));
    let queue = queue();
    let worker = WorkerId::new("meter-1");
    let budget = Budget::standard();

    let before = engine.usage().await.expect("a reading cannot fail");
    let before = reading(&before);

    // A hundred jobs that finish, one that fails on its first attempt and succeeds on its second,
    // and two attempts that end with nobody having finished them.
    for index in 0..100 {
        let id = engine
            .submit(SubmitRequest::new(
                queue.clone(),
                serde_json::json!({ "n": index }),
            ))
            .await
            .expect("submitting cannot fail");
        engine
            .claim(&worker, &queue, 1)
            .await
            .expect("claiming cannot fail");
        engine
            .complete(&worker, &id)
            .await
            .expect("completing cannot fail");
    }
    let retried = engine
        .submit(SubmitRequest::new(
            queue.clone(),
            serde_json::json!({ "n": 100 }),
        ))
        .await
        .expect("submitting cannot fail");
    engine
        .claim(&worker, &queue, 1)
        .await
        .expect("claiming cannot fail");
    engine
        .fail(&worker, &retried, "the socket timed out")
        .await
        .expect("recording a failure cannot fail");
    engine
        .claim(&worker, &queue, 1)
        .await
        .expect("claiming cannot fail");
    engine
        .complete(&worker, &retried)
        .await
        .expect("completing cannot fail");

    let with_a_retry = engine.usage().await.expect("a reading cannot fail");
    let with_a_retry = budget.burn_between(&before, &reading(&with_a_retry));
    assert_eq!(
        with_a_retry.bad(),
        1,
        "one attempt of this window reported a failure"
    );
    assert_eq!(
        with_a_retry.total(),
        102,
        "a retried attempt is two settles and one job, and the settles are what a queue's work is "
    );
    assert!(
        !with_a_retry.exceeded(),
        "one failed attempt out of {} spent the budget of a 99% queue: a retry is the engine doing \
         what it was built to do, and paging on it is how an alert becomes noise ({with_a_retry:?})",
        with_a_retry.total()
    );

    // Two attempts that end without anybody finishing them: a worker that stopped holding them, or
    // a lease that ran out. No failure counter in this crate moves for either.
    for index in 0..2 {
        engine
            .submit(SubmitRequest::new(
                queue.clone(),
                serde_json::json!({ "lost": index }),
            ))
            .await
            .expect("submitting cannot fail");
    }
    let claimed = engine
        .claim(&worker, &queue, 2)
        .await
        .expect("claiming cannot fail");
    assert_eq!(claimed.len(), 2);
    let released = store.release(&worker).await.expect("releasing cannot fail");
    assert_eq!(
        released, 2,
        "both attempts were handed back rather than finished"
    );

    let after = engine.usage().await.expect("a reading cannot fail");
    let burned = budget.burn_between(&before, &reading(&after));
    assert_eq!(
        burned.bad(),
        3,
        "the failure and the two attempts nobody finished"
    );
    assert!(
        burned.exceeded(),
        "a window that lost every job it claimed read as healthy: the two reclaims are what no \
         failure counter in this crate counts, and a budget that cannot see work nobody finished is \
         a budget that reads zero through an outage ({burned:?})"
    );
}

/// One queue's reading of the counters, out of a store's report.
fn reading(report: &[jobq::engine::QueueUsage]) -> Usage {
    report
        .iter()
        .find(|usage| usage.queue() == &queue())
        .map_or_else(Usage::default, jobq::engine::QueueUsage::usage)
}

/// A share below the floor is not a claim about a queue, and a share above it is.
#[test]
fn a_window_below_the_floor_cannot_page() {
    let budget = Budget::standard();
    let quiet = signals::Burn::of(5, 20, budget);
    assert!(
        quiet.below_floor(),
        "twenty settled attempts is not enough traffic for a share to mean anything ({quiet:?})"
    );
    assert!(
        !quiet.exceeded(),
        "five failures in twenty attempts is a 25% share and also one bad minute on a queue nobody \
         was using: paging on it pages on every quiet hour, and that is how an alert stops being \
         read ({quiet:?})"
    );
    let loud = signals::Burn::of(5, 120, budget);
    assert!(!loud.below_floor());
    assert_eq!(
        loud.allowed(),
        1,
        "1% of 120 settled attempts is one bad attempt, rounded down: the budget is a share, and \
         rounding it up would let a window spend a failure it was not given"
    );
    assert!(
        loud.exceeded(),
        "five failures in 120 attempts is 4% of a queue that promised 99% ({loud:?})"
    );
    assert!(
        !signals::Burn::of(1, 120, budget).exceeded(),
        "and one failure in 120 is inside it: the rule is about the share, not about a failure \
         having happened"
    );
    assert_eq!(
        signals::signals()
            .iter()
            .filter(|family| family.kind() == SignalKind::Gauge)
            .count(),
        1,
        "exactly one of these families is a reading rather than a total, and it is the one a query \
         cannot ask \"how fast\" about"
    );
}
