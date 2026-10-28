//! probe = "async/backpressure@apply"
//! title = "The queue has a bottom"
//! hint = "Count what is queued plus what is running: at capacity the answer is “full”, not “wait” — an unbounded queue turns overload into memory exhaustion."
//!
//! Step `relay/2.14-bounded-concurrency` bounds the work a service accepts. The failure it
//! prevents is not a slow request: an unbounded queue accepts everything, memory grows with the
//! backlog, and the process dies under the load it was supposed to absorb. A bounded queue says
//! "full" while it can still say anything at all.
//!
//! The queue here is a counter and a `VecDeque`, because a probe runs offline and in seconds. The
//! shape is the channel's: capacity counts jobs waiting **and** jobs in flight, so a worker that
//! started a job has not freed a slot.
//!
//! ## What you write
//!
//! [`Queue::submit`] — accept or refuse:
//!
//! * queued plus running below capacity → the job is queued, [`Submit::Accepted`];
//! * at capacity → [`Submit::Full`], and the job is **not** queued: a refusal that quietly keeps
//!   the job is an unbounded queue with extra steps.
//!
//! [`Queue::start`] and [`Queue::finish`] are given: they move a job from waiting to running and
//! release it, which is how a slot comes back.
//!
//! ## What the tests ask
//!
//! Filling to capacity; a started job still holding its slot; a finished job freeing one; capacity
//! zero; a refused job never running; and a hundred submissions against a capacity of three.

use std::collections::VecDeque;

/// What a submission got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submit {
    /// The job is queued.
    Accepted,
    /// The queue is at capacity; the job was not queued.
    Full,
}

/// A bounded queue.
#[derive(Debug)]
pub struct Queue {
    capacity: usize,
    waiting: VecDeque<u64>,
    running: usize,
}

impl Queue {
    /// A queue that accepts at most `capacity` jobs at once.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            waiting: VecDeque::new(),
            running: 0,
        }
    }

    /// Jobs waiting plus jobs running.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.waiting.len() + self.running
    }

    /// Move the oldest waiting job into a worker's hands.
    pub fn start(&mut self) -> Option<u64> {
        let job = self.waiting.pop_front()?;
        self.running += 1;
        Some(job)
    }

    /// Release one running job's slot.
    pub fn finish(&mut self) {
        self.running = self.running.saturating_sub(1);
    }

    /// Offer a job to the queue.
    pub fn submit(&mut self, id: u64) -> Submit {
        todo!("at capacity say Full; otherwise queue it")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queue_fills_to_capacity_and_then_refuses() {
        let mut queue = Queue::new(2);
        assert_eq!(queue.submit(1), Submit::Accepted);
        assert_eq!(queue.submit(2), Submit::Accepted);
        assert_eq!(queue.submit(3), Submit::Full);
        assert_eq!(queue.in_flight(), 2);
    }

    #[test]
    fn a_job_in_flight_still_holds_its_slot() {
        let mut queue = Queue::new(2);
        queue.submit(1);
        queue.submit(2);
        assert_eq!(queue.start(), Some(1));
        assert_eq!(queue.submit(3), Submit::Full, "starting a job freed a slot");
    }

    #[test]
    fn finishing_a_job_frees_a_slot() {
        let mut queue = Queue::new(1);
        queue.submit(1);
        assert_eq!(queue.start(), Some(1));
        assert_eq!(queue.submit(2), Submit::Full);
        queue.finish();
        assert_eq!(queue.submit(2), Submit::Accepted);
    }

    #[test]
    fn capacity_zero_refuses_everything() {
        let mut queue = Queue::new(0);
        assert_eq!(queue.submit(1), Submit::Full);
        assert_eq!(queue.start(), None);
    }

    #[test]
    fn a_refused_job_never_runs() {
        let mut queue = Queue::new(1);
        assert_eq!(queue.submit(1), Submit::Accepted);
        assert_eq!(queue.submit(2), Submit::Full);
        assert_eq!(queue.start(), Some(1));
        assert_eq!(queue.start(), None, "the refused job was queued after all");
    }

    #[test]
    fn a_hundred_submissions_against_three_slots_accept_exactly_three() {
        let mut queue = Queue::new(3);
        let accepted = (0..100)
            .filter(|id| queue.submit(*id) == Submit::Accepted)
            .count();
        assert_eq!(accepted, 3);
        assert_eq!(queue.in_flight(), 3);
    }
}
