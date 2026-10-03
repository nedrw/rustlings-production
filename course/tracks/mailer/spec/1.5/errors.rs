//! Spec for `mailer/1.5-errors`: failures are part of the API — a message a person can act on, a
//! cause a caller can walk, and an exit code a script can branch on.
//!
//! `kiln run` copies this file into the project as `tests/spec_errors.rs`; it is the course's file,
//! not the learner's (plan §3 D2). Three of the four tests run the built binary, because that is
//! what a user sees. The fourth writes a throwaway crate that depends on this one and asks the
//! compiler whether a *caller* can match on the failure type — the one question no amount of
//! printing can answer (authoring.md §6.6).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

/// A directory the tests write their fixtures into, removed when the test ends.
struct Dir(PathBuf);

impl Dir {
    /// A fresh directory, named after the test that asked for it.
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mailer-errors-{label}-{}-{unique}",
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

    /// Write a file below the directory, creating parents, and answer where it landed.
    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("{} is creatable: {error}", parent.display()));
        }
        std::fs::write(&path, contents)
            .unwrap_or_else(|error| panic!("{} is writable: {error}", path.display()));
        path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The learner's project, which the API probe depends on.
fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Run the service in `dir`, with nothing in the environment but what the test passes.
fn run(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mailer"));
    command.args(args).current_dir(dir).env_clear();
    for (name, value) in env {
        command.env(name, value);
    }
    command
        .output()
        .unwrap_or_else(|error| panic!("the mailer binary could not be started: {error}"))
}

/// Everything the process said, whichever stream it said it on.
fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The exit code of a run that failed, or a panic explaining why it did not fail properly.
fn refused(output: &Output, label: &str) -> i32 {
    assert!(
        !output.status.success(),
        "{label}: a failure has to exit non-zero, this run exited 0 — a service that reports \
         success for a request it did not satisfy is worse than one that does not start"
    );
    let text = said(output);
    assert!(
        !text.contains("panicked"),
        "{label}: report the failure, do not panic: {text}"
    );
    output.status.code().unwrap_or_else(|| {
        panic!("{label}: the process has to exit with a code, not be killed by a signal")
    })
}

/// One way of failing: what to run, what to write, and what the service has to answer.
struct Failure {
    /// How the failure is described when the test complains.
    label: &'static str,
    /// Arguments the process gets.
    args: &'static [&'static str],
    /// Environment the process gets, on top of an otherwise empty one.
    env: &'static [(&'static str, &'static str)],
    /// A file to write first, as `(name, contents)`.
    file: Option<(&'static str, &'static str)>,
    /// Exit code the process has to end with.
    code: i32,
    /// Fragments the report has to contain, so a human knows what to fix.
    says: &'static [&'static str],
}

/// Every failure this step promises a code and a message for.
///
/// `2` is the kind the caller fixes: they asked for something this service does not have. `1` is
/// the kind the deployment fixes: the configuration could not be read or parsed. A script that sees
/// `2` knows that retrying will not help.
const FAILURES: &[Failure] = &[
    Failure {
        label: "an argument this build does not have",
        args: &["--nope"],
        env: &[],
        file: None,
        code: 2,
        says: &["--nope"],
    },
    Failure {
        label: "a flag given without its value",
        args: &["--config"],
        env: &[],
        file: None,
        code: 2,
        says: &["--config"],
    },
    Failure {
        label: "a flag value the setting cannot use",
        args: &["--listen-port", "70000"],
        env: &[],
        file: None,
        code: 2,
        says: &["listen.port", "70000"],
    },
    Failure {
        label: "an environment variable this service does not know",
        args: &["--check-config"],
        env: &[("MAILER__LISTEN__PROT", "9111")],
        file: None,
        code: 2,
        says: &["MAILER__LISTEN__PROT"],
    },
    Failure {
        label: "a setting value the service cannot use",
        args: &["--check-config"],
        env: &[("MAILER__LOG__FORMAT", "xml")],
        file: None,
        code: 2,
        says: &["log.format", "xml"],
    },
    Failure {
        label: "a configuration file that is not there",
        args: &["--check-config", "--config", "absent.toml"],
        env: &[],
        file: None,
        code: 1,
        says: &["absent.toml"],
    },
    Failure {
        label: "a configuration file the parser refuses",
        args: &["--check-config", "--config", "unparseable.toml"],
        env: &[],
        file: Some(("unparseable.toml", "[listen]\nport = \"nope\"\n")),
        code: 1,
        says: &["unparseable.toml"],
    },
];

#[test]
fn each_kind_of_failure_has_its_own_exit_code_and_a_message_that_names_it() {
    let dir = Dir::new("codes");
    let mut codes = Vec::new();

    for failure in FAILURES {
        if let Some((name, contents)) = failure.file {
            dir.write(name, contents);
        }
        let output = run(dir.path(), failure.args, failure.env);
        let code = refused(&output, failure.label);
        assert_eq!(
            code,
            failure.code,
            "{}: expected exit code {}, got {code} — {}: {}",
            failure.label,
            failure.code,
            "the code is how a script tells the two kinds apart",
            said(&output)
        );
        codes.push(code);
        let text = said(&output);
        for fragment in failure.says {
            assert!(
                text.contains(fragment),
                "{}: the report has to mention {fragment:?}, it said: {text}",
                failure.label
            );
        }
    }

    codes.sort_unstable();
    codes.dedup();
    assert_eq!(
        codes,
        [1, 2],
        "the two kinds of failure must not share a code: a caller has to be able to tell \"you \
         asked for something I cannot do\" from \"I could not read what you pointed me at\""
    );
}

#[test]
fn the_cause_of_a_failure_reaches_the_user() {
    let dir = Dir::new("cause");

    // The service's own sentence, then the cause it carried: a read failure without "No such file
    // or directory" leaves the reader to guess which half is wrong.
    let missing = run(
        dir.path(),
        &["--check-config", "--config", "absent.toml"],
        &[],
    );
    refused(&missing, "a file that is not there");
    let text = said(&missing).to_lowercase();
    assert!(
        text.contains("absent.toml") && text.contains("no such file"),
        "the report has to keep the cause of the failure, not only the sentence the service \
         wrote: {}",
        said(&missing)
    );

    // A parser failure carries where it stopped, which is what makes it fixable.
    dir.write("unparseable.toml", "[listen]\nport = \"nope\"\n");
    let unparseable = run(
        dir.path(),
        &["--check-config", "--config", "unparseable.toml"],
        &[],
    );
    refused(&unparseable, "a file the parser refuses");
    let text = said(&unparseable);
    assert!(
        text.contains("line 2"),
        "the parser's position has to survive to the report: {text}"
    );
}

/// A crate that uses the failure type the way a caller does: from outside, by matching on it.
///
/// Writing `[workspace]` keeps it out of this project's workspace, and `CARGO_TARGET_DIR` points
/// it at the project's own target directory so the dependencies it needs are the ones this run
/// already built.
const PROBE: &str = r#"
//! Written by tests/spec_errors.rs: the failure type has to be usable from another crate.
use std::error::Error as _;

use mailer::{Error, SettingSource};

/// Which kind of failure this is — the thing a caller branches on.
fn kind(error: &Error) -> &'static str {
    match error {
        Error::UnknownArgument { .. } | Error::MissingValue { .. } => "usage",
        Error::InvalidSetting { .. } | Error::UnknownSetting { .. } => "configuration",
        Error::UnreadableConfig { .. } | Error::InvalidConfig { .. } => "environment",
        // A variant a later step adds is not this step's business: the probe asserts the API this
        // step promised, and the course runs this spec against trees that already have more
        // variants than step 5 introduced.
        _ => "other",
    }
}

fn main() {
    let failures = [
        Error::UnknownArgument {
            argument: "--nope".to_owned(),
        },
        Error::MissingValue {
            flag: "--config",
            expected: "the path of a file",
        },
        Error::InvalidSetting {
            key: "listen.port".to_owned(),
            value: "70000".to_owned(),
            expected: "a port number between 0 and 65535",
            origin: SettingSource::Flag("--listen-port"),
        },
        Error::UnknownSetting {
            name: "MAILER__LISTEN__PROT".to_owned(),
        },
        Error::UnreadableConfig {
            path: "mailer.toml".into(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "gone"),
        },
    ];

    for failure in &failures {
        assert!(
            !failure.to_string().is_empty(),
            "every variant has a message for a human"
        );
        println!(
            "{}: {failure} (cause: {})",
            kind(failure),
            failure.source().is_some()
        );
    }
    println!("origin: {}", SettingSource::File("mailer.toml".into()));
}
"#;

#[test]
fn a_caller_can_match_on_the_failure_type() {
    let dir = Dir::new("api");
    dir.write(
        "Cargo.toml",
        &format!(
            "[package]\nname = \"mailer-error-probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nmailer = {{ path = \"{}\" }}\n\n[workspace]\n",
            project().display()
        ),
    );
    dir.write("src/main.rs", PROBE);

    // `cargo run` rather than `cargo check`: matching on the variants is half of it, and the other
    // half is that the message and the cause are reachable without the course's help.
    let output = Command::new("cargo")
        .args(["run", "--quiet", "--offline"])
        .current_dir(dir.path())
        .env(
            "CARGO_TARGET_DIR",
            project().join("target/kiln-error-probe"),
        )
        .output()
        .unwrap_or_else(|error| panic!("cargo has to be on PATH: {error}"));

    let printed = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        output.status.success(),
        "another crate has to be able to match on the failure type and walk its cause — the \
         compiler or the probe said: {}{}",
        said(&output),
        printed
    );
    assert!(
        printed.contains("environment: cannot read `mailer.toml` (cause: true)"),
        "a failure that carries a cause has to expose it through `source()`: {printed}"
    );
    assert!(
        printed.contains("usage: unknown argument `--nope`"),
        "the variants have to keep their messages and be matchable from outside: {printed}"
    );
}
