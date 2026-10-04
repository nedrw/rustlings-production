//! The public surface of `jobq`, as of step `jobq/3.2-public-api-baseline`, written so the
//! compiler can disagree with it.
//!
//! This file is the course's, not the learner's: `kiln check` copies it into the project as
//! `tests/semver_baseline.rs` and runs `cargo test --test semver_baseline`. Nothing here is ever
//! called — compiling *is* the assertion (authoring.md §6.6: the compiler is the only tool that can
//! be asked about a name's shape). What it pins:
//!
//! * **every public function**, as a function pointer coercion: a parameter type, a return type or
//!   a `self` shape that changes stops this file from compiling, which is what "a break is a compile
//!   error" means for a library;
//! * **every public type**, by naming it in a position that requires it to exist with the kind of
//!   generic parameters it has (`BoxFuture<'a, T>` and `Engine<S>` included);
//! * **every public enum variant**, with an exhaustive `match` (a new variant is a compile error
//!   here — and a caller that wrote `_ =>` is not rebuilt);
//! * **`..` in every struct pattern**, so a field added to `Job` or `SubmitRequest` does not
//!   invalidate code that never named it.
//!
//! Two decisions worth knowing before editing:
//!
//! 1. **The wildcard arms are load-bearing.** [`#![deny(unreachable_patterns)]`] turns a wildcard
//!    that cannot be reached into an error, so the arms below compile *only* while `JobState` and
//!    `Error` are `#[non_exhaustive]`. Delete those attributes and this file stops compiling,
//!    which is exactly how the step's structural promise is checked.
//! 2. **What this file cannot see**: whether a module is `pub(crate)`. Rust has no "this path must
//!    not resolve" construct outside a compile-fail test, and this course has no tooling for one
//!    (docs/authoring.md §6.6), so privacy and the seal on the port are judged by
//!    `spec/3.2/public_api.rs` reading the sources, and the reason is written down there.
//!
//! Breaking this file deliberately — removing a public item, changing a signature, adding an enum
//! variant — is allowed, but it is then a breaking change: it costs a major version bump in
//! `Cargo.toml` and a line in `CHANGELOG.md` (the discipline step `jobq/3.25-release-artifacts`
//! writes down).

#![deny(unreachable_patterns)]
// Nothing in this file is called, on purpose: a witness is read by the compiler, not by a test run.
#![allow(dead_code)]

use std::fmt::{self, Display, Formatter};

use chrono::{DateTime, Utc};
use jobq::engine::{
    BoxFuture, DEFAULT_MAX_ATTEMPTS, Engine, Error, Job, JobId, JobState, MemoryStore,
    QUEUE_NAME_MAX, Queue, Store, SubmitRequest, WorkerId,
};
use serde_json::Value;
use uuid::Uuid;

// ---------------------------------------------------------------------------------------------
// The vocabulary: `Queue`, `JobId`, `WorkerId`, `JobState`, `Job`, `SubmitRequest`.
// ---------------------------------------------------------------------------------------------

/// A queue name: fallible to build, cheap to read, printable, the default, and parseable.
const _: fn(&str) -> Result<Queue, Error> = Queue::parse;
const _: fn(&Queue) -> &str = Queue::as_str;
const _: fn(&Queue, &mut Formatter<'_>) -> fmt::Result = <Queue as Display>::fmt;
const _: fn() -> Queue = <Queue as Default>::default;
const _: fn(&str) -> Result<Queue, Error> = <Queue as std::str::FromStr>::from_str;

/// A job's identity: fresh, or read back out of a row; `Uuid` in both directions.
const _: fn() -> JobId = JobId::new;
const _: fn(Uuid) -> JobId = JobId::from_uuid;
const _: fn(JobId) -> Uuid = JobId::as_uuid;
const _: fn() -> JobId = <JobId as Default>::default;
const _: fn(&JobId, &mut Formatter<'_>) -> fmt::Result = <JobId as Display>::fmt;
const _: fn(&str) -> Result<JobId, uuid::Error> = <JobId as std::str::FromStr>::from_str;

/// The name a worker puts on the rows it holds. `WorkerId::new` takes `impl Into<String>`, which
/// cannot be turbofished (`rustc` E0107) — a one-argument closure is how a pointer to it is written
/// at every lifetime.
const _: for<'a> fn(&'a str) -> WorkerId = |name| WorkerId::new(name);
const _: fn(String) -> WorkerId = WorkerId::new;
const _: fn(&WorkerId) -> &str = WorkerId::as_str;
const _: fn(&WorkerId, &mut Formatter<'_>) -> fmt::Result = <WorkerId as Display>::fmt;

/// Where a job is: the name that goes in a column, the terminal question, the printed form.
const _: fn(JobState) -> &'static str = JobState::as_str;
const _: fn(JobState) -> bool = JobState::is_terminal;
const _: fn(&JobState, &mut Formatter<'_>) -> fmt::Result = <JobState as Display>::fmt;

/// The row a caller reads: every accessor, in the shape the store hands it over in.
const _: fn(&Job) -> JobId = Job::id;
const _: fn(&Job) -> &Queue = Job::queue;
const _: fn(&Job) -> &Value = Job::payload;
const _: fn(&Job) -> JobState = Job::state;
const _: fn(&Job) -> i32 = Job::priority;
const _: fn(&Job) -> DateTime<Utc> = Job::run_at;
const _: fn(&Job) -> u32 = Job::attempts;
const _: fn(&Job) -> u32 = Job::max_attempts;
const _: fn(&Job) -> Option<&str> = Job::last_error;
const _: fn(&Job) -> bool = Job::may_retry;

/// The request a producer builds: one constructor, three setters, five readers.
const _: fn(Queue, Value) -> SubmitRequest = SubmitRequest::new;
const _: fn(SubmitRequest, i32) -> SubmitRequest = SubmitRequest::with_priority;
const _: fn(SubmitRequest, DateTime<Utc>) -> SubmitRequest = SubmitRequest::with_run_at;
const _: fn(SubmitRequest, u32) -> SubmitRequest = SubmitRequest::with_max_attempts;
const _: fn(&SubmitRequest) -> &Queue = SubmitRequest::queue;
const _: fn(&SubmitRequest) -> &Value = SubmitRequest::payload;
const _: fn(&SubmitRequest) -> i32 = SubmitRequest::priority;
const _: fn(&SubmitRequest) -> Option<DateTime<Utc>> = SubmitRequest::run_at;
const _: fn(&SubmitRequest) -> u32 = SubmitRequest::max_attempts;

/// The two numbers a caller needs in order to stay inside the limits the engine enforces.
const _: u32 = DEFAULT_MAX_ATTEMPTS;
const _: usize = QUEUE_NAME_MAX;

// ---------------------------------------------------------------------------------------------
// The error type and the two doors into the engine.
// ---------------------------------------------------------------------------------------------

/// A failure is classified for the shell that has to exit with a code.
const _: fn(&Error) -> u8 = Error::exit_code;

/// The port's five methods, in the shape the engine holds a backend behind a reference.
///
/// Each shape is named once because that is the only way `clippy` lets a type this long be written
/// (`type_complexity`): the pin below then reads as the method it pins.
type Enqueue = for<'a> fn(&'a MemoryStore, SubmitRequest) -> BoxFuture<'a, Result<Job, Error>>;
type Claim = for<'a> fn(
    &'a MemoryStore,
    &'a WorkerId,
    &'a Queue,
    usize,
) -> BoxFuture<'a, Result<Vec<Job>, Error>>;
type Complete =
    for<'a> fn(&'a MemoryStore, &'a WorkerId, &'a JobId) -> BoxFuture<'a, Result<(), Error>>;
type Fail = for<'a> fn(
    &'a MemoryStore,
    &'a WorkerId,
    &'a JobId,
    &'a str,
) -> BoxFuture<'a, Result<JobState, Error>>;
type Get = for<'a> fn(&'a MemoryStore, &'a JobId) -> BoxFuture<'a, Result<Option<Job>, Error>>;

const _: Enqueue = <MemoryStore as Store>::enqueue;
const _: Claim = <MemoryStore as Store>::claim;
const _: Complete = <MemoryStore as Store>::complete;
const _: Fail = <MemoryStore as Store>::fail;
const _: Get = <MemoryStore as Store>::get;

/// The in-memory backend, as a value and as a `Default`.
const _: fn() -> MemoryStore = MemoryStore::new;
const _: fn() -> MemoryStore = <MemoryStore as Default>::default;

/// The engine over that backend: the door a caller actually holds.
const _: fn(MemoryStore) -> Engine<MemoryStore> = Engine::new;
const _: fn(&Engine<MemoryStore>) -> &MemoryStore = Engine::store;

/// `Engine`'s five methods. An `async fn` has no nameable return type, so each one is pinned by
/// boxing the future it hands back — which is also the shape the port already uses, and the reason
/// the engine can hold a store behind a reference at all.
type Submit =
    for<'a> fn(&'a Engine<MemoryStore>, SubmitRequest) -> BoxFuture<'a, Result<JobId, Error>>;
type EngineClaim = for<'a> fn(
    &'a Engine<MemoryStore>,
    &'a WorkerId,
    &'a Queue,
    usize,
) -> BoxFuture<'a, Result<Vec<Job>, Error>>;
type EngineComplete = for<'a> fn(
    &'a Engine<MemoryStore>,
    &'a WorkerId,
    &'a JobId,
) -> BoxFuture<'a, Result<(), Error>>;
type EngineFail = for<'a> fn(
    &'a Engine<MemoryStore>,
    &'a WorkerId,
    &'a JobId,
    &'a str,
) -> BoxFuture<'a, Result<JobState, Error>>;
type Inspect = for<'a> fn(&'a Engine<MemoryStore>, &'a JobId) -> BoxFuture<'a, Result<Job, Error>>;

const _: Submit = |engine: &Engine<MemoryStore>, request| Box::pin(engine.submit(request));
const _: EngineClaim = |engine: &Engine<MemoryStore>, worker, queue, limit| {
    Box::pin(engine.claim(worker, queue, limit))
};
const _: EngineComplete =
    |engine: &Engine<MemoryStore>, worker, id| Box::pin(engine.complete(worker, id));
const _: EngineFail =
    |engine: &Engine<MemoryStore>, worker, id, reason| Box::pin(engine.fail(worker, id, reason));
const _: Inspect = |engine: &Engine<MemoryStore>, id| Box::pin(engine.inspect(id));

/// `BoxFuture` is the alias every one of those signatures is written in, and this is what it means:
/// `Pin<Box<dyn Future<Output = T> + Send + 'a>>`. Spelling it out is what stops the crate from
/// quietly growing a second alias with a different shape.
type Expanded<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

const _: fn(BoxFuture<'static, u8>) -> Expanded<'static, u8> = |future| future;

// ---------------------------------------------------------------------------------------------
// Every variant of every public enum, named.
// ---------------------------------------------------------------------------------------------

/// Every state, named. A state added later is a compile error *here*, and the wildcard below is the
/// arm a caller of this crate is expected to write.
///
/// The body of an arm is deliberately the variant's own name: this file enumerates the surface, it
/// does not claim behaviour. What a state *means* is asserted by the spec of the step that teaches
/// it, where a failure can say something useful.
#[must_use]
fn every_job_state(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "Queued",
        JobState::Running => "Running",
        JobState::Succeeded => "Succeeded",
        JobState::Failed => "Failed",
        // Unreachable while that list is the whole list, so `deny(unreachable_patterns)` above
        // fails this build the moment `JobState` stops being `#[non_exhaustive]`.
        _ => "a state this witness has not been taught",
    }
}

/// Every failure class, named. The `..` in each pattern is what lets a variant grow a field without
/// invalidating this witness — step `jobq/3.3-library-errors` gives two of them a `#[source]`, and
/// `jobq/3.5-store-port-conformance` gives the engine a second backend to fail in.
#[must_use]
fn every_error_class(error: &Error) -> &'static str {
    match error {
        Error::Invalid { .. } => "Invalid",
        Error::NotFound { .. } => "NotFound",
        Error::Store { .. } => "Store",
        Error::Config { .. } => "Config",
        Error::Io { .. } => "Io",
        _ => "a failure this witness has not been taught",
    }
}

// ---------------------------------------------------------------------------------------------
// The structs, read through `..`.
// ---------------------------------------------------------------------------------------------

/// A row and a request are read field by field, never taken apart: `..` is what lets the store add
/// a lease or a schedule to the row without every reader having to be rebuilt.
#[must_use]
fn a_row_is_read_not_taken_apart(job: &Job, request: &SubmitRequest) -> JobId {
    let Job { .. } = job;
    let SubmitRequest { .. } = request;
    job.id()
}
