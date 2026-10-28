//! probe = "async/graceful-shutdown@apply"
//! title = "Close the door, then finish the room"
//! hint = "Stop accepting first, then drain; a job dropped without a count is an outage nobody can see."
//!
//! Step `mailer/1.14-worker` is the shutdown order: the listener stops accepting, the queue stops
//! taking work, and whatever was already accepted is finished before the process leaves — bounded
//! by a deadline, because "wait for the queue" without a bound is a process that never exits.
//!
//! The service here has no runtime, because a probe runs offline and in seconds. The deadline is
//! a budget of jobs rather than a clock: a probe that waits on wall time is a probe that flakes.
//! What is re-checked is the protocol, and it has three parts that fail independently:
//!
//! * **the door closes first** — after [`Service::close`], [`Service::submit`] refuses, so a caller
//!   learns it must retry rather than losing the request inside the process;
//! * **the queue is drained in order** — accepted jobs run first-in-first-out, up to the budget;
//! * **what did not fit is reported** — jobs the deadline did not leave time for are named in
//!   [`Report::dropped`], not silently discarded. A count nobody sees is the same as data loss.
//!
//! ## What you write
//!
//! * [`Service::submit`] — accept a job, unless the door is closed;
//! * [`Service::drain`] — run at most `budget` jobs, oldest first, and report what ran and what is
//!   left. A drained job is gone: a second drain does not run it again, and a job that is neither
//!   completed nor dropped does not exist.
//!
//! ## What the tests ask
//!
//! Work accepted before the close; the door refusing afterwards, including after a drain; a short
//! budget reporting the exact tail it dropped; a zero budget; the partition being exact (every
//! accepted job lands in exactly one of the two lists); and a second drain finding nothing.

use std::collections::VecDeque;

/// A submission that arrived after the door closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

/// What one drain did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Jobs that ran, in the order they ran.
    pub completed: Vec<u64>,
    /// Accepted jobs the budget did not leave time for, in the order they were queued.
    pub dropped: Vec<u64>,
}

/// A worker with a queue and a door.
#[derive(Debug, Default)]
pub struct Service {
    pending: VecDeque<u64>,
    closed: bool,
}

impl Service {
    /// A worker with nothing queued and the door open.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Close the door: nothing new is accepted, and everything already accepted still runs.
    pub fn close(&mut self) {
        self.closed = true;
    }

    /// Accept a job.
    ///
    /// # Errors
    ///
    /// [`Closed`] once the door is closed.
    pub fn submit(&mut self, id: u64) -> Result<(), Closed> {
        if self.closed {
            return Err(Closed);
        }
        self.pending.push_back(id);
        Ok(())
    }

    /// Run up to `budget` jobs in submission order and report what ran and what is left.
    #[must_use]
    pub fn drain(&mut self, budget: usize) -> Report {
        let mut completed = Vec::new();
        while completed.len() < budget {
            let Some(id) = self.pending.pop_front() else {
                break;
            };
            completed.push(id);
        }
        // Everything still queued did not fit the budget: it is abandoned, and naming it is what
        // turns a deadline from silent data loss into a number the caller can act on.
        let dropped: Vec<u64> = self.pending.drain(..).collect();
        Report { completed, dropped }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_submitted_before_the_close_still_runs() {
        let mut service = Service::new();
        assert_eq!(service.submit(1), Ok(()));
        assert_eq!(service.submit(2), Ok(()));
        service.close();

        let report = service.drain(10);
        assert_eq!(report.completed, [1, 2]);
        assert!(report.dropped.is_empty());
    }

    #[test]
    fn the_door_closes_before_the_queue_is_drained() {
        let mut service = Service::new();
        service.close();
        assert_eq!(service.submit(7), Err(Closed));

        let report = service.drain(10);
        assert!(report.completed.is_empty(), "a refused job was queued anyway");
        assert!(report.dropped.is_empty());
    }

    #[test]
    fn a_short_budget_reports_the_tail_it_dropped() {
        let mut service = Service::new();
        for id in 1..=5 {
            assert_eq!(service.submit(id), Ok(()));
        }
        service.close();

        let report = service.drain(2);
        assert_eq!(report.completed, [1, 2]);
        assert_eq!(report.dropped, [3, 4, 5]);
    }

    #[test]
    fn zero_budget_drops_everything_and_says_so() {
        let mut service = Service::new();
        for id in 1..=3 {
            assert_eq!(service.submit(id), Ok(()));
        }
        let report = service.drain(0);
        assert!(report.completed.is_empty());
        assert_eq!(report.dropped, [1, 2, 3]);
    }

    #[test]
    fn every_accepted_job_is_completed_exactly_once_or_dropped() {
        let mut service = Service::new();
        for id in 0..8 {
            assert_eq!(service.submit(id), Ok(()));
        }
        let report = service.drain(5);
        assert_eq!(report.completed, [0, 1, 2, 3, 4]);
        assert_eq!(report.dropped, [5, 6, 7]);
        assert_eq!(report.completed.len() + report.dropped.len(), 8);
    }

    #[test]
    fn a_second_drain_has_nothing_to_do() {
        let mut service = Service::new();
        assert_eq!(service.submit(1), Ok(()));
        service.close();
        let first = service.drain(10);
        assert_eq!(first.completed, [1]);

        let second = service.drain(10);
        assert!(second.completed.is_empty(), "a drained job ran twice");
        assert!(second.dropped.is_empty());
    }

    #[test]
    fn submit_stays_refused_after_a_drain() {
        let mut service = Service::new();
        assert_eq!(service.submit(1), Ok(()));
        service.close();
        let _ = service.drain(10);
        assert_eq!(service.submit(2), Err(Closed));
    }
}
