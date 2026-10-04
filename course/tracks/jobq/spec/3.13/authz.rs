//! Spec for `jobq/3.13-queue-authz-attribution`: who may touch a queue, and what it consumed.
//!
//! `kiln run` copies this file into the project as `tests/spec_authz.rs`. Four decisions about what
//! this file is allowed to assert:
//!
//! * **the model is data, and it is judged as data first.** [`QueueDeclaration`] is where the three
//!   actions and the widening rule live, so the case that pins them needs no store and no container:
//!   the owner holds every action, a grant holds what it names, a wider grant covers a narrower one,
//!   and a name that appears anywhere in the model can see the queue;
//! * **a refusal does not leak existence.** A tenant asking about somebody else's queue and the same
//!   tenant asking about a queue nobody ever declared get the *same bytes back* — the case compares
//!   the two answers before and after the queue is declared — and a job the caller may not see is
//!   answered exactly like a job that is not there (`Ok(None)` from the port, `NotFound` from the
//!   engine). That is `relay/2.16-authz-audit`'s rule ("`403` is about an action, never about a
//!   row"), kept where the query is: the filter is in the statements, not in a check somebody has to
//!   remember;
//! * **the counters are the answer to "how much", and the arithmetic is exact.** A scripted workload
//!   through one queue produces five numbers and no ambiguity — including the case where an
//!   attempt ends by being reclaimed rather than settled — and a *refused* settle moves nothing,
//!   because the counter is written in the transaction that changes the state;
//! * **a privileged change is one commit with its row.** The trail names who moved the queue and
//!   whose it is now, a refused change leaves nothing, and a change whose row cannot be written does
//!   not happen — that last one is about a *transaction*, so it starts a container of its own and
//!   says so at the case (which is also why this step declares an image: the durable backend's
//!   atomicity is not something the in-memory one can be asked about, because nothing outside a
//!   store can make its next write fail).
//!
//! `support::store::backend` builds the store `KILN_BACKEND` names (`memory` when nothing says
//! otherwise). The clock is `jobq/3.8-clock-injection`'s, handed to the store, because a lease that
//! runs out is one of the five numbers and a case that cannot move time can only wait for it.

mod support;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use jobq::engine::{
    Clock, Error, Job, JobId, MemoryStore, PostgresStore, Queue, QueueAction, QueueDeclaration,
    QueueUsage, Store, SubmitRequest, TENANT_NAME_MAX, Tenant, Usage, WorkerId,
};
use sqlx::postgres::PgPoolOptions;
use support::postgres::Postgres;
use support::store::{Backend, backend};

/// The queue the tenant cases declare.
const QUEUE: &str = "signup";
/// A second queue of the same tenant, for the case about the trail's order.
const OTHER_QUEUE: &str = "exports";
/// A queue nobody declares, for the refusal that has to look like the other refusal.
const GHOST_QUEUE: &str = "nobody-owns-this";
/// How long a claim lasts in this file: a lease that runs out is one of the five counters, and the
/// case that watches it moves the clock rather than waiting.
const LEASE: Duration = Duration::from_secs(1);
/// The instant every case starts at.
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
        let at: DateTime<Utc> = instant
            .parse()
            .unwrap_or_else(|error| panic!("`{instant}` is an instant: {error}"));
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

/// A queue, by name.
fn queue(name: &str) -> Queue {
    Queue::parse(name).unwrap_or_else(|error| panic!("`{name}` is a queue name: {error}"))
}

/// A tenant, by name.
fn tenant(name: &str) -> Tenant {
    Tenant::parse(name).unwrap_or_else(|error| panic!("`{name}` is a tenant name: {error}"))
}

/// A payload of the shape a producer would send.
fn payload(body: &str) -> serde_json::Value {
    serde_json::json!({ "to": body })
}

/// The worker every case claims with.
fn worker() -> WorkerId {
    WorkerId::new("authz-suite")
}

/// A store of this run's backend, reading every instant from `clock` and leasing for [`LEASE`].
async fn store_for(backend: &Backend, clock: &Arc<FakeClock>) -> Arc<dyn Store> {
    let clock = Arc::clone(clock) as Arc<dyn Clock>;
    match backend {
        Backend::Memory(_) => Arc::new(MemoryStore::new().with_clock(clock).with_lease(LEASE)),
        Backend::Postgres { container, .. } => Arc::new(
            PostgresStore::connect(&container.url())
                .await
                .expect("the container the fixture started is reachable")
                .with_clock(clock)
                .with_lease(LEASE),
        ),
    }
}

/// One submit through the port, with a failure the case can read.
async fn submit(store: &dyn Store, queue: &Queue, body: &str) -> Result<JobId, Error> {
    submit_ranked(store, queue, body, 0).await
}

/// One submit at a rank of its own, so that the order a case reads is the engine's policy rather
/// than the tie-break between three jobs written at the same instant of a clock that does not move.
async fn submit_ranked(
    store: &dyn Store,
    queue: &Queue,
    body: &str,
    priority: i32,
) -> Result<JobId, Error> {
    store
        .enqueue(SubmitRequest::new(queue.clone(), payload(body)).with_priority(priority))
        .await
        .map(|job| job.id())
}

/// One claimed job, with a failure a case can read.
///
/// The cases here claim a limit of one from a queue they know has work, so anything else is the
/// store answering a question nobody asked.
async fn claim_one(store: &dyn Store, queue: &Queue) -> Job {
    let mut claimed = store
        .claim(&worker(), queue, 1)
        .await
        .unwrap_or_else(|error| panic!("claiming cannot fail: {error}"));
    claimed
        .pop()
        .unwrap_or_else(|| panic!("`{queue}` has work, so claiming one answers one job"))
}

/// The model is data: three actions, a widening rule, and visibility that is not a fourth action.
#[test]
fn the_model_is_data_and_a_wider_grant_covers_a_narrower_one() {
    let queue = queue(QUEUE);
    let acme = tenant("acme");
    let bob = tenant("bob");
    let carol = tenant("carol");

    let model = QueueDeclaration::new(queue.clone(), acme.clone());
    assert_eq!(model.queue(), &queue, "the model is about one queue");
    assert_eq!(model.owner(), &acme, "and it names exactly one owner");
    assert!(model.grants().is_empty(), "with nobody else in it");
    for action in [
        QueueAction::Submit,
        QueueAction::Claim,
        QueueAction::Administer,
    ] {
        assert!(
            model.allows(&acme, action),
            "the owner may `{action}`: an owner that could not administer its own queue would need \
             a grant on it, and the model would have two ways to say the same thing"
        );
    }
    assert!(model.visible_to(&acme), "so the queue exists for it");
    assert!(
        !model.visible_to(&bob) && !model.allows(&bob, QueueAction::Submit),
        "and for nobody else, in either sense"
    );

    // A grant is visibility *and* permission: the moment a name is in the model at all, the queue is
    // a queue that tenant can ask about — which is what makes a refusal distinguishable from a
    // refusal about a queue that does not exist.
    let model = model.with_grant(bob.clone(), QueueAction::Submit);
    assert!(
        model.visible_to(&bob),
        "a tenant the model names can see the queue"
    );
    assert!(model.allows(&bob, QueueAction::Submit), "and may submit");
    assert!(
        !model.allows(&bob, QueueAction::Claim),
        "but not claim: `submit` is the narrowest grant there is, and a store that read it as \
         \"somebody\" would hand the work to whoever could put it there"
    );
    assert!(!model.allows(&bob, QueueAction::Administer));

    let model = model.with_grant(carol.clone(), QueueAction::Administer);
    for action in [
        QueueAction::Submit,
        QueueAction::Claim,
        QueueAction::Administer,
    ] {
        assert!(
            model.allows(&carol, action),
            "`administer` covers `{action}`: one direction rather than a table of exceptions, the \
             rule `relay/2.16-authz-audit`'s matrix is written with"
        );
    }
    assert_eq!(
        model.grants().len(),
        2,
        "the grants are readable as data: two rows, which is what a reviewer of an access model \
         reads and what the migration stores"
    );

    // The three action names are the ones the schema stores and the refusal prints, so they are
    // pinned here rather than left to whoever writes the `check` constraint.
    assert_eq!(QueueAction::Submit.as_str(), "submit");
    assert_eq!(QueueAction::Claim.as_str(), "claim");
    assert_eq!(QueueAction::Administer.as_str(), "administer");

    // A tenant name is a name the engine can write down, like a queue's: same alphabet, same reason.
    assert_eq!(tenant("acme").as_str(), "acme");
    assert!(
        Tenant::parse("").is_err(),
        "an empty tenant name is refused"
    );
    assert!(
        Tenant::parse("Acme").is_err(),
        "so is one outside the alphabet"
    );
    assert!(
        Tenant::parse(&"t".repeat(TENANT_NAME_MAX + 1)).is_err(),
        "and one longer than the bound"
    );
    assert!(
        Tenant::parse(&"t".repeat(TENANT_NAME_MAX)).is_ok(),
        "while the bound itself is a name"
    );
}

/// A tenant sees its own queues, and somebody else's is answered like nobody's.
#[tokio::test]
async fn a_tenant_sees_its_own_queue_and_nothing_else() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let acme = store.scoped(&tenant("acme"));
    let bob = store.scoped(&tenant("bob"));
    let ghost = queue(GHOST_QUEUE);
    let queue = queue(QUEUE);

    // Before anything is declared, the queue is nobody's — and this is the answer the case below
    // compares against.
    let before = bob
        .claim(&worker(), &queue, 1)
        .await
        .expect_err("a queue nobody declared is not claimable");
    assert!(
        matches!(before, Error::NoQueue { .. }),
        "a queue no model names is `Error::NoQueue` ({} backend), got {before:?}",
        backend.name()
    );

    acme.declare_queue(&QueueDeclaration::new(queue.clone(), tenant("acme")))
        .await
        .expect("the owner declares its own queue");

    let after = bob
        .claim(&worker(), &queue, 1)
        .await
        .expect_err("another tenant's queue is not claimable either");
    assert_eq!(
        after.to_string(),
        before.to_string(),
        "the refusal for somebody else's queue is the refusal a queue nobody declared gives, byte \
         for byte ({} backend): a refusal that differed would answer the one question a tenant that \
         is guessing has — does this queue exist",
        backend.name()
    );
    for refusal in [
        bob.enqueue(SubmitRequest::new(queue.clone(), payload("bob")))
            .await
            .expect_err("nor is it submittable"),
        bob.claim(&worker(), &ghost, 1)
            .await
            .expect_err("nor is a queue nobody has"),
    ] {
        assert!(
            matches!(refusal, Error::NoQueue { .. }),
            "expected `Error::NoQueue`, got {refusal:?}"
        );
    }

    // The owner's own handle works, which is what makes the refusal above a statement about the
    // caller rather than about the queue.
    let id = submit(acme.as_ref(), &queue, "alice@example.com")
        .await
        .expect("the owner may submit to its own queue");

    // And a job is refused the same way an identity nobody ever wrote is: the port answers `None`
    // where the engine answers `NotFound`, which is what `jobq/3.6-transaction-isolation` already
    // promised for a job that is not there.
    assert!(
        bob.get(&id)
            .await
            .expect("a read cannot fail here")
            .is_none(),
        "another tenant's job does not exist as far as this handle is concerned"
    );
    assert!(
        bob.get(&JobId::new())
            .await
            .expect("a read cannot fail here")
            .is_none(),
        "and neither does one nobody wrote: same answer, same bytes"
    );
    assert!(
        bob.events(&id)
            .await
            .expect("a read cannot fail here")
            .is_empty(),
        "the trail of somebody else's job is empty, exactly as the trail of a job nobody wrote is"
    );
    let settled = bob
        .complete(&worker(), &id)
        .await
        .expect_err("nor may it settle what it cannot see");
    assert!(
        matches!(settled, Error::NotFound { .. }),
        "a settle of a foreign job is `Error::NotFound` ({} backend), got {settled:?}: the caller \
         holding an identity it may not use is in exactly the position of one holding an identity \
         that does not exist",
        backend.name()
    );
    assert!(
        acme.get(&id)
            .await
            .expect("a read cannot fail here")
            .is_some(),
        "while the owner sees the same row: the refusal is about the caller"
    );
}

/// A grant is what lets somebody else work in your queue — and only what it says.
#[tokio::test]
async fn a_grant_is_what_lets_somebody_else_work_in_your_queue() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let queue = queue(QUEUE);
    let acme = tenant("acme");
    let bob = tenant("bob");
    let carol = tenant("carol");
    let acme_store = store.scoped(&acme);
    let bob_store = store.scoped(&bob);

    acme_store
        .declare_queue(
            &QueueDeclaration::new(queue.clone(), acme.clone())
                .with_grant(bob.clone(), QueueAction::Submit),
        )
        .await
        .expect("an owner grants a producer");

    let id = submit(bob_store.as_ref(), &queue, "bob@example.com")
        .await
        .expect("a producer may put work in");
    assert!(
        bob_store
            .get(&id)
            .await
            .expect("a read cannot fail here")
            .is_some(),
        "and can read what it wrote: a name the model holds sees the queue's jobs"
    );

    let refused = bob_store
        .claim(&worker(), &queue, 1)
        .await
        .expect_err("a producer is not a consumer");
    assert!(
        matches!(refused, Error::Forbidden { .. }),
        "a queue this caller *can* see has only the action left to refuse with: expected \
         `Error::Forbidden`, got {refused:?} ({} backend)",
        backend.name()
    );
    let refused = bob_store
        .declare_queue(&QueueDeclaration::new(queue.clone(), acme.clone()))
        .await
        .expect_err("and not an administrator either");
    assert!(
        matches!(refused, Error::Forbidden { .. }),
        "`submit` does not reach `administer`: expected `Error::Forbidden`, got {refused:?}",
    );

    // An administrator changes the *model*, keeping the queue where it is: handing a queue to
    // somebody else is the operator's door, and the case below proves that half by trying it.
    acme_store
        .declare_queue(
            &QueueDeclaration::new(queue.clone(), acme.clone())
                .with_grant(bob.clone(), QueueAction::Administer),
        )
        .await
        .expect("the owner delegates administration");
    bob_store
        .declare_queue(
            &QueueDeclaration::new(queue.clone(), acme.clone())
                .with_grant(bob.clone(), QueueAction::Administer)
                .with_grant(carol.clone(), QueueAction::Claim),
        )
        .await
        .expect("an administrator may change the grants without changing the owner");
    let handed_over = bob_store
        .declare_queue(&QueueDeclaration::new(queue.clone(), bob.clone()))
        .await
        .expect_err("but may not take the queue");
    assert!(
        matches!(handed_over, Error::Forbidden { .. }),
        "a transfer is `Error::Forbidden` for a tenant: expected it, got {handed_over:?}"
    );

    let claimed = store
        .scoped(&carol)
        .claim(&worker(), &queue, 1)
        .await
        .expect("the tenant the model grants `claim` may claim");
    assert_eq!(
        claimed.len(),
        1,
        "and the job the producer wrote is the one it takes"
    );
    assert_eq!(
        store
            .scoped(&carol)
            .usage()
            .await
            .expect("a read cannot fail here")
            .len(),
        0,
        "while the numbers stay with the queue's owner: `usage` is a tenant's *own* queues, and \
         carol owns none"
    );
}

/// A scripted workload produces exactly the expected attribution.
#[tokio::test]
async fn the_counters_add_up_for_a_scripted_workload() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let other = queue(OTHER_QUEUE);
    let ghost = queue(GHOST_QUEUE);
    let queue = queue(QUEUE);
    let acme = tenant("acme");
    let acme_store = store.scoped(&acme);

    acme_store
        .declare_queue(&QueueDeclaration::new(queue.clone(), acme.clone()))
        .await
        .expect("the owner declares its queue");
    acme_store
        .declare_queue(&QueueDeclaration::new(other.clone(), acme.clone()))
        .await
        .expect("and a second one, which nothing will be written to");
    store
        .scoped(&tenant("bob"))
        .declare_queue(&QueueDeclaration::new(ghost.clone(), tenant("bob")))
        .await
        .expect("another tenant declares its own");

    // Three jobs, ranked so that the order this case reads is the engine's policy rather than the
    // tie-break between three rows written at one instant of a clock that does not move: every
    // claim below is deterministic, which is what lets the numbers be asserted exactly.
    for (body, rank) in [("one", 0), ("two", -1), ("three", -2)] {
        submit_ranked(acme_store.as_ref(), &queue, body, rank)
            .await
            .expect("the owner submits");
    }

    let first = claim_one(acme_store.as_ref(), &queue).await;
    assert_eq!(first.payload(), &payload("one"), "the highest rank first");
    acme_store
        .complete(&worker(), &first.id())
        .await
        .expect("the first attempt finishes");

    // A failure that goes back to the queue does so at the rank the job was submitted with: the
    // retry the case reads next is the same job, because a retry is not a demotion.
    let second = claim_one(acme_store.as_ref(), &queue).await;
    assert_eq!(second.payload(), &payload("two"));
    acme_store
        .fail(&worker(), &second.id(), "the sink refused it")
        .await
        .expect("a failed attempt is recorded");
    let retried = claim_one(acme_store.as_ref(), &queue).await;
    assert_eq!(
        retried.id(),
        second.id(),
        "the job that failed is ahead of the one that has not run yet, because nothing moved it \
         down the order"
    );
    acme_store
        .complete(&worker(), &retried.id())
        .await
        .expect("the retry finishes");

    // The third ending: an attempt nobody settles. Its lease runs out and the reclaimer takes it
    // back, which is one of the five numbers and the one a case cannot see without moving time.
    let abandoned = claim_one(acme_store.as_ref(), &queue).await;
    assert_eq!(abandoned.payload(), &payload("three"));
    clock.advance(TimeDelta::seconds(2));
    assert_eq!(
        acme_store
            .reclaim_expired()
            .await
            .expect("the reclaimer answers"),
        1,
        "the attempt whose lease ran out is taken back, and that is a number an operator reads"
    );
    let recovered = claim_one(acme_store.as_ref(), &queue).await;
    assert_eq!(
        recovered.id(),
        abandoned.id(),
        "the reclaimed job is the same job, queued again"
    );
    acme_store
        .complete(&worker(), &recovered.id())
        .await
        .expect("its second attempt finishes");

    // A refused settle moves nothing, and this is the half of "the counter is written in the same
    // transaction as the state change" that a case can see: completing one job twice is refused
    // (`jobq/3.6-transaction-isolation`), and the numbers must not have noticed the attempt.
    acme_store
        .complete(&worker(), &first.id())
        .await
        .expect_err("a job is completed once");
    assert_eq!(
        numbers_of(acme_store.as_ref(), &queue).await,
        (3, 5, 3, 1, 1),
        "submitted, claimed, succeeded, failed and reclaimed for the whole script ({} backend): \
         three jobs, five attempts, and one attempt on each of the three ways an attempt can end. A \
         counter written outside the transaction that changed the row would count the refused \
         completion too",
        backend.name()
    );
    assert_eq!(
        usage(acme_store.as_ref(), &queue).await.running(),
        0,
        "and nothing is left running: `running` is the difference between what was claimed and the \
         three ways an attempt ends, so a queue whose work has all finished adds up to zero"
    );

    // The report is the tenant's own queues, in queue order, including one nothing ever happened to
    // — and never somebody else's, however busy it is.
    let named: Vec<String> = acme_store
        .usage()
        .await
        .expect("a read cannot fail here")
        .iter()
        .map(|line| line.queue().as_str().to_owned())
        .collect();
    assert_eq!(
        named,
        [OTHER_QUEUE, QUEUE],
        "`{acme}`'s numbers are its own two queues"
    );
    assert_eq!(
        numbers_of(acme_store.as_ref(), &other).await,
        (0, 0, 0, 0, 0),
        "a declared queue that has never been used answers zeros rather than no line: a queue an \
         operator just created is a queue it can watch"
    );
    assert!(
        store
            .usage()
            .await
            .expect("a read cannot fail here")
            .iter()
            .any(|line| line.queue().as_str() == GHOST_QUEUE),
        "while the operator's door sees every queue it could be asked about, the other tenant's \
         included: this is the report \"who consumed the worker pool last night\" is answered from"
    );
}

/// A usage line's five numbers, in the order the whole file reads them.
async fn numbers_of(store: &dyn Store, queue: &Queue) -> (u64, u64, u64, u64, u64) {
    let usage = usage(store, queue).await;
    (
        usage.submitted(),
        usage.claimed(),
        usage.succeeded(),
        usage.failed(),
        usage.reclaimed(),
    )
}

/// The numbers one queue's line carries.
async fn usage(store: &dyn Store, queue: &Queue) -> Usage {
    let lines: Vec<QueueUsage> = store.usage().await.expect("a read cannot fail here");
    lines
        .iter()
        .find(|line| line.queue() == queue)
        .unwrap_or_else(|| panic!("`{queue}` has a line"))
        .usage()
}

/// A privileged change leaves a row, a refused one leaves nothing, and the row names both parties.
#[tokio::test]
async fn a_privileged_change_leaves_a_row_and_a_refused_one_leaves_none() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let acme = tenant("acme");
    let bob = tenant("bob");
    let acme_store = store.scoped(&acme);
    let other = queue(OTHER_QUEUE);
    let queue = queue(QUEUE);

    acme_store
        .declare_queue(&QueueDeclaration::new(queue.clone(), acme.clone()))
        .await
        .expect("the owner declares a queue");
    // A second decision a second later, so that "newest first" is a fact about instants rather than
    // about the tie-break two rows written in the same microsecond would need.
    clock.advance(TimeDelta::seconds(1));
    acme_store
        .declare_queue(&QueueDeclaration::new(other.clone(), acme.clone()))
        .await
        .expect("and a second queue, later");

    let trail = acme_store.audit(10).await.expect("a read cannot fail here");
    assert_eq!(
        trail.len(),
        2,
        "one line per accepted change of the access model ({} backend)",
        backend.name()
    );
    assert_eq!(
        trail
            .iter()
            .map(|line| line.queue().as_str())
            .collect::<Vec<&str>>(),
        [OTHER_QUEUE, QUEUE],
        "newest first: the question the trail exists for is \"who moved this, and when\", and its \
         answer is the end of the story"
    );
    assert_eq!(trail[0].actor(), &acme, "the line names who moved it");
    assert_eq!(
        trail[0].owner(),
        &acme,
        "and whose it is now — two names, because a delegated administrator changes one without \
         being the other"
    );
    assert_eq!(
        trail[1].at(),
        FakeClock::at(START).now(),
        "and when: the caller's clock, which is the clock everything else in this engine is \
         stamped with"
    );

    // A change that was refused changed nothing, so there is nothing to record: the unit here is a
    // change to the model, and a change that did not happen is not a decision about a queue.
    acme_store
        .declare_queue(
            &QueueDeclaration::new(queue.clone(), acme.clone())
                .with_grant(bob.clone(), QueueAction::Submit),
        )
        .await
        .expect("a producer is granted");
    let refused = store
        .scoped(&bob)
        .declare_queue(&QueueDeclaration::new(queue.clone(), acme.clone()))
        .await
        .expect_err("a producer may not change the model");
    assert!(matches!(refused, Error::Forbidden { .. }));
    assert_eq!(
        acme_store
            .audit(10)
            .await
            .expect("a read cannot fail here")
            .len(),
        3,
        "three accepted changes, no line for the refusal"
    );
    assert!(
        store
            .scoped(&tenant("carol"))
            .audit(10)
            .await
            .expect("a read cannot fail here")
            .is_empty(),
        "and a tenant whose model names no queue of acme's reads an empty trail: the trail is \
         filtered by the same rule as everything else, not by who is asking"
    );
}

/// A change whose row cannot be written does not happen: the model and the trail are one commit.
#[tokio::test]
async fn a_change_that_cannot_be_recorded_does_not_happen() {
    // This case starts a container of its own, and that is the difference rather than an
    // implementation detail: it is about a *durable* backend's atomicity — the in-memory one's is
    // structural (one lock, and no way for a caller to make the second write fail from outside),
    // while a transaction can be watched failing by taking away the table it writes. It is also why
    // this step declares an image: the case needs the database whether or not `KILN_BACKEND` says
    // `postgres`. The table's name is the course's (`migrations/0005_queues_and_usage.sql` pins it
    // for exactly this reason).
    let container = Postgres::start();
    let url = container.url();
    let clock = FakeClock::at(START);
    let store = PostgresStore::connect(&url)
        .await
        .expect("the container the fixture started is reachable")
        .with_clock(Arc::clone(&clock) as Arc<dyn Clock>)
        .with_lease(LEASE);
    store
        .migrate()
        .await
        .expect("this project's migration applies to the schema it describes");
    let store: Arc<dyn Store> = Arc::new(store);
    let acme = tenant("acme");
    let acme_store = store.scoped(&acme);
    let never = queue(OTHER_QUEUE);
    let declared = queue(QUEUE);

    acme_store
        .declare_queue(&QueueDeclaration::new(declared.clone(), acme.clone()))
        .await
        .expect("the first declaration has everything it needs");

    // And the refusals this file is about are the *durable* backend's too, not only the in-memory
    // one's: a second tenant over the same database cannot see the queue, and what it is told is
    // what a queue nobody declared answers.
    let bob = store.scoped(&tenant("bob"));
    let refused = bob
        .claim(&worker(), &declared, 1)
        .await
        .expect_err("another tenant's queue is not claimable");
    assert!(
        matches!(refused, Error::NoQueue { .. }),
        "the durable backend refuses the same way the in-memory one does, and the answer comes \
         from the query's own rule (`queue_permissions` in `migrations/0005_queues_and_usage.sql`) \
         rather than from a check in Rust: got {refused:?}"
    );

    let pool = PgPoolOptions::new()
        .connect(&url)
        .await
        .expect("the container the fixture started is reachable");
    sqlx::query("drop table queue_audit")
        .execute(&pool)
        .await
        .expect("the container lets the case take the trail away");

    let refused = acme_store
        .declare_queue(&QueueDeclaration::new(never.clone(), acme.clone()))
        .await
        .expect_err("a change that cannot be recorded is a change that does not happen");
    assert!(
        matches!(refused, Error::Store { .. }),
        "a row the durable backend cannot write is the store's class: the engine does not invent \
         a category for a table somebody dropped, and what this case is about is that the \
         declaration was refused *at all* rather than half-made"
    );
    let lines = acme_store.usage().await.expect("a read cannot fail here");
    assert_eq!(
        lines
            .iter()
            .map(|line| line.queue().as_str())
            .collect::<Vec<&str>>(),
        [QUEUE],
        "the queue that was declared before the table went away is the only one there: the refused \
         declaration rolled back, and the model did not move"
    );
}
