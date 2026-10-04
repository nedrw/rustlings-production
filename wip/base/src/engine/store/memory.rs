//! The in-memory store: the engine's second implementation, and the one every fast test uses.
//!
//! It is not a toy. It holds the same invariants as the Postgres backend (one claim per job, the
//! attempt budget, the ordering), which is exactly why the conformance suite can run against both
//! (step `jobq/3.5-store-port-conformance`) and why a bug that only shows up in the other backend
//! is a bug in the engine, not in the test harness.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::engine::domain::{Job, JobId, JobState, Queue, SubmitRequest, WorkerId};
use crate::engine::error::Error;
use crate::engine::sealed::Sealed;
use crate::engine::store::{BoxFuture, Store};

/// Jobs in a map, protected by a mutex. One process, no durability, honest about both.
///
/// It is a backend this crate certifies, so it carries the seal that lets the port accept it (step
/// `jobq/3.2-public-api-baseline`): the conformance suite of step `jobq/3.5-store-port-conformance`
/// is written against it and against the durable one, which is the whole reason an outside
/// implementation is not welcome.
#[derive(Debug, Default)]
pub struct MemoryStore {
    /// Every job this store knows, keyed by identity so two writes to one job cannot both win.
    jobs: Mutex<BTreeMap<JobId, Job>>,
}

impl MemoryStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The map, taken without a panic path: a poisoned mutex means some other thread died holding
    /// it, and refusing to read the jobs after that would turn one thread's panic into the whole
    /// process's.
    fn jobs(&self) -> MutexGuard<'_, BTreeMap<JobId, Job>> {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
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
    fn due(
        jobs: &BTreeMap<JobId, Job>,
        queue: &Queue,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Vec<JobId> {
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
                run_at: request.run_at.unwrap_or_else(chrono::Utc::now),
                attempts: 0,
                max_attempts: request.max_attempts,
                last_error: None,
            };
            self.jobs().insert(job.id, job.clone());
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
            // The memory store has no leases yet (step `jobq/3.7-leases-and-recovery` adds them),
            // so who holds a job is not recorded here. The names are still the interface: the two
            // implementations must answer the same questions.
            let _ = worker;
            let now = chrono::Utc::now();
            // One lock, one decision: choosing the due jobs and stamping them happen in the same
            // critical section, so a second claim on this store cannot read the same row as free
            // while this one is on its way to marking it running.
            let mut jobs = self.jobs();
            let due = Self::due(&jobs, queue, now);
            let mut claimed = Vec::new();
            for id in due.into_iter().take(limit) {
                if let Some(job) = jobs.get_mut(&id) {
                    job.state = JobState::Running;
                    // An attempt starts when a worker takes the job, not when it finishes: a
                    // worker that dies holding a job must still have spent one.
                    job.attempts += 1;
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
            let _ = worker;
            let mut jobs = self.jobs();
            let Some(job) = jobs.get_mut(id) else {
                return Err(Error::NotFound { id: *id });
            };
            if job.state != JobState::Running {
                return Err(Error::Invalid {
                    detail: format!(
                        "job {id} is {}, not running: only the worker that claimed it can complete \
                         it",
                        job.state
                    ),
                });
            }
            job.state = JobState::Succeeded;
            job.last_error = None;
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
            let _ = worker;
            let mut jobs = self.jobs();
            let Some(job) = jobs.get_mut(id) else {
                return Err(Error::NotFound { id: *id });
            };
            if job.state != JobState::Running {
                return Err(Error::Invalid {
                    detail: format!(
                        "job {id} is {}, not running: a failure is only recorded against an \
                         attempt somebody actually started",
                        job.state
                    ),
                });
            }
            job.last_error = Some(reason.to_owned());
            job.state = if job.attempts < job.max_attempts {
                JobState::Queued
            } else {
                JobState::Failed
            };
            Ok(job.state)
        })
    }

    fn get<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<Option<Job>, Error>> {
        Box::pin(async move { Ok(self.jobs().get(id).cloned()) })
    }
}
