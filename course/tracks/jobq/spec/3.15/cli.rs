//! Spec for `jobq/3.15-cli-contract`: the command line judged by running it.
//!
//! `kiln run` copies this file into the project as `tests/spec_cli.rs`. Four claims, and every one
//! of them is about the bytes a *process* writes rather than about a function that builds them:
//!
//! * **stdout is the answer, stderr is the diagnosis.** Every failure below asserts two things at
//!   once — nothing on stdout, a sentence on stderr — because that pair is the rule a script is
//!   built on. A command that printed half an answer and a warning on the way out would pass a test
//!   that only looked at `$?` and fail the parser downstream of it;
//! * **one exit code per failure class.** `2` when the call was wrong, `3` when the thing asked
//!   about is not there, `4` when the store refused, `1` when the deployment is broken — the table
//!   `jobq/3.3-library-errors` wrote, read by a shell. Each case below is a different class, and
//!   the four are all different from `0`;
//! * **`--json` is a format, not a second command.** The same call is run twice, with and without
//!   it: the code is the same, stdout is either one JSON document or a line that is not one, and a
//!   failure is empty either way;
//! * **the grammar is the library's.** The binary is a shell, so the same answer is asserted twice
//!   — once from `jobq::cli::run` inside this process and once from `CARGO_BIN_EXE_jobqc` — and the
//!   equality of the two is what makes "the binary only writes it" a fact rather than a comment.
//!
//! The last case is a script against a real database: `submit`, `submit --stdin` with a payload
//! full of the characters that break naive JSON writers, `inspect` of both, `list` with a filter
//! and a limit, `usage`, and an id that does not exist. It starts a container of its own, which is
//! also why this step declares an image: a command line is an interface *to a queue*, and a queue
//! the test cannot write to is not one. The schema is prepared through the library
//! (`PostgresStore::migrate`) rather than by `jobqc`: a tool that migrated on every `list` would be
//! a tool that writes on a read, and `jobqd` already owns that step.

mod support;

use std::io::Write;
use std::process::{Command, Stdio};

use jobq::cli::{self, USAGE};
use jobq::engine::{
    JobFilter, JobId, PostgresStore, Queue, QueueDeclaration, Store, SubmitRequest, Tenant,
};
use serde_json::Value;
use support::postgres::Postgres;

/// The queue the end-to-end script uses.
const QUEUE: &str = "signup";

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

impl Ran {
    /// The exit code, as the shell would read it.
    fn code(&self) -> i32 {
        self.code
    }
}

/// Run the real binary, with exactly the environment the case names and `input` on standard input.
///
/// `env_clear` is the point of the signature: a script's environment is part of the interface, and a
/// case that inherited the developer's `JOBQ__…` variables would be a case about this machine
/// rather than about the tool. Standard input is `/dev/null` unless the case says otherwise, so a
/// command that wrongly waited for input fails instead of hanging the suite.
fn jobqc(args: &[&str], env: &[(&str, &str)], input: Option<&str>) -> Ran {
    let mut command = Command::new(env!("CARGO_BIN_EXE_jobqc"));
    command
        .args(args)
        .env_clear()
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("`jobqc {args:?}` could not be started: {error}"));
    if let Some(input) = input {
        let mut pipe = child
            .stdin
            .take()
            .unwrap_or_else(|| panic!("the pipe this case asked for is there"));
        pipe.write_all(input.as_bytes())
            .unwrap_or_else(|error| panic!("the payload could not be written: {error}"));
        // Dropping the pipe closes it, which is how `--stdin` learns that the payload has ended.
    }
    let output = child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("`jobqc {args:?}` could not be waited for: {error}"));
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
        ran.code(),
        code,
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
        "{what} must say what went wrong on stderr: an exit code on its own sends the reader to the \
         source"
    );
}

/// Asking for help is output, not an error, and it lists what the tool can do.
#[test]
fn help_is_a_successful_run_on_stdout_and_names_every_command() {
    for asked in [vec!["--help"], vec!["-h"], vec!["help"], vec![]] {
        let ran = jobqc(&asked, &[], None);
        assert_eq!(
            ran.code(),
            0,
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
        for command in ["submit", "inspect", "list", "usage"] {
            assert!(
                ran.stdout.contains(command),
                "`--help` must name `{command}`: a command nobody can discover from the help text \
                 is a command that exists only in a wiki"
            );
        }
    }

    let version = jobqc(&["--version"], &[], None);
    assert_eq!(version.code(), 0);
    assert!(
        version.stdout.starts_with("jobqc ") && version.stdout.contains(env!("CARGO_PKG_VERSION")),
        "`--version` prints the version this binary was built with: {}",
        version.stdout
    );
}

/// A call the tool cannot understand is the caller's class, and the message names what it refused.
#[test]
fn an_unknown_command_is_a_usage_error_that_names_it() {
    // The argument every binary is probed with by the `exit-code` gate, so the gate and this case
    // are looking at the same failure path.
    let probed = jobqc(&["--kiln-exit-code-probe"], &[], None);
    refused(&probed, 2, "an unknown argument");
    assert!(
        probed.stderr.contains("--kiln-exit-code-probe"),
        "the message must name the argument it refused: a tool that says `unknown argument` sends \
         the reader back to their script to guess which one, and it said `{}`",
        probed.stderr
    );

    let command = jobqc(&["frobnicate"], &[], None);
    refused(&command, 2, "an unknown command");
    assert!(
        command.stderr.contains("frobnicate"),
        "and an unknown command is named too: {}",
        command.stderr
    );

    // A question a script cannot answer is a usage error rather than a prompt: each of these is
    // missing something, and the message says what.
    refused(
        &jobqc(&["submit", "--queue", QUEUE], &[], None),
        2,
        "a submit with no payload",
    );
    refused(
        &jobqc(&["submit", "--payload", "{}"], &[], None),
        2,
        "a submit with no queue",
    );
    refused(&jobqc(&["inspect"], &[], None), 2, "an inspect with no id");
    refused(
        &jobqc(&["list", "--limit"], &[], None),
        2,
        "a flag with no value",
    );
    refused(
        &jobqc(&["usage", "extra"], &[], None),
        2,
        "a command with an argument",
    );
}

/// The code is the failure's class, `--json` does not change it, and stdout stays empty.
#[test]
fn each_failure_class_has_its_own_code_and_json_does_not_move_it() {
    // `2`: the call was wrong. The cases below reach the parser before any store does, so they
    // answer the same way whether or not a queue is reachable.
    let cases: [(&[&str], i32, &str); 5] = [
        (
            &["submit", "--queue", "Not A Queue", "--payload", "{}"],
            2,
            "a queue name this engine cannot write down",
        ),
        (
            &["submit", "--queue", QUEUE, "--payload", "{oops"],
            2,
            "a payload that is not JSON",
        ),
        (&["inspect", "not-a-uuid"], 2, "an identity that is not one"),
        (
            &["list", "--state", "dead"],
            2,
            "a state the model does not have",
        ),
        (
            &["list", "--limit", "many"],
            2,
            "a limit that is not a number",
        ),
    ];
    for (args, code, what) in cases {
        let text = jobqc(args, &[], None);
        refused(&text, code, what);
        let mut json = args.to_vec();
        json.push("--json");
        let with_json = jobqc(&json, &[], None);
        refused(&with_json, code, what);
        assert_eq!(
            with_json.code(),
            text.code(),
            "`--json` is a format and not a second command: `{args:?}` exits the same either way"
        );
        assert_eq!(
            with_json.stderr.trim(),
            text.stderr.trim(),
            "and the diagnosis does not depend on how the answer would have been printed"
        );
    }

    // `3`: the thing asked about is not there. A memory store answers `NotFound` for an identity it
    // never wrote, which is the same `3` a real queue gives.
    refused(
        &jobqc(
            &["inspect", "67e55044-10b1-426f-9247-bb680e5fe0c8"],
            &[("JOBQ__STORE__URL", "memory")],
            None,
        ),
        3,
        "a job nobody wrote",
    );

    // `4`: the store refused. A connection string this build cannot open is the deployment's
    // problem — the class a script must *not* retry on its own — and it is the fast half of `4`: a
    // store that is merely unreachable takes the driver's own pool timeout to say so, and a spec
    // that spent thirty seconds per run proving that would be the slowest step in the track.
    refused(
        &jobqc(
            &["list"],
            &[("JOBQ__STORE__URL", "not a connection string")],
            None,
        ),
        4,
        "a store this build cannot open",
    );

    // `1`: the environment is broken rather than the call — here, a settings file that is not there.
    refused(
        &jobqc(&["list", "--config", "/nonexistent/jobqc.toml"], &[], None),
        1,
        "a configuration file that cannot be read",
    );
}

/// `--json` is one document, and the same command without it is not a document at all.
#[test]
fn json_is_one_value_where_the_text_shape_is_a_line() {
    let empty = jobqc(&["list", "--json"], &[("JOBQ__STORE__URL", "memory")], None);
    assert_eq!(empty.code(), 0, "an empty list is an answer: {empty:?}");
    assert_eq!(
        empty.stdout.trim_end(),
        "[]",
        "an empty answer is `[]` rather than nothing: a script that has to special-case no output \
         gets it wrong once and stops trusting the tool"
    );

    let submitted = jobqc(
        &[
            "--json",
            "submit",
            "--queue",
            QUEUE,
            "--payload",
            "{\"to\":\"ada@example.com\"}",
        ],
        &[("JOBQ__STORE__URL", "memory")],
        None,
    );
    assert_eq!(submitted.code(), 0, "{submitted:?}");
    let document: Value = serde_json::from_str(&submitted.stdout)
        .unwrap_or_else(|error| panic!("`--json` prints one JSON document: {error}"));
    assert!(
        document["id"].as_str().is_some_and(|id| !id.is_empty()),
        "and `submit` answers with the identity a later command takes: {document}"
    );

    // The same command without `--json`: one line, and not a JSON document.
    let text = jobqc(
        &[
            "submit",
            "--queue",
            QUEUE,
            "--payload",
            "{\"to\":\"ada@example.com\"}",
        ],
        &[("JOBQ__STORE__URL", "memory")],
        None,
    );
    assert_eq!(text.code(), 0, "{text:?}");
    assert!(
        text.stdout.starts_with("queued "),
        "the text shape says what happened and then the identity: `{}`",
        text.stdout
    );
    assert!(
        serde_json::from_str::<Value>(&text.stdout).is_err(),
        "and it is not a JSON document — a script that asked for a machine-readable answer gets \
         one, and a script that did not gets a line to read: `{}`",
        text.stdout
    );
}

/// The library answers the same bytes the binary writes, which is what keeps the binary a shell.
#[test]
fn the_grammar_is_the_librarys_and_the_binary_only_writes_it() {
    let asked = cli::run(["--help".to_owned()], &[], &mut std::io::empty());
    assert_eq!(asked.code(), 0);
    assert_eq!(asked.stdout(), USAGE, "the usage text is the library's");
    assert_eq!(asked.stderr(), "", "and help is not a diagnosis");

    let process = jobqc(&["--help"], &[], None);
    assert_eq!(
        process.stdout,
        asked.stdout(),
        "the binary writes exactly what the library answered: a second copy of the grammar in the \
         binary is a second grammar"
    );
    assert_eq!(process.code(), i32::from(asked.code()));

    let refused = cli::run(
        ["--kiln-exit-code-probe".to_owned()],
        &[],
        &mut std::io::empty(),
    );
    assert_eq!(refused.code(), 2, "a usage error is the caller's class");
    assert_eq!(refused.stdout(), "", "with nothing on the answer stream");
    assert!(
        refused.stderr().contains("--kiln-exit-code-probe"),
        "and the argument's name in the diagnosis: {}",
        refused.stderr()
    );
}

/// The whole contract against a real queue: a script that submits, looks, lists and reports.
#[tokio::test]
async fn a_script_submits_inspects_lists_and_reports_against_a_real_queue() {
    // One container for the whole script, and the schema is prepared through the library: `jobqc`
    // is an operator's tool, not a migration runner.
    let container = Postgres::start();
    let url = container.url();
    let store = PostgresStore::connect(&url)
        .await
        .expect("the container the fixture started is reachable");
    store
        .migrate()
        .await
        .expect("this project's migration applies to the schema it describes");
    let store_url: &str = &url;
    let env: [(&str, &str); 1] = [("JOBQ__STORE__URL", store_url)];

    // `submit`, and the identity comes out of stdout — the only channel a script has.
    let first = jobqc(
        &[
            "submit",
            "--queue",
            QUEUE,
            "--payload",
            "{\"to\":\"ada@example.com\"}",
            "--priority",
            "5",
        ],
        &env,
        None,
    );
    assert_eq!(first.code(), 0, "{first:?}");
    assert_eq!(
        first.stderr, "",
        "a command that answered says nothing on stderr: the two streams are the contract"
    );
    let first_id = first.stdout.trim().to_owned();
    assert!(
        first_id.starts_with("queued "),
        "a queued job is answered with `queued <id>`: `{}`",
        first.stdout
    );
    let id: JobId = first_id
        .trim_start_matches("queued ")
        .parse()
        .unwrap_or_else(|error| panic!("the identity is one a later command takes: {error}"));

    // `submit --stdin`, with the characters that break a JSON writer which escapes by hand.
    let payload =
        "{\"to\":\"grace@example.com\",\"note\":\"line1\\nline2 \\\"quoted\\\" \\\\ back\"}";
    let second = jobqc(
        &["submit", "--queue", QUEUE, "--stdin"],
        &env,
        Some(payload),
    );
    assert_eq!(second.code(), 0, "{second:?}");
    let second_id = second.stdout.trim().trim_start_matches("queued ");
    assert!(
        second_id.parse::<JobId>().is_ok(),
        "the second submit answers its identity too: `{}`",
        second.stdout
    );

    // `inspect` in the text shape: one line, and the payload is on it.
    let looked = jobqc(&["inspect", &id.to_string()], &env, None);
    assert_eq!(looked.code(), 0, "{looked:?}");
    assert_eq!(
        looked.stdout,
        format!("{id} {QUEUE} queued {{\"to\":\"ada@example.com\"}}\n"),
        "the text shape is one line of four fields, and the payload is compact JSON on it — which \
         is what keeps a line one job even when the payload is not"
    );

    // The same job with `--json`: one document, the same four fields, and the *payload as a value*
    // rather than as a string of JSON — `jq '.payload.note'` has to work without `fromjson`.
    let json = jobqc(&["inspect", &id.to_string(), "--json"], &env, None);
    assert_eq!(json.code(), 0, "{json:?}");
    let document: Value = serde_json::from_str(&json.stdout)
        .unwrap_or_else(|error| panic!("`inspect --json` prints one document: {error}"));
    assert_eq!(document["id"], Value::from(id.to_string()));
    assert_eq!(document["queue"], Value::from(QUEUE));
    assert_eq!(document["state"], Value::from("queued"));
    assert_eq!(document["payload"]["to"], Value::from("ada@example.com"));

    // What came back through `--stdin` is exactly what went in.
    let round_trip = jobqc(&["inspect", second_id.trim(), "--json"], &env, None);
    let document: Value = serde_json::from_str(&round_trip.stdout)
        .unwrap_or_else(|error| panic!("a payload with quotes survives the trip: {error}"));
    assert_eq!(
        document["payload"],
        serde_json::from_str::<Value>(payload).expect("the payload this case sent was JSON"),
        "a payload with quotes, backslashes and newlines must come back byte-for-byte as a value: \
         machine-readable-looking is not machine-readable"
    );

    // `list`: the whole queue, in the port's order — highest priority first — and then the same
    // answer filtered and bounded.
    let listed = jobqc(&["list", "--queue", QUEUE], &env, None);
    assert_eq!(listed.code(), 0, "{listed:?}");
    let lines: Vec<&str> = listed.stdout.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "two jobs went in, two lines come out: `{}`",
        listed.stdout
    );
    assert!(
        lines[0].starts_with(&id.to_string()),
        "the higher priority job is first, which is the order the port's `claim` documents: `{}`",
        lines[0]
    );
    // The payload on the line is the *engine's* compact form of the value: one JSON value written
    // the only way it fits on a line, which is what keeps one line one job.
    let canonical = serde_json::from_str::<Value>(payload)
        .expect("the payload this case sent was JSON")
        .to_string();
    assert_eq!(
        lines[1],
        format!("{second_id} {QUEUE} queued {canonical}"),
        "and the second line is the second job, payload and all"
    );

    let bounded = jobqc(&["list", "--limit", "1", "--json"], &env, None);
    let document: Value = serde_json::from_str(&bounded.stdout)
        .unwrap_or_else(|error| panic!("`list --json` prints one array: {error}"));
    assert_eq!(
        document.as_array().map(Vec::len),
        Some(1),
        "`--limit 1` is a page of one: {document}"
    );
    assert_eq!(
        document[0]["id"],
        Value::from(id.to_string()),
        "and the page holds the job the order puts first"
    );

    let elsewhere = jobqc(&["list", "--queue", "exports", "--json"], &env, None);
    assert_eq!(elsewhere.stdout.trim_end(), "[]");
    let failed = jobqc(&["list", "--state", "failed", "--json"], &env, None);
    assert_eq!(
        failed.stdout.trim_end(),
        "[]",
        "a filter that matches nothing is an empty answer, not an error"
    );

    // `usage`: the counters `jobq/3.13-queue-authz-attribution` writes where the work happens.
    let usage = jobqc(&["usage", "--json"], &env, None);
    assert_eq!(usage.code(), 0, "{usage:?}");
    let document: Value = serde_json::from_str(&usage.stdout)
        .unwrap_or_else(|error| panic!("`usage --json` prints one array: {error}"));
    let line = document
        .as_array()
        .and_then(|lines| lines.iter().find(|line| line["queue"] == QUEUE))
        .unwrap_or_else(|| panic!("`{QUEUE}` has a line in the report: {document}"));
    assert_eq!(
        line["submitted"],
        Value::from(2),
        "both submits are counted, and the counter came from the store rather than from this tool"
    );
    assert_eq!(
        line["running"],
        Value::from(0),
        "nothing is holding a job: `running` is the derived number, and it is the one an operator \
         reads first"
    );

    // An identity nobody wrote is the class `3`, against the real queue this time — the same
    // answer the memory store gave, because the class comes from the engine rather than from a
    // backend.
    let ghost = jobqc(
        &["inspect", "67e55044-10b1-426f-9247-bb680e5fe0c8"],
        &env,
        None,
    );
    refused(&ghost, 3, "an identity the queue does not have");

    // And the database is untouched by the refusals: two jobs, no more.
    assert_eq!(
        jobqc(&["list", "--json"], &env, None)
            .stdout
            .matches("\"id\"")
            .count(),
        2,
        "the whole answer is still the two jobs the script submitted"
    );
}

/// A listing answers with what *this* caller may see, on every backend.
///
/// `Store::list` is this step's method, and its promise is the port's: a row the handle may not see
/// is a row that is not in the answer — the same rule `get` and `events` answer one job with. The
/// case is asked of the store the conformance fixture builds, so the step's `conformance@2` gate
/// runs it against the container too: an implementation that forgets the scope on one backend is a
/// listing that leaks another tenant's work, which is the failure this file exists to catch.
#[tokio::test]
async fn a_listing_hides_the_queues_the_handle_may_not_see() {
    let backend = support::store::backend().await;
    let store = backend.store();
    let acme = Tenant::parse("acme").expect("a legal tenant name");
    let bob = Tenant::parse("bob").expect("a legal tenant name");
    let mine = Queue::parse("acme-mail").expect("a legal queue name");
    let theirs = Queue::parse("bob-mail").expect("a legal queue name");

    for (queue, owner) in [(&mine, &acme), (&theirs, &bob)] {
        let handle = store.scoped(owner);
        handle
            .declare_queue(&QueueDeclaration::new(queue.clone(), owner.clone()))
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "the {} backend refused a declaration: {error}",
                    backend.name()
                )
            });
        handle
            .enqueue(SubmitRequest::new(
                queue.clone(),
                serde_json::json!({ "to": queue.as_str() }),
            ))
            .await
            .unwrap_or_else(|error| {
                panic!("the {} backend refused a submit: {error}", backend.name())
            });
    }

    let listing = store
        .scoped(&acme)
        .list(&JobFilter::new())
        .await
        .unwrap_or_else(|error| {
            panic!("the {} backend refused a listing: {error}", backend.name())
        });
    let queues: Vec<&str> = listing.iter().map(|job| job.queue().as_str()).collect();
    assert_eq!(
        queues,
        [mine.as_str()],
        "`acme` submitted to its own queue and to nothing else, so its listing holds exactly that          job: the {} backend answered {queues:?} instead. A listing is read by a tool that shows an          operator what is in a queue, and a handle that answers with another tenant's queue is a          handle that leaks what it may not see (`jobq/3.13-queue-authz-attribution`)",
        backend.name()
    );

    let everything = store.list(&JobFilter::new()).await.unwrap_or_else(|error| {
        panic!("the {} backend refused a listing: {error}", backend.name())
    });
    let mut seen: Vec<&str> = everything.iter().map(|job| job.queue().as_str()).collect();
    seen.sort_unstable();
    assert_eq!(
        seen,
        ["acme-mail", "bob-mail"],
        "the operator's door sees every queue, so the filter is a narrowing rather than a          replacement: the {} backend answered {seen:?}",
        backend.name()
    );
}
