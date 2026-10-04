//! probe = "time/clock-injection@apply"
//! title = "Two clocks, and the one the arithmetic uses"
//! hint = "Timestamps come from the wall clock; deadlines, waits and `how long since` come from the monotonic one — and a wait that has already passed is zero, never a subtraction that panics."
//!
//! Step `jobq/3.8-clock-injection` makes time a dependency of the engine: every reading arrives as
//! a parameter, business code may not call `SystemTime::now`, and a test that used to `sleep`
//! instead advances a fake clock by hand.
//!
//! The reason there are **two** clocks, and not one, is what this probe is about:
//!
//! * the **wall clock** is the one an operator reads. It can jump — NTP steps it, somebody fixes
//!   the timezone, a laptop wakes from sleep — so it is good for saying *when* something happened
//!   and useless for saying *how long* something may take;
//! * the **monotonic clock** never goes backwards. Deadlines, wait-before-retry and "how long have
//!   I held this" are arithmetic on it, which is why a lease taken for thirty seconds stays thirty
//!   seconds even if the wall clock is corrected by an hour halfway through.
//!
//! ## What you write
//!
//! * [`Lease::take`] — the wall stamp (for the audit row) and the monotonic deadline (for the
//!   arithmetic), from one reading each;
//! * [`Lease::renew`] — a fresh deadline counted from *now*, which is the decision a renewal is: a
//!   worker that re-stamps gets the whole window again rather than the remainder of the old one;
//! * [`Lease::expired`] — `now >= deadline`, so a lease that ends at `t` is not held at `t`;
//! * [`Lease::until_expiry`] — how long is left, **saturating**: an expired lease has nothing left,
//!   and `deadline - now` on a `u64` is a panic in the middle of a reaper loop;
//! * [`Lease::held_for`] — how long the worker has held it, monotonic, which is the number an
//!   operator reads when they ask whether a worker is stuck.
//!
//! [`Clock`] is the port, [`FakeClock`] is the clock the tests drive (it ships in the file rather
//! than in `#[cfg(test)]`, because a clock a test can drive is a *seam* rather than a mock), and
//! both are given.
//!
//! ## What the tests ask
//!
//! A lease is taken, and its deadline walks to the edge: alive one millisecond before, expired at
//! the deadline, nothing left afterwards. The wall clock is then moved backwards by an hour and the
//! lease is unaffected; it is moved forwards by a week and `held_for` is unaffected. A renewal
//! starts the window over from now, and two renewals in a row do not accumulate. And a sweep
//! asserts the two answers about one lease cannot disagree: `expired` is exactly
//! `until_expiry() == 0`, at every reading, including past the end.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, and **no lint**: the step's other half is `clippy.toml` refusing
//! `SystemTime::now` in business code, and a probe cannot run a lint. What is re-checked is the
//! property that makes the lint possible to obey — every reading in this file is a parameter, so no
//! function here *could* read a clock — and the fake clock drives a lease through expiry without
//! one `sleep`, which is the difference between a test that runs in milliseconds and one that runs
//! in a minute.
//!
//! Missing on purpose: there is no reaper and no fencing token, so "the worker came back after its
//! lease had already been given away" is `db/atomic-claim@design`'s problem; nothing here decides
//! when to wake anybody (`sched/delayed-timers@apply`); and the two readings live on one trait
//! because a probe does not need the real step's split into a clock and a wall clock — what matters
//! is which reading each number came from, and that is visible at the call site.

/// The clock the engine is handed: one reading of each kind.
///
/// Both are milliseconds, both end up on the row, and neither may be read inside the engine: a
/// function that reaches for a clock cannot be judged without one, which is why the port exists.
pub trait Clock {
    /// Milliseconds since the process started, monotonic: this reading never goes backwards.
    fn monotonic_ms(&self) -> u64;

    /// Milliseconds since the Unix epoch. This one jumps, forwards and backwards.
    fn wall_ms(&self) -> i64;
}

/// The clock a test drives by hand. Advancing it is how a test waits without waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FakeClock {
    monotonic_ms: u64,
    wall_ms: i64,
}

impl FakeClock {
    /// A clock with a wall reading and a monotonic one that starts at zero.
    #[must_use]
    pub fn new(wall_ms: i64) -> Self {
        Self {
            monotonic_ms: 0,
            wall_ms,
        }
    }

    /// Move both clocks forward by `ms`: time passing, the way a test makes it pass.
    pub fn advance(&mut self, ms: u64) {
        self.monotonic_ms += ms;
        self.wall_ms += i64::try_from(ms).unwrap_or(i64::MAX);
    }

    /// Set the wall clock to a reading without moving the monotonic one: an NTP step, a timezone
    /// fix, a laptop that was asleep. This is the event a lease must not care about.
    pub fn set_wall(&mut self, wall_ms: i64) {
        self.wall_ms = wall_ms;
    }
}

impl Clock for FakeClock {
    fn monotonic_ms(&self) -> u64 {
        self.monotonic_ms
    }

    fn wall_ms(&self) -> i64 {
        self.wall_ms
    }
}

/// One worker's claim on one job, with both clocks written on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    /// The worker holding it.
    pub worker: String,
    /// The job it holds.
    pub job: u64,
    /// When it was taken, as the wall clock read it: the number that goes in the audit row.
    pub taken_wall_ms: i64,
    /// When it was taken, monotonic: the number [`Lease::held_for`] counts from.
    taken_ms: u64,
    /// When it stops counting, monotonic. Never derived from `taken_wall_ms`.
    deadline_ms: u64,
}

impl Lease {
    /// Take a lease from `clock`, good for `lease_ms`.
    #[must_use]
    pub fn take(clock: &dyn Clock, worker: &str, job: u64, lease_ms: u64) -> Self {
        todo!()
    }

    /// Renew it: the deadline becomes `now + lease_ms`, counted from the reading this call was
    /// handed rather than from the old deadline.
    ///
    /// Counting from the old deadline is the alternative, and it is wrong in a way that compounds:
    /// every late renewal leaves the next one a little shorter, until a worker that is perfectly
    /// healthy loses its lease because it renews at four seconds instead of five.
    pub fn renew(&mut self, clock: &dyn Clock, lease_ms: u64) {
        todo!()
    }

    /// `true` when the lease has run out at this reading.
    ///
    /// The edge belongs to the reaper: a lease whose deadline *is* this instant is over, so two
    /// workers cannot both believe they hold the job at the moment it is handed over.
    #[must_use]
    pub fn expired(&self, clock: &dyn Clock) -> bool {
        todo!()
    }

    /// How long is left, in milliseconds; `0` once it has run out.
    ///
    /// Saturating on purpose. `self.deadline_ms - now` is a panic in debug and a number near
    /// `u64::MAX` in release, and it would happen in the reaper — the one loop that has to keep
    /// running when everything else has gone wrong.
    #[must_use]
    pub fn until_expiry(&self, clock: &dyn Clock) -> u64 {
        todo!()
    }

    /// How long the worker has held the job, monotonic.
    #[must_use]
    pub fn held_for(&self, clock: &dyn Clock) -> u64 {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wall reading that looks like a real audit row, and a lease of thirty seconds.
    const WALL: i64 = 1_760_000_000_000;
    const LEASE_MS: u64 = 30_000;

    fn clock() -> FakeClock {
        FakeClock::new(WALL)
    }

    #[test]
    fn a_lease_is_taken_from_the_clock_it_was_handed() {
        let mut now = clock();
        let lease = Lease::take(&now, "worker-1", 7, LEASE_MS);

        assert_eq!(
            lease.taken_wall_ms, WALL,
            "the audit stamp is the wall reading, not a number the caller made up"
        );
        assert_eq!(lease.taken_wall_ms, now.wall_ms());
        assert_eq!(lease.worker, "worker-1");
        assert_eq!(lease.job, 7);
        assert_eq!(
            lease.until_expiry(&now),
            LEASE_MS,
            "a lease taken now has all of its window left"
        );
        assert!(!lease.expired(&now));

        // The edge: one millisecond before is still held, the deadline itself is not.
        now.advance(LEASE_MS - 1);
        assert!(!lease.expired(&now), "one millisecond early is not expired");
        assert_eq!(lease.until_expiry(&now), 1);
        now.advance(1);
        assert!(
            lease.expired(&now),
            "the deadline is the reaper's, not the holder's: a lease that ends at `t` is not held \
             at `t`"
        );
        assert_eq!(
            lease.until_expiry(&now),
            0,
            "there is nothing left, and asking past the end is still an answer"
        );

        // Past the end, and far past it: still an answer, never a wrapping number.
        now.advance(60 * 60 * 1_000);
        assert_eq!(lease.until_expiry(&now), 0, "saturating, not wrapping");
        assert!(lease.expired(&now));
    }

    #[test]
    fn a_wall_clock_that_jumps_does_not_move_the_deadline() {
        let mut now = clock();
        let lease = Lease::take(&now, "worker-1", 7, LEASE_MS);

        // An hour backwards, as if somebody corrected the time. The lease has not been held for
        // less time because the clock says so.
        now.set_wall(WALL - 3_600_000);
        now.advance(LEASE_MS - 1);
        assert!(
            !lease.expired(&now),
            "a wall clock correction must not expire a lease that has not run out"
        );
        assert_eq!(
            lease.until_expiry(&now),
            1,
            "the monotonic clock is the one that decides"
        );
        assert_eq!(
            lease.taken_wall_ms, WALL,
            "and the audit stamp still says when it was taken"
        );

        // Forwards by a week: the same, from the other side.
        now.set_wall(WALL + 7 * 24 * 3_600_000);
        assert_eq!(
            lease.held_for(&now),
            LEASE_MS - 1,
            "`held_for` counts monotonic time: a wall jump is not a busy worker"
        );
        assert!(!lease.expired(&now));

        // The rest of the window, on the monotonic clock, and only the rest.
        now.advance(1);
        assert!(lease.expired(&now));
    }

    #[test]
    fn a_renewal_starts_the_window_over_from_now() {
        let mut now = clock();
        let mut lease = Lease::take(&now, "worker-1", 7, LEASE_MS);

        now.advance(20_000);
        assert_eq!(
            lease.until_expiry(&now),
            10_000,
            "ten seconds left before renewing"
        );
        lease.renew(&now, LEASE_MS);
        assert_eq!(
            lease.until_expiry(&now),
            LEASE_MS,
            "a renewal is a fresh window from this reading, not the remainder plus thirty seconds"
        );
        assert_eq!(
            lease.taken_wall_ms, WALL,
            "renewing does not rewrite the audit stamp"
        );
        assert_eq!(
            lease.held_for(&now),
            20_000,
            "the job has still been held since it was taken"
        );

        // Two renewals at the same instant do not accumulate: same reading, same deadline.
        lease.renew(&now, LEASE_MS);
        assert_eq!(lease.until_expiry(&now), LEASE_MS);

        now.advance(LEASE_MS - 1);
        assert!(!lease.expired(&now));
        now.advance(1);
        assert!(
            lease.expired(&now),
            "the renewed window ends when it says it does"
        );
    }

    #[test]
    fn a_lease_nobody_renews_runs_out_while_the_worker_is_away() {
        let mut now = clock();
        let lease = Lease::take(&now, "worker-1", 7, LEASE_MS);

        // The worker was busy for a day: one advance, no sleeping, no threads. This is what makes
        // the fake clock worth injecting — the test says "a day passed" in one line.
        now.advance(24 * 60 * 60 * 1_000);
        assert!(
            lease.expired(&now),
            "a lease that is not renewed while its worker is stuck has to run out, or the job is \
             stuck with it"
        );
        assert_eq!(lease.until_expiry(&now), 0);
        assert_eq!(
            lease.held_for(&now),
            24 * 60 * 60 * 1_000,
            "and the number an operator sees says how long ago this went wrong"
        );

        // The next worker takes it, with its own window, and the two leases are not one row.
        let later = Lease::take(&now, "worker-2", 7, LEASE_MS);
        assert_eq!(later.worker, "worker-2");
        assert!(!later.expired(&now));
        assert_eq!(
            later.taken_wall_ms,
            lease.taken_wall_ms + 24 * 60 * 60 * 1_000
        );
    }

    #[test]
    fn the_two_answers_about_one_lease_never_disagree() {
        let mut now = clock();
        let mut lease = Lease::take(&now, "worker-1", 7, 5);

        // A sweep over the whole life of a lease, renewed in the middle, asserting the invariant a
        // hand-rolled subtraction breaks: "expired" and "nothing left" are one fact.
        for step in 0..40_u64 {
            if step == 20 {
                lease.renew(&now, 5);
            }
            let left = lease.until_expiry(&now);
            assert_eq!(
                lease.expired(&now),
                left == 0,
                "at step {step}, {left}ms were left and `expired` disagreed"
            );
            if left > 0 {
                assert!(left <= 5, "a lease is never longer than its window: {left}");
            }
            now.advance(1);
        }
        assert!(lease.expired(&now), "the sweep ran past the second window");
    }
}
