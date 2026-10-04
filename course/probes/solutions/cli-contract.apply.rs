//! probe = "cli/contract@apply"
//! title = "An interface a script can use"
//! hint = "One exit code per failure class, `--json` on stdout and diagnostics on stderr, nothing on stdout when the command failed, and no question a non-interactive caller has to answer."
//!
//! Step `jobq/3.15-cli-contract` is where `jobqc` stops being a way for a developer to poke at the
//! engine and becomes an interface: **the customer is a script.** A script cannot read a friendly
//! sentence, cannot answer a prompt and cannot grok a table — it reads `$?`, it reads stdout, and
//! it reads stderr into a log. Everything in this file follows from that one sentence.
//!
//! * **one exit code per failure class.** `0` is done, `2` is "you called me wrong" (fix the
//!   script), `3` is "that thing is not there", `4` is "the store refused", `1` is "this
//!   deployment is broken". A script that cannot tell a typo from an outage either retries forever
//!   or pages somebody at three in the morning;
//! * **stdout is the answer, stderr is the diagnosis.** `--json` prints one JSON value on stdout and
//!   nothing else, so `jobqc list --json | jq` works; when the command *fails*, stdout is **empty**
//!   — a script that parses its input must not be handed half a document and a warning;
//! * **nothing prompts.** Every input arrives as a flag or on stdin, and a run that is missing one
//!   is a usage error that names the flag. A command that waits for a human is a command that hangs
//!   in CI, and the hang is the failure mode nobody debugs;
//! * **`--help` is output, not an error.** Asking for help is a successful run with the help on
//!   stdout; a script checking `$?` must not be told something went wrong.
//!
//! ## What you write
//!
//! * [`run`] — the whole grammar: the subcommand, the flags, the stdin, the exit code, and which
//!   stream each byte goes to. [`Outcome`] is "what the process would have written", because a
//!   probe has no process;
//! * [`json_string`] — a JSON string literal for a value that came from a caller. A payload with a
//!   quote in it must not produce output that `jq` refuses to parse; that is the whole difference
//!   between machine-readable and machine-readable-looking.
//!
//! [`Engine`] is the engine port, [`Job`] is what it answers with, [`Failure`] is what it refuses
//! with (and already carries the exit code the CLI has to use), and the exact output shapes are in
//! the table below. All of it is given: the vocabulary is the engine's, and a probe that let you
//! rename the subcommands would not be checking an interface.
//!
//! ## The output this CLI promises
//!
//! | command | stdout | code |
//! | --- | --- | --- |
//! | `jobqc` / `jobqc --help` | the usage block | 0 |
//! | `jobqc submit --queue Q --payload P` | `queued <id>` | 0 |
//! | `jobqc submit --queue Q --stdin` | `queued <id>` | 0 |
//! | `jobqc list --queue Q` | one `<id> <queue> <state> <payload>` line per job | 0 |
//! | `jobqc inspect <id>` | one `<id> <queue> <state> <payload>` line | 0 |
//! | any of the above with `--json` | `{"id":7}`, `[{…}]`, `{"id":7,"queue":"…","state":"…","payload":"…"}` | 0 |
//! | anything the engine refused | *empty* | `2`, `3`, `4` or `1` |
//! | anything the caller got wrong, a missing `--queue`, a flag with no value, `inspect abc` | *empty* | 2 |
//!
//! The field order is written down and asserted, although JSON does not define one: a golden test
//! that reorders its fields on every refactor is a test nobody reads, and a `TextDiff` in a review
//! is worth more than a set comparison that hides a rename.
//!
//! ## What the tests ask
//!
//! A submit prints one line a script can read, and `--json` prints one JSON object and nothing else.
//! Every way of calling it wrong is code 2 with an empty stdout and a diagnostic on stderr — while
//! `--help` is code 0 with the help on stdout. Each engine refusal gets its own code, and the four
//! are all different. A payload with quotes, backslashes and newlines survives as valid JSON. The
//! engine is asked for exactly what the caller typed, a flag value that looks like a flag is taken
//! as written, and an empty list is `[]` rather than nothing.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no process**: a real CLI is `std::process::exit` and two file
//! descriptors written from different threads, and here it is one function that returns what the
//! process would have written. That is the one narrowing that matters and it is a *stronger* check
//! in one way: because nothing can be written outside [`Outcome`], a test can assert that stdout is
//! empty on every failure path, which is exactly the property a subprocess test has to work harder
//! to see.
//!
//! Also missing on purpose: no `clap` (the real step learns an argument parser; a probe that
//! re-checked the library's API would be checking the library), no completions, no configuration
//! file, and no `--output` beyond `--json` — the second format is where an interface starts to
//! sprawl, and one machine format plus one human one is the decision this file is defending.

use std::fmt;

/// One job, as the CLI prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// Identity.
    pub id: u64,
    /// The queue it is in.
    pub queue: String,
    /// Where it is in its life, as the store spells it.
    pub state: String,
    /// What the handler will be given, as the caller sent it.
    pub payload: String,
}

/// What the engine can refuse, in the vocabulary the exit codes are made of.
///
/// The codes are the engine's own (`jobq/3.15` and `engine::Error::exit_code` say the same thing):
/// a bad call is 2, a missing thing is 3, a refused store is 4, and a broken environment is 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The caller's input was rejected before anything was written.
    Invalid {
        /// What was wrong.
        detail: String,
    },
    /// No job with that identity.
    NotFound {
        /// Identity that was asked for.
        id: u64,
    },
    /// The store refused the operation.
    Store {
        /// What the store said.
        detail: String,
    },
    /// The service cannot read its own configuration: nothing the caller can do.
    Config {
        /// Which setting.
        detail: String,
    },
}

impl Failure {
    /// The exit code this refusal exits with.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Failure::Invalid { .. } => 2,
            Failure::NotFound { .. } => 3,
            Failure::Store { .. } => 4,
            Failure::Config { .. } => 1,
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Invalid { detail } => write!(formatter, "invalid input: {detail}"),
            Failure::NotFound { id } => write!(formatter, "no job {id}"),
            Failure::Store { detail } => write!(formatter, "store: {detail}"),
            Failure::Config { detail } => write!(formatter, "configuration: {detail}"),
        }
    }
}

/// The engine, as the CLI uses it: three commands and no configuration.
///
/// In the service this is an HTTP client to the daemon; in a probe it is whatever the test hands
/// over. The CLI may not care which — an interface does not know how the thing behind it is
/// deployed, and a CLI that reached around the port into a database would be a second
/// implementation of the engine's rules.
pub trait Engine {
    /// Queue a job and hand back its identity.
    ///
    /// # Errors
    ///
    /// Whatever the engine refused with.
    fn submit(&mut self, queue: &str, payload: &str) -> Result<u64, Failure>;

    /// The jobs in a queue, oldest first.
    ///
    /// # Errors
    ///
    /// Whatever the engine refused with.
    fn list(&mut self, queue: &str) -> Result<Vec<Job>, Failure>;

    /// One job, if it is still there.
    ///
    /// # Errors
    ///
    /// Whatever the engine refused with.
    fn inspect(&mut self, id: u64) -> Result<Option<Job>, Failure>;
}

/// What the process would have written, and the code it would have exited with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The exit code.
    pub code: u8,
    /// Everything written to stdout, byte for byte. Empty on every failure.
    pub stdout: String,
    /// Everything written to stderr. Empty on success.
    pub stderr: String,
}

/// Run one command: the grammar, the streams and the exit code.
///
/// `argv` is what the shell handed over without the program's own name, so `argv[0]` is the
/// subcommand; `stdin` is the whole of standard input, already read, because a probe cannot block
/// on a pipe and a CLI must not block on a human.
#[must_use]
pub fn run(engine: &mut dyn Engine, argv: &[&str], stdin: &str) -> Outcome {
    // The usage block, which `--help` prints on stdout and every usage error is not a reason to
    // print anywhere: a script that got the call wrong has a log to read, not a screen.
    const USAGE: &str = "jobqc: a job queue client\n\
                         subcommands: submit, list, inspect\n\
                         submit --queue Q --payload P|--stdin [--json]\n\
                         list --queue Q [--json]\n\
                         inspect ID [--json]\n";

    if argv.is_empty() || matches!(argv[0], "--help" | "-h") {
        return ok(USAGE.to_owned());
    }
    let mut json = false;
    let mut queue: Option<String> = None;
    let mut payload: Option<String> = None;
    let mut from_stdin = false;
    let mut positional: Vec<String> = Vec::new();
    let subcommand = argv[0].to_owned();
    let mut arguments = argv[1..].iter();
    while let Some(argument) = arguments.next() {
        match *argument {
            "--json" => json = true,
            "--queue" => match arguments.next() {
                Some(value) => queue = Some((*value).to_owned()),
                None => return usage("`--queue` needs a queue name"),
            },
            "--payload" => match arguments.next() {
                // A flag's value is taken as written, however much it looks like a flag: a payload
                // of `--json` is a payload, and a parser that guessed otherwise would be rewriting
                // the caller's data.
                Some(value) => payload = Some((*value).to_owned()),
                None => return usage("`--payload` needs a value"),
            },
            "--stdin" => from_stdin = true,
            other if other.starts_with("--") => {
                return usage(&format!("unknown flag `{other}`"));
            }
            other => positional.push(other.to_owned()),
        }
    }

    match subcommand.as_str() {
        "submit" => {
            let Some(queue) = queue else {
                return usage("`submit` needs `--queue Q`");
            };
            let payload = match (payload, from_stdin) {
                (Some(_), true) => {
                    return usage("`--payload` and `--stdin` are one source or the other");
                }
                (Some(value), false) => value,
                (None, true) if stdin.is_empty() => {
                    return usage(
                        "`--stdin` was empty: a job with no payload is a mistake, not a line to wait behind",
                    );
                }
                // Verbatim, newline included: the caller chose what to pipe, and a CLI that
                // trimmed it would be editing a payload on the way in.
                (None, true) => stdin.to_owned(),
                (None, false) => {
                    return usage(
                        "`submit` needs `--payload P` or `--stdin`; nothing reads from a terminal",
                    );
                }
            };
            match engine.submit(&queue, &payload) {
                Ok(id) if json => ok(format!("{{\"id\":{id}}}\n")),
                Ok(id) => ok(format!("queued {id}\n")),
                Err(failure) => refused(&failure),
            }
        }
        "list" => {
            let Some(queue) = queue else {
                return usage("`list` needs `--queue Q`");
            };
            match engine.list(&queue) {
                Ok(jobs) if json => {
                    let items: Vec<String> = jobs.iter().map(job_json).collect();
                    ok(format!("[{}]\n", items.join(",")))
                }
                Ok(jobs) => {
                    let mut out = String::new();
                    for job in &jobs {
                        out.push_str(&job_line(job));
                    }
                    ok(out)
                }
                Err(failure) => refused(&failure),
            }
        }
        "inspect" => {
            let Some(text) = positional.first() else {
                return usage("`inspect` needs a job id");
            };
            let Ok(id) = text.parse::<u64>() else {
                return usage(&format!("`{text}` is not a job id"));
            };
            match engine.inspect(id) {
                Ok(Some(job)) if json => ok(format!("{}\n", job_json(&job))),
                Ok(Some(job)) => ok(job_line(&job)),
                Ok(None) => refused(&Failure::NotFound { id }),
                Err(failure) => refused(&failure),
            }
        }
        other => usage(&format!("unknown subcommand `{other}`")),
    }
}

/// A JSON string literal for `text`, escapes included.
///
/// The two characters that need escaping for the text to stay valid JSON are the quote and the
/// backslash, and the control characters need the escape form because a literal newline inside a
/// JSON string is not JSON at all. Everything else is passed through: this is not an encoder, it is
/// the one transformation between a payload and a document a parser will accept.
#[must_use]
pub fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control < ' ' => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// A successful run: everything on stdout, nothing on stderr.
fn ok(stdout: String) -> Outcome {
    Outcome {
        code: 0,
        stdout,
        stderr: String::new(),
    }
}

/// A run the caller got wrong: code 2, nothing on stdout, one diagnostic line.
fn usage(problem: &str) -> Outcome {
    Outcome {
        code: 2,
        stdout: String::new(),
        stderr: format!("jobqc: {problem}\n"),
    }
}

/// A run the engine refused: the engine's own code, nothing on stdout, its own words on stderr.
fn refused(failure: &Failure) -> Outcome {
    Outcome {
        code: failure.exit_code(),
        stdout: String::new(),
        stderr: format!("jobqc: {failure}\n"),
    }
}

/// One job as a line of the human output.
fn job_line(job: &Job) -> String {
    format!("{} {} {} {}\n", job.id, job.queue, job.state, job.payload)
}

/// One job as a JSON object, with the field order this CLI promises.
fn job_json(job: &Job) -> String {
    format!(
        "{{\"id\":{},\"queue\":{},\"state\":{},\"payload\":{}}}",
        job.id,
        json_string(&job.queue),
        json_string(&job.state),
        json_string(&job.payload)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine the tests hand over: what it was asked, what it answers, and what it refuses.
    #[derive(Default)]
    struct Fake {
        jobs: Vec<Job>,
        next_id: u64,
        failure: Option<Failure>,
        seen: Vec<String>,
    }

    impl Fake {
        fn job(id: u64, queue: &str, state: &str, payload: &str) -> Job {
            Job {
                id,
                queue: queue.to_owned(),
                state: state.to_owned(),
                payload: payload.to_owned(),
            }
        }

        fn refusing(failure: Failure) -> Self {
            Self {
                failure: Some(failure),
                ..Self::default()
            }
        }

        fn answer<T>(&self, answer: T) -> Result<T, Failure> {
            match &self.failure {
                Some(failure) => Err(failure.clone()),
                None => Ok(answer),
            }
        }
    }

    impl Engine for Fake {
        fn submit(&mut self, queue: &str, payload: &str) -> Result<u64, Failure> {
            self.seen.push(format!("submit {queue} {payload}"));
            self.next_id += 1;
            let id = self.next_id;
            self.answer(id)
        }

        fn list(&mut self, queue: &str) -> Result<Vec<Job>, Failure> {
            self.seen.push(format!("list {queue}"));
            let jobs: Vec<Job> = self
                .jobs
                .iter()
                .filter(|job| job.queue == queue)
                .cloned()
                .collect();
            self.answer(jobs)
        }

        fn inspect(&mut self, id: u64) -> Result<Option<Job>, Failure> {
            self.seen.push(format!("inspect {id}"));
            let job = self.jobs.iter().find(|job| job.id == id).cloned();
            self.answer(job)
        }
    }

    #[test]
    fn a_submit_prints_one_line_and_the_json_form_is_the_same_fact() {
        let mut engine = Fake::default();
        let human = run(
            &mut engine,
            &["submit", "--queue", "mail", "--payload", "hello"],
            "",
        );
        assert_eq!(
            human,
            Outcome {
                code: 0,
                stdout: "queued 1\n".to_owned(),
                stderr: String::new()
            },
            "a script reads one line, and stderr is empty when nothing went wrong"
        );

        let json = run(
            &mut engine,
            &["submit", "--queue", "mail", "--payload", "hello", "--json"],
            "",
        );
        assert_eq!(
            json.stdout, "{\"id\":2}\n",
            "and the JSON form says the same thing"
        );
        assert_eq!(json.code, 0);
        assert!(
            json.stderr.is_empty(),
            "progress and success do not belong on stderr: a script logs stderr"
        );
        assert_eq!(
            engine.seen,
            vec![
                "submit mail hello".to_owned(),
                "submit mail hello".to_owned()
            ],
            "the engine is asked for what the caller typed, once per run"
        );
    }

    #[test]
    fn help_is_output_and_a_wrong_call_is_not() {
        let mut engine = Fake::default();
        for argv in [vec![], vec!["--help"], vec!["-h"]] {
            let outcome = run(&mut engine, &argv, "");
            assert_eq!(outcome.code, 0, "{argv:?} is a successful run");
            assert!(
                outcome
                    .stdout
                    .contains("subcommands: submit, list, inspect"),
                "the help is the answer, so it goes to stdout: {outcome:?}"
            );
            assert!(
                outcome.stderr.is_empty(),
                "asking for help is not a diagnostic: {argv:?}"
            );
        }
        assert!(engine.seen.is_empty(), "help does not talk to the engine");
    }

    #[test]
    fn every_way_of_calling_it_wrong_is_code_2_with_an_empty_stdout() {
        let mut engine = Fake::default();
        let wrong: Vec<(Vec<&str>, &str)> = vec![
            (vec!["frobnicate"], "unknown subcommand"),
            (
                vec!["submit", "--queue", "mail"],
                "needs `--payload P` or `--stdin`",
            ),
            (vec!["submit", "--payload", "x"], "needs `--queue Q`"),
            (vec!["submit", "--queue"], "needs a queue name"),
            (
                vec!["submit", "--queue", "mail", "--payload"],
                "needs a value",
            ),
            (vec!["submit", "--queue", "mail", "--yaml"], "unknown flag"),
            (
                vec!["submit", "--queue", "mail", "--payload", "x", "--stdin"],
                "one source or the other",
            ),
            (vec!["list"], "needs `--queue Q`"),
            (vec!["inspect"], "needs a job id"),
            (vec!["inspect", "abc"], "is not a job id"),
        ];
        for (argv, expected) in wrong {
            let outcome = run(&mut engine, &argv, "");
            assert_eq!(outcome.code, 2, "{argv:?} is the caller's mistake");
            assert!(
                outcome.stdout.is_empty(),
                "{argv:?} wrote to stdout: a script piping into `jq` must get nothing, not half a \
                 document: {outcome:?}"
            );
            assert!(
                outcome.stderr.contains(expected),
                "{argv:?} should have said something containing `{expected}`: {outcome:?}"
            );
        }

        // An empty `--stdin` is the same class of mistake as forgetting the flag: it is not a job
        // with an empty payload, and it is certainly not a prompt.
        let empty = run(&mut engine, &["submit", "--queue", "mail", "--stdin"], "");
        assert_eq!(empty.code, 2, "{empty:?}");
        assert!(empty.stderr.contains("--stdin` was empty"));
        assert!(
            engine.seen.is_empty(),
            "nothing the caller got wrong may reach the engine: {:?}",
            engine.seen
        );
    }

    #[test]
    fn each_refusal_from_the_engine_keeps_its_own_exit_code() {
        let cases = [
            (
                Failure::Invalid {
                    detail: "queue name is empty".to_owned(),
                },
                2,
                "invalid input: queue name is empty",
            ),
            (Failure::NotFound { id: 9 }, 3, "no job 9"),
            (
                Failure::Store {
                    detail: "connection reset".to_owned(),
                },
                4,
                "store: connection reset",
            ),
            (
                Failure::Config {
                    detail: "no database.url".to_owned(),
                },
                1,
                "configuration: no database.url",
            ),
        ];
        let mut codes = Vec::new();
        for (failure, code, message) in cases {
            let mut engine = Fake::refusing(failure);
            let outcome = run(&mut engine, &["list", "--queue", "mail"], "");
            assert_eq!(outcome.code, code, "the engine's code is the CLI's code");
            assert!(outcome.stdout.is_empty(), "{outcome:?}");
            assert!(
                outcome.stderr.contains(message),
                "the engine's own words, prefixed so a log reader knows who is talking: {outcome:?}"
            );
            codes.push(outcome.code);
        }
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(
            codes.len(),
            4,
            "a script branches on `$?`: four classes, four codes, and none of them shared"
        );

        // An id that is not there is a refusal, not an empty success.
        let mut engine = Fake::default();
        let missing = run(&mut engine, &["inspect", "7", "--json"], "");
        assert_eq!(missing.code, 3);
        assert!(missing.stdout.is_empty());
        assert!(missing.stderr.contains("no job 7"));

        // ...and the store's refusal of the same command is a different code, which is the whole
        // point of the table.
        let mut broken = Fake::refusing(Failure::Store {
            detail: "down".to_owned(),
        });
        assert_eq!(run(&mut broken, &["inspect", "7"], "").code, 4);
    }

    #[test]
    fn a_list_is_a_document_even_when_it_is_empty() {
        let mut empty = Fake::default();
        assert_eq!(
            run(&mut empty, &["list", "--queue", "mail"], "").stdout,
            "",
            "the human form has nothing to say about nothing"
        );
        assert_eq!(
            run(&mut empty, &["list", "--queue", "mail", "--json"], "").stdout,
            "[]\n",
            "the machine form has to be parseable: an empty answer is a document with nothing in \
             it, not the absence of one"
        );

        let mut engine = Fake::default();
        engine.jobs = vec![
            Fake::job(1, "mail", "queued", "first"),
            Fake::job(2, "mail", "running", "second"),
            Fake::job(3, "other", "queued", "not this queue"),
        ];
        assert_eq!(
            run(&mut engine, &["list", "--queue", "mail"], "").stdout,
            "1 mail queued first\n2 mail running second\n",
            "one line per job, in the order the engine answered with"
        );
        assert_eq!(
            run(&mut engine, &["list", "--queue", "mail", "--json"], "").stdout,
            "[{\"id\":1,\"queue\":\"mail\",\"state\":\"queued\",\"payload\":\"first\"},\
             {\"id\":2,\"queue\":\"mail\",\"state\":\"running\",\"payload\":\"second\"}]\n",
            "the same fact, in the field order this CLI promises"
        );
    }

    #[test]
    fn a_payload_with_quotes_and_newlines_is_still_a_document() {
        let mut engine = Fake::default();
        let hostile = "say \"hi\"\\now\nand\tthen\u{7}";
        engine.jobs = vec![Fake::job(4, "mail", "queued", hostile)];

        let outcome = run(&mut engine, &["inspect", "4", "--json"], "");
        let expected = format!(
            "{{\"id\":4,\"queue\":\"mail\",\"state\":\"queued\",\"payload\":{}}}\n",
            json_string(hostile)
        );
        assert_eq!(
            outcome.stdout, expected,
            "a payload that came from a caller cannot be trusted to be JSON, and the escape is \
             what makes the output parseable"
        );
        assert!(
            !outcome.stdout.contains('\n') || outcome.stdout.ends_with("\n"),
            "one trailing newline and no other, or a parser that reads line by line sees two"
        );
        assert!(
            outcome.stdout.matches('\n').count() == 1,
            "a literal newline inside the payload would have been a second line: {}",
            outcome.stdout
        );

        assert_eq!(json_string("plain"), "\"plain\"");
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_string("a\nb"), "\"a\\nb\"");
        assert_eq!(json_string("\u{1}"), "\"\\u0001\"");
    }

    #[test]
    fn stdin_is_a_payload_and_a_flag_value_that_looks_like_a_flag_is_data() {
        let mut engine = Fake::default();
        let piped = run(
            &mut engine,
            &["submit", "--queue", "mail", "--stdin", "--json"],
            "{\"to\":\"ada\"}\n",
        );
        assert_eq!(piped.stdout, "{\"id\":1}\n");
        assert_eq!(
            engine.seen,
            vec!["submit mail {\"to\":\"ada\"}\n".to_owned()],
            "verbatim: the caller decided what to pipe, and a newline they did not want is a \
             payload they can change"
        );

        // `--payload --json` is a payload of `--json`: a flag's value is data, not grammar.
        let looks_like_a_flag = run(
            &mut engine,
            &["submit", "--queue", "mail", "--payload", "--json"],
            "",
        );
        assert_eq!(
            looks_like_a_flag.stdout, "queued 2\n",
            "the value of a flag is taken as written"
        );
        assert_eq!(engine.seen[1], "submit mail --json");

        // And the flags may arrive in any order, with the same meaning.
        let reordered = run(
            &mut engine,
            &["submit", "--json", "--payload", "x", "--queue", "mail"],
            "",
        );
        assert_eq!(reordered.stdout, "{\"id\":3}\n");
    }
}
