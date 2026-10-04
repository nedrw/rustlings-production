//! probe = "delivery/retry-budget@design"
//! title = "A budget, not a loop"
//! hint = "Delay the attempt from the id, cap the window, and let the breaker — not the worker — decide when an endpoint is dead."
//!
//! Step `relay/2.8-retry-budget` is two policies, and the interesting part of both is what they
//! do to a *fleet* of workers, not to one delivery:
//!
//! 1. **the wait grows, is capped, and is spread.** `base × 2^attempt` is the ceiling, not the
//!    delay: plenty of clients have waited exactly that, together, and hit a dead endpoint in
//!    lockstep the moment it came back. [`Retry::delay_for`] answers a wait that is *derived from
//!    the delivery's id*, so two subscribers that failed at the same second do not come back at the
//!    same second, and the same `(id, attempt)` always answers the same number — a retry policy
//!    that cannot be reproduced cannot be tested;
//! 2. **the breaker is what stops a dead endpoint from eating the worker.** After a run of failures
//!    it refuses for a cooldown, and the deliveries *stay queued* instead of each one burning an
//!    attempt on a target that is not there. One request goes through when the cooldown passes, and
//!    its outcome decides: success resets everything, failure starts the cooldown over.
//!
//! ## What you write
//!
//! * [`Retry::delay_for`] — the wait after `attempt` failures, for one delivery id. `attempt` is
//!   1-based: `delay_for(0, …)` is the attempt that has not failed yet and waits for nothing. The
//!   window is `base × 2^(attempt - 1)` saturating at `cap_ms`, and the answer stays inside it;
//! * [`Breaker::check`] — may this call make an attempt *now*? [`Verdict::Refused`] is the whole
//!   point: no attempt is made, so nothing is burned;
//! * [`Breaker::record`] — what the attempt it allowed did to the breaker.
//!
//! [`Breaker::state`], [`Retry::breaker`] and [`jitter`] are given. The jitter mixer is a
//! deterministic hash rather than a random number on purpose — see below — and it is not the
//! lesson; the window, the cap and the saturation are.
//!
//! ## What the tests ask
//!
//! Same `(id, attempt)` twice is the same number; 64 ids at one attempt are not; no wait ever
//! leaves its window, including at attempt 1 000, where `base << attempt` is the arithmetic bug
//! that turns a retry into a panic. The breaker is walked through its three states with a clock the
//! test owns: open after a run of failures, one request through when the cooldown passes, the
//! cooldown restarted when that probe fails, and everything reset when it succeeds.
//!
//! ## Where this is honest, and where it is not
//!
//! The clock is a parameter, because a probe that sleeps through a real cooldown is a probe nobody
//! runs; the real breaker reads `Instant`. There is no HTTP client, no `tokio` and no `rand`: the
//! waits are deterministic rather than random, which is a *stronger* property than the service
//! needs and the one that makes the delay testable. Deliberately missing: the breaker belongs to
//! one target rather than one delivery (the real service keeps a breaker per endpoint, and that map
//! is not this exercise), `max_attempts` is part of the profile but spending it is
//! `delivery/dlq@apply`'s business, and nothing here decides *which* delivery to retry next.

/// The retry profile: the numbers an ADR names, in one place, so the worker does not carry them
/// around in three constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    /// The window after the first failure, and what every later window is built from.
    pub base_ms: u64,
    /// The ceiling: no wait is longer than this, whatever the attempt.
    pub cap_ms: u64,
    /// How many attempts a delivery gets before it is out of budget (`relay/2.10`).
    pub max_attempts: u32,
    /// Consecutive failures that open the breaker.
    pub open_after: u32,
    /// How long the breaker refuses before one request is let through.
    pub cooldown_ms: u64,
}

impl Retry {
    /// The wait before the attempt that follows `attempt` failures of this delivery.
    ///
    /// `attempt` is 1-based. The window is `base_ms × 2^(attempt - 1)`, saturating at `cap_ms` —
    /// saturating, not wrapping, because "attempt 64" is a number a broken loop reaches and an
    /// overflowing shift is a panic in the middle of a retry. The answer is inside the window, and
    /// it is derived from `id`: the same delivery and the same attempt always answer the same wait.
    #[must_use]
    pub fn delay_for(&self, attempt: u32, id: &str) -> u64 {
        todo!()
    }

    /// The breaker that goes with this profile.
    #[must_use]
    pub fn breaker(&self) -> Breaker {
        Breaker::new(self.open_after, self.cooldown_ms)
    }
}

/// A deterministic wait inside `0..=window`.
///
/// Full jitter — spread over the whole window rather than added to it — mixed from the delivery id
/// and the attempt. A probe has no `rand` and does not want one: a service that seeds this from
/// something it already has (the delivery id) gets the same de-correlation as a random number, and
/// a test can say what the wait is instead of bracketing it. The finalizer is the usual one, so two
/// ids that differ in one character are not one millisecond apart.
#[must_use]
pub fn jitter(id: &str, attempt: u32, window: u64) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in id.as_bytes().iter().chain(&attempt.to_le_bytes()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^= hash >> 27;
    hash = hash.wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^= hash >> 31;
    hash % window.saturating_add(1)
}

/// What one attempt did, as far as the breaker is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The endpoint answered.
    Success,
    /// It did not: a timeout, a connection error, a 5xx.
    Failure,
}

/// The breaker's answer to "may I make an attempt now?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Go ahead. The outcome has to come back through [`Breaker::record`].
    Allowed,
    /// Do not. The delivery stays queued and keeps its attempts for later.
    Refused,
}

/// Where the breaker is, as of one clock reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Working: attempts are allowed.
    Closed,
    /// A run of failures: attempts are refused until the cooldown passes.
    Open,
    /// The cooldown passed, and exactly one attempt is allowed through to find out whether the
    /// endpoint is back.
    HalfOpen,
}

/// The breaker: a run of failures, and the cooldown that follows it.
#[derive(Debug)]
pub struct Breaker {
    /// Consecutive failures that open it.
    open_after: u32,
    /// How long it refuses after opening or after a failed probe.
    cooldown_ms: u64,
    /// The run of failures since the last success, while it is closed.
    failures: u32,
    /// When it opened, if it is not closed.
    opened_at: Option<u64>,
    /// Whether the one request the half-open state allows has already gone out.
    probing: bool,
}

impl Breaker {
    /// A closed breaker that opens after `open_after` consecutive failures.
    #[must_use]
    pub fn new(open_after: u32, cooldown_ms: u64) -> Self {
        Self {
            open_after,
            cooldown_ms,
            failures: 0,
            opened_at: None,
            probing: false,
        }
    }

    /// May the caller make an attempt now?
    ///
    /// While the breaker is open the answer is [`Verdict::Refused`], and a refused delivery is a
    /// delivery that was not attempted — the attempt it keeps is what the queue is for. The one
    /// request the half-open state allows is consumed here, so a second caller in the same
    /// millisecond is refused rather than racing the first one to a dead endpoint.
    #[must_use]
    pub fn check(&mut self, now: u64) -> Verdict {
        todo!()
    }

    /// Record what the attempt [`Breaker::check`] allowed did.
    pub fn record(&mut self, outcome: Outcome, now: u64) {
        todo!()
    }

    /// Where the breaker is as of `now`.
    #[must_use]
    pub fn state(&self, now: u64) -> State {
        match self.opened_at {
            Some(opened) if now < opened.saturating_add(self.cooldown_ms) => State::Open,
            Some(_) => State::HalfOpen,
            None => State::Closed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The profile the tests use: the shape of the real one, with numbers a test can hold in its
    /// head.
    fn profile() -> Retry {
        Retry {
            base_ms: 100,
            cap_ms: 30_000,
            max_attempts: 5,
            open_after: 3,
            cooldown_ms: 1_000,
        }
    }

    #[test]
    fn the_wait_is_a_function_of_the_id_and_the_attempt() {
        let retry = profile();

        assert_eq!(
            retry.delay_for(0, "ada"),
            0,
            "`attempt` counts failures, so the first attempt waits for nothing"
        );
        for id in ["ada", "grace", "ana"] {
            for attempt in 1..=8 {
                assert_eq!(
                    retry.delay_for(attempt, id),
                    retry.delay_for(attempt, id),
                    "the same delivery and the same attempt answer the same wait, every time: a \
                     delay nobody can reproduce is a delay nobody can test"
                );
            }
        }

        // 64 subscribers that failed at the same second do not come back at the same second.
        let waits: Vec<u64> = (0..64)
            .map(|index| retry.delay_for(6, &format!("subscriber-{index}")))
            .collect();
        let mut distinct = waits.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert!(
            distinct.len() >= 60,
            "the waits are spread, not clustered around one value: {} distinct of 64",
            distinct.len()
        );
        let lowest = waits.iter().min().copied().unwrap_or_default();
        let highest = waits.iter().max().copied().unwrap_or_default();
        assert!(
            highest - lowest > 2_800,
            "the waits cover the window (100 ms x 2^5 = 3200 ms) rather than a corner of it: \
             {lowest}..{highest}"
        );
    }

    #[test]
    fn the_window_grows_with_the_attempt_and_stops_at_the_cap() {
        let retry = profile();

        for attempt in 1..=12u32 {
            let window = (retry.base_ms << (attempt - 1)).min(retry.cap_ms);
            for index in 0..32 {
                let delay = retry.delay_for(attempt, &format!("subscriber-{index}"));
                assert!(
                    delay <= window,
                    "the wait stays inside the window: {delay} > {window} at attempt {attempt}"
                );
                assert!(
                    delay <= retry.cap_ms,
                    "and inside the cap: {delay} > {} at attempt {attempt}",
                    retry.cap_ms
                );
            }
        }

        // A loop that keeps failing must not walk off the end of the arithmetic: `base << attempt`
        // is fine at 12 and a panic at 64, and the cap is what makes both of them a wait.
        for attempt in [32, 33, 64, 1_000] {
            let delay = retry.delay_for(attempt, "ada");
            assert!(
                delay <= retry.cap_ms,
                "attempt {attempt} waited {delay} ms: the window saturates at the cap"
            );
        }
        assert_eq!(
            retry.delay_for(0, "ada"),
            0,
            "and the window is not consulted for an attempt that has not failed"
        );
    }

    #[test]
    fn a_run_of_failures_opens_the_breaker_and_the_deliveries_stay_queued() {
        let retry = profile();
        let mut breaker = retry.breaker();

        assert_eq!(breaker.state(0), State::Closed, "it starts closed");
        assert_eq!(breaker.check(0), Verdict::Allowed);

        breaker.record(Outcome::Failure, 10);
        breaker.record(Outcome::Failure, 20);
        assert_eq!(
            breaker.state(30),
            State::Closed,
            "fewer than {} consecutive failures is not a dead endpoint",
            retry.open_after
        );
        assert_eq!(breaker.check(30), Verdict::Allowed);

        breaker.record(Outcome::Failure, 40);
        assert_eq!(
            breaker.state(40),
            State::Open,
            "the failure that completes the run opens it"
        );
        assert_eq!(
            breaker.check(41),
            Verdict::Refused,
            "a refused delivery is one that was not attempted: the endpoint is down, and the \
             worker does not spend an attempt finding out"
        );
        assert_eq!(
            breaker.check(1_039),
            Verdict::Refused,
            "and it stays refused for the whole cooldown"
        );
    }

    #[test]
    fn after_the_cooldown_exactly_one_request_goes_through() {
        let retry = profile();
        let mut breaker = retry.breaker();
        for at in [10, 20, 30] {
            breaker.record(Outcome::Failure, at);
        }
        assert_eq!(
            breaker.state(1_040),
            State::HalfOpen,
            "the cooldown is over"
        );

        assert_eq!(
            breaker.check(1_040),
            Verdict::Allowed,
            "one request probes the target"
        );
        assert_eq!(
            breaker.check(1_040),
            Verdict::Refused,
            "and one is all: the second caller waits for the first one's outcome instead of \
             joining a stampede on a target that may still be down"
        );

        // The probe failed: whatever is there is not coming back yet, and the cooldown starts over
        // rather than handing out one request per tick.
        breaker.record(Outcome::Failure, 1_041);
        assert_eq!(breaker.state(1_042), State::Open);
        assert_eq!(breaker.check(2_000), Verdict::Refused);
        assert_eq!(
            breaker.state(2_041),
            State::HalfOpen,
            "a fresh cooldown, from the probe"
        );

        // This time it worked, and the endpoint is back.
        assert_eq!(breaker.check(2_041), Verdict::Allowed);
        breaker.record(Outcome::Success, 2_050);
        assert_eq!(
            breaker.state(2_060),
            State::Closed,
            "a success resets the breaker: the endpoint answered, so the deliveries may flow \
             again"
        );
        assert_eq!(breaker.check(2_060), Verdict::Allowed);
    }

    #[test]
    fn a_success_resets_the_count_of_consecutive_failures() {
        let retry = profile();
        let mut breaker = retry.breaker();

        breaker.record(Outcome::Failure, 10);
        breaker.record(Outcome::Failure, 20);
        breaker.record(Outcome::Success, 30);
        breaker.record(Outcome::Failure, 40);
        breaker.record(Outcome::Failure, 50);
        assert_eq!(
            breaker.state(60),
            State::Closed,
            "four failures with one success in the middle are two runs of two, not a run of {}: \
             the count is of *consecutive* failures",
            retry.open_after
        );
        assert_eq!(breaker.check(60), Verdict::Allowed);

        breaker.record(Outcome::Failure, 70);
        assert_eq!(
            breaker.state(71),
            State::Open,
            "the third failure of the second run is what opens it"
        );
    }

    #[test]
    fn a_refused_delivery_keeps_the_attempt_it_did_not_spend() {
        let retry = profile();
        let mut breaker = retry.breaker();

        // The endpoint goes down.
        for at in [10, 20, 30] {
            breaker.record(Outcome::Failure, at);
        }

        // Twelve workers ask inside one cooldown. Every one of them is refused, which is twelve
        // deliveries that still have their attempts — the difference between a budget and a loop.
        let refused = (0..12)
            .filter(|worker| breaker.check(100 + *worker) == Verdict::Refused)
            .count();
        assert_eq!(
            refused, 12,
            "an open breaker turns a delivery into a refusal, not a failure"
        );

        // Once the cooldown passes, one of them goes through and the other eleven wait for its
        // outcome rather than all of them finding out the same thing the hard way.
        let allowed = (0..12)
            .filter(|worker| breaker.check(1_100 + *worker) == Verdict::Allowed)
            .count();
        assert_eq!(allowed, 1, "one probe per cooldown, not twelve");
    }
}
