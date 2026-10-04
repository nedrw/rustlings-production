//! Spec for `jobq/3.3-library-errors`: the engine's failure type judged as a library's API.
//!
//! `kiln run` copies this file into the project as `tests/spec_library_errors.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Four claims, one per test below:
//!
//! * **the message is a promise** — every variant's `Display` text is pinned, including the text of
//!   a failure that wraps somebody else's;
//! * **a cause is walkable** — `source()` hands back the backend's own error (the very value, not a
//!   copy of its sentence) and the operating system's, and a chain that is two links deep walks both;
//! * **retryability is a class, not a guess** — one failure another attempt could change, four that
//!   another attempt cannot;
//! * **the exit codes the two binaries already use do not move** — a shell reads `$?` and a deploy
//!   script branches on it.
//!
//! This file is written against the interface the step is *about* (`is_retryable`, and the store's
//! cause as a `#[source]`), so on the tree from the previous step it does not compile. That is how a
//! step without a scaffold fails before its reference solution (docs/authoring.md §5.4): the
//! interface is missing, and the compile error names it.

use std::error::Error as StdError;
use std::fmt;
use std::io;

use jobq::engine::{Error, JobId};

/// `Error` is a well-behaved library failure: a caller may box it into its own chain (an `anyhow`,
/// a `Box<dyn Error>`), keep it in a struct and send it to another thread. `thiserror` gives this
/// for free, and a hand-written implementation that lost `Send + Sync` would take it away — which is
/// why the claim is written down rather than assumed.
const _: fn(Error) -> Box<dyn StdError + Send + Sync> = |error| Box::new(error);

/// A backend failure of a type a caller can hold on to.
///
/// The point of a spec-local type is the `downcast_ref` below: a chain that carries *this* value
/// proves the cause was kept, while a sentence copied into a `String` field would pass every test
/// that only reads `to_string()`.
#[derive(Debug)]
struct Refused;

impl fmt::Display for Refused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("connection refused")
    }
}

impl StdError for Refused {}

/// A pool failure that has a cause of its own, so a chain can be two links deep.
#[derive(Debug)]
struct NoPool;

impl fmt::Display for NoPool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the pool is closed")
    }
}

impl StdError for NoPool {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&Refused)
    }
}

/// A store failure of the shape a backend hands the engine.
fn refused() -> Error {
    Error::Store {
        source: Box::new(Refused),
    }
}

/// An identity, parsed through the engine's own surface, so the message the spec pins is the one a
/// caller builds.
fn a_job_id() -> JobId {
    "67e55044-10b1-426f-9247-bb680e5fe0c8"
        .parse()
        .expect("that is a uuid the engine accepts")
}

/// Every variant, with the text it is allowed to print.
///
/// The message is what an operator greps for in a log line and what a caller may show a user, so it
/// is part of the API: a rewrite that is not deliberate fails here.
#[test]
fn the_display_text_of_every_variant_is_pinned() {
    let pinned = [
        (
            "Invalid",
            "invalid input: worker.batch must be at least 1",
            Error::Invalid {
                detail: "worker.batch must be at least 1".to_owned(),
            },
        ),
        (
            "NotFound",
            "no job 67e55044-10b1-426f-9247-bb680e5fe0c8",
            Error::NotFound { id: a_job_id() },
        ),
        ("Store", "store: connection refused", refused()),
        (
            "Config",
            "configuration: worker.batch must be at least 1",
            Error::Config {
                detail: "worker.batch must be at least 1".to_owned(),
            },
        ),
        (
            "Io",
            "io: no such file",
            Error::from(io::Error::new(io::ErrorKind::NotFound, "no such file")),
        ),
    ];
    for (variant, expected, error) in pinned {
        assert_eq!(
            error.to_string(),
            expected,
            "the `Display` text of `Error::{variant}` is part of the API — it is what ends up in a \
             log line and in a ticket. A new wording is allowed; a new wording nobody noticed is \
             not, so this assertion is the notice"
        );
    }
}

/// The cause a caller walks, rather than the sentence the engine wrote about it.
#[test]
fn a_wrapped_failure_is_the_one_the_caller_gets_back() {
    // The store's failure: the backend's own error, whole.
    let error = refused();
    let source = StdError::source(&error).unwrap_or_else(|| {
        panic!(
            "the store's failure must be reachable through `source()`: a caller that can only read \
             `store: connection refused` cannot tell a connection reset from a constraint \
             violation, and the enum cannot tell it either because the value was formatted away"
        )
    });
    assert_eq!(
        source.to_string(),
        "connection refused",
        "the source is what the backend said, not a paraphrase of it"
    );
    assert!(
        source.downcast_ref::<Refused>().is_some(),
        "the chain must carry the backend's own error, so that a caller which knows the concrete \
         type can get it back with `downcast_ref`: a `detail: String` field formats the failure \
         into text, which passes every assertion above this one and fails this one"
    );

    // The operating system's failure: `?` on an `io::Result` stays the shortest path to it.
    let error = Error::from(io::Error::new(io::ErrorKind::NotFound, "no such file"));
    let source =
        StdError::source(&error).expect("an io failure must be reachable through `source()`");
    assert_eq!(
        source.downcast_ref::<io::Error>().map(io::Error::kind),
        Some(io::ErrorKind::NotFound),
        "the whole `std::io::Error` is kept, kind included: the kind is what a caller branches on, \
         and a caller that has to match on the text of a message is one refactor away from a bug"
    );

    // A chain that is deeper than one link walks all of it: `Display` shows the first link, which is
    // why `Display` alone is not the answer to "what happened".
    let error = Error::Store {
        source: Box::new(NoPool),
    };
    assert_eq!(error.to_string(), "store: the pool is closed");
    let mut chain = Vec::new();
    let mut link: Option<&(dyn StdError + 'static)> = StdError::source(&error);
    while let Some(error) = link {
        chain.push(error.to_string());
        link = error.source();
    }
    assert_eq!(
        chain,
        vec![
            "the pool is closed".to_owned(),
            "connection refused".to_owned()
        ],
        "`source()` starts at the store's own error and walks to the one that broke, in that order"
    );
}

/// Retry or do not: the engine's classification, which the caller's policy is built on.
#[test]
fn only_what_another_attempt_could_change_is_retryable() {
    let pinned = [
        (
            "Invalid",
            false,
            "the same call gets the same answer, so an attempt is the same work for the same \
             rejection",
            Error::Invalid {
                detail: "the payload is not an object".to_owned(),
            },
        ),
        (
            "NotFound",
            false,
            "looking again does not create the row",
            Error::NotFound { id: a_job_id() },
        ),
        (
            "Store",
            true,
            "a backend that could not answer is the one failure another attempt can change",
            refused(),
        ),
        (
            "Config",
            false,
            "a file that says something this build cannot use does not say something else a moment \
             later",
            Error::Config {
                detail: "worker.batch must be at least 1".to_owned(),
            },
        ),
        (
            "Io",
            false,
            "a full disk does not become empty by asking again",
            Error::from(io::Error::new(io::ErrorKind::StorageFull, "no space left")),
        ),
    ];
    for (variant, expected, why, error) in pinned {
        assert_eq!(
            error.is_retryable(),
            expected,
            "`Error::{variant}` is retryable = {expected}: {why}. A queue that retries what it \
             cannot fix turns one broken job into a loop, and one that gives up on what it can fix \
             drops work a second attempt would have finished"
        );
    }
}

/// The codes the daemon and the CLI already leave behind them.
#[test]
fn the_exit_codes_the_two_binaries_already_use_do_not_move() {
    let pinned = [
        (
            "Invalid",
            2,
            "the caller's fault: a script that sees 2 must not retry the command",
            Error::Invalid {
                detail: "unknown argument `--nope`".to_owned(),
            },
        ),
        (
            "NotFound",
            3,
            "the thing asked about is not there",
            Error::NotFound { id: a_job_id() },
        ),
        (
            "Store",
            4,
            "the queue is unreachable: it is the deployment that has a problem, not the call",
            refused(),
        ),
        (
            "Config",
            1,
            "a broken environment",
            Error::Config {
                detail: "log level `nope`: not a level".to_owned(),
            },
        ),
        (
            "Io",
            1,
            "the same class as a broken configuration: the machine, not the caller",
            Error::from(io::Error::new(io::ErrorKind::PermissionDenied, "denied")),
        ),
    ];
    for (variant, expected, why, error) in pinned {
        assert_eq!(
            error.exit_code(),
            expected,
            "`Error::{variant}` exits {expected}: {why}. `jobqd` and `jobqc` already exit with \
             these codes (step `jobq/3.15-cli-contract` writes the table down), so this is the one \
             part of this step that must not change"
        );
    }
}
