//! probe = "sched/fairness@design"
//! title = "Priority that cannot be captured"
//! hint = "Priority decides until somebody has waited past the starvation window — then the longest wait wins, and a queue at its in-flight cap is skipped whatever its priority."
//!
//! Step `jobq/3.9-priority-fairness` is where the engine stops being fair by accident. A queue
//! engine that serves the highest priority first has two ways to stop working, and both of them
//! look fine in a dashboard:
//!
//! 1. **starvation.** One tenant submitting at a high priority forever means the low-priority
//!    queue is served "when there is time", which is never. Throughput is excellent, latency for
//!    one customer is infinite, and nothing in the metrics says so — the backlog of the quiet
//!    queue is the only number that grows;
//! 2. **capture.** Priority alone also lets one tenant take the *whole* worker pool: a hundred
//!    workers all busy with the loud tenant's work while the quiet tenant's job waits for a slot
//!    that is never free. The fix is not a bigger pool, it is a per-queue ceiling on what one
//!    tenant may hold at once.
//!
//! ## The policy this file is about
//!
//! [`Policy`] is two numbers an ADR can defend, and [`Scheduler::next`] is the rule they make:
//!
//! * a job is **eligible** when it is due (`run_at <= now`) and its queue holds fewer than
//!   `max_in_flight_per_queue` jobs;
//! * if any eligible job has waited `starvation_ms` or more, the **longest wait wins**, whatever
//!   the priorities are. Waiting longer than the window is the one thing that jumps the queue;
//! * otherwise the **highest priority wins**, and ties go to the job that was enqueued first (then
//!   to the smaller id, so the answer never depends on the order a `Vec` happens to hold);
//! * the rule is **work-conserving**: if something is eligible, `next` returns a job. A capped
//!   queue is skipped, it does not stop the scheduler — otherwise one tenant's ceiling would idle
//!   the pool the other tenant could have used.
//!
//! The trap this design exists to avoid is written into the tests, because it is the one an
//! obvious implementation falls into: **giving every waiting job one extra rank per interval never
//! closes the gap.** The loud tenant's oldest job is aging at exactly the same rate, so a job that
//! started ten ranks behind stays ten ranks behind, forever, while the dashboard shows an ageing
//! mechanism that is working. Promotion past a threshold is not a refinement of that idea — it is
//! the different idea, and it is the one that bounds the wait.
//!
//! ## What you write
//!
//! [`Scheduler::submit`], [`Scheduler::next`], [`Scheduler::release`],
//! [`Scheduler::in_flight`], [`Scheduler::waiting`] and [`Scheduler::oldest_wait_ms`].
//! [`Job`] and [`Policy`] are given: the policy's numbers are the design's, and a probe that lets
//! you rename them would not be checking the design.
//!
//! ## What the tests ask
//!
//! Priority decides while everybody is inside the window, and ties go to the earlier job. A job
//! that has waited past the window is served next, even against a queue submitting at the top
//! priority — and a job waiting *one millisecond* short of the window is not. Then the scenario the
//! step exists for: a thousand high-priority jobs against one low-priority job, with a bound on how
//! many of them may be served first. The per-queue cap is checked from both sides (a capped queue
//! is skipped; a queue under its cap is served), and so is the number an operator watches.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, and no database: the waiting jobs are a `Vec`, "a worker finished"
//! is [`Scheduler::release`], and the clock is a parameter — the real step drains a Postgres-backed
//! queue with several workers and its spec asserts the same starvation property against that
//! system. What is re-checked here is the ordering rule, which is the part a learner has to be able
//! to argue: no priority scheme survives contact with a noisy tenant unless something bounds the
//! wait and something bounds the share.
//!
//! Missing on purpose: no rate limiting (`http/rate-limit@design` is where a tenant's *submission*
//! rate is capped, and a scheduler cannot see the difference between a burst and a flood), no
//! accounting of who used what (`sec/authz-tenant-isolation@operate`), and no fairness *between
//! workers* — a worker that holds a slot forever is `db/atomic-claim@design`'s problem, not this
//! policy's.

use std::collections::BTreeMap;

/// One job waiting for a worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// Identity, chosen by the caller.
    pub id: u64,
    /// The queue it belongs to. A queue is the unit of fairness here: one tenant, one ceiling.
    pub queue: String,
    /// Higher runs first.
    pub priority: i32,
    /// When it was submitted: what "how long has this waited" counts from.
    pub enqueued_ms: u64,
    /// Not before this instant.
    pub run_at: u64,
}

/// The two numbers this policy is: an ADR names them, the code does not invent them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// How many jobs one queue may hold running at the same time. A queue at this ceiling is
    /// skipped by [`Scheduler::next`], so one tenant cannot occupy the pool.
    pub max_in_flight_per_queue: usize,
    /// How long a job may wait before it is served next regardless of priority. This is the number
    /// that bounds starvation, and the one to make shorter when a tenant complains.
    pub starvation_ms: u64,
}

/// The waiting jobs and what each queue already holds.
#[derive(Debug)]
pub struct Scheduler {
    policy: Policy,
    waiting: Vec<Job>,
    in_flight: BTreeMap<String, usize>,
}

impl Scheduler {
    /// A scheduler with this policy and nothing waiting.
    #[must_use]
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            waiting: Vec::new(),
            in_flight: BTreeMap::new(),
        }
    }

    /// Put a job in the waiting set.
    pub fn submit(&mut self, job: Job) {
        self.waiting.push(job);
    }

    /// Pick the next job to hand to a worker and mark it in flight, or `None` when nothing is
    /// eligible.
    ///
    /// A job is eligible when it is due and its queue is under its ceiling. Among the eligible
    /// ones: anything that has waited at least `starvation_ms` wins, and the longest wait wins
    /// among those; otherwise the highest priority wins, ties broken by `enqueued_ms` and then by
    /// `id`.
    ///
    /// A queue at its ceiling is *skipped*, not blocking: the scheduler keeps looking, so the pool
    /// is never idle while somebody else's work could run.
    #[must_use]
    pub fn next(&mut self, now_ms: u64) -> Option<Job> {
        let mut chosen: Option<usize> = None;
        for (index, job) in self.waiting.iter().enumerate() {
            if job.run_at > now_ms || !self.under_ceiling(&job.queue) {
                continue;
            }
            if chosen.is_none_or(|current| better(self, job, &self.waiting[current], now_ms)) {
                chosen = Some(index);
            }
        }
        let job = self.waiting.remove(chosen?);
        *self.in_flight.entry(job.queue.clone()).or_insert(0) += 1;
        Some(job)
    }

    /// A worker finished (or gave up) a job from `queue`, freeing one of its slots.
    pub fn release(&mut self, queue: &str) {
        if let Some(count) = self.in_flight.get_mut(queue) {
            *count = count.saturating_sub(1);
        }
    }

    /// How many jobs `queue` holds right now.
    #[must_use]
    pub fn in_flight(&self, queue: &str) -> usize {
        self.in_flight.get(queue).copied().unwrap_or(0)
    }

    /// How many jobs are waiting, whatever their queue.
    #[must_use]
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// The longest a due job has waited, which is the number an operator watches: it stops climbing
    /// when the policy is working and grows without bound when it is not.
    #[must_use]
    pub fn oldest_wait_ms(&self, now_ms: u64) -> Option<u64> {
        self.waiting
            .iter()
            .filter(|job| job.run_at <= now_ms)
            .map(|job| now_ms.saturating_sub(job.enqueued_ms))
            .max()
    }

    fn under_ceiling(&self, queue: &str) -> bool {
        self.in_flight(queue) < self.policy.max_in_flight_per_queue
    }

    fn waited(&self, job: &Job, now_ms: u64) -> u64 {
        now_ms.saturating_sub(job.enqueued_ms)
    }
}

/// Is `candidate` the better choice than `current`?
///
/// The starvation window is checked first and on its own: a job past it is served before any job
/// inside it, however high the other one's priority is. That is what makes the wait bounded — the
/// alternative, "everybody gains a rank per interval", leaves the loud tenant's oldest job gaining
/// ranks at exactly the same rate, so the gap never closes.
fn better(scheduler: &Scheduler, candidate: &Job, current: &Job, now_ms: u64) -> bool {
    let window = scheduler.policy.starvation_ms;
    let candidate_waited = scheduler.waited(candidate, now_ms);
    let current_waited = scheduler.waited(current, now_ms);
    match (candidate_waited >= window, current_waited >= window) {
        (true, true) => {
            candidate_waited > current_waited
                || (candidate_waited == current_waited && earlier(candidate, current))
        }
        (true, false) => true,
        (false, true) => false,
        (false, false) => {
            candidate.priority > current.priority
                || (candidate.priority == current.priority && earlier(candidate, current))
        }
    }
}

/// The tie-break inside one priority class: whoever has been waiting since earlier, and the smaller
/// id when two jobs were submitted in the same millisecond.
fn earlier(candidate: &Job, current: &Job) -> bool {
    (candidate.enqueued_ms, candidate.id) < (current.enqueued_ms, current.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ceiling wide enough that only the starvation rule is under test.
    const WIDE: Policy = Policy {
        max_in_flight_per_queue: 8,
        starvation_ms: 1_000,
    };

    fn job(id: u64, queue: &str, priority: i32, enqueued_ms: u64) -> Job {
        Job {
            id,
            queue: queue.to_owned(),
            priority,
            enqueued_ms,
            run_at: 0,
        }
    }

    /// Drain what the scheduler will hand out before `limit` picks, releasing each job so the
    /// ceilings do not turn the loop into a single-tenant test by accident.
    fn drain(scheduler: &mut Scheduler, now_ms: &mut u64, step_ms: u64, limit: usize) -> Vec<u64> {
        let mut picked = Vec::new();
        while picked.len() < limit {
            let Some(job) = scheduler.next(*now_ms) else {
                break;
            };
            picked.push(job.id);
            scheduler.release(&job.queue);
            *now_ms += step_ms;
        }
        picked
    }

    #[test]
    fn priority_decides_and_ties_go_to_whoever_waited_longest() {
        let mut scheduler = Scheduler::new(WIDE);
        scheduler.submit(job(1, "quiet", 1, 0));
        scheduler.submit(job(2, "loud", 5, 0));
        scheduler.submit(job(3, "loud", 5, 0));

        let first = scheduler.next(0).expect("three jobs are due");
        assert_eq!(
            first.priority, 5,
            "the highest priority runs first: {first:?}"
        );
        assert_eq!(
            first.id, 2,
            "and two jobs at one priority run in submission order"
        );

        let second = scheduler.next(0).expect("the other priority-5 job is due");
        assert_eq!(second.id, 3);
        let third = scheduler
            .next(0)
            .expect("the low-priority job is still due");
        assert_eq!(third.id, 1, "nobody was starved, so priority stands");

        assert_eq!(scheduler.waiting(), 0);
        assert!(scheduler.next(0).is_none(), "nothing is left to hand out");
    }

    #[test]
    fn a_job_past_the_window_is_served_before_a_higher_priority() {
        let mut scheduler = Scheduler::new(WIDE);
        // Waiting 999ms: inside the window by one millisecond, so it keeps losing.
        scheduler.submit(job(1, "quiet", 0, 0));
        scheduler.submit(job(2, "loud", 10, WIDE.starvation_ms - 1));

        let first = scheduler
            .next(WIDE.starvation_ms - 1)
            .expect("both are due");
        assert_eq!(
            first.id, 2,
            "one millisecond short of the window is still inside it: priority decides"
        );

        // The same job, one millisecond later: the window is reached and it jumps the queue.
        let mut waiting = Scheduler::new(WIDE);
        waiting.submit(job(1, "quiet", 0, 0));
        waiting.submit(job(2, "loud", 10, WIDE.starvation_ms));
        let jumped = waiting
            .next(WIDE.starvation_ms)
            .expect("both are due at the window's edge");
        assert_eq!(
            jumped.id, 1,
            "the longest wait wins the moment it reaches the window, whatever the priorities are"
        );
        assert_eq!(jumped.priority, 0, "and it is still the low-priority job");

        // Among jobs past the window, the longest wait goes first — not the highest priority.
        let mut over = Scheduler::new(WIDE);
        over.submit(job(1, "quiet", 0, 0));
        over.submit(job(2, "loud", 10, WIDE.starvation_ms - 500));
        let first = over.next(WIDE.starvation_ms + 1_000).expect("both are due");
        assert_eq!(
            first.id, 1,
            "two jobs past the window are ordered by how long they have waited: {first:?}"
        );
    }

    #[test]
    fn a_flooded_queue_cannot_starve_a_quiet_one() {
        // A wide ceiling, so the only thing that can save the quiet job is the starvation rule.
        let policy = Policy {
            max_in_flight_per_queue: 4,
            starvation_ms: 100,
        };
        let mut scheduler = Scheduler::new(policy);
        let quiet = 1;
        scheduler.submit(job(quiet, "quiet", 0, 0));
        for id in 2..=1_001 {
            scheduler.submit(job(id, "hog", 10, 0));
        }

        let mut now = 0;
        let picked = drain(&mut scheduler, &mut now, 10, 400);
        let position = picked
            .iter()
            .position(|id| *id == quiet)
            .expect("the quiet queue was starved: it never got a worker");
        let bound = policy.starvation_ms / 10 + 2;
        assert!(
            position <= bound as usize,
            "the quiet job was served at pick {position}, which is past the bound of {bound} the \
             window implies: a low-priority job waits at most the window plus a pick, and a rank \
             per interval would not have bounded it at all"
        );
        assert!(
            position >= 1,
            "the loud queue is not punished for being loud: priority decides until the window"
        );
    }

    #[test]
    fn the_ceiling_keeps_one_queue_out_of_the_pool() {
        let policy = Policy {
            max_in_flight_per_queue: 2,
            starvation_ms: 60_000,
        };
        let mut scheduler = Scheduler::new(policy);
        for id in 1..=5 {
            scheduler.submit(job(id, "hog", 10, 0));
        }
        scheduler.submit(job(9, "quiet", 0, 0));

        let first = scheduler.next(0).expect("the loud queue is first");
        let second = scheduler.next(0).expect("and it may take its ceiling");
        assert_eq!(
            (first.queue.as_str(), second.queue.as_str()),
            ("hog", "hog")
        );
        assert_eq!(
            scheduler.in_flight("hog"),
            2,
            "the ceiling is what the queue counts"
        );

        let third = scheduler
            .next(0)
            .expect("the quiet queue is due and under its ceiling");
        assert_eq!(
            third.queue, "quiet",
            "a queue at its ceiling is skipped even when its priority would win, which is the \
             difference between a busy pool and a captured one"
        );
        assert_eq!(third.id, 9);
        assert!(
            scheduler.next(0).is_none(),
            "the loud queue has three jobs left and may not run any of them; the quiet queue has \
             nothing left: nothing is eligible"
        );
        assert_eq!(
            scheduler.waiting(),
            3,
            "and the loud queue's work is still queued"
        );
    }

    #[test]
    fn a_queue_under_its_ceiling_is_never_left_waiting() {
        let policy = Policy {
            max_in_flight_per_queue: 1,
            starvation_ms: 60_000,
        };
        let mut scheduler = Scheduler::new(policy);
        scheduler.submit(job(1, "hog", 10, 0));
        scheduler.submit(job(2, "hog", 10, 0));
        scheduler.submit(job(3, "quiet", 0, 0));
        scheduler.submit(job(4, "quiet", 0, 0));

        assert_eq!(scheduler.next(0).map(|job| job.id), Some(1));
        assert_eq!(
            scheduler.next(0).map(|job| job.id),
            Some(3),
            "the pool is not left idle because the loud queue hit its ceiling"
        );
        assert!(
            scheduler.next(0).is_none(),
            "both queues are at their ceiling: `None` is the honest answer, not a second slot for \
             the loud one"
        );

        // A worker finishing frees exactly one slot, and the queue that had two jobs waiting is the
        // one that gets it back — the ceiling is not a queue-level lock.
        scheduler.release("hog");
        assert_eq!(scheduler.in_flight("hog"), 0);
        assert_eq!(scheduler.next(0).map(|job| job.id), Some(2));
        assert!(
            scheduler.next(0).is_none(),
            "the quiet queue is still at its ceiling"
        );
        scheduler.release("quiet");
        assert_eq!(scheduler.next(0).map(|job| job.id), Some(4));

        // Releasing something nobody holds is not a way to invent capacity.
        scheduler.release("quiet");
        scheduler.release("quiet");
        assert_eq!(
            scheduler.in_flight("quiet"),
            0,
            "a slot count never goes below zero"
        );
    }

    #[test]
    fn the_number_an_operator_watches_is_the_oldest_wait() {
        let mut scheduler = Scheduler::new(WIDE);
        assert_eq!(
            scheduler.oldest_wait_ms(1_000),
            None,
            "an empty queue has nothing waiting, which is not the same as waiting zero"
        );

        scheduler.submit(job(1, "a", 0, 0));
        scheduler.submit(job(2, "b", 0, 400));
        assert_eq!(scheduler.oldest_wait_ms(500), Some(500));

        // A delayed job is not waiting for a worker yet: counting it would make the metric alarm
        // about work that is on time.
        let mut delayed = Job {
            run_at: 10_000,
            ..job(3, "c", 0, 0)
        };
        delayed.run_at = 10_000;
        scheduler.submit(delayed);
        assert_eq!(
            scheduler.oldest_wait_ms(500),
            Some(500),
            "the delayed job is not due, so it is not part of the wait"
        );
        assert_eq!(
            scheduler.oldest_wait_ms(12_000),
            Some(12_000),
            "and once it is due, its wait is counted like anybody else's"
        );

        // Once the oldest job is served the number falls back to the next one, which is what makes
        // it worth watching: it is the age of the queue, not a counter that only grows.
        let _ = scheduler.next(500);
        assert_eq!(scheduler.oldest_wait_ms(500), Some(100));
    }
}
