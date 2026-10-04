//! Spec for `jobq/3.12-dedup-window`: a key, and the window that makes it a window.
//!
//! `kiln run` copies this file into the project as `tests/spec_dedup.rs`. Five decisions about what
//! this file is allowed to assert:
//!
//! * **the boundary is a value, and the value is judged first.** [`Dedup`] is where the crate writes
//!   down what a key may be and which side of the instant is which, so the case that pins it needs
//!   no store, no container and no clock it has to move — and a failure there points at the
//!   arithmetic rather than at a backend;
//! * **a repeat inside the window is answered, not refused.** The second request comes back with
//!   the *first* job's identity and the first job's payload, and the queue holds one job. The
//!   second request's payload is never stored, which is why the case sends a different one;
//! * **the window does not move, and it does not round.** A repeat carrying a *longer* window is
//!   still answered by the first request's window, one second before the end it is still a
//!   duplicate, and at exactly `first + window` it is new again — the boundary the probe of this
//!   slot states as `live` meaning `now < first + window`;
//! * **a key is a key of one queue.** The same string asked in two queues produces two jobs, and
//!   two keys in one queue keep two windows: expiry is a property of the row, not of the queue;
//! * **the store's housekeeping is not the answer.** Nothing in this file sweeps the table, and
//!   every boundary case still answers correctly, because the instant a row carries is what makes
//!   it dead — an expired row that is still there changes nothing, and a window of zero is the
//!   limit of that: no deduplication at all.
//!
//! `support::store::backend` builds the store `KILN_BACKEND` names (`memory` when nothing says
//! otherwise), so the fast run judges the in-memory backend and the `conformance@2` gate re-runs
//! this file with `KILN_BACKEND=postgres`: the window is a promise of the *port*, and the two
//! backends keep it with an upsert and with a map. The clock is `jobq/3.8-clock-injection`'s, handed
//! to the store, because "the window has passed" is a statement about time and a case that cannot
//! move time can only wait for it.

mod support;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use jobq::engine::{
    Clock, DEDUP_KEY_MAX, Dedup, Error, Job, JobId, JobState, MemoryStore, PostgresStore, Queue,
    Store, SubmitRequest, WorkerId,
};
use support::store::{Backend, backend};

/// The queue every keyed case in this file uses.
const QUEUE: &str = "emails";
/// The other queue, for the case that says a key belongs to one of them.
const OTHER_QUEUE: &str = "exports";
/// The window the cases open their keys with.
const WINDOW: Duration = Duration::from_secs(60);
/// The instant the fake clocks start at, where no wall clock will reach during the run.
const START: &str = "2030-01-01T00:00:00Z";

/// A clock the case moves by hand.
#[derive(Debug)]
struct FakeClock {
    /// The instant everything measured from it is measured from.
    now: Mutex<DateTime<Utc>>,
}

impl FakeClock {
    /// A clock stopped at `instant` (RFC 3339).
    fn at(instant: &str) -> Arc<Self> {
        Arc::new(Self {
            now: Mutex::new(at(instant)),
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

/// An instant written the way this file reads them.
fn at(text: &str) -> DateTime<Utc> {
    text.parse()
        .unwrap_or_else(|error| panic!("`{text}` is an instant: {error}"))
}

/// A queue, by name.
fn queue(name: &str) -> Queue {
    Queue::parse(name).unwrap_or_else(|error| panic!("`{name}` is a queue name: {error}"))
}

/// A payload of the shape a producer would send.
fn payload(body: &str) -> serde_json::Value {
    serde_json::json!({ "to": body })
}

/// The worker the cases claim with.
fn worker() -> WorkerId {
    WorkerId::new("dedup-suite")
}

/// A request that names its work `key` for `window`.
fn keyed(queue: Queue, body: &str, key: &str, window: Duration) -> SubmitRequest {
    SubmitRequest::new(queue, payload(body))
        .with_dedup_key(key)
        .with_dedup_window(window)
}

/// A store of this run's backend, reading every instant from `clock`.
///
/// The fixture hands out a store by reference and this is a second one over the same backend, the
/// shape `jobq/3.7-leases-and-recovery` established: the cases want `&dyn Store` (and one of them
/// wants to hand clones to tasks), the durable backend's container is the same database either way,
/// and the clock is the case's so that "the window has passed" is something it can do.
async fn store_for(backend: &Backend, clock: &Arc<FakeClock>) -> Arc<dyn Store> {
    let clock = Arc::clone(clock) as Arc<dyn Clock>;
    match backend {
        Backend::Memory(_) => Arc::new(MemoryStore::new().with_clock(clock)),
        Backend::Postgres { container, .. } => Arc::new(
            PostgresStore::connect(&container.url())
                .await
                .expect("the container the fixture started is reachable")
                .with_clock(clock),
        ),
    }
}

/// One submit, as a producer makes it: the answer is the job the request produced, and
/// `Engine::submit` answers that job's identity (`jobq/3.2-public-api-baseline` pinned it), so this
/// is the same promise one layer down.
async fn submit(store: &dyn Store, request: SubmitRequest) -> JobId {
    store
        .enqueue(request)
        .await
        .unwrap_or_else(|error| panic!("the store refused a request the port accepts: {error}"))
        .id()
}

/// What one queue holds that a worker can take.
async fn due(store: &dyn Store, queue: &Queue, limit: usize) -> Vec<Job> {
    store
        .claim(&worker(), queue, limit)
        .await
        .unwrap_or_else(|error| panic!("the store refused a claim: {error}"))
}

/// The window is half-open, and a key is a name the engine can actually write down.
#[test]
fn the_window_is_half_open_and_a_key_is_what_the_engine_can_keep() {
    let start = at(START);
    let key = Dedup::new("signup-1", WINDOW).expect("a plain key is one the engine can keep");
    assert_eq!(key.key(), "signup-1", "the name comes back as it was given");
    assert_eq!(key.window(), WINDOW, "and so does the window");
    assert_eq!(
        key.expiry(start),
        start + TimeDelta::from_std(WINDOW).expect("a window this small fits a `TimeDelta`"),
        "the window closes `window` after the request that took it"
    );

    assert!(
        key.live(start, start + TimeDelta::seconds(59)),
        "one second before the end the key still answers"
    );
    assert!(
        !key.live(start, start + TimeDelta::seconds(60)),
        "at exactly `first + window` it does not: the window is half-open, so `live` means \
         `now < first + window`, and an implementation that read that as `<=` would answer a \
         legitimate repeat for ever after the key was taken"
    );
    assert!(
        !key.live(start, start + TimeDelta::seconds(61)),
        "and it does not come back a second later"
    );

    // A window of zero is a request rather than a mistake: the key is dead the instant it is
    // taken, which is what a request that wants no deduplication asks for.
    let none = Dedup::new("ephemeral", Duration::ZERO).expect("a zero window is a window");
    assert_eq!(none.expiry(start), start, "it closes where it opened");
    assert!(
        !none.live(start, start),
        "so a request that took it at `t` is not a duplicate at `t`"
    );

    // What a key may not be, decided at the value both stores build on: an empty name is every
    // request that forgot to name one, and a long one is a column nobody can index.
    assert!(
        matches!(Dedup::new("", WINDOW), Err(Error::Invalid { .. })),
        "an empty key is refused, not stored as a name every unnamed request would share"
    );
    let oversized = "k".repeat(DEDUP_KEY_MAX + 1);
    assert!(
        matches!(Dedup::new(oversized, WINDOW), Err(Error::Invalid { .. })),
        "a key longer than {DEDUP_KEY_MAX} bytes is refused"
    );
    assert!(
        Dedup::new("k".repeat(DEDUP_KEY_MAX), WINDOW).is_ok(),
        "and one of exactly {DEDUP_KEY_MAX} bytes is not: the bound is the bound"
    );
    assert!(
        matches!(Dedup::new("two\nlines", WINDOW), Err(Error::Invalid { .. })),
        "a key carrying a control character is refused: the name is printed next to the job it \
         named, and a name that can end a line can forge one"
    );
}

/// A repeat inside the window is answered with the first job, and writes nothing.
#[tokio::test]
async fn a_repeat_inside_the_window_answers_the_first_job() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let store = store.as_ref();
    let queue = queue(QUEUE);

    let first = submit(
        store,
        keyed(queue.clone(), "first@example.com", "signup-1", WINDOW),
    )
    .await;

    clock.advance(TimeDelta::seconds(30));
    // A second request that says something *different*: the key is the producer's statement that
    // the two are one piece of work, whatever the message it rebuilt in between looks like.
    let repeat = submit(
        store,
        keyed(queue.clone(), "second@example.com", "signup-1", WINDOW),
    )
    .await;
    assert_eq!(
        repeat,
        first,
        "a repeat inside the window answers the job the first request produced ({} backend): a \
         producer handed a *second* identity cannot tell whether its retry queued another copy of \
         the work it is retrying",
        backend.name()
    );

    let held = due(store, &queue, 10).await;
    assert_eq!(
        held.len(),
        1,
        "and the queue holds one job, so the repeat wrote nothing ({} backend): a row per request \
         is the same duplicate one table further along",
        backend.name()
    );
    assert_eq!(
        held[0].payload(),
        &payload("first@example.com"),
        "the job is the *first* request's, too: the payload carried by a duplicate is never stored"
    );

    // The answer is the row, not a copy taken when the key was written: a repeat after the job has
    // been settled still answers that job, and now says what it has become.
    store
        .complete(&worker(), &first)
        .await
        .expect("the job this case claimed is this worker's to complete");
    assert_eq!(
        submit(
            store,
            keyed(queue.clone(), "third@example.com", "signup-1", WINDOW),
        )
        .await,
        first,
        "still the same job, still inside its window"
    );
    let row = store
        .get(&first)
        .await
        .expect("the store answers a read")
        .expect("the job this case created is there");
    assert_eq!(
        row.state(),
        JobState::Succeeded,
        "and it is read as it is now rather than as it was when the key was taken"
    );
}

/// The window closes at exactly `first + window`, and a repeat does not move it.
#[tokio::test]
async fn the_boundary_is_exactly_the_window_after_the_first_request() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let store = store.as_ref();
    let queue = queue(QUEUE);

    let first = submit(store, keyed(queue.clone(), "first", "signup-1", WINDOW)).await;

    // A repeat that asks for a *much* longer window: the window belongs to the key as it was first
    // taken, so this must change nothing. A store that wrote the window it was handed on the way
    // past would answer for the next ten minutes.
    clock.advance(TimeDelta::seconds(30));
    assert_eq!(
        submit(
            store,
            keyed(queue.clone(), "second", "signup-1", WINDOW * 10),
        )
        .await,
        first,
        "a repeat is answered whatever window it asks for ({} backend)",
        backend.name()
    );

    clock.advance(TimeDelta::seconds(29));
    assert_eq!(
        submit(store, keyed(queue.clone(), "third", "signup-1", WINDOW)).await,
        first,
        "one second before the end the key still answers, so the repeat did not move the boundary"
    );

    clock.advance(TimeDelta::seconds(1));
    let reopened = submit(store, keyed(queue.clone(), "fourth", "signup-1", WINDOW)).await;
    assert_ne!(
        reopened,
        first,
        "at exactly `first + window` the key is new again and the request produces a job of its \
         own ({} backend): the window is half-open, and the row from the first window is dead \
         because of the instant it carries — nothing swept it, and nothing has to",
        backend.name()
    );

    clock.advance(TimeDelta::seconds(1));
    assert_eq!(
        submit(store, keyed(queue.clone(), "fifth", "signup-1", WINDOW)).await,
        reopened,
        "and the new window is stamped from the request that reopened the key, not from the first \
         request"
    );

    assert_eq!(
        due(store, &queue, 10).await.len(),
        2,
        "two windows, two jobs: the expired row is still in the table and it changed no answer"
    );
}

/// A key belongs to one queue, and a window to one key.
#[tokio::test]
async fn a_key_is_a_key_of_one_queue_and_one_window() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let store = store.as_ref();
    let other = queue(OTHER_QUEUE);
    let queue = queue(QUEUE);

    let here = submit(store, keyed(queue.clone(), "here", "signup-1", WINDOW)).await;
    let there = submit(store, keyed(other.clone(), "there", "signup-1", WINDOW)).await;
    assert_ne!(
        there,
        here,
        "the same string asked in another queue is another key ({} backend): a key that answered \
         across queues would tie two queues' work together every time a producer reused a name",
        backend.name()
    );
    assert_eq!(
        submit(store, keyed(queue.clone(), "again", "signup-1", WINDOW)).await,
        here,
        "and the first queue's key is still the key it was"
    );
    assert_eq!(due(store, &queue, 10).await.len(), 1, "one job here");
    assert_eq!(due(store, &other, 10).await.len(), 1, "and one over there");

    // Two keys in one queue keep two windows: expiry is the row's own instant, so a key taken
    // later neither extends nor shortens the one beside it.
    clock.advance(TimeDelta::seconds(30));
    let younger = submit(store, keyed(queue.clone(), "younger", "signup-2", WINDOW)).await;
    assert_ne!(younger, here, "a second name is a second job");

    clock.advance(TimeDelta::seconds(30));
    let reopened = submit(
        store,
        keyed(queue.clone(), "older again", "signup-1", WINDOW),
    )
    .await;
    assert_ne!(
        reopened,
        here,
        "at `first + window` the older key is new again ({})",
        backend.name()
    );
    assert_ne!(
        reopened, younger,
        "and it is not the younger key's job: one queue, two keys, two windows"
    );
    assert_eq!(
        submit(
            store,
            keyed(queue.clone(), "younger again", "signup-2", WINDOW)
        )
        .await,
        younger,
        "while the younger key, taken thirty seconds later, still answers — a single window per \
         queue would have got one of these two wrong"
    );

    // A window of zero is the limit of that: the key is dead the instant it is taken, so two
    // requests carrying it are two jobs.
    let first_zero = submit(
        store,
        keyed(queue.clone(), "zero", "signup-3", Duration::ZERO),
    )
    .await;
    let second_zero = submit(
        store,
        keyed(queue.clone(), "zero", "signup-3", Duration::ZERO),
    )
    .await;
    assert_ne!(
        first_zero,
        second_zero,
        "a zero window is a request that asks for no deduplication at all ({} backend)",
        backend.name()
    );
}

/// Callers racing for one key get one job, and one answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn callers_racing_for_one_key_get_one_job() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;

    // Eight producers that were told the same thing at the same moment — a fan-out that retried, a
    // client that reconnected and sent its request twice — all naming one piece of work. Every one
    // of them gets the *same* identity back and the queue ends up holding one job: what makes that
    // true is that the store decides in one statement (`insert … on conflict … do update … where`)
    // rather than reading and then writing, and this is the case that cannot be passed by a store
    // that reads first: the readers all see nothing, all write, and either one of them fails on the
    // constraint or two jobs come out of it.
    let racers: Vec<_> = (0..8)
        .map(|index| {
            let store = Arc::clone(&store);
            let queue = queue(QUEUE);
            tokio::spawn(async move {
                submit(
                    store.as_ref(),
                    keyed(queue, &format!("racer{index}"), "signup-1", WINDOW),
                )
                .await
            })
        })
        .collect();
    let mut answers = Vec::new();
    for racer in racers {
        answers.push(
            racer
                .await
                .expect("a racing submit is not a task that panics"),
        );
    }
    let first = answers[0];
    assert!(
        answers.iter().all(|answer| *answer == first),
        "all eight racers were answered with one identity ({} backend), got {answers:?}: a racer          that got a fresh identity was told its request queued work that the key had already          produced",
        backend.name()
    );
    assert_eq!(
        due(store.as_ref(), &queue(QUEUE), 10).await.len(),
        1,
        "and one job is queued ({})",
        backend.name()
    );
}

/// A request the engine cannot answer is refused before anything is written.
#[tokio::test]
async fn a_key_the_engine_cannot_keep_is_refused_before_anything_is_written() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let store = store.as_ref();
    let queue = queue(QUEUE);

    // Each of these is a request the store would otherwise accept: the only thing wrong with it is
    // the key, so what is being judged is the refusal rather than the request.
    let refused: Vec<(&str, SubmitRequest)> = vec![
        ("an empty key", keyed(queue.clone(), "body", "", WINDOW)),
        (
            "a key longer than the engine writes down",
            keyed(
                queue.clone(),
                "body",
                &"k".repeat(DEDUP_KEY_MAX + 1),
                WINDOW,
            ),
        ),
        (
            "a key with no window",
            SubmitRequest::new(queue.clone(), payload("body")).with_dedup_key("signup-1"),
        ),
    ];
    for (what, request) in refused {
        let error = store
            .enqueue(request)
            .await
            .expect_err("a request this engine cannot answer is refused");
        assert!(
            matches!(error, Error::Invalid { .. }),
            "{what} must be `Error::Invalid`, got {error:?}"
        );
        assert!(
            due(store, &queue, 10).await.is_empty(),
            "{what} was refused *after* something was written ({})",
            backend.name()
        );
    }

    // A window with no key is not a refusal and not a key either: there is no name to remember, so
    // the request is an ordinary one and two of them are two jobs.
    let windowed =
        |body: &str| SubmitRequest::new(queue.clone(), payload(body)).with_dedup_window(WINDOW);
    let first = submit(store, windowed("body")).await;
    let second = submit(store, windowed("body")).await;
    assert_ne!(
        first,
        second,
        "a window nobody named does not deduplicate anything ({} backend)",
        backend.name()
    );
    assert_eq!(due(store, &queue, 10).await.len(), 2, "two unkeyed jobs");
}
