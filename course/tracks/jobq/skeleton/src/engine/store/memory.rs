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
use crate::engine::store::{BoxFuture, Store};

/// Jobs in a map, protected by a mutex. One process, no durability, honest about both.
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
    /// Claim order is the engine's promise, which is why it lives here and not in a query: higher
    /// priority first, then the job that has been due longest, then identity — never the order a
    /// map happens to hand out. Step `jobq/3.9-priority-fairness` is where this gets a policy.
    fn due(&self, queue: &Queue, now: chrono::DateTime<chrono::Utc>) -> Vec<JobId> {
        let jobs = self.jobs();
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

impl Store for MemoryStore {
    fn enqueue<'a>(&'a self, request: SubmitRequest) -> BoxFuture<'a, Result<Job, Error>> {
        Box::pin(async move {
            let _ = request;
            todo!("jobq/3.1-engine-boundary: validate the request and write it into the map")
        })
    }

    fn claim<'a>(
        &'a self,
        worker: &'a WorkerId,
        queue: &'a Queue,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<Job>, Error>> {
        Box::pin(async move {
            let _ = (worker, queue, limit);
            todo!("jobq/3.1-engine-boundary: take the first `limit` due jobs and mark them running")
        })
    }

    fn complete<'a>(
        &'a self,
        worker: &'a WorkerId,
        id: &'a JobId,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let _ = (worker, id);
            todo!("jobq/3.1-engine-boundary: mark the job succeeded")
        })
    }

    fn fail<'a>(
        &'a self,
        worker: &'a WorkerId,
        id: &'a JobId,
        reason: &'a str,
    ) -> BoxFuture<'a, Result<JobState, Error>> {
        Box::pin(async move {
            let _ = (worker, id, reason);
            todo!("jobq/3.1-engine-boundary: count the attempt and decide queued-or-failed")
        })
    }

    fn get<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<Option<Job>, Error>> {
        Box::pin(async move {
            let _ = id;
            todo!("jobq/3.1-engine-boundary: read one job")
        })
    }
}
