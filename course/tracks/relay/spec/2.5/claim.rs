//! Spec for `relay/2.5-atomic-claim`: which worker gets which piece of the delivery work.
//!
//! `kiln run` copies this file into the project as `tests/spec_claim.rs`; it is the course's file,
//! not the learner's (plan §3 D2). The claim is judged from two sides at once, because neither one
//! alone can see the property that matters:
//!
//! * **eight workers, one database.** The tasks below run at the same time on one pool, and what
//!   they come back with has to be *disjoint* and, between them, *everything that was claimable*.
//!   A claim that reads and then writes gives the same row to two workers and passes every
//!   single-threaded test there is (the symptom is a subscriber that receives one event twice);
//! * **`psql`, for what the row now says.** `locked_by`, `lease_expires_at` and `state` are the
//!   claim's public half: a service that hands a row out without stamping who holds it cannot tell
//!   a retry from a duplicate, and one that never sets a lease leaves the row to a worker that is
//!   gone.
//!
//! This file writes the rows it needs, and it does so with `psql` rather than through the API: the
//! step that turns an accepted event into one delivery per endpoint is `relay/2.6-at-least-once`,
//! and judging the claim on its own is the point here.
//!
//! Nothing here sleeps. Where a row has to become due again, the fixture moves the row's own
//! `next_attempt_at` — the same thing an injected clock would do, without the waiting.

mod support;

use std::collections::BTreeSet;

use relay::claim::{self, BATCH, LEASE};
use relay::db::Db;
use serde_json::json;
use sqlx::PgPool;
use support::app::{self, App};
use support::client::Request;
use support::postgres::Postgres;
use support::var;
use uuid::Uuid;

/// The tenant these tests seed, and the key it authenticates with.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// The target the seeded deliveries point at. Nothing dials it: this step never sends anything, and
/// a port that answers nothing is the honest target for a claim.
const TARGET: &str = "http://127.0.0.1:9/hook";
/// A row that is due, written as SQL because that is what the fixture inserts.
const DUE: &str = "now() - interval '1 minute'";
/// A row that is not due yet.
const LATER: &str = "now() + interval '1 hour'";

/// One database and one in-process service, for the length of one test.
struct Fixture {
    app: App,
    postgres: Postgres,
    /// The endpoint every seeded delivery points at: `deliveries.endpoint_id` is a foreign key, so
    /// the row has to exist before a claim can take anything.
    endpoint: String,
}

impl Fixture {
    /// Start a service with one tenant and one endpoint, over a database of its own.
    fn start() -> Self {
        let postgres = Postgres::start();
        let app = App::start(app::config(&[
            var(app::DATABASE_URL, postgres.url()),
            var(app::BOOTSTRAP_TENANTS, format!("{}={}", ACME.0, ACME.1)),
        ]));
        // The fixture's readiness check is the *port*, and the kernel answers a connect before the
        // service has finished starting: connecting to the database, migrating it and seeding the
        // configured tenants all happen first. This file asks the tables directly, so it waits for
        // an answer only the running service can give — no sleeping, and a failure here says what
        // was waited for.
        let serving = app.client().get("/healthz");
        assert_eq!(
            serving.status, 200,
            "the service has to be serving before this file asks its database anything: {}",
            serving.body
        );
        let mut fixture = Self {
            app,
            postgres,
            endpoint: String::new(),
        };
        let created = fixture.app.client().send(
            Request::post("/endpoints", Vec::new())
                .bearer(ACME.1)
                .json(&json!({ "url": TARGET })),
        );
        assert_eq!(
            created.status, 201,
            "the fixture registers the endpoint the seeded deliveries point at: {}",
            created.body
        );
        fixture.endpoint = created
            .field("id")
            .as_str()
            .expect("an endpoint answers with its id")
            .to_owned();
        fixture
    }

    /// Seed `count` deliveries in one state, all for this fixture's endpoint, each with an event of
    /// its own (one event and one endpoint is one delivery). Answers the ids it wrote.
    fn seed(&self, count: usize, state: &str, due: &str) -> Vec<String> {
        if count == 0 {
            return Vec::new();
        }
        let tenant = self
            .postgres
            .query("select id from tenants where name = 'acme'");
        self.postgres
            .query(&format!(
                "with fresh as ( \
                     insert into events (id, tenant_id, idempotency_key, kind, payload) \
                     select gen_random_uuid(), '{tenant}'::uuid, gen_random_uuid()::text, \
                            'invoice.paid', '{{\"n\": 1}}'::jsonb \
                     from generate_series(1, {count}) \
                     returning id), \
                 made as ( \
                     insert into deliveries (id, event_id, endpoint_id, state, attempts, \
                                             next_attempt_at) \
                     select gen_random_uuid(), fresh.id, '{}'::uuid, '{state}', 0, {due} \
                     from fresh \
                     returning id) \
                 select id from made order by id",
                self.endpoint
            ))
            .lines()
            .map(|line| line.trim().to_owned())
            .collect()
    }

    /// One row's state, failure count and failure reason: `pending|0|`.
    fn row(&self, delivery: Uuid) -> String {
        self.postgres.query(&format!(
            "select state, attempts, coalesce(last_error, '') from deliveries \
              where id = '{delivery}'"
        ))
    }

    /// What the claim stamped: who holds the row, and whether the lease is still ahead.
    fn holding(&self, delivery: Uuid) -> String {
        self.postgres.query(&format!(
            "select coalesce(locked_by, '-'), locked_at is not null, \
                    coalesce(lease_expires_at > now(), false) \
               from deliveries where id = '{delivery}'"
        ))
    }

    /// Make every `retrying` row due again, the way an injected clock would.
    fn move_the_clock_forward(&self) {
        self.postgres.execute(
            "update deliveries set next_attempt_at = now() - interval '1 second' \
              where state = 'retrying'",
        );
    }

    /// The pool the workers share: the service's own pool policy, so a claim in a test runs on the
    /// connections a claim in production runs on.
    async fn pool(&self) -> PgPool {
        Db::connect(&self.postgres.url())
            .await
            .expect("the fixture's pool connects")
            .pool()
            .clone()
    }
}

/// Eight workers claiming at the same time are handed disjoint work, and all of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workers_claiming_at_the_same_time_never_hold_the_same_row() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    let seeded = fixture.seed(40, "pending", DUE);
    let not_due = fixture.seed(5, "pending", LATER);

    let mut workers = Vec::new();
    for index in 0..8 {
        let pool = pool.clone();
        workers.push(tokio::spawn(async move {
            let worker = format!("worker-{index}");
            let mut mine = Vec::new();
            loop {
                let batch = claim::claim(&pool, &worker, 5, LEASE)
                    .await
                    .expect("a claim this service issued cannot fail against its own schema");
                if batch.is_empty() {
                    return mine;
                }
                mine.extend(batch.into_iter().map(|claimed| claimed.delivery));
            }
        }));
    }

    let mut handed_out = Vec::new();
    for worker in workers {
        handed_out.extend(worker.await.expect("every worker finishes its claim"));
    }

    let unique: BTreeSet<Uuid> = handed_out.iter().copied().collect();
    assert_eq!(
        unique.len(),
        handed_out.len(),
        "two workers were handed the same delivery. The claim has to take the rows it is about to \
         stamp: a `select` that says which rows are free, followed by an `update`, lets every \
         worker in that window pick the same ones — and the subscriber receives those events twice \
         ({} of {} claims were repeats)",
        handed_out.len() - unique.len(),
        handed_out.len()
    );
    assert_eq!(
        handed_out.len(),
        seeded.len(),
        "every claimable row has to be handed out exactly once: {} rows were seeded as due, {} came \
         back. A claim that waits on a row another worker holds (no `skip locked`) leaves workers \
         queued behind each other, and one that filters wrongly loses rows silently",
        seeded.len(),
        handed_out.len()
    );

    let held: BTreeSet<Uuid> = fixture
        .postgres
        .query("select id from deliveries where state = 'claimed'")
        .lines()
        .map(|line| line.trim().parse().expect("a uuid column parses"))
        .collect();
    assert_eq!(
        held, unique,
        "the rows the workers came back with are the rows the database says are claimed"
    );
    assert_eq!(
        fixture.postgres.query(
            "select count(*) from deliveries where state = 'claimed' \
               and locked_by is not null and lease_expires_at > now()"
        ),
        "40",
        "every claim stamps the worker that holds it and an instant the lease ends: without the \
         lease, work a dead worker took is invisible to everybody, for good"
    );
    assert_eq!(
        fixture
            .postgres
            .query("select count(*) from deliveries where state = 'pending'"),
        "5",
        "a row whose `next_attempt_at` has not arrived stays where it is: it is not due, so no \
         worker may take it"
    );
    assert!(
        not_due.iter().all(|waiting| !handed_out
            .iter()
            .any(|claimed| claimed.to_string() == *waiting)),
        "a row that is not due was handed out: {not_due:?} against {handed_out:?}"
    );
}

/// A claim carries the work itself, and only its holder can hand it back.
#[tokio::test]
async fn a_claim_carries_the_work_and_only_its_holder_hands_it_back() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    let seeded = fixture.seed(1, "pending", DUE);

    let batch = claim::claim(&pool, "worker-a", BATCH, LEASE)
        .await
        .expect("the claim runs");
    assert_eq!(batch.len(), 1, "one row is due, one row comes back");
    let claimed = &batch[0];
    assert_eq!(
        claimed.delivery.to_string(),
        seeded[0],
        "the claim answers with the row it took"
    );
    assert_eq!(
        claimed.attempts, 0,
        "a delivery that has never failed is on its first attempt"
    );
    assert_eq!(
        claimed.url.as_str(),
        TARGET,
        "the claim brings the target with it: a worker that has to look the endpoint up is a second \
         query per row, and a second chance for the row to have changed"
    );
    assert_eq!(
        claimed.event.id.to_string(),
        fixture.postgres.query(&format!(
            "select event_id from deliveries where id = '{}'",
            claimed.delivery
        )),
        "and the event it is owed for"
    );
    assert_eq!(
        claimed.event.kind, "invoice.paid",
        "the event carries what happened and the payload the subscriber is handed"
    );
    assert_eq!(
        claimed.event.payload,
        json!({ "n": 1 }),
        "the payload is the JSON the tenant sent, decoded once here instead of in every worker"
    );

    assert!(
        claim::claim(&pool, "worker-b", BATCH, LEASE)
            .await
            .expect("the claim runs")
            .is_empty(),
        "a row another worker holds is not handed out again while the lease lasts: `state` is the \
         lock's public half, and a claim that ignores it delivers the same event twice"
    );
    assert!(
        !claim::release(&pool, claimed.delivery, "worker-b")
            .await
            .expect("the release runs"),
        "a worker cannot release work it does not hold: the claim records the holder, and a release \
         that ignored it would let one worker make another's rows claimable at will"
    );
    assert!(
        claim::release(&pool, claimed.delivery, "worker-a")
            .await
            .expect("the release runs"),
        "the holder hands its claim back"
    );
    assert_eq!(
        fixture.holding(claimed.delivery),
        "-|f|f",
        "a released row holds no worker and no lease"
    );
    let again = claim::claim(&pool, "worker-b", BATCH, LEASE)
        .await
        .expect("the claim runs");
    assert_eq!(
        again.len(),
        1,
        "what was handed back is claimable again, or a worker that stops between the claim and the \
         attempt takes the row out of circulation"
    );
    assert_eq!(
        again[0].delivery, claimed.delivery,
        "and it is the same piece of work"
    );
    assert_eq!(
        fixture.holding(claimed.delivery),
        "worker-b|t|t",
        "the new holder replaced the old one"
    );
}

/// The instant a row carries decides whether it may be taken.
#[tokio::test]
async fn the_next_attempt_at_decides_when_a_row_is_claimable() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    fixture.seed(1, "pending", LATER);
    assert!(
        claim::claim(&pool, "worker-a", BATCH, LEASE)
            .await
            .expect("the claim runs")
            .is_empty(),
        "a first attempt scheduled for the future is not due: the instant is a promise this service \
         made to the subscriber, and a claim that ignores `next_attempt_at` hammers a subscriber \
         that asked for room"
    );

    // A `retrying` row is work in the same sense as a `pending` one: only ever later.
    let retrying = fixture.seed(1, "retrying", DUE);
    let batch = claim::claim(&pool, "worker-a", BATCH, LEASE)
        .await
        .expect("the claim runs");
    assert_eq!(
        batch
            .iter()
            .map(|claimed| claimed.delivery.to_string())
            .collect::<Vec<_>>(),
        retrying,
        "a failed attempt that is due again is claimable: leaving `retrying` out of the claimable \
         states means every failure is a delivery that is never retried"
    );
}

/// A failed attempt grows the count, keeps the reason, and moves the row forward.
#[tokio::test]
async fn a_failed_attempt_is_recorded_and_the_row_comes_back() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    fixture.seed(1, "pending", DUE);
    let claimed = claim::claim(&pool, "worker-a", BATCH, LEASE)
        .await
        .expect("the claim runs")
        .remove(0);
    let due_again = chrono::Utc::now() + chrono::Duration::seconds(45);

    assert!(
        claim::mark_retry(
            &pool,
            claimed.delivery,
            "the subscriber timed out",
            due_again
        )
        .await
        .expect("the update runs"),
        "the worker that holds the row records its failure"
    );
    assert_eq!(
        fixture.row(claimed.delivery),
        "retrying|1|the subscriber timed out",
        "the state, the failure count and the reason are all written down: `attempts` is what the \
         *next* worker reads to know how patient to be, and the reason is what an operator reads \
         when a delivery finally stops"
    );
    assert_eq!(
        fixture.holding(claimed.delivery),
        "-|f|f",
        "a row that is owed again is nobody's: the worker that failed it may be gone"
    );
    assert!(
        claim::claim(&pool, "worker-b", BATCH, LEASE)
            .await
            .expect("the claim runs")
            .is_empty(),
        "the next attempt happens at `next_attempt_at`, not at once: a retry that ignores the \
         instant is a subscriber that is hammered while it is recovering"
    );

    // The clock, moved by the fixture: nothing in this file waits for a retry to become due.
    fixture.move_the_clock_forward();
    let second = claim::claim(&pool, "worker-b", BATCH, LEASE)
        .await
        .expect("the claim runs");
    assert_eq!(second.len(), 1, "the row is due again and comes back");
    assert_eq!(
        second[0].attempts, 1,
        "and it carries the failure count with it: a claim that restarted `attempts` at zero would \
         retry a permanently broken subscriber forever"
    );
    assert!(
        claim::mark_retry(
            &pool,
            second[0].delivery,
            "the subscriber timed out again",
            due_again
        )
        .await
        .expect("the update runs")
    );
    assert_eq!(
        fixture.row(second[0].delivery),
        "retrying|2|the subscriber timed out again",
        "`attempts` counts the failed attempts of this delivery, nothing else"
    );
    assert_eq!(
        fixture.postgres.query(&format!(
            "select case when next_attempt_at > now() + interval '30 seconds' \
                    then 'later' else 'too soon' end \
               from deliveries where id = '{}'",
            second[0].delivery
        )),
        "later",
        "the instant the caller passed is the instant the row carries"
    );
}

/// Only a claimed row records an outcome, and a terminal row is terminal.
#[tokio::test]
async fn an_outcome_is_recorded_against_a_claim() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    let idle = fixture.seed(1, "pending", DUE);
    let delivery: Uuid = idle[0].parse().expect("the fixture seeds a uuid");

    assert!(
        !claim::mark_sent(&pool, delivery)
            .await
            .expect("the update runs"),
        "a row nobody has claimed cannot be marked sent: bookkeeping that does not follow a claim \
         is a worker recording an outcome for work it may not have done"
    );
    assert!(
        !claim::mark_retry(&pool, delivery, "no", chrono::Utc::now())
            .await
            .expect("the update runs"),
        "and it cannot be failed either"
    );
    assert_eq!(
        fixture.row(delivery),
        "pending|0|",
        "a refused write changes nothing at all"
    );

    let claimed = claim::claim(&pool, "worker-a", BATCH, LEASE)
        .await
        .expect("the claim runs")
        .remove(0);
    assert!(
        claim::mark_sent(&pool, claimed.delivery)
            .await
            .expect("the update runs"),
        "the holder records the delivery as done"
    );
    assert_eq!(fixture.row(claimed.delivery), "sent|0|");
    assert_eq!(
        fixture.holding(claimed.delivery),
        "-|f|f",
        "a finished delivery holds nothing"
    );
    assert!(
        !claim::mark_sent(&pool, claimed.delivery)
            .await
            .expect("the update runs"),
        "recording it twice is not an error, it is a write that changes nothing: a worker that \
         answered late must not be able to move a delivery that is already done"
    );
    assert!(
        claim::claim(&pool, "worker-b", BATCH, LEASE)
            .await
            .expect("the claim runs")
            .is_empty(),
        "a sent row is never handed out again"
    );

    // And the other terminal state: given up on, with the reason kept.
    fixture.seed(1, "retrying", DUE);
    let next = claim::claim(&pool, "worker-a", BATCH, LEASE)
        .await
        .expect("the claim runs")
        .remove(0);
    assert!(
        claim::mark_dead(&pool, next.delivery, "the subscriber is gone")
            .await
            .expect("the update runs"),
        "a worker gives up on a delivery it holds"
    );
    assert_eq!(fixture.row(next.delivery), "dead|0|the subscriber is gone");
    assert!(
        claim::claim(&pool, "worker-b", BATCH, LEASE)
            .await
            .expect("the claim runs")
            .is_empty(),
        "and `dead` is as final as `sent`: trying it again is a decision a person makes, not \
         something the queue does on its own"
    );
}

/// One event and one endpoint is one delivery, however often the fan-out runs.
#[tokio::test]
async fn one_event_and_one_endpoint_is_one_delivery() {
    let fixture = Fixture::start();
    let id: Uuid = fixture.seed(1, "pending", DUE)[0]
        .parse()
        .expect("the fixture seeds a uuid");
    let (event, endpoint): (String, String) = fixture
        .postgres
        .query(&format!(
            "select event_id, endpoint_id from deliveries where id = '{id}'"
        ))
        .split_once('|')
        .map(|(event, endpoint)| (event.to_owned(), endpoint.to_owned()))
        .expect("psql answers one row as `event|endpoint`");

    let refused = fixture
        .postgres
        .try_execute(&format!(
            "insert into deliveries (id, event_id, endpoint_id, state, attempts, next_attempt_at) \
             values (gen_random_uuid(), '{event}'::uuid, '{endpoint}'::uuid, 'pending', 0, now())"
        ))
        .expect_err("the same event owed to the same endpoint is one delivery, not two");
    assert!(
        refused.contains("deliveries_one_per_target") || refused.to_lowercase().contains("unique"),
        "the refusal names the constraint that refused it, or nobody can tell a duplicate fan-out \
         from a typo: {refused}"
    );
    assert_eq!(
        fixture.postgres.query("select count(*) from deliveries"),
        "1",
        "the refused insert left the table alone"
    );
}

/// The claim runs every tick, so it has an index in the order it reads.
#[test]
fn the_claim_has_an_index_to_walk() {
    let fixture = Fixture::start();
    let indexes = fixture
        .postgres
        .query("select indexdef from pg_indexes where tablename = 'deliveries'");
    let walkable = indexes
        .lines()
        .map(|line| line.replace(" DESC", "").replace(" ASC", ""))
        .any(|line| line.contains("next_attempt_at") && line.contains("id"));
    assert!(
        walkable,
        "the claim reads the oldest work that is due (`order by next_attempt_at, id`), so one index \
         has to be in that order: without it the database sorts every delivery this service has ever \
         owed, once per tick, and the cost of a claim grows with the tenant's history. Indexes on \
         `deliveries`: {indexes}"
    );
}
