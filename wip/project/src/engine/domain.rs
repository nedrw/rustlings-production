//! The engine's vocabulary: queues, jobs, and what has happened to them.
//!
//! Everything in this module is a value: it does no I/O, reads no clock and knows no store. That
//! is what lets the scheduler, the two store implementations, the daemon and the command line
//! talk about a job without agreeing on anything else.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::engine::error::Error;

/// Longest queue name the engine accepts, in bytes.
pub const QUEUE_NAME_MAX: usize = 64;

/// How many attempts a job gets when the caller does not say.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// A queue name: the unit of ordering, of fairness and of accounting.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Queue(String);

impl Queue {
    /// Parse a queue name.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when the name is empty, longer than [`QUEUE_NAME_MAX`] bytes, or
    /// contains anything but lower-case ASCII letters, digits, `.`, `_` and `-`. A name is a key in
    /// an operator's dashboard and a column in a table, so the set is deliberately tiny.
    pub fn parse(name: &str) -> Result<Self, Error> {
        let invalid = |detail: String| Error::Invalid { detail };
        if name.is_empty() {
            return Err(invalid("a queue name cannot be empty".to_owned()));
        }
        if name.len() > QUEUE_NAME_MAX {
            return Err(invalid(format!(
                "`{name}` is {} bytes; the limit is {QUEUE_NAME_MAX}",
                name.len()
            )));
        }
        if !name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        }) {
            return Err(invalid(format!(
                "`{name}` may only contain lower-case letters, digits, `.`, `_` and `-`: a queue \
                 name is a table column, a metric label and something a person types, so it stays \
                 in the character set all three can spell"
            )));
        }
        Ok(Self(name.to_owned()))
    }

    /// The name, as it is written everywhere.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Queue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Default for Queue {
    /// The queue a job goes to when the caller does not care, spelled so that it satisfies the
    /// rules above without a fallible call inside a `Default`.
    fn default() -> Self {
        Self("default".to_owned())
    }
}

impl FromStr for Queue {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

/// A job's identity, stable across attempts, workers and processes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct JobId(Uuid);

impl JobId {
    /// A fresh identity.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// The identity a stored job already has.
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// The underlying UUID, for SQL bindings and log lines.
    #[must_use]
    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl FromStr for JobId {
    type Err = uuid::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(text)?))
    }
}

/// Who is holding a job. A name, because that is what an operator has to be able to grep for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkerId(String);

impl WorkerId {
    /// Name a worker (`host:pid`, a container name — whatever the deployment already has).
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Where a job is in its life. The row's `state` column is this enum's encoding, never the model
/// itself (step `jobq/3.14-job-state-machine` is where the legal moves get written down).
///
/// `#[non_exhaustive]`, because this list grows: a lease that ran out and a cancellation are states
/// this engine will have before the track is over, and a caller that wrote a wildcard arm on day
/// one does not have to be rebuilt the day one of them arrives (step
/// `jobq/3.2-public-api-baseline`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JobState {
    /// Waiting to be claimed.
    Queued,
    /// Claimed by a worker, which is responsible for finishing it or handing it back.
    Running,
    /// Done, and it must never run again.
    Succeeded,
    /// Out of attempts. Terminal: a human decides what happens next.
    Failed,
}

impl JobState {
    /// The name stored in the database and printed by the CLI.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Succeeded => "succeeded",
            JobState::Failed => "failed",
        }
    }

    /// `true` when no worker will ever run this job again.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, JobState::Succeeded | JobState::Failed)
    }
}

impl fmt::Display for JobState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A job as the store holds it.
///
/// Fields are private and read through accessors: a job is written by the store and read by
/// everyone else, and later steps add to it (a lease, a schedule, a deduplication key) without
/// every caller having to be rebuilt.
///
/// `#[non_exhaustive]` writes that promise into the type (step `jobq/3.2-public-api-baseline`):
/// private fields already make the constructor the door, and this says out loud that the row is
/// going to grow rather than that it happens to look finished today. It has grown once so far: the
/// two lease fields of step `jobq/3.7-leases-and-recovery` arrived without any caller of `Job`
/// having to be rebuilt, which is the promise being kept rather than described.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// Identity.
    pub(crate) id: JobId,
    /// Queue it belongs to.
    pub(crate) queue: Queue,
    /// What the handler is given.
    pub(crate) payload: serde_json::Value,
    /// Where it is.
    pub(crate) state: JobState,
    /// Higher runs first.
    pub(crate) priority: i32,
    /// Not before this instant.
    pub(crate) run_at: DateTime<Utc>,
    /// Attempts that have been started, finished or not.
    pub(crate) attempts: u32,
    /// Attempts allowed in total.
    pub(crate) max_attempts: u32,
    /// Why the last attempt failed, when one did.
    pub(crate) last_error: Option<String>,
    /// Who holds it, when somebody does.
    pub(crate) lease_owner: Option<WorkerId>,
    /// When the claim runs out, when somebody holds it.
    pub(crate) lease_expires_at: Option<DateTime<Utc>>,
}

impl Job {
    /// Identity.
    #[must_use]
    pub fn id(&self) -> JobId {
        self.id
    }

    /// Queue.
    #[must_use]
    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// What the handler will be given.
    #[must_use]
    pub fn payload(&self) -> &serde_json::Value {
        &self.payload
    }

    /// Where it is now.
    #[must_use]
    pub fn state(&self) -> JobState {
        self.state
    }

    /// Higher runs first.
    #[must_use]
    pub fn priority(&self) -> i32 {
        self.priority
    }

    /// Earliest instant this job may be claimed.
    #[must_use]
    pub fn run_at(&self) -> DateTime<Utc> {
        self.run_at
    }

    /// Attempts started so far.
    #[must_use]
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Attempts allowed in total.
    #[must_use]
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// Why the last attempt failed, when one did.
    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Who holds this job, when somebody does.
    ///
    /// `jobq/3.7-leases-and-recovery` is where this stops being `None` for a running job: a claim
    /// stamps the worker that made it, `Store::renew` moves the instant next to it, and a *settle*
    /// refuses a write from a worker the row no longer names — which is what stops a worker that
    /// was already declared dead from settling the attempt its successor is running right now.
    #[must_use]
    pub fn lease_owner(&self) -> Option<&WorkerId> {
        self.lease_owner.as_ref()
    }

    /// When the claim on this job runs out, when somebody holds it.
    ///
    /// After this instant the row is presumed abandoned and `Store::reclaim_expired` puts it back
    /// in its queue: the owner is a hint, and this is the deadline that decides.
    #[must_use]
    pub fn lease_expires_at(&self) -> Option<DateTime<Utc>> {
        self.lease_expires_at
    }

    /// `true` when another attempt is allowed after this one fails.
    #[must_use]
    pub fn may_retry(&self) -> bool {
        self.attempts < self.max_attempts
    }
}

/// What a caller says when it wants a job queued.
///
/// Built rather than written as a struct literal, so that a default (priority 0, as soon as
/// possible, [`DEFAULT_MAX_ATTEMPTS`] attempts) stays one place in the code — and
/// `#[non_exhaustive]` keeps it that way when a request grows a field (step
/// `jobq/3.2-public-api-baseline`).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct SubmitRequest {
    /// Queue to put it in.
    pub(crate) queue: Queue,
    /// What the handler will be given.
    pub(crate) payload: serde_json::Value,
    /// Higher runs first.
    pub(crate) priority: i32,
    /// Not before this instant; `None` means as soon as a worker looks.
    pub(crate) run_at: Option<DateTime<Utc>>,
    /// Attempts allowed in total.
    pub(crate) max_attempts: u32,
}

impl SubmitRequest {
    /// A job for `queue` with the given payload, at the default priority and attempt budget.
    #[must_use]
    pub fn new(queue: Queue, payload: serde_json::Value) -> Self {
        Self {
            queue,
            payload,
            priority: 0,
            run_at: None,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }

    /// Higher runs first.
    #[must_use]
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Do not claim this job before `at`.
    #[must_use]
    pub fn with_run_at(mut self, at: DateTime<Utc>) -> Self {
        self.run_at = Some(at);
        self
    }

    /// Attempts allowed in total. Zero is rejected when the request reaches a store: a job nobody
    /// may run is a bug at the call site, not a way to park work.
    #[must_use]
    pub fn with_max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = attempts;
        self
    }

    /// Queue this request is for.
    #[must_use]
    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// Payload the handler will be given.
    #[must_use]
    pub fn payload(&self) -> &serde_json::Value {
        &self.payload
    }

    /// Priority this request asked for.
    #[must_use]
    pub fn priority(&self) -> i32 {
        self.priority
    }

    /// Instant it asked for, if it asked.
    #[must_use]
    pub fn run_at(&self) -> Option<DateTime<Utc>> {
        self.run_at
    }

    /// Attempt budget it asked for.
    #[must_use]
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }
}
