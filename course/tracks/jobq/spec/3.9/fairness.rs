//! Spec for `jobq/3.9-priority-fairness`: who waits, and for how long.
//!
//! `kiln run` copies this file into the project as `tests/spec_fairness.rs`. Four claims:
//!
//! * **a queue that keeps refilling with high-priority work cannot starve the rest** — a job that
//!   has waited long enough wins against fresh work, and the case says by how much;
//! * **a cap is a cap** — one queue may not hold more than its share of the pool, and the jobs over
//!   it stay queued rather than failing;
//! * **strict priority is still the default** — every spec written before this step answers exactly
//!   as it did, because fairness is something a deployment asks for (that is what makes this step
//!   additive, and the case below is what keeps it that way);
//! * **the daemon says which policy it will apply** — a knob nobody can read in `--check-config` is
//!   a knob that will be discovered in production.
//!
//! The aging cases use the clock of `jobq/3.8-clock-injection`, because "has waited long enough" is
//! a statement about time and a test that cannot move time can only wait for it.

use std::process::Command;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration as Lease;

use chrono::{DateTime, TimeDelta, Utc};
use jobq::engine::{Clock, Engine, JobState, MemoryStore, Policy, Queue, SubmitRequest, WorkerId};

/// A clock the test moves by hand, starting where no wall clock will reach during the run.
#[derive(Debug)]
struct FakeClock {
    /// The instant everything below is measured from.
    now: Mutex<DateTime<Utc>>,
}

impl FakeClock {
    /// A clock stopped at `instant` (RFC 3339).
    fn at(instant: &str) -> Arc<Self> {
        let at: DateTime<Utc> = instant
            .parse()
            .unwrap_or_else(|error| panic!("`{instant}` is not an instant: {error}"));
        Arc::new(Self {
            now: Mutex::new(at),
        })
    }

    /// Move it forward.
    fn advance(&self, by: TimeDelta) {
        let mut now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        *now += by;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The instant the fake clock starts at.
const START: &str = "2030-01-01T00:00:00Z";

/// A queue, by name.
fn queue(name: &str) -> Queue {
    Queue::parse(name).unwrap_or_else(|error| panic!("`{name}` is a queue name: {error}"))
}

/// A payload of the shape a producer would send.
fn payload(body: &str) -> serde_json::Value {
    serde_json::json!({ "to": body })
}

/// A store with `policy` and the test's clock.
fn store(clock: &Arc<FakeClock>, policy: Policy) -> MemoryStore {
    MemoryStore::new()
        .with_clock(Arc::clone(clock) as Arc<dyn Clock>)
        .with_policy(policy)
}

#[tokio::test]
async fn a_queue_that_keeps_refilling_cannot_starve_the_rest() {
    let clock = FakeClock::at(START);
    // One level per minute of waiting: a job that has been due for twenty-one minutes outranks a
    // *fresh* job submitted ten levels above it.
    let policy = Policy::strict().with_aging(Lease::from_secs(60));
    let engine = Engine::new(store(&clock, policy));
    let queue = queue("exports");
    let worker = WorkerId::new("test-worker");

    let starved = engine
        .submit(SubmitRequest::new(queue.clone(), payload("old@example.com")).with_priority(-10))
        .await
        .expect("submitting cannot fail");

    // The wait comes first and the flood second, which is the shape of the failure this step is
    // about: a queue that keeps refilling with *new* high-priority work. Aging every job at the same
    // rate would change nothing (both sides rise together), so the scenario has to be the real one.
    clock.advance(TimeDelta::minutes(21));
    for index in 0..20 {
        engine
            .submit(
                SubmitRequest::new(queue.clone(), payload(&format!("fresh{index}@example.com")))
                    .with_priority(10),
            )
            .await
            .expect("submitting cannot fail");
    }

    let claimed = engine
        .claim(&worker, &queue, 1)
        .await
        .expect("claim cannot fail");
    assert_eq!(
        claimed.len(),
        1,
        "the queue has due work, so a turn returns a job"
    );
    assert_eq!(
        claimed[0].id(),
        starved,
        "the job that has waited twenty-one minutes outranks the fresh ones ten levels above it \
         (rank 11 against 10): without aging the flood wins and the quiet queue waits forever"
    );
}

#[tokio::test]
async fn a_cap_keeps_one_queue_from_holding_the_whole_pool() {
    let clock = FakeClock::at(START);
    let policy = Policy::strict().with_cap(2);
    let engine = Engine::new(store(&clock, policy));
    let queue = queue("emails");
    let worker = WorkerId::new("test-worker");

    for index in 0..5 {
        engine
            .submit(SubmitRequest::new(
                queue.clone(),
                payload(&format!("user{index}")),
            ))
            .await
            .expect("submitting cannot fail");
    }

    let claimed = engine
        .claim(&worker, &queue, 10)
        .await
        .expect("claim cannot fail");
    assert_eq!(
        claimed.len(),
        2,
        "the queue may hold two jobs running: a claim of ten must come back with the cap's worth, \
         not with ten"
    );

    // The ones over the cap are not failed and not lost: they are queued, with their attempts
    // untouched, which is what a later claim and a resumed worker both depend on.
    engine
        .complete(&worker, &claimed[0].id())
        .await
        .expect("completing a claimed job cannot fail");
    let next = engine
        .claim(&worker, &queue, 10)
        .await
        .expect("claim cannot fail");
    assert_eq!(next.len(), 1, "one slot freed, one job admitted");

    for job in &claimed[1..] {
        let running = engine
            .inspect(&job.id())
            .await
            .expect("the job is still there");
        assert_eq!(running.state(), JobState::Running);
    }
    let waiting = engine
        .claim(&worker, &queue, 10)
        .await
        .expect("claim cannot fail");
    assert!(
        waiting.is_empty(),
        "both slots are taken again, so nothing else may be claimed"
    );
}

#[tokio::test]
async fn strict_priority_is_still_what_a_store_does_until_it_is_asked_otherwise() {
    // The regression that keeps this step additive: the engine's default answers exactly as it did
    // before the policy existed, which is what `jobq/3.5-store-port-conformance` asserts about order
    // and what every other spec relies on.
    let engine = Engine::new(MemoryStore::new());
    let queue = queue("emails");
    let worker = WorkerId::new("test-worker");

    let waited = engine
        .submit(SubmitRequest::new(queue.clone(), payload("old")).with_priority(-5))
        .await
        .expect("submitting cannot fail");
    // The same shape as the aging case above, and the opposite answer: with strict priority, how
    // long a job has waited changes nothing at all.
    let urgent = engine
        .submit(SubmitRequest::new(queue.clone(), payload("urgent")).with_priority(5))
        .await
        .expect("submitting cannot fail");

    let claimed = engine
        .claim(&worker, &queue, 1)
        .await
        .expect("claim cannot fail");
    assert_eq!(claimed.len(), 1);
    assert_eq!(
        claimed[0].id(),
        urgent,
        "without a policy the highest priority goes first however long the other job has waited — \
         that is the starvation the aging case measures, and the reason this default must not move \
         on an upgrade"
    );
    assert_ne!(claimed[0].id(), waited);
}

#[test]
fn the_daemon_reports_the_policy_it_will_apply() {
    let output = Command::new(env!("CARGO_BIN_EXE_jobqd"))
        .arg("--check-config")
        .env_clear()
        .env("JOBQ__WORKER__AGING_SECONDS", "60")
        .env("JOBQ__WORKER__MAX_IN_FLIGHT_PER_QUEUE", "4")
        .output()
        .unwrap_or_else(|error| panic!("jobqd --check-config could not be started: {error}"));
    assert!(
        output.status.success(),
        "`--check-config` must answer with the effective settings, exit {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8_lossy(&output.stdout).into_owned();
    for expected in [
        "worker.aging_seconds = 60",
        "worker.max_in_flight_per_queue = 4",
    ] {
        assert!(
            report.contains(expected),
            "the report must name {expected:?}: the fairness policy is a setting, and a setting \
             nobody can read is one that is discovered from the pager: {report}"
        );
    }
    // The default the daemon ships: aging on, cap off. A file that does not mention either knob
    // still gets the policy the body argues for.
    let default = Command::new(env!("CARGO_BIN_EXE_jobqd"))
        .arg("--check-config")
        .env_clear()
        .output()
        .unwrap_or_else(|error| panic!("jobqd --check-config could not be started: {error}"));
    let default = String::from_utf8_lossy(&default.stdout).into_owned();
    assert!(
        default.contains("worker.aging_seconds = 300"),
        "aging is on by default, at the interval the step's body argues for: {default}"
    );
    assert!(
        default.contains("worker.max_in_flight_per_queue = 0"),
        "and the cap is off by default: a cap nobody chose looks like a stuck queue: {default}"
    );
}
