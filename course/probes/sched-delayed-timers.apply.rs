//! probe = "sched/delayed-timers@apply"
//! title = "A timer that costs nothing while it waits"
//! hint = "One heap keyed by `(ready_at, id)`, a map of where each id really is, and a pop that looks at the due entries and stops — a scan over a million timers a year out is the bug."
//!
//! Step `jobq/3.11-delayed-timers` schedules work for later, and the numbers are what make it a
//! design rather than a loop: a queue with a million jobs a year in the future must not become a
//! million wake-ups, a million rows read per tick, or a million comparisons per claim.
//!
//! The shape is three decisions, and each one is checkable without a clock:
//!
//! * **one heap, keyed by `(ready_at, id)`.** The earliest instant is at the top, so "when must this
//!   process wake up?" is one peek rather than a scan over the schedule. The `id` is in the key so
//!   that two timers for the same millisecond come out in a fixed order — a ready set whose order
//!   depends on insertion is a ready set whose tests pass on the developer's machine;
//! * **a map of where each id really is.** A job that is delayed again is *the same job*, and it
//!   must not fire twice. Moving it in the heap is not possible, so the heap keeps the old entry and
//!   [`Timers::latest`] is what says which entry is real: the stale one is dropped when it reaches
//!   the top, once, instead of re-heapifying on every move;
//! * **the cost is counted, not timed.** [`Timers::examined`] is how many entries the last pop
//!   looked at. That number is what a budget can be held to on a shared build machine where
//!   milliseconds are noise, and it is what catches the rewrite that scans the whole schedule
//!   looking for what is due.
//!
//! ## What you write
//!
//! * [`Timers::schedule`] — write down when an id is due, and keep the heap usable;
//! * [`Timers::next_wake`] — the instant the process must wake up, or `None` when nothing waits;
//! * [`Timers::pop_ready`] — everything due at `now`, in `(ready_at, id)` order, and nothing that
//!   is not due; each pop is what tells the caller whether it can sleep at all.
//!
//! [`Timers`]'s fields, [`Timer`], `new` and the three counters are given: the shape is the
//! lesson's starting point here, and a counter the learner cannot increment would not be a budget.
//!
//! ## What the tests ask
//!
//! Nothing waiting means nothing to wake for. Three timers scheduled out of order come out in
//! instant order, and `next_wake` moves forward as they are popped. A timer that is delayed again
//! fires once, at its new instant, and costs one stale entry. Then the number the step exists for: a
//! hundred thousand timers a year out, three due now, and a pop that looks at four entries — the
//! three and the peek that told it to stop. A scan would look at a hundred thousand and three, and
//! the counter is what says so on a machine where a stopwatch would not.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no `tokio`, no sleeping and no database**: the heap is in the
//! process and the clock is a parameter. The other half of the real step — the claim query that
//! reads `run_at` so a worker does not have to hold a million timers in memory at all — cannot be
//! re-checked here, because it is SQL; what this file re-checks is the in-process ready set that
//! sits in front of it, and the rule that the two agree on *when* is stated in the step's own spec
//! rather than in a probe.
//!
//! Also missing: nothing here wakes a thread (`next_wake` is a number, and a runtime decides what to
//! do with it), and there is no persistence, so a restart loses the ready set — the store is where
//! the jobs are, and this is a cache of *when*.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

/// One job that becomes due at an instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timer {
    /// The job.
    pub id: u64,
    /// The instant it is due, in the caller's unit (milliseconds, in the engine).
    pub ready_at: i64,
}

/// One place in the heap. Ordered by `ready_at` and then `id`, so the top is the next wake-up and
/// two timers for one instant have one order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Entry {
    ready_at: i64,
    id: u64,
}

/// The delayed jobs this process is holding.
#[derive(Debug)]
pub struct Timers {
    heap: BinaryHeap<Reverse<Entry>>,
    latest: BTreeMap<u64, i64>,
    examined: u64,
    stale: u64,
}

impl Timers {
    /// An empty ready set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            latest: BTreeMap::new(),
            examined: 0,
            stale: 0,
        }
    }

    /// Make `id` due at `ready_at`.
    ///
    /// Scheduling an id that is already waiting *moves* it: one job, one instant, one entry that
    /// fires. The entry it leaves behind in the heap is the price of never having to find a job
    /// inside a heap to change its instant.
    pub fn schedule(&mut self, id: u64, ready_at: i64) {
        todo!()
    }

    /// The instant this process has to wake up, or `None` when nothing is waiting.
    ///
    /// Stale entries at the top are dropped here rather than on every move: a scheduler that
    /// re-heapified when a job was delayed would pay `O(log n)` on the path a caller uses most.
    #[must_use]
    pub fn next_wake(&mut self) -> Option<i64> {
        todo!()
    }

    /// Everything due at `now`, in `(ready_at, id)` order, and nothing else.
    ///
    /// Nothing else is the point: the pop stops at the first entry that is not due, which is why a
    /// million timers a year out cost a peek rather than a scan. A pop also resets
    /// [`Timers::examined`], which is how the tests hold it to a number.
    pub fn pop_ready(&mut self, now: i64) -> Vec<Timer> {
        todo!()
    }

    /// How many jobs are waiting: the heap may hold more entries, and that is not what a dashboard
    /// should show.
    #[must_use]
    pub fn scheduled(&self) -> usize {
        self.latest.len()
    }

    /// Entries the *last* [`Timers::pop_ready`] looked at. The budget this step is held to, in a
    /// unit a shared machine cannot make flaky.
    #[must_use]
    pub fn examined(&self) -> u64 {
        self.examined
    }

    /// Entries dropped because the job they were for has moved. Reported rather than hidden: it is
    /// the cost of the design, and it should stay small.
    #[must_use]
    pub fn stale(&self) -> u64 {
        self.stale
    }
}

impl Default for Timers {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A year, in the milliseconds the engine works in.
    const YEAR_MS: i64 = 365 * 24 * 60 * 60 * 1_000;

    #[test]
    fn nothing_waiting_means_nothing_to_wake_for() {
        let mut timers = Timers::new();
        assert_eq!(timers.next_wake(), None);
        assert!(timers.pop_ready(i64::MAX).is_empty());
        assert_eq!(timers.scheduled(), 0);
        assert_eq!(timers.examined(), 0, "an empty pop looks at nothing");
    }

    #[test]
    fn the_heap_answers_with_the_earliest_instant_and_moves_forward() {
        let mut timers = Timers::new();
        timers.schedule(7, 500);
        timers.schedule(3, 100);
        timers.schedule(9, 300);

        assert_eq!(
            timers.next_wake(),
            Some(100),
            "the earliest instant is the next wake-up, whatever order the jobs arrived in"
        );

        // Only what is due: the pop stops at the first entry that is not.
        assert_eq!(
            timers.pop_ready(100),
            vec![Timer {
                id: 3,
                ready_at: 100
            }]
        );
        assert_eq!(timers.next_wake(), Some(300));
        assert_eq!(timers.scheduled(), 2, "the popped job is no longer waiting");

        // Everything due at once comes out in instant order, then in id order.
        assert_eq!(
            timers.pop_ready(1_000),
            vec![
                Timer {
                    id: 9,
                    ready_at: 300
                },
                Timer {
                    id: 7,
                    ready_at: 500
                },
            ]
        );
        assert_eq!(timers.next_wake(), None);
        assert_eq!(timers.scheduled(), 0);
    }

    #[test]
    fn two_timers_for_one_instant_have_one_order() {
        let mut timers = Timers::new();
        for id in [5, 1, 3] {
            timers.schedule(id, 42);
        }
        assert_eq!(
            timers
                .pop_ready(42)
                .iter()
                .map(|timer| timer.id)
                .collect::<Vec<u64>>(),
            vec![1, 3, 5],
            "the id is part of the key: a ready set whose order depends on insertion order is a \
             ready set whose tests pass on the machine that wrote them"
        );
    }

    #[test]
    fn a_job_that_is_delayed_again_fires_once_at_its_new_instant() {
        let mut timers = Timers::new();
        timers.schedule(1, 100);
        timers.schedule(1, 5_000);

        assert_eq!(
            timers.scheduled(),
            1,
            "one job, however many times it moved"
        );
        assert_eq!(
            timers.pop_ready(100),
            Vec::new(),
            "the entry for the instant it was moved away from is not a run"
        );
        assert_eq!(
            timers.stale(),
            1,
            "it is dropped once, at the top of the heap"
        );
        assert_eq!(
            timers.examined(),
            1,
            "the pop looked at the entry that is still current, and at nothing else"
        );

        assert_eq!(timers.next_wake(), Some(5_000));
        assert_eq!(
            timers.pop_ready(5_000),
            vec![Timer {
                id: 1,
                ready_at: 5_000
            }],
            "it fires at the instant it was moved to"
        );
        assert_eq!(
            timers.pop_ready(i64::MAX),
            Vec::new(),
            "and it does not fire twice"
        );
        assert_eq!(timers.scheduled(), 0);
    }

    #[test]
    fn a_timer_in_the_past_is_due_now_and_the_clock_is_the_callers() {
        let mut timers = Timers::new();
        timers.schedule(1, 0);
        timers.schedule(2, i64::MAX - 1);

        assert_eq!(
            timers
                .pop_ready(1_000)
                .iter()
                .map(|timer| timer.id)
                .collect::<Vec<u64>>(),
            vec![1],
            "a job whose instant has passed is due, not lost"
        );
        assert_eq!(
            timers.next_wake(),
            Some(i64::MAX - 1),
            "and the far one is still just an instant"
        );

        // Nothing here reads a clock, so the pop can be handed any instant — including one no clock
        // will ever report.
        assert_eq!(timers.pop_ready(i64::MAX).len(), 1);
        assert_eq!(timers.scheduled(), 0);
    }

    #[test]
    fn a_hundred_thousand_timers_a_year_out_do_not_cost_a_wake_up() {
        let mut timers = Timers::new();
        let now = 1_760_000_000_000;
        for id in 1..=100_000 {
            timers.schedule(id, now + YEAR_MS + id as i64);
        }
        // Three that are due, mixed in with the far ones: a heap does not care where they are.
        timers.schedule(200_001, now - 5);
        timers.schedule(200_002, now - 5);
        timers.schedule(200_003, now);

        let due = timers.pop_ready(now);
        assert_eq!(
            due.iter().map(|timer| timer.id).collect::<Vec<u64>>(),
            vec![200_001, 200_002, 200_003],
            "the three due jobs come out, oldest instant first"
        );
        assert_eq!(
            timers.examined(),
            4,
            "the pop looked at the three due entries and at the one peek that told it to stop. A \
             scan over the schedule would have looked at 100_003 of them, and this number is what \
             says so without a stopwatch: {} entries examined",
            timers.examined()
        );
        assert_eq!(
            timers.scheduled(),
            100_000,
            "the far ones are still waiting"
        );
        assert!(
            timers.next_wake().expect("the far ones are still there") > now + YEAR_MS,
            "and the next wake-up is a year away, not a tick away"
        );
        assert_eq!(
            timers.stale(),
            0,
            "nothing moved, so nothing had to be dropped"
        );
    }
}
