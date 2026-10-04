//! The trail: what has happened to a job, in the order it happened.
//!
//! The `jobs` table says where a job *is*. It cannot say how it got there: a retried attempt, an
//! attempt nobody watched, a completion that followed two failures — each of those leaves a row
//! that looks exactly like the row a first success leaves. Step `jobq/3.6-transaction-isolation`
//! is where the trail becomes something the project keeps, and this module is the vocabulary it is
//! kept in: one [`JobEvent`] per settle, naming the job, what happened, which attempt it happened
//! to, why, and when.
//!
//! Three decisions are written into the shapes below, and the step argues them:
//!
//! * **the trail is about settles, not about state.** [`EventKind::Completed`],
//!   [`EventKind::AttemptFailed`] and [`EventKind::Reclaimed`] are the three ways an attempt ends.
//!   A claim is deliberately not one of them: an attempt that has been *started* is on the row
//!   (`attempts`), and a trail that recorded starts would double the writes a claim does for
//!   nothing an operator reads. What the trail answers is "what happened to this job"; "somebody
//!   is holding it right now" is the row's own answer.
//! * **an event is a value, and it carries the instant it was appended to.** The clock is the
//!   caller's — a store stamps what it is given — because two stores each reading their own clock
//!   is the disagreement `jobq/3.5-store-port-conformance` exists to prevent.
//! * **`attempt` is the attempt the event *describes*.** Two lines can name the same attempt (a
//!   reclaim of an attempt that later fails), so the identity of a line is
//!   `(job_id, kind, attempt)` rather than its position — which is what lets a store append one
//!   twice and get one line instead of two.

use std::fmt;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::engine::domain::JobId;

/// What happened to a job.
///
/// `#[non_exhaustive]`, for the same reason [`crate::engine::JobState`] is: this list grows — a
/// replay, a purge and a run somebody cancelled are all events an operator has to be able to read —
/// and a caller that wrote a wildcard arm on day one should not have to be rebuilt the day one of
/// them arrives.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EventKind {
    /// The attempt finished the job.
    Completed,
    /// The attempt failed and the job moved on: back to the queue, or out of it.
    AttemptFailed,
    /// The attempt was abandoned rather than finished — the worker holding it stopped, or its
    /// lease ran out — and the job went back to the queue.
    Reclaimed,
}

impl EventKind {
    /// The name this kind is stored and printed under.
    ///
    /// `kebab-case`, like the `state` column: the string is what a migration, a query and an
    /// operator's `grep` all have to agree on, so it is written once, here.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Completed => "completed",
            EventKind::AttemptFailed => "attempt-failed",
            EventKind::Reclaimed => "reclaimed",
        }
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One line of the trail: a settle, and which attempt it settled.
///
/// Fields are private and read through accessors, like [`crate::engine::Job`]: a store writes the
/// row and everybody else reads it, so a later step adding a column (the worker that settled it,
/// the queue it belonged to) is a change inside the crate rather than a rebuild of every caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobEvent {
    /// This line's own identity, distinct from the job's.
    id: Uuid,
    /// The job it is about.
    job_id: JobId,
    /// What happened.
    kind: EventKind,
    /// Which attempt it happened to: the row's own count, as the settle saw it.
    attempt: u32,
    /// Why, when there is a why: a failure's reason, or the note a reclaimer leaves.
    detail: Option<String>,
    /// When it was appended, by the caller's clock.
    at: DateTime<Utc>,
}

impl JobEvent {
    /// A fresh line.
    #[must_use]
    pub fn new(
        job_id: JobId,
        kind: EventKind,
        attempt: u32,
        detail: Option<String>,
        at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            job_id,
            kind,
            attempt,
            detail,
            at,
        }
    }

    /// This line's identity, for a store that wants to answer which row it wrote.
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// The job it is about.
    #[must_use]
    pub fn job_id(&self) -> JobId {
        self.job_id
    }

    /// What happened.
    #[must_use]
    pub fn kind(&self) -> EventKind {
        self.kind
    }

    /// Which attempt it happened to.
    #[must_use]
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Why, when there is a why.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// When the caller said it happened.
    #[must_use]
    pub fn at(&self) -> DateTime<Utc> {
        self.at
    }
}

impl fmt::Display for JobEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} of attempt {} of job {}",
            self.kind, self.attempt, self.job_id
        )
    }
}
