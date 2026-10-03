//! Spec for `mailer/1.6-logging`: a log line is data, not a sentence — a collector reads fields
//! out of it, and every line of one run says which run it belongs to.
//!
//! `kiln run` copies this file into the project as `tests/spec_logging.rs`; it is the course's
//! file, not the learner's (plan §3 D2). The service is started as a process and judged on what it
//! printed: `json` mode is parsed field by field, `pretty` mode is checked for the same facts in a
//! form a human reads, and both are checked for the one thing that must never appear in a log
//! line — a secret.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

/// A directory the test writes its fixtures into, removed when the test ends.
struct Dir(PathBuf);

impl Dir {
    /// A fresh directory, named after the test that asked for it.
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mailer-logging-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("{} is creatable: {error}", path.display()));
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One run of the service: what it printed, how it ended, and which process it was.
struct Run {
    stdout: String,
    stderr: String,
    code: Option<i32>,
    /// The process id the operating system gave it — the id a log line has to carry.
    pid: u32,
}

impl Run {
    /// Everything the process said, whichever stream it said it on.
    fn said(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }

    /// The lines of `json` mode, parsed: a collector has to be able to do exactly this.
    fn events(&self) -> Vec<Value> {
        self.stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line).unwrap_or_else(|error| {
                    panic!(
                        "`json` mode has to print one JSON object per line, and this line is not \
                         one ({error}): `{line}`"
                    )
                })
            })
            .collect()
    }

    /// The id every line of this run carries, taken from the first event that has one.
    fn run_id(&self) -> String {
        let events = self.events();
        let first = events
            .first()
            .unwrap_or_else(|| panic!("`json` mode printed no event at all: {}", self.said()));
        first["span"]["run.id"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "every event has to run inside the span that carries the run's id, this one \
                     has no `span.run.id`: {first}"
                )
            })
            .to_owned()
    }
}

/// Start the service in `dir` with an empty environment plus what the test passes.
fn start(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (Child, u32) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mailer"));
    command
        .args(args)
        .current_dir(dir)
        .env_clear()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    let child = command
        .spawn()
        .unwrap_or_else(|error| panic!("the mailer binary could not be started: {error}"));
    let pid = child.id();
    (child, pid)
}

/// Run the service and collect everything it said.
fn run(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Run {
    let (child, pid) = start(dir, args, env);
    let output = child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("the mailer process has to finish: {error}"));
    Run {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        code: output.status.code(),
        pid,
    }
}

/// The `json` mode of the service, with the level the caller asks for.
fn json_run(dir: &Path, level: &str, extra: &[(&str, &str)]) -> Run {
    let mut env = vec![
        ("MAILER__LOG__FORMAT", "json"),
        ("MAILER__LOG__LEVEL", level),
    ];
    env.extend_from_slice(extra);
    let run = run(dir, &[], &env);
    assert_eq!(
        run.code,
        Some(0),
        "the service has to exit successfully here: {}",
        run.said()
    );
    run
}

/// One field of an event, or a panic naming what the event actually had.
fn field<'a>(event: &'a Value, name: &str) -> &'a Value {
    event["fields"]
        .get(name)
        .unwrap_or_else(|| panic!("this event has no field `{name}`: {event}"))
}

#[test]
fn json_mode_is_one_object_per_line_with_the_fields_a_collector_indexes() {
    let dir = Dir::new("json");
    let run = json_run(dir.path(), "info", &[]);
    let events = run.events();
    let event = events
        .first()
        .unwrap_or_else(|| panic!("a service that starts has something to say: {}", run.said()));

    assert_eq!(event["level"], "INFO", "{event}");
    assert_eq!(field(event, "message"), "mailer is starting", "{event}");
    assert_eq!(
        field(event, "host"),
        "127.0.0.1",
        "{event} — the settings that decided how this process runs belong in the line that says \
         it started"
    );
    assert_eq!(field(event, "port"), 8080, "{event}");
    assert_eq!(field(event, "log_format"), "json", "{event}");
    assert_eq!(
        field(event, "database"),
        "unset",
        "{event} — whether a secret is set is worth a line, the secret is not"
    );

    assert_eq!(event["span"]["name"], "run", "{event}");
    assert_eq!(
        run.run_id(),
        run.pid.to_string(),
        "every line of a run carries the id of the run — the process the operating system \
         started: {}",
        run.said()
    );
}

#[test]
fn pretty_mode_is_for_a_human_and_carries_the_same_fields() {
    let dir = Dir::new("pretty");
    let run = run(dir.path(), &[], &[("MAILER__LOG__FORMAT", "pretty")]);
    assert_eq!(run.code, Some(0), "{}", run.said());

    let line = run
        .stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_else(|| panic!("`pretty` mode prints the same event: {}", run.said()));
    assert!(
        !line.trim_start().starts_with('{'),
        "`pretty` mode is the format a person reads, not JSON: `{line}`"
    );
    for fragment in ["mailer is starting", "host=127.0.0.1", "port=8080"] {
        assert!(
            line.contains(fragment),
            "`pretty` mode has to show the same facts as `json` mode; {fragment:?} is missing \
             from: `{line}`"
        );
    }
    assert!(
        line.contains(&format!("run.id={}", run.pid)),
        "the span and its id are there in both formats, otherwise the two modes are two \
         different services: `{line}`"
    );
}

#[test]
fn the_level_from_the_configuration_decides_what_a_run_pays_for() {
    let dir = Dir::new("levels");

    // The default (`info`) reports that the service started and nothing more.
    let quiet = json_run(dir.path(), "info", &[]);
    let quiet_levels: BTreeSet<String> = quiet
        .events()
        .iter()
        .map(|event| event["level"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(
        !quiet_levels.contains("DEBUG"),
        "`info` is the level a healthy service runs at; a per-build debug line belongs to `debug` \
         — this run logged {}",
        quiet.said()
    );

    // `debug` adds the line that says there is no listener yet, and both lines carry one id.
    let loud = json_run(dir.path(), "debug", &[]);
    let events = loud.events();
    let debug: Vec<&Value> = events
        .iter()
        .filter(|event| event["level"] == "DEBUG")
        .collect();
    assert!(
        !debug.is_empty(),
        "`MAILER__LOG__LEVEL=debug` has to unlock the debug lines this build has: {}",
        loud.said()
    );
    assert!(
        debug[0]["fields"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("no listener yet"),
        "the debug line says what this build does not have yet: {}",
        loud.said()
    );
    let ids: BTreeSet<String> = events
        .iter()
        .map(|event| {
            event["span"]["run.id"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(
        ids.len(),
        1,
        "one run is one id: the span carries it, and the events below it inherit it — this run \
         logged {} ids",
        ids.len()
    );

    // `error` keeps the terminal quiet when nothing is wrong.
    let silent = json_run(dir.path(), "error", &[]);
    assert!(
        silent.stdout.trim().is_empty(),
        "a service told to log only errors has nothing to say when it starts: {}",
        silent.said()
    );
}

#[test]
fn a_secret_never_reaches_a_log_line() {
    let dir = Dir::new("secret");
    const SECRET: &str = "sup3r-secret";
    const URL: &str = "postgres://mailer:sup3r-secret@localhost/mailer";

    let json = json_run(dir.path(), "info", &[("MAILER__DATABASE__URL", URL)]);
    assert_eq!(
        field(&json.events()[0], "database"),
        "set",
        "a line may say that a secret is configured, never what it is: {}",
        json.said()
    );
    assert!(
        !json.said().contains(SECRET) && !json.said().contains("postgres://"),
        "the database URL must not be in a log line: {}",
        json.said()
    );

    let pretty = run(
        dir.path(),
        &[],
        &[
            ("MAILER__LOG__FORMAT", "pretty"),
            ("MAILER__DATABASE__URL", URL),
        ],
    );
    assert!(
        !pretty.said().contains(SECRET) && !pretty.said().contains("postgres://"),
        "the format is not what makes a secret a secret: {}",
        pretty.said()
    );
}
