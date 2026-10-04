//! Spec for `jobq/3.8-clock-injection`: every instant the engine uses comes from the clock it was
//! built with.
//!
//! `kiln run` copies this file into the project as `tests/spec_clock.rs`. Five claims:
//!
//! * **a job becomes due when the clock moves**, not when a test waits;
//! * **a lease expires on the injected clock**, so recovery is a fact rather than a race with the
//!   wall clock of whichever process happens to be asking;
//! * **a renewal moves the deadline**, which is what makes a long attempt safe;
//! * **the default is the system clock**: nothing has to be configured for a process to work;
//! * **the crate says where the wall clock may be read**, so the rule survives the next edit that
//!   would quietly reach for `Utc::now()`.
//!
//! The fake clock below is the whole point of the step: it is the caller the port exists for, and
//! it is written the way a caller writes one — from outside the crate, through the public trait.
//! The clock starts in the future on purpose. A job scheduled an hour after it is *not* due while
//! the real clock says it is, so a store that ignored the injected clock and read the system one
//! would answer "claimable" where this file expects "nothing yet", and the case names it.

use std::sync::{Arc, Mutex, PoisonError};

use std::time::Duration as Lease;

use chrono::{DateTime, TimeDelta, Utc};
use jobq::engine::{Clock, Engine, JobState, MemoryStore, Queue, Store, SubmitRequest, WorkerId};

/// A clock the test moves by hand, starting at an instant no wall clock will reach during the run.
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

    /// The instant it is stopped at.
    fn read(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Move it forward.
    fn advance(&self, by: TimeDelta) {
        let mut now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        *now += by;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        self.read()
    }
}

/// The instant the fake clock starts at: far enough ahead that no wall clock reaches it mid-run.
const START: &str = "2030-01-01T00:00:00Z";

/// The queue every case uses.
fn queue() -> Queue {
    Queue::parse("emails").expect("`emails` is a valid queue name")
}

/// A payload of the shape a producer would send.
fn payload(body: &str) -> serde_json::Value {
    serde_json::json!({ "to": body })
}

/// A store whose instants come from `clock`.
fn store(clock: &Arc<FakeClock>) -> MemoryStore {
    MemoryStore::new().with_clock(Arc::clone(clock) as Arc<dyn Clock>)
}

#[tokio::test]
async fn a_job_becomes_due_when_the_clock_moves_and_not_before() {
    let clock = FakeClock::at(START);
    let engine = Engine::new(store(&clock));
    let queue = queue();
    let worker = WorkerId::new("test-worker");

    engine
        .submit(
            SubmitRequest::new(queue.clone(), payload("alice@example.com"))
                .with_run_at(clock.read() + TimeDelta::hours(1)),
        )
        .await
        .expect("submitting cannot fail");

    let too_early = engine
        .claim(&worker, &queue, 10)
        .await
        .expect("claim cannot fail");
    assert!(
        too_early.is_empty(),
        "the job is due an hour after the store's own clock says it is, so nothing may be claimed \
         yet — a store that reads the system clock instead would hand it out immediately"
    );

    clock.advance(TimeDelta::minutes(61));

    let due = engine
        .claim(&worker, &queue, 10)
        .await
        .expect("claim cannot fail");
    assert_eq!(
        due.len(),
        1,
        "the clock moved past `run_at`, so the job is due: if this is still empty, the store is \
         not reading the clock it was built with"
    );
}

#[tokio::test]
async fn a_lease_expires_on_the_injected_clock() {
    let clock = FakeClock::at(START);
    let engine = Engine::new(store(&clock).with_lease(Lease::from_secs(60)));
    let queue = queue();
    let worker = WorkerId::new("test-worker");

    let id = engine
        .submit(SubmitRequest::new(
            queue.clone(),
            payload("bob@example.com"),
        ))
        .await
        .expect("submitting cannot fail");
    engine
        .claim(&worker, &queue, 1)
        .await
        .expect("claim cannot fail");

    let reclaimed = engine
        .store()
        .reclaim_expired()
        .await
        .expect("reclaiming cannot fail");
    assert_eq!(
        reclaimed, 0,
        "the lease is sixty seconds from the injected now"
    );

    clock.advance(TimeDelta::seconds(61));
    let reclaimed = engine
        .store()
        .reclaim_expired()
        .await
        .expect("reclaiming cannot fail");
    assert_eq!(
        reclaimed, 1,
        "the lease expired on the clock the store was built with: a store stamping or reading the \
         system clock instead would report nothing to recover"
    );

    let recovered = engine.inspect(&id).await.expect("the job is still there");
    assert_eq!(recovered.state(), JobState::Queued);
    assert!(
        recovered.attempts() >= 1,
        "the attempt that was abandoned is still counted: recovery is not a free retry"
    );
}

#[tokio::test]
async fn a_renewed_lease_survives_the_instant_it_would_have_lapsed() {
    let clock = FakeClock::at(START);
    let engine = Engine::new(store(&clock).with_lease(Lease::from_secs(60)));
    let queue = queue();
    let worker = WorkerId::new("test-worker");

    engine
        .submit(SubmitRequest::new(
            queue.clone(),
            payload("carol@example.com"),
        ))
        .await
        .expect("submitting cannot fail");
    engine
        .claim(&worker, &queue, 1)
        .await
        .expect("claim cannot fail");

    clock.advance(TimeDelta::seconds(30));
    let renewed = engine
        .store()
        .renew(&worker)
        .await
        .expect("renewing cannot fail");
    assert_eq!(
        renewed, 1,
        "the worker holds one job, so one lease is extended"
    );

    // Past the instant the *first* lease would have lapsed, before the renewed one does.
    clock.advance(TimeDelta::seconds(40));
    let reclaimed = engine
        .store()
        .reclaim_expired()
        .await
        .expect("reclaiming cannot fail");
    assert_eq!(
        reclaimed, 0,
        "a lease that is renewed on a deadline that is *moved* survives this instant; one that \
         merely refreshes a different field would not"
    );
}

#[tokio::test]
async fn the_system_clock_is_the_default_and_needs_no_configuration() {
    // The same engine the earlier steps build: no clock is passed, and a job with no `run_at` is
    // due immediately. This is the case that keeps `jobq/3.1-engine-boundary` and every later spec
    // true — injection must not make the ordinary path require an injection.
    let engine = Engine::new(MemoryStore::new());
    let queue = queue();
    let worker = WorkerId::new("test-worker");

    engine
        .submit(SubmitRequest::new(
            queue.clone(),
            payload("dave@example.com"),
        ))
        .await
        .expect("submitting cannot fail");
    let claimed = engine
        .claim(&worker, &queue, 1)
        .await
        .expect("claim cannot fail");
    assert_eq!(
        claimed.len(),
        1,
        "a job with no instant is due as soon as somebody looks"
    );
}

#[test]
fn the_crate_keeps_the_wall_clock_in_one_place() {
    // The behavioural half is above (a store that ignored the injected clock fails those cases).
    // This half is what keeps them true: `clippy@3` denies `disallowed_methods` on library code and
    // the list lives in the project's own `clippy.toml`, so a new module that reads `Utc::now()`
    // is refused by the compiler rather than by a reviewer's memory. The check here is that the
    // list is there and names the wall clock — the compiler is the judge, this is the pointer.
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("clippy.toml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} could not be read ({error}): the rule that only the clock port reads the wall \
             clock is enforced by `clippy@3` through this file (authoring.md §6.6: a structural \
             fact, with the reason written here)",
            path.display()
        )
    });
    for banned in ["chrono::Utc::now", "std::time::SystemTime::now"] {
        assert!(
            text.contains(banned),
            "`clippy.toml` must refuse {banned:?} for library code: without it, the next module \
             that wants the time reads the wall clock and the injected one stops being the only \
             authority: {text}"
        );
    }
}
