//! Spec for `jobq/3.26-dlq-cli`: the dead letters from the command line, judged by running it.
//!
//! `kiln run` copies this file into the project as `tests/spec_dlq_cli.rs`. Eleven cases, in four
//! groups, and the grouping is the argument:
//!
//! * **the door is a command line**, so it is judged by running the binary (`CARGO_BIN_EXE_jobqc`)
//!   with the environment a script would give it. The usage text, the refusal classes and the
//!   stdout/stderr split are all facts about a *process*, and the one case that needs a queue two
//!   processes can share starts a container of its own — which is why this step declares a
//!   Postgres image at all: an operator's tool that cannot be pointed at the queue the operator is
//!   looking at is a tool nobody can try.
//! * **the act is a port method**, so it is judged through the port: `Store::operate` takes an
//!   [`Order`], writes the line the record keeps, and answers it — and `Store::acts` reads the
//!   record back. Those cases run against the store `KILN_BACKEND` names (`memory` unless the
//!   environment says otherwise), which is what makes `conformance@2` mean something here: the
//!   same arithmetic, the same refusals and the same record on both backends.
//! * **the record is the deliverable**, so the two things that make it worth anything are asserted
//!   directly: a *refused* act leaves no line and moves nothing, and a *discarded* job leaves the
//!   line and nothing else — no row, no trail, nothing for the next command to find. A record that
//!   could be written for an act that did not happen is worse than no record at all, because it is
//!   the one an auditor trusts.
//! * **the port the acts go through is a port this step edits**, so the promises those files
//!   already made are asserted on the same store the acts were taken on: a job the door put back
//!   is admitted by the queue's own policy (the cap is counted over the work that is *running*,
//!   and aging can make a patient job win), and every life the door gives a job is a line in its
//!   trail. A shared file is judged by the suite of the step that changes it, and three of these
//!   four cases are the ones that would have noticed otherwise.
//!
//! The instant every port-level case runs at is a fake clock stopped in 2030 (`jobq/3.8-clock-injection`),
//! so "when the store took the act" is a value this file states rather than one it waits for; the
//! container case runs at the real clock, because the *binary* reads it and this file may not
//! pretend to know what it read.

mod support;

use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use jobq::cli::{self, USAGE};
use jobq::engine::{
    Act, Clock, Error, EventKind, Job, JobAudit, JobFilter, JobId, JobState, Order, Policy,
    PostgresStore, Queue, QueueDeclaration, Store, SubmitRequest, Tenant, WorkerId, dead_letters,
};
use support::postgres::Postgres;
use support::store::{Backend, backend};

/// The queue every dead letter in this file dies in.
const QUEUE: &str = "signup";

/// The instant the fake clocks start at: far enough away that a backend reading a database clock
/// instead of the injected one cannot be mistaken for one that does not.
const START: &str = "2030-01-01T00:00:00Z";

/// The sentence a dead letter's last attempt died with.
const REASON: &str = "the smtp server said 550 and meant it";

/// One `jobqc` run: what a script would have seen.
#[derive(Debug)]
struct Ran {
    /// The exit code, or a panic's worth of explanation when the process was killed.
    code: i32,
    /// What standard output received.
    stdout: String,
    /// What standard error received.
    stderr: String,
}

/// Run the real binary, with exactly the environment the case names.
///
/// `env_clear` is the point of the signature: a script's environment is part of the interface, and
/// a case that inherited the developer's `JOBQ__…` variables would be a case about this machine
/// rather than about the tool. Standard input is `/dev/null`, so a command that wrongly waited for
/// input fails instead of hanging the suite.
fn jobqc(args: &[&str], env: &[(&str, &str)]) -> Ran {
    let mut command = Command::new(env!("CARGO_BIN_EXE_jobqc"));
    command
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("`jobqc {args:?}` could not be started: {error}"));
    Ran {
        code: output.status.code().unwrap_or_else(|| {
            panic!("`jobqc {args:?}` was killed by a signal: it must exit with a code")
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
         half a document, and it got `{}`",
        ran.stdout
    );
    assert!(
        !ran.stderr.trim().is_empty(),
        "{what} must say what went wrong on stderr: an exit code on its own sends the reader to \
         the source"
    );
}

/// The failure rule for a call the tool could not understand, with and without `--json`: the code
/// is the failure's class and a format does not move it (`jobq/3.15-cli-contract`).
fn refused_the_same_either_way(args: &[&str], env: &[(&str, &str)], code: i32, what: &str) {
    let text = jobqc(args, env);
    refused(&text, code, what);
    let mut json = args.to_vec();
    json.push("--json");
    let with_json = jobqc(&json, env);
    refused(&with_json, code, what);
    assert_eq!(
        with_json.code, text.code,
        "`--json` is a format and not a second command: `{args:?}` exits the same either way"
    );
}

/// A clock the case moves by hand, so that "the instant the act was taken at" is a value.
#[derive(Debug)]
struct FakeClock {
    /// The instant everything measured from it is measured from.
    now: Mutex<DateTime<Utc>>,
}

impl FakeClock {
    /// A clock stopped at `instant` (RFC 3339).
    fn at(instant: &str) -> Arc<Self> {
        Arc::new(Self {
            now: Mutex::new(at(instant)),
        })
    }

    /// The same clock, `seconds` later.
    ///
    /// Waiting is the one thing these cases may not do (`jobq/3.22`: a suite that sleeps is a
    /// suite that fails on a loaded machine), and the two promises that are measured *after* time
    /// has passed are measured by moving the clock the store reads.
    fn advance(&self, seconds: i64) {
        let mut now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        *now += chrono::TimeDelta::seconds(seconds);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// An instant written the way this file reads them.
fn at(text: &str) -> DateTime<Utc> {
    text.parse()
        .unwrap_or_else(|error| panic!("`{text}` is an instant: {error}"))
}

/// A queue, by name.
fn queue(name: &str) -> Queue {
    Queue::parse(name).unwrap_or_else(|error| panic!("`{name}` is a queue name: {error}"))
}

/// A tenant, by name.
fn tenant(name: &str) -> Tenant {
    Tenant::parse(name).unwrap_or_else(|error| panic!("`{name}` is a tenant name: {error}"))
}

/// A job identity written the way a caller has to type it.
fn uuid() -> String {
    JobId::new().to_string()
}

/// The worker the cases claim with.
fn worker() -> WorkerId {
    WorkerId::new("dlq-cli-suite")
}

/// A store of this run's backend, reading every instant from `clock`.
///
/// The fixture hands out a store by reference and this is a second one over the same backend, the
/// shape `jobq/3.12-dedup-window` established: the cases want `&dyn Store`, the durable backend's
/// container is the same database either way, and the clock is the case's so that an act's instant
/// is something it can name.
async fn store_for(backend: &Backend, clock: &Arc<FakeClock>) -> Arc<dyn Store> {
    store_with(backend, clock, Policy::strict()).await
}

/// A store of this run's backend under `policy`.
///
/// The policy is a parameter for the same reason the clock is: a promise about *admission* (a cap,
/// an aging interval) is a promise the queue keeps for every kind of work, and the only way to
/// assert one is to ask a store that was built to keep it — on both backends, because both of them
/// carry the policy into the statement or the scan that chooses the rows.
async fn store_with(backend: &Backend, clock: &Arc<FakeClock>, policy: Policy) -> Arc<dyn Store> {
    let clock = Arc::clone(clock) as Arc<dyn Clock>;
    match backend {
        Backend::Memory(_) => Arc::new(
            jobq::engine::MemoryStore::new()
                .with_clock(clock)
                .with_policy(policy),
        ),
        Backend::Postgres { container, .. } => Arc::new(
            PostgresStore::connect(&container.url())
                .await
                .expect("the container the fixture started is reachable")
                .with_clock(clock)
                .with_policy(policy),
        ),
    }
}

/// One dead letter, built through the port: a job with one attempt, claimed and failed.
async fn dead_letter(store: &dyn Store, queue: &Queue, priority: i32) -> JobId {
    let id = store
        .enqueue(
            SubmitRequest::new(
                queue.clone(),
                serde_json::json!({ "to": "ada@example.com" }),
            )
            .with_priority(priority)
            .with_max_attempts(1),
        )
        .await
        .unwrap_or_else(|error| panic!("the port accepts this request: {error}"))
        .id();
    store
        .claim(&worker(), queue, 1)
        .await
        .unwrap_or_else(|error| panic!("claiming from a due queue cannot fail here: {error}"));
    store
        .fail(&worker(), &id, REASON)
        .await
        .unwrap_or_else(|error| panic!("recording a failure cannot fail here: {error}"));
    id
}

/// One row, as the port holds it: `None` for a row that is not there.
async fn row(store: &dyn Store, id: JobId) -> Option<Job> {
    store
        .get(&id)
        .await
        .unwrap_or_else(|error| panic!("reading one row cannot fail here: {error}"))
}

/// The record, as the case reads it: newest first, at most `limit` of them.
async fn record(store: &dyn Store, limit: usize) -> Vec<JobAudit> {
    store
        .acts(limit)
        .await
        .unwrap_or_else(|error| panic!("reading the record cannot fail here: {error}"))
}

/// The fields of one record line, as a tuple a case can compare with `assert_eq!`.
fn fields(line: &JobAudit) -> (Act, JobId, String, String, DateTime<Utc>, Option<String>) {
    (
        line.action(),
        line.job(),
        line.queue().as_str().to_owned(),
        line.actor().to_owned(),
        line.at(),
        line.reason().map(str::to_owned),
    )
}

/// Asking the tool what it can do is output, and it names the commands that act on a dead letter.
#[test]
fn the_help_text_names_the_commands_that_act_on_a_dead_letter() {
    let asked = cli::run(["--help".to_owned()], &[], &mut std::io::empty());
    assert_eq!(
        asked.stdout(),
        USAGE,
        "the usage text is the library's, byte for byte"
    );

    let ran = jobqc(&["--help"], &[]);
    assert_eq!(ran.code, 0, "asking for help is not a failure: {ran:?}");
    assert_eq!(ran.stderr, "", "and nothing is a diagnosis here");
    assert_eq!(
        ran.stdout, USAGE,
        "the binary writes exactly what the library answered: a second copy of the grammar in the \
         binary is a second grammar"
    );
    for command in ["dlq", "list", "replay", "discard", "audit"] {
        assert!(
            ran.stdout.contains(command),
            "`--help` must name `{command}`: a command nobody can discover from the help text is a \
             command that exists only in a wiki"
        );
    }
    assert!(
        ran.stdout.contains("--reason") && ran.stdout.contains("--by"),
        "and the two flags that make an act reviewable are in it: a tool whose `--help` does not \
         mention the reason it demands is a tool that refuses a script for a reason nobody read"
    );

    // The four commands `jobq/3.15-cli-contract` shipped are still there, and still spelled the
    // same: this step grows the grammar rather than replacing it.
    for command in ["submit", "inspect", "list", "usage"] {
        assert!(
            ran.stdout.contains(command),
            "`{command}` is still a command `jobq/3.15-cli-contract` promised"
        );
    }
}

/// A call the tool cannot understand is the caller's class, and the message names what it refused.
#[test]
fn a_call_the_tool_cannot_understand_is_a_usage_error_that_names_it() {
    // The group itself, with nothing to do.
    refused(&jobqc(&["dlq"], &[]), 2, "`dlq` with no subcommand");
    refused_the_same_either_way(
        &["dlq", "frobnicate"],
        &[],
        2,
        "a subcommand the group does not have",
    );
    assert!(
        jobqc(&["dlq", "frobnicate"], &[])
            .stderr
            .contains("frobnicate"),
        "and the refusal names the word it did not understand"
    );

    // The flags, one by one. None of these reaches a store, so they answer the same way whether or
    // not a queue is reachable — which is what makes `2` a code a script can act on.
    refused_the_same_either_way(
        &["dlq", "list", "--limit", "many"],
        &[],
        2,
        "a limit that is not a number",
    );
    refused_the_same_either_way(
        &["dlq", "list", "--queue", "Not A Queue"],
        &[],
        2,
        "a queue name this engine cannot write down",
    );
    refused_the_same_either_way(
        &["dlq", "list", "--frobnicate"],
        &[],
        2,
        "an option `dlq list` does not have",
    );
    refused_the_same_either_way(
        &["dlq", "replay", "not-a-uuid", "--by", "ada"],
        &[],
        2,
        "an identity that is not one",
    );
    let first = uuid();
    let second = uuid();
    refused_the_same_either_way(
        &["dlq", "replay", &first, "--by", "ada", &second],
        &[],
        2,
        "two identities for one act",
    );
    refused_the_same_either_way(
        &["dlq", "replay", &first],
        &[],
        2,
        "an act with nobody's name on it",
    );
    refused_the_same_either_way(
        &["dlq", "discard", &first, "--by", "ada"],
        &[],
        2,
        "a discard with no reason",
    );
    refused_the_same_either_way(
        &[
            "dlq", "replay", &first, "--reason", "why not", "--by", "ada",
        ],
        &[("JOBQ__STORE__URL", "memory")],
        3,
        "a replay of an identity nobody wrote",
    );
}

/// The whole door against a real queue: an operator looks, replays one, throws one away, and reads
/// what they did.
///
/// One container for the whole walk, because the tool is a *process*: the dead letters are built
/// through the port (the tool cannot fail a job — `jobqd` does that), and every command below is
/// the real binary against the same database.
#[tokio::test]
async fn an_operator_walks_the_dead_letters_from_the_command_line() {
    let container = Postgres::start();
    let url = container.url();
    let store = PostgresStore::connect(&url)
        .await
        .expect("the container the fixture started is reachable");
    store
        .migrate()
        .await
        .expect("this project's migration applies to the schema it describes");
    let queue = queue(QUEUE);
    let urgent = dead_letter(&store, &queue, 5).await;
    let ordinary = dead_letter(&store, &queue, 0).await;
    let env: [(&str, &str); 1] = [("JOBQ__STORE__URL", &url)];

    // What is stuck: two rows, the way the operator's *decide* is shaped — how many attempts each
    // one has spent, and the sentence its last attempt died with. The payload is not here, and its
    // absence is the point: an operator deciding whether to replay an invoice does not need the
    // customer's data on their screen, in a ticket, or in a scrollback.
    let listed = jobqc(&["dlq", "list"], &env);
    assert_eq!(listed.code, 0, "{listed:?}");
    assert_eq!(listed.stderr, "", "an answer is not a diagnosis");
    let lines: Vec<&str> = listed.stdout.lines().collect();
    assert_eq!(
        lines,
        [
            format!("{urgent} {QUEUE} attempts=1 \"{REASON}\"").as_str(),
            format!("{ordinary} {QUEUE} attempts=1 \"{REASON}\"").as_str(),
        ],
        "the view is one line per dead letter in the port's order — highest priority first — and \
         the error is escaped as a JSON string, which is what keeps one line one job: `{}`",
        listed.stdout
    );

    let bounded = jobqc(&["dlq", "list", "--limit", "1", "--json"], &env);
    assert_eq!(bounded.code, 0, "{bounded:?}");
    let document: serde_json::Value = serde_json::from_str(&bounded.stdout)
        .unwrap_or_else(|error| panic!("`dlq list --json` prints one document: {error}"));
    assert_eq!(
        document[0]["id"],
        serde_json::Value::from(urgent.to_string()),
        "the page holds the job the order puts first, and its identity is the one a later command \
         takes: {document}"
    );
    assert_eq!(document[0]["attempts"], serde_json::Value::from(1));
    assert_eq!(
        document[0]["last_error"],
        serde_json::Value::from(REASON),
        "and the reason the last attempt died with is a *value*, not a sentence to be parsed out \
         of a line"
    );
    assert_eq!(
        document[0].get("payload"),
        None,
        "the payload is not in the operator's view at all: {document}"
    );

    // A replay, signed, with a note. The answer is the line the record was given.
    let replayed = jobqc(
        &[
            "dlq",
            "replay",
            &urgent.to_string(),
            "--by",
            "ada",
            "--reason",
            "the outage is over",
        ],
        &env,
    );
    assert_eq!(replayed.code, 0, "{replayed:?}");
    let words: Vec<&str> = replayed.stdout.split_whitespace().collect();
    assert_eq!(
        words.get(1..5),
        Some(["replay", &urgent.to_string(), QUEUE, "ada"].as_slice()),
        "an act answers with the record line it just wrote — what, to which job, in which queue, \
         by whom — and the instant comes first because the line is a line: `{}`",
        replayed.stdout
    );
    assert_eq!(
        words.get(5..),
        Some(["the", "outage", "is", "over"].as_slice()),
        "and the note the operator left is the rest of the line, unquoted: a reason is a sentence \
         somebody typed, and a second escaping rule is a second thing to learn"
    );
    let at = words.first().copied().unwrap_or_default();
    assert!(
        DateTime::parse_from_rfc3339(at).is_ok(),
        "the first field is the instant the store took the act at, as RFC 3339: `{at}`"
    );
    assert!(
        row(&store, urgent)
            .await
            .is_some_and(|job| job.state() == JobState::Queued),
        "and the job is owed an attempt again: the line describes something that happened"
    );

    // A discard, signed, with the reason that is required of it.
    let discarded = jobqc(
        &[
            "dlq",
            "discard",
            &ordinary.to_string(),
            "--by",
            "grace",
            "--reason",
            "customer asked us to stop",
        ],
        &env,
    );
    assert_eq!(discarded.code, 0, "{discarded:?}");
    assert_eq!(
        discarded.stdout.split_whitespace().nth(1),
        Some("discard"),
        "the other act answers its own line: `{}`",
        discarded.stdout
    );
    assert!(
        row(&store, ordinary).await.is_none(),
        "the row is gone, which is what makes the line the only account of it there is"
    );
    assert_eq!(
        container.query(&format!(
            "select count(*) from jobs where id = '{}'",
            ordinary.as_uuid()
        )),
        "0",
        "and gone from the table rather than from this process's view of it: a tool that said it \
         threw something away and left the row behind is a tool whose record an auditor cannot \
         trust"
    );

    // What operators have done, newest first, with the reason on the one that needed it.
    let audit = jobqc(&["dlq", "audit", "--json"], &env);
    assert_eq!(audit.code, 0, "{audit:?}");
    let document: serde_json::Value = serde_json::from_str(&audit.stdout)
        .unwrap_or_else(|error| panic!("`dlq audit --json` prints one document: {error}"));
    let lines = document
        .as_array()
        .unwrap_or_else(|| panic!("the record is an array: {document}"));
    assert_eq!(lines.len(), 2, "two acts, two lines: {document}");
    assert_eq!(lines[0]["action"], serde_json::Value::from("discard"));
    assert_eq!(lines[0]["actor"], serde_json::Value::from("grace"));
    assert_eq!(
        lines[0]["reason"],
        serde_json::Value::from("customer asked us to stop")
    );
    assert_eq!(
        lines[0]["job"],
        serde_json::Value::from(ordinary.to_string())
    );
    assert_eq!(lines[1]["action"], serde_json::Value::from("replay"));
    assert_eq!(lines[1]["actor"], serde_json::Value::from("ada"));
    assert_eq!(
        lines[1]["reason"],
        serde_json::Value::from("the outage is over")
    );

    // The text shape and the JSON shape are the same facts in the same order, so a person and a
    // script reading the record are reading one thing.
    let text = jobqc(&["dlq", "audit"], &env);
    assert_eq!(text.code, 0, "{text:?}");
    let first = text.stdout.lines().next().unwrap_or_default().to_owned();
    let mut fields = first.split_whitespace();
    assert_eq!(fields.next(), lines[0]["at"].as_str());
    assert_eq!(fields.next(), lines[0]["action"].as_str());
    assert_eq!(fields.next(), lines[0]["job"].as_str());
    assert_eq!(fields.next(), lines[0]["queue"].as_str());
    assert_eq!(fields.next(), lines[0]["actor"].as_str());

    // The queue is empty of dead letters: one job was put back, the other is gone — and the record
    // is not the queue's, so it still answers about a job that no longer exists anywhere else.
    assert_eq!(
        jobqc(&["dlq", "list", "--json"], &env).stdout.trim_end(),
        "[]",
        "an empty answer is `[]` rather than nothing at all"
    );

    // The refusals, each a different class, each against the real queue.
    refused(
        &jobqc(&["dlq", "inspect", &ordinary.to_string()], &env),
        2,
        "a subcommand `dlq` does not have",
    );
    refused(
        &jobqc(
            &[
                "dlq",
                "discard",
                &urgent.to_string(),
                "--by",
                "grace",
                "--reason",
                "again",
            ],
            &env,
        ),
        2,
        "a discard of a job that is owed an attempt rather than dead",
    );
    assert!(
        jobqc(
            &[
                "dlq",
                "discard",
                &urgent.to_string(),
                "--by",
                "grace",
                "--reason",
                "again"
            ],
            &env
        )
        .stderr
        .contains("queued"),
        "and the refusal names the state the job is in, which is the whole of what the operator \
         needs to know about it"
    );
    refused(
        &jobqc(&["dlq", "replay", &uuid(), "--by", "ada"], &env),
        3,
        "an identity the queue does not have",
    );
    refused(
        &jobqc(
            &[
                "dlq",
                "discard",
                &ordinary.to_string(),
                "--by",
                "grace",
                "--reason",
                "again",
            ],
            &env,
        ),
        3,
        "a second discard of something that is not there any more",
    );
}

/// An act is on the record, with the name of whoever took it and the instant the store took it.
#[tokio::test]
async fn an_act_is_recorded_with_whoever_took_it() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let queue = queue(QUEUE);
    let replayed = dead_letter(&store, &queue, 0).await;
    let thrown_away = dead_letter(&store, &queue, 0).await;

    assert_eq!(
        record(&store, 10).await,
        Vec::new(),
        "the record starts empty, and looking at the dead letters is not an act: the case below \
         is about what a *change* leaves, not about what a command leaves"
    );

    // The whitespace in the name and the reason is the point: a person types a trailing space, and
    // two spellings of one name in a record is a record nobody can count.
    let line = store
        .operate(&Order::replay(replayed, " ada "))
        .await
        .unwrap_or_else(|error| panic!("a signed replay of a dead letter is allowed: {error}"));
    assert_eq!(
        fields(&line),
        (
            Act::Replay,
            replayed,
            QUEUE.to_owned(),
            "ada".to_owned(),
            at(START),
            None,
        ),
        "one act, one line: what was done, to which job, in which queue, by whom, when, and why — \
         and a replay without a note has no why"
    );

    let note = store
        .operate(&Order::discard(
            thrown_away,
            "grace",
            "  duplicate of the other one  ",
        ))
        .await
        .unwrap_or_else(|error| panic!("a discard with a reason is allowed: {error}"));
    assert_eq!(
        fields(&note),
        (
            Act::Discard,
            thrown_away,
            QUEUE.to_owned(),
            "grace".to_owned(),
            at(START),
            Some("duplicate of the other one".to_owned()),
        ),
        "the act that cannot be undone carries the reason, trimmed: whitespace is not \
         documentation"
    );

    // Two acts, two lines, newest first — and the trail of `jobq/3.6-transaction-isolation` is
    // still there for the replay, because the record is about *acts* and the trail is about the
    // job: a replayed job carries both, and the one that was thrown away carries neither.
    assert_eq!(
        record(&store, 10).await,
        vec![note.clone(), line.clone()],
        "the newest act is the first line"
    );
    assert_eq!(
        record(&store, 1).await,
        vec![note],
        "and a page of one is the newest act: a record grows with every incident, so reading it \
         is reading a page"
    );
    let trail = store
        .events(&replayed)
        .await
        .unwrap_or_else(|error| panic!("reading one job's trail cannot fail here: {error}"));
    let kinds: Vec<EventKind> = trail.iter().map(|line| line.kind()).collect();
    assert_eq!(
        kinds,
        [EventKind::AttemptFailed, EventKind::Replayed],
        "the replayed job's trail is its whole life and then the act: the line that says the          attempt died, and the line the replay left — which is the one thing the record of acts          does not replace ({trail:?})"
    );
    assert_eq!(
        store
            .events(&thrown_away)
            .await
            .unwrap_or_else(|error| panic!("reading an empty trail cannot fail here: {error}")),
        Vec::new(),
        "and the job that was thrown away has no trail left: `migrations/0002_job_events.sql` \
         cascades, which is why the record had to be a table of its own"
    );
}

/// Both acts are refused before anything moves when the order does not hold, and the row decides
/// which of them is possible at all.
#[tokio::test]
async fn an_act_that_does_not_hold_changes_nothing() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let queue = queue(QUEUE);
    let dead = dead_letter(&store, &queue, 0).await;
    let live = store
        .enqueue(SubmitRequest::new(
            queue.clone(),
            serde_json::json!({ "to": "grace@example.com" }),
        ))
        .await
        .unwrap_or_else(|error| panic!("the port accepts this request: {error}"))
        .id();

    let refusals: [(&str, Result<JobAudit, Error>); 4] = [
        (
            "a discard with no reason at all",
            store.operate(&Order::discard(dead, "ada", "")).await,
        ),
        (
            "a discard whose reason is only whitespace",
            store.operate(&Order::discard(dead, "ada", "   ")).await,
        ),
        (
            "an act nobody is named for",
            store.operate(&Order::replay(dead, "  ")).await,
        ),
        (
            "a discard of a job that is owed an attempt rather than dead",
            store
                .operate(&Order::discard(live, "ada", "not ours"))
                .await,
        ),
    ];
    for (what, answer) in refusals {
        let refusal = match answer {
            Ok(line) => panic!("{what} has to be refused, and it was answered with {line:?}"),
            Err(refusal) => refusal,
        };
        assert_eq!(
            refusal.exit_code(),
            2,
            "{what} is the caller's class: the same call gets the same answer, and the caller is \
             the one who can fix it — {refusal}"
        );
    }
    let named = store
        .operate(&Order::discard(live, "ada", "not ours"))
        .await
        .err()
        .unwrap_or_else(|| panic!("a discard of a live job has to be refused"));
    assert!(
        named.to_string().contains("queued"),
        "and the state is named, because 'already owed an attempt' and 'finished' are two \
         different answers about a job the operator is looking at — it said `{named}`"
    );

    assert_eq!(
        record(&store, 10).await,
        Vec::new(),
        "not one refusal left a line: a record of acts that did not happen is the record an \
         auditor cannot use"
    );
    assert!(
        row(&store, dead)
            .await
            .is_some_and(|job| job.state() == JobState::Failed),
        "and the dead letter is still dead — a refused act discards nothing"
    );
    assert_eq!(
        row(&store, live).await.map(|job| job.state()),
        Some(JobState::Queued),
        "and so is the job that was not a dead letter"
    );
}

/// The acts keep the promises the port's port-shaped methods keep: one transaction each, the same
/// answer from both backends, and no counter moved by an act that is not work.
#[tokio::test]
async fn an_act_is_the_ports_and_not_one_backends() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let queue = queue(QUEUE);
    let replayed = dead_letter(&store, &queue, 0).await;
    let thrown_away = dead_letter(&store, &queue, 0).await;
    let before = store
        .usage()
        .await
        .unwrap_or_else(|error| panic!("the usage report cannot fail here: {error}"));
    let before = before
        .iter()
        .find(|line| line.queue() == &queue)
        .map(|line| line.usage().claimed())
        .unwrap_or_default();

    store
        .operate(&Order::replay(replayed, "ada"))
        .await
        .unwrap_or_else(|error| panic!("a signed replay of a dead letter is allowed: {error}"));
    store
        .operate(&Order::discard(
            thrown_away,
            "grace",
            "customer asked us to stop",
        ))
        .await
        .unwrap_or_else(|error| panic!("a discard with a reason is allowed: {error}"));

    // The replay is what the second life is: the job is owed exactly one more attempt, which is a
    // claim handing it back rather than the ceiling being left where it was — a replay that only
    // moved the row would be invisible to the worker that is supposed to run it.
    let claimed = store
        .claim(&worker(), &queue, 1)
        .await
        .unwrap_or_else(|error| panic!("claiming from a due queue cannot fail here: {error}"));
    let job = claimed
        .first()
        .unwrap_or_else(|| panic!("the replayed job is owed an attempt, so a claim takes it"));
    assert_eq!(job.id(), replayed, "and it is the job that was replayed");
    assert_eq!(job.attempts(), 2, "one attempt spent, one owed");
    assert_eq!(
        job.max_attempts(),
        2,
        "the ceiling moved to one past the count: `jobq/3.16-dlq-tooling`'s decision, taken \
         through this step's door as well"
    );

    // An act is not work. The counters of `jobq/3.13-queue-authz-attribution` are written where
    // the work happens — a submit, a claim, a settle — and an operator moving a dead letter is
    // none of those: a report that counted replays would say a queue did work it did not.
    let after = store
        .usage()
        .await
        .unwrap_or_else(|error| panic!("the usage report cannot fail here: {error}"));
    let after = after
        .iter()
        .find(|line| line.queue() == &queue)
        .map(|line| line.usage().claimed())
        .unwrap_or_default();
    assert_eq!(
        after,
        before + 1,
        "the only claim between the two reports is the one this case made: the replay and the \
         discard moved no counter"
    );
    assert_eq!(
        record(&store, 10).await.len(),
        2,
        "and both acts are on the record, which is where an operator's work is counted: {}",
        record(&store, 10).await.len()
    );
}

/// Every life the door gives a job is a line of its own in the trail, and one act is one line in
/// the record — two questions about the same act, answered by two places.
///
/// The identity of a trail line is `(kind, attempt)` (`jobq/3.6-transaction-isolation`), and this
/// case is the one that shows the door's line is a line *per life*: the act that gives a life back
/// and the failure that ends it are different lines, and a trail that collapsed them by kind alone
/// would lose the history an incident is reconstructed from.
#[tokio::test]
async fn every_life_the_door_gives_a_job_is_a_line_in_its_trail() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let queue = queue(QUEUE);
    let dead = dead_letter(&store, &queue, 0).await;

    // Two lives, in order, with the clock moved between the four moves so that the trail is the
    // order things happened in rather than a tie the port breaks by kind.
    for life in 1..=2u32 {
        clock.advance(10);
        store
            .operate(&Order::replay(dead, "ada"))
            .await
            .unwrap_or_else(|error| panic!("replay {life} of a dead letter is signed: {error}"));
        clock.advance(10);
        let claimed = store
            .claim(&worker(), &queue, 1)
            .await
            .unwrap_or_else(|error| panic!("a replayed job is due, so a claim takes it: {error}"));
        assert_eq!(
            claimed.first().map(Job::id),
            Some(dead),
            "life {life} is the job that was replayed"
        );
        assert_eq!(
            claimed.first().map(Job::attempts),
            Some(life + 1),
            "and life {life} is the attempt the replay did not spend: the count is the history and \
             the ceiling is the policy (`jobq/3.16-dlq-tooling`)"
        );
        clock.advance(10);
        store
            .fail(&worker(), &dead, REASON)
            .await
            .unwrap_or_else(|error| panic!("recording a failure cannot fail here: {error}"));
    }

    let trail = store
        .events(&dead)
        .await
        .unwrap_or_else(|error| panic!("reading one job's trail cannot fail here: {error}"));
    let lines: Vec<(EventKind, u32)> = trail
        .iter()
        .map(|line| (line.kind(), line.attempt()))
        .collect();
    assert_eq!(
        lines,
        [
            (EventKind::AttemptFailed, 1),
            (EventKind::Replayed, 1),
            (EventKind::AttemptFailed, 2),
            (EventKind::Replayed, 2),
            (EventKind::AttemptFailed, 3),
        ],
        "the death that made it dead, the two acts and the two deaths they led to: five lines, in \
         the order they happened. A trail with {} line(s) has lost one, and the attempt (with the \
         kind) is what tells two of them apart",
        trail.len()
    );
    assert_eq!(
        record(&store, 10).await.len(),
        2,
        "and the record of acts is the other question: two replays, two lines, whoever took them"
    );
}

/// A job the door put back is admitted by the queue's own cap, and the cap is counted over the
/// work that is **running** — so a queue full of dead letters is a queue with room, and a slot
/// that is taken is a slot the next replay waits for.
#[tokio::test]
async fn a_replayed_job_waits_for_a_slot_like_any_other_work() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let policy = Policy::strict().with_cap(1);
    let store = store_with(&backend, &clock, policy).await;
    let queue = queue(QUEUE);
    let first = dead_letter(&store, &queue, 0).await;
    let second = dead_letter(&store, &queue, 0).await;
    for id in [first, second] {
        store
            .operate(&Order::replay(id, "ada"))
            .await
            .unwrap_or_else(|error| panic!("a signed replay of a dead letter is allowed: {error}"));
    }

    // Nothing is running, so the cap leaves room for the job the door put back. A count taken over
    // the rows that are *queued* — the shape a filter that lost its state comparison has — reads
    // two, subtracts them from the cap, and refuses the queue its own slot.
    let claimed = store
        .claim(&worker(), &queue, 1)
        .await
        .unwrap_or_else(|error| panic!("claiming from a due queue cannot fail here: {error}"));
    let running = claimed.first().map(Job::id).unwrap_or_else(|| {
        panic!("the queue had room, so the claim takes the job the replay put back: {claimed:?}")
    });
    let waiting = if running == first { second } else { first };

    // The slot is the cap, and it is now taken: the second replayed job is not refused, it is not
    // admitted yet.
    assert_eq!(
        store
            .claim(&worker(), &queue, 1)
            .await
            .unwrap_or_else(|error| panic!("a claim with no room cannot fail here: {error}")),
        Vec::new(),
        "one running job is the whole cap, so the claim takes nothing: what is waiting is waiting \
         for the slot rather than for its turn"
    );

    // And finishing the job that holds it hands the slot over.
    store
        .complete(&worker(), &running)
        .await
        .unwrap_or_else(|error| panic!("finishing a running job cannot fail here: {error}"));
    let next = store
        .claim(&worker(), &queue, 1)
        .await
        .unwrap_or_else(|error| panic!("claiming after a completion cannot fail here: {error}"));
    assert_eq!(
        next.first().map(Job::id),
        Some(waiting),
        "the replayed job that was waiting takes the slot the finished one left"
    );
}

/// A replay makes the job due at the instant the replay happened (`jobq/3.16-dlq-tooling`'s
/// arithmetic), and from there it is work like any other: the queue's aging can make it win against
/// a higher priority that arrived later, because age is what a fairness policy trades priority for.
#[tokio::test]
async fn a_replayed_job_ages_like_any_other_work() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let policy = Policy::strict().with_aging(Duration::from_secs(60));
    let store = store_with(&backend, &clock, policy).await;
    let queue = queue(QUEUE);
    let replayed = dead_letter(&store, &queue, 0).await;
    store
        .operate(&Order::replay(replayed, "ada"))
        .await
        .unwrap_or_else(|error| panic!("a signed replay of a dead letter is allowed: {error}"));

    // Ten intervals later a job with a real priority arrives. The replayed one has been due since
    // the act was taken, which is ten ranks of age against five of priority.
    clock.advance(600);
    let urgent = store
        .enqueue(
            SubmitRequest::new(
                queue.clone(),
                serde_json::json!({ "to": "grace@example.com" }),
            )
            .with_priority(5),
        )
        .await
        .unwrap_or_else(|error| panic!("the port accepts this request: {error}"))
        .id();
    let claimed = store
        .claim(&worker(), &queue, 1)
        .await
        .unwrap_or_else(|error| panic!("claiming from a due queue cannot fail here: {error}"));
    assert_eq!(
        claimed.first().map(Job::id),
        Some(replayed),
        "the job that has been due for ten minutes outranks the one that just arrived: an aged \
         rank that subtracted the wait instead of adding it puts the fresh one first \
         (`jobq/3.9-priority-fairness`, and the replay is where the wait starts over)"
    );
    assert_ne!(
        claimed.first().map(Job::id),
        Some(urgent),
        "and the job that arrived ten minutes later is still due, so the next claim takes it"
    );
}

/// The handle is the identity: what a store may act on is what its access model names.
#[tokio::test]
async fn the_handle_decides_whose_dead_letters_are_acted_on() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let acme = tenant("acme");
    let bob = tenant("bob");
    let mine = queue("acme-mail");
    let theirs = queue("bob-mail");
    for (name, owner) in [(&mine, &acme), (&theirs, &bob)] {
        let handle = store.scoped(owner);
        handle
            .declare_queue(&QueueDeclaration::new(name.clone(), owner.clone()))
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "the {} backend refused a declaration: {error}",
                    backend.name()
                )
            });
        handle
            .enqueue(
                SubmitRequest::new(name.clone(), serde_json::json!({ "to": "ada@example.com" }))
                    .with_max_attempts(1),
            )
            .await
            .unwrap_or_else(|error| {
                panic!("the {} backend refused a submit: {error}", backend.name())
            });
        let id = store
            .list(&JobFilter::new().with_queue(name.clone()))
            .await
            .unwrap_or_else(|error| {
                panic!("the {} backend refused a listing: {error}", backend.name())
            })
            .first()
            .map(Job::id)
            .unwrap_or_else(|| panic!("the job this case just submitted is in the listing"));
        let claim = handle
            .claim(&worker(), name, 1)
            .await
            .unwrap_or_else(|error| {
                panic!("the {} backend refused a claim: {error}", backend.name())
            });
        assert_eq!(claim.len(), 1, "each queue has one job due");
        handle
            .fail(&worker(), &id, REASON)
            .await
            .unwrap_or_else(|error| {
                panic!("the {} backend refused a failure: {error}", backend.name())
            });
    }
    let mine_dead = store
        .list(&dead_letters().with_queue(mine.clone()))
        .await
        .unwrap_or_else(|error| panic!("the {} backend refused a listing: {error}", backend.name()))
        .first()
        .map(Job::id)
        .unwrap_or_else(|| panic!("`{mine}` has a dead letter"));

    let owner = store.scoped(&acme);
    let acted = owner
        .operate(&Order::discard(
            mine_dead,
            "ada",
            "customer asked us to stop",
        ))
        .await
        .unwrap_or_else(|error| panic!("a queue's owner may throw its own work away: {error}"));
    assert_eq!(acted.queue(), &mine);
    let as_acme = store.scoped(&acme);
    let as_bob = store.scoped(&bob);
    assert_eq!(
        record(as_acme.as_ref(), 10).await.len(),
        1,
        "the owner reads its own act"
    );
    assert_eq!(
        record(as_bob.as_ref(), 10).await,
        Vec::new(),
        "and `bob` reads none of it: a line about a queue a handle may not see is a line that is \
         not in the answer, the rule `Store::list` and `Store::events` already keep"
    );
    assert_eq!(
        record(&store, 10).await.len(),
        1,
        "while the operator's door sees every queue"
    );

    // And a handle may not act on somebody else's work: the refusal is the one a row nobody wrote
    // gets, because a caller that may not use an identity must not be able to tell it from one
    // that does not exist.
    let theirs_dead = store
        .list(&dead_letters().with_queue(theirs.clone()))
        .await
        .unwrap_or_else(|error| panic!("the {} backend refused a listing: {error}", backend.name()))
        .first()
        .map(Job::id)
        .unwrap_or_else(|| panic!("`{theirs}` has a dead letter"));
    let ours = store.scoped(&acme);
    let refusal = ours
        .operate(&Order::replay(theirs_dead, "ada"))
        .await
        .err()
        .unwrap_or_else(|| panic!("`acme` may not replay `bob`'s dead letter"));
    assert_eq!(
        refusal.exit_code(),
        3,
        "it is answered like a row nobody wrote: {refusal}"
    );
    assert_eq!(
        record(&store, 10).await.len(),
        1,
        "and nothing was written for it"
    );
}

/// The page is part of the deliverable: an operator woken at three in the morning reads it.
#[test]
fn the_project_has_a_page_an_operator_can_follow() {
    let page = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/dlq.md");
    let text = std::fs::read_to_string(&page).unwrap_or_else(|error| {
        panic!(
            "`{}` could not be read: {error} — the procedure is part of this step, not a comment \
             on it",
            page.display()
        )
    });
    for command in ["jobqc dlq list", "jobqc dlq replay", "jobqc dlq discard"] {
        assert!(
            text.contains(command),
            "`{}` has to show `{command}`: a page about the dead-letter queue that does not name \
             the commands is a page somebody has to reconstruct from `--help`",
            page.display()
        );
    }
    for code in ["`3`", "`2`"] {
        assert!(
            text.contains(code),
            "and the classes an operator's script branches on are on it, `{code}` included: a tool \
             whose refusals are only in its source is a tool nobody automates"
        );
    }
    assert!(
        !text.contains("TODO"),
        "`{}` still has a placeholder in it",
        page.display()
    );
}
