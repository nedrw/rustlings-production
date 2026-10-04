//! The in-memory store: the engine's second implementation, and the one every fast test uses.
//!
//! It is not a toy. It holds the same invariants as the Postgres backend (one claim per job, the
//! attempt budget, the ordering), which is exactly why the conformance suite can run against both
//! (step `jobq/3.5-store-port-conformance`) and why a bug that only shows up in the other backend
//! is a bug in the engine, not in the test harness.
//!
//! `jobq/3.6-transaction-isolation` adds the second half of that shape: the trail. A durable backend
//! gets "the state change and the line that describes it commit together" from a transaction, and
//! this one gets it from the lock it already holds — the row and the line are written while the same
//! guard is alive, so a settle that is refused writes neither, and a settle that answers writes
//! both. The *idempotence* of an append is the same kind of translation: a unique index under a
//! table, and here the key of a map.
//!
//! `jobq/3.7-leases-and-recovery` adds the third: the lease. The in-memory store stamps one, renews
//! one, releases one and reclaims an expired one, and every one of those is the same decision the
//! durable backend makes in SQL — including the guard that matters most, which is that a settle only
//! counts from the worker the row still names.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::engine::domain::{Job, JobId, JobState, Queue, SubmitRequest, WorkerId};
use crate::engine::error::Error;
use crate::engine::events::{EventKind, JobEvent};
use crate::engine::sealed::Sealed;
use crate::engine::store::{ABANDONED, BoxFuture, DEFAULT_LEASE, HANDED_BACK, Store};

/// What this store knows, under one lock.
///
/// One struct rather than two maps with two mutexes, because the promise of
/// `jobq/3.6-transaction-isolation` is about a *pair* of writes: a settle that changed the row and
/// then lost the lock on its way to the trail would leave exactly the disagreement the trail exists
/// to prevent, and a store that cannot get that wrong by construction does not need a comment
/// explaining the order two locks have to be taken in.
#[derive(Debug, Default)]
struct Ledger {
    /// Every job this store knows, keyed by identity so two writes to one job cannot both win.
    jobs: BTreeMap<JobId, Job>,
    /// Every job's trail, appended in the order things happened.
    events: BTreeMap<JobId, Vec<JobEvent>>,
}

impl Ledger {
    /// Append one line, answering whether it was new.
    ///
    /// The identity of a line is `(kind, attempt)` within its job — the same identity the durable
    /// backend's unique index holds — so a second append of the same settle is a no-op here too.
    /// A scan of a job's lines rather than a keyed map: a trail is a handful of lines, and the
    /// vector is what makes "oldest first" the order they were written in.
    fn record(&mut self, event: &JobEvent) -> bool {
        let trail = self.events.entry(event.job_id()).or_default();
        if trail
            .iter()
            .any(|line| line.kind() == event.kind() && line.attempt() == event.attempt())
        {
            return false;
        }
        trail.push(event.clone());
        true
    }

    /// One job's trail, oldest first, ties broken the way the port documents.
    fn events_of(&self, id: &JobId) -> Vec<JobEvent> {
        let mut trail = self.events.get(id).cloned().unwrap_or_default();
        trail.sort_by(|left, right| {
            left.at()
                .cmp(&right.at())
                .then_with(|| left.kind().cmp(&right.kind()))
                .then_with(|| left.attempt().cmp(&right.attempt()))
        });
        trail
    }

    /// Whether this row is this worker's to settle.
    ///
    /// The check the durable backend writes as `and lease_owner = $2` in its `where` clause, in the
    /// shape a map can ask it: the row has to be running *and* still name this worker, because a
    /// worker whose lease was reclaimed and handed to somebody else is a worker whose attempt is
    /// over — whatever it thinks.
    fn holds(job: &Job, worker: &WorkerId) -> bool {
        job.state == JobState::Running && job.lease_owner.as_ref() == Some(worker)
    }
}

/// Jobs in a map, protected by a mutex. One process, no durability, honest about both.
///
/// It is a backend this crate certifies, so it carries the seal that lets the port accept it (step
/// `jobq/3.2-public-api-baseline`): the conformance suite of step `jobq/3.5-store-port-conformance`
/// is written against it and against the durable one, which is the whole reason an outside
/// implementation is not welcome.
#[derive(Debug)]
pub struct MemoryStore {
    /// The rows and the trail behind one lock.
    ledger: Mutex<Ledger>,
    /// How long a claim by this store is good for.
    lease: Duration,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self {
            ledger: Mutex::default(),
            lease: DEFAULT_LEASE,
        }
    }
}

impl MemoryStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The same store, leasing claims for `lease`.
    ///
    /// The lease is a *setting* rather than a constant because the right value is the deployment's:
    /// it has to be longer than the slowest attempt the operator is willing to wait for and shorter
    /// than the recovery they are willing to accept, and a test that has to watch a lease run out
    /// needs it in millimetres (`jobq/3.7-leases-and-recovery`).
    #[must_use]
    pub fn with_lease(mut self, lease: Duration) -> Self {
        self.lease = lease;
        self
    }

    /// The store's contents, taken without a panic path: a poisoned mutex means some other thread
    /// died holding it, and refusing to read the jobs after that would turn one thread's panic into
    /// the whole process's.
    fn ledger(&self) -> MutexGuard<'_, Ledger> {
        self.ledger.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Jobs in one queue that are due, in claim order.
    ///
    /// Claim order is the port's promise, which is why it lives here and not in a query: higher
    /// priority first, then the job that has been due longest, then identity — never the order a
    /// map happens to hand out. Step `jobq/3.9-priority-fairness` is where this gets a policy.
    ///
    /// The map is passed in rather than locked here, and that is what makes the answer safe to act
    /// on: `claim` selects and stamps under *one* lock, so the rows this function names cannot be
    /// claimed by somebody else in between (step `jobq/3.5-store-port-conformance` is the step that
    /// found the other spelling of this — select, unlock, stamp — handing one job to two workers).
    fn due(jobs: &BTreeMap<JobId, Job>, queue: &Queue, now: DateTime<Utc>) -> Vec<JobId> {
        let mut due: Vec<&Job> = jobs
            .values()
            .filter(|job| job.queue == *queue && job.state == JobState::Queued && job.run_at <= now)
            .collect();
        due.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.run_at.cmp(&right.run_at))
                .then_with(|| left.id.cmp(&right.id))
        });
        due.into_iter().map(|job| job.id).collect()
    }
}

// The seal the port requires (step `jobq/3.2-public-api-baseline`): this backend is the one the
// crate certifies with its own suite, and no outside implementation can say the same about itself.
impl Sealed for MemoryStore {}

impl Store for MemoryStore {
    fn enqueue<'a>(&'a self, request: SubmitRequest) -> BoxFuture<'a, Result<Job, Error>> {
        Box::pin(async move {
            if request.max_attempts == 0 {
                return Err(Error::Invalid {
                    detail: "a job with a budget of zero attempts can never run: either give it \
                             one attempt or do not queue it"
                        .to_owned(),
                });
            }
            let job = Job {
                id: JobId::new(),
                queue: request.queue,
                payload: request.payload,
                state: JobState::Queued,
                priority: request.priority,
                // `None` means "as soon as somebody looks", and the store is where that becomes an
                // instant: the caller's clock is not the store's, and only one of them is the
                // authority on when a job is due (step `jobq/3.8-clock-injection` moves this read
                // behind an injected clock).
                run_at: request.run_at.unwrap_or_else(Utc::now),
                attempts: 0,
                max_attempts: request.max_attempts,
                last_error: None,
                // Nobody holds a job that has just been written: the lease arrives with the claim.
                lease_owner: None,
                lease_expires_at: None,
            };
            self.ledger().jobs.insert(job.id, job.clone());
            Ok(job)
        })
    }

    fn claim<'a>(
        &'a self,
        worker: &'a WorkerId,
        queue: &'a Queue,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<Job>, Error>> {
        Box::pin(async move {
            let now = Utc::now();
            // One lock, one decision: choosing the due jobs, stamping them running and leasing them
            // happen in the same critical section, so a second claim on this store cannot read the
            // same row as free while this one is on its way to marking it running.
            let mut ledger = self.ledger();
            let due = Self::due(&ledger.jobs, queue, now);
            let mut claimed = Vec::new();
            for id in due.into_iter().take(limit) {
                if let Some(job) = ledger.jobs.get_mut(&id) {
                    job.state = JobState::Running;
                    // An attempt starts when a worker takes the job, not when it finishes: a
                    // worker that dies holding a job must still have spent one.
                    job.attempts += 1;
                    // And so does the lease: from here until somebody renews it, this row is this
                    // worker's, and the row is the only place that says so.
                    job.lease_owner = Some(worker.clone());
                    job.lease_expires_at = Some(now + self.lease);
                    claimed.push(job.clone());
                }
            }
            Ok(claimed)
        })
    }

    fn complete<'a>(
        &'a self,
        worker: &'a WorkerId,
        id: &'a JobId,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let mut ledger = self.ledger();
            let Some(job) = ledger.jobs.get_mut(id) else {
                return Err(Error::NotFound { id: *id });
            };
            if !Ledger::holds(job, worker) {
                return Err(Error::Invalid {
                    detail: refused(job, worker, "only the worker that claimed it can complete it"),
                });
            }
            let attempt = job.attempts;
            job.state = JobState::Succeeded;
            job.last_error = None;
            job.lease_owner = None;
            job.lease_expires_at = None;
            // The line goes in while the same guard is held: this store has no transaction to commit
            // or roll back, so the lock *is* the transaction, and a refusal above returns before
            // either write happens.
            ledger.record(&JobEvent::new(
                *id,
                EventKind::Completed,
                attempt,
                None,
                Utc::now(),
            ));
            Ok(())
        })
    }

    fn fail<'a>(
        &'a self,
        worker: &'a WorkerId,
        id: &'a JobId,
        reason: &'a str,
    ) -> BoxFuture<'a, Result<JobState, Error>> {
        Box::pin(async move {
            let mut ledger = self.ledger();
            let Some(job) = ledger.jobs.get_mut(id) else {
                return Err(Error::NotFound { id: *id });
            };
            if !Ledger::holds(job, worker) {
                return Err(Error::Invalid {
                    detail: refused(
                        job,
                        worker,
                        "a failure is only recorded against an attempt somebody actually started",
                    ),
                });
            }
            let attempt = job.attempts;
            job.last_error = Some(reason.to_owned());
            job.state = if job.attempts < job.max_attempts {
                JobState::Queued
            } else {
                JobState::Failed
            };
            let state = job.state;
            job.lease_owner = None;
            job.lease_expires_at = None;
            // The attempt the line names is the one the claim started, read off the row the same
            // guard is holding: a trail built from a second read could name an attempt that had
            // already moved on (step `jobq/3.6-transaction-isolation`).
            ledger.record(&JobEvent::new(
                *id,
                EventKind::AttemptFailed,
                attempt,
                Some(reason.to_owned()),
                Utc::now(),
            ));
            Ok(state)
        })
    }

    fn get<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<Option<Job>, Error>> {
        Box::pin(async move { Ok(self.ledger().jobs.get(id).cloned()) })
    }

    fn record<'a>(&'a self, event: &'a JobEvent) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(self.ledger().record(event)) })
    }

    fn events<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<Vec<JobEvent>, Error>> {
        Box::pin(async move { Ok(self.ledger().events_of(id)) })
    }

    fn migrate<'a>(&'a self) -> BoxFuture<'a, Result<(), Error>> {
        // A store with no schema has nothing to prepare, and saying so is the whole answer: a
        // caller that has to know which backend it is holding does not have a port.
        Box::pin(async move { Ok(()) })
    }

    fn renew<'a>(&'a self, worker: &'a WorkerId) -> BoxFuture<'a, Result<u32, Error>> {
        Box::pin(async move {
            let until = Utc::now() + self.lease;
            let mut ledger = self.ledger();
            let mut renewed = 0;
            for job in ledger.jobs.values_mut() {
                // The filter is the safety property: a worker renews what it holds, not what
                // it remembers holding.
                if job.state == JobState::Running && job.lease_owner.as_ref() == Some(worker) {
                    job.lease_expires_at = Some(until);
                    renewed += 1;
                }
            }
            Ok(renewed)
        })
    }

    fn release<'a>(&'a self, worker: &'a WorkerId) -> BoxFuture<'a, Result<u32, Error>> {
        Box::pin(async move {
            let now = Utc::now();
            let mut ledger = self.ledger();
            let mut handed_back = Vec::new();
            for (id, job) in ledger.jobs.iter_mut() {
                if job.state == JobState::Running && job.lease_owner.as_ref() == Some(worker) {
                    handed_back.push((*id, put_back(job, HANDED_BACK)));
                }
            }
            for (id, attempt) in &handed_back {
                ledger.record(&JobEvent::new(
                    *id,
                    EventKind::Reclaimed,
                    *attempt,
                    Some(HANDED_BACK.to_owned()),
                    now,
                ));
            }
            Ok(u32::try_from(handed_back.len()).unwrap_or(u32::MAX))
        })
    }

    fn reclaim_expired<'a>(&'a self) -> BoxFuture<'a, Result<u32, Error>> {
        Box::pin(async move {
            let now = Utc::now();
            let mut ledger = self.ledger();
            let mut expired = Vec::new();
            for (id, job) in ledger.jobs.iter_mut() {
                // The instant decides, not the owner: the worker that took this row may still be
                // alive and merely stuck, and it is the lease — the promise it failed to keep —
                // that says the row belongs to somebody else now.
                if job.state == JobState::Running && job.lease_expires_at.is_some_and(|at| at <= now) {
                    expired.push((*id, put_back(job, ABANDONED)));
                }
            }
            for (id, attempt) in &expired {
                ledger.record(&JobEvent::new(
                    *id,
                    EventKind::Reclaimed,
                    *attempt,
                    Some(ABANDONED.to_owned()),
                    now,
                ));
            }
            Ok(u32::try_from(expired.len()).unwrap_or(u32::MAX))
        })
    }
}

/// Put one row back in its queue, and answer which attempt the line it gets will name.
///
/// The two rules the whole recovery story rests on are in this function, and the durable backend
/// writes them as one `case` expression:
///
/// * the attempt that was abandoned stays counted — it was started, whether or not anybody watched
///   it end, and a count that forgave it would let one payload that kills its worker be retried for
///   ever;
/// * the job goes back to `queued` while the budget has a try left and to `failed` when it does
///   not, which is what makes a poison payload die: nobody has to *fail* it for the queue to stop
///   handing it out.
fn put_back(job: &mut Job, reason: &str) -> u32 {
    job.state = if job.attempts < job.max_attempts {
        JobState::Queued
    } else {
        JobState::Failed
    };
    job.last_error = Some(reason.to_owned());
    job.lease_owner = None;
    job.lease_expires_at = None;
    job.attempts
}

/// Why one worker may not settle this row, in the two cases that look alike and are not.
fn refused(job: &Job, worker: &WorkerId, why: &str) -> String {
    match &job.lease_owner {
        Some(owner) if owner != worker => format!(
            "{} is held by {owner}, not by {worker}: {why}, and a claim that was reclaimed belongs \
             to whoever holds it now",
            job.id
        ),
        _ => format!("{} is {}, not running: {why}", job.id, job.state),
    }
}
