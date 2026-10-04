//! The front door: what callers hold instead of a store.
//!
//! The engine is deliberately thin here and grows by policy: today a submit is a store write and a
//! claim is a store read, and later steps put the decisions that are the engine's own (priority
//! and fairness, delayed runs, deduplication, authorisation, the metrics of a turn) behind the same
//! five methods. A caller that only ever sees [`Engine`] does not have to be rebuilt when one of
//! those decisions changes — which is the point of an engine at all.

use crate::engine::domain::{Job, JobId, JobState, Queue, SubmitRequest, WorkerId};
use crate::engine::error::Error;
use crate::engine::store::Store;

/// A queue engine over one store.
pub struct Engine<S: Store> {
    /// Where the jobs are.
    store: S,
}

impl<S: Store> Engine<S> {
    /// An engine over `store`.
    #[must_use]
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// The store underneath, for the operations that are genuinely the backend's (a migration, a
    /// health probe, a fixture).
    #[must_use]
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Queue a job and answer with its identity.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] when the request could never run, [`Error::Store`] when the backend
    /// refused the write.
    pub async fn submit(&self, request: SubmitRequest) -> Result<JobId, Error> {
        let job = self.store.enqueue(request).await?;
        Ok(job.id())
    }

    /// Take up to `limit` due jobs for `worker`.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when the backend refused the claim.
    pub async fn claim(
        &self,
        worker: &WorkerId,
        queue: &Queue,
        limit: usize,
    ) -> Result<Vec<Job>, Error> {
        self.store.claim(worker, queue, limit).await
    }

    /// Mark a job this worker holds as done.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when there is no such job, [`Error::Store`] when the backend refused the
    /// write.
    pub async fn complete(&self, worker: &WorkerId, id: &JobId) -> Result<(), Error> {
        self.store.complete(worker, id).await
    }

    /// Record a failed attempt, and answer where the job went: back to `queued`, or to `failed`.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when there is no such job, [`Error::Store`] when the backend refused the
    /// write.
    pub async fn fail(
        &self,
        worker: &WorkerId,
        id: &JobId,
        reason: &str,
    ) -> Result<JobState, Error> {
        // The store decides where a failed job goes, because only it knows the attempt count and
        // the budget it was given; the engine hands the decision back unchanged.
        self.store.fail(worker, id, reason).await
    }

    /// Read one job, or say there is none.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when there is no such job, [`Error::Store`] when the backend refused the
    /// read.
    pub async fn inspect(&self, id: &JobId) -> Result<Job, Error> {
        self.store.get(id).await?.ok_or(Error::NotFound { id: *id })
    }
}
