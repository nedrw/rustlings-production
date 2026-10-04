//! The durable backend: the port's five questions, answered by a table.
//!
//! Nothing in this file is allowed to be a *new* promise. The ordering, the attempt count, the
//! refusal to settle a job nobody is running — every one of them is written down on [`Store`], and
//! the conformance suite of step `jobq/3.5-store-port-conformance` is what holds this backend and
//! the in-memory one to the same answers. A backend that is *better* than the port is a second
//! contract, and the second contract is the one nobody tests.
//!
//! What is genuinely different here is that the questions are asked *concurrently*. The in-memory
//! store serialises every caller behind one mutex; this one has a pool, and two workers really can
//! be in the middle of a claim at the same instant. That single fact decides the shape of `claim`:
//! it is one statement, so there is no window between deciding which rows are free and writing the
//! decision down — the window a read-then-write claim leaves open is the window that hands one job
//! to two workers.
//!
//! The instant a job is due comes from the process, not from the database's `now()`. Both backends
//! compare against the same clock, so `claim` answers the same question whichever one the engine
//! was handed; a database clock would be a second authority, and the two would drift apart on the
//! day the container's clock does. `jobq/3.8-clock-injection` is where that clock becomes
//! something a test can move.
//!
//! `jobq/3.6-transaction-isolation` gives the durable backend two things the in-memory one has to
//! imitate rather than learn. The first is a **trail**: every settle writes a line into
//! `job_events`, *in the same transaction* as the row it describes, because a trail that can
//! disagree with the job it is about is worse than no trail at all — an operator reading "failed
//! twice" next to a row that says `queued` has two answers and no way to pick. The second is
//! **isolation**: a settle runs at `repeatable read` and hands the caller
//! [`Error::Conflict`](crate::engine::Error::Conflict) when a concurrent writer made the database
//! refuse it, so the loser of a race is told "again" instead of being left to act on a value that
//! was true a moment ago.
//!
//! The schema this file talks to is `migrations/0001_jobs.sql` and `migrations/0002_job_events.sql`,
//! and the port's methods are the only place the column names appear: no other file in the crate
//! knows that jobs live in a table.

use chrono::{DateTime, Utc};
use sqlx::postgres::{PgConnection, PgPoolOptions};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::engine::domain::{Job, JobId, JobState, Queue, SubmitRequest, WorkerId};
use crate::engine::error::Error;
use crate::engine::events::{EventKind, JobEvent};
use crate::engine::sealed::Sealed;
use crate::engine::store::{BoxFuture, Store};

// Why the statements below are written out rather than assembled, and why they look the way they
// do: `sqlx` 0.9 takes a `&'static str` and nothing else (`SqlSafeStr`), so a query built with
// `format!` is a compile error rather than a habit — the right trade here, where no caller data
// reaches the SQL text (every value travels as a bind parameter) and the column list is therefore
// spelled out in full wherever it is projected. The payload is cast to `text` in each projection
// rather than decoded by the driver: the JSON is a `serde_json` value this crate already owns, and
// the driver is asked for a string instead of being taught a second JSON type.

/// Jobs in a Postgres table, reached through one pool.
///
/// It is a backend this crate certifies, so it carries the seal that lets the port accept it (step
/// `jobq/3.2-public-api-baseline`): the suite of `jobq/3.5-store-port-conformance` runs against this
/// type and the in-memory one, which is the promise the seal exists to keep.
#[derive(Debug)]
pub struct PostgresStore {
    /// The pool every statement in this file runs on.
    pool: PgPool,
}

impl PostgresStore {
    /// Connect a pool to `url`.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when the pool cannot be opened — a refused connection, a database that does
    /// not exist, a connection string this driver cannot read.
    pub async fn connect(url: &str) -> Result<Self, Error> {
        // The pool's own defaults, deliberately: how many connections a deployment wants is a
        // setting, and the configuration that would carry one belongs to the process that runs a
        // pool of workers rather than to the store. `connect` waits for a real connection, so a URL
        // that cannot be reached is refused here instead of at the first claim.
        let pool = PgPoolOptions::new().connect(url).await.map_err(refused)?;
        Ok(Self { pool })
    }

    /// Bring the schema up to date.
    ///
    /// Runs on every start of the daemon, against a database that is usually already current, so
    /// "the second run changes nothing" is part of what this method owes rather than a detail of
    /// how it is written (step `mailer/1.9-postgres` made the same promise for one service).
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when a migration fails.
    pub async fn migrate(&self) -> Result<(), Error> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|error| refused(sqlx::Error::Migrate(Box::new(error))))
    }
}

impl Sealed for PostgresStore {}

impl Store for PostgresStore {
    fn enqueue<'a>(&'a self, request: SubmitRequest) -> BoxFuture<'a, Result<Job, Error>> {
        Box::pin(async move {
            if request.max_attempts == 0 {
                return Err(Error::Invalid {
                    detail: "a job with a budget of zero attempts can never run: either give it \
                             one attempt or do not queue it"
                        .to_owned(),
                });
            }
            let payload = serde_json::to_string(&request.payload)
                .map_err(|error| unreadable("payload", error.to_string()))?;
            // The identity is the engine's, not the database's: a row whose key was invented by
            // whichever backend happened to store it is a row whose key changes when the deployment
            // moves it to the other one.
            let id = JobId::new();
            let row: JobRow = sqlx::query_as(
                "insert into jobs \
                     (id, queue, payload, state, priority, run_at, attempts, max_attempts) \
                 values ($1, $2, $3::jsonb, $4, $5, $6, 0, $7) \
                 returning id, queue, payload::text as payload, state, priority, run_at, \
                           attempts, max_attempts, last_error",
            )
            .bind(id.as_uuid())
            .bind(request.queue.as_str())
            .bind(payload)
            .bind(JobState::Queued.as_str())
            .bind(request.priority)
            // `None` means "as soon as somebody looks", and the instant that becomes is the
            // engine's clock — the same one the in-memory backend reads (step
            // `jobq/3.8-clock-injection` moves it behind an injected clock for both of them at
            // once).
            .bind(request.run_at.unwrap_or_else(Utc::now))
            // `i64::from(u32)` is total, which is why the budget column is a `bigint`: storing it
            // as a 32-bit integer would make "the caller asked for more attempts than the table
            // holds" a case this method has to answer, and there is no good answer to it.
            .bind(i64::from(request.max_attempts))
            .fetch_one(&self.pool)
            .await
            .map_err(refused)?;
            row.into_job()
        })
    }

    fn claim<'a>(
        &'a self,
        worker: &'a WorkerId,
        queue: &'a Queue,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<Job>, Error>> {
        Box::pin(async move {
            // Nothing records who holds a job yet (step `jobq/3.7-leases-and-recovery` adds the
            // lease), so the worker is not written down here either — the two backends have to
            // answer the same questions, and an answer the in-memory store cannot give is not one
            // the port may rely on. The name is still the interface.
            let _ = worker;
            let now = Utc::now();
            // One statement, and the shape of it is the whole promise: the rows are chosen with
            // `for update skip locked` *inside* the update that stamps them, so a second claim
            // running at the same time either waits for nothing (the row is already taken and it
            // skips it) or finds a row that is still queued. Selecting the rows first and stamping
            // them afterwards would leave the window in which both workers come back with the same
            // job — the one concession the port does not make, because the engine would then run
            // the same job twice with no way to notice.
            //
            // The outer `select` is what puts the answer in the port's claim order (priority, then
            // longest due, then identity): `returning` says which rows changed, not in which
            // order, and an order that comes out of the database by accident is an order that
            // changes the day an index does.
            let rows: Vec<JobRow> = sqlx::query_as(
                "with taken as ( \
                     update jobs set state = 'running', attempts = jobs.attempts + 1 \
                     where id in ( \
                         select id from jobs \
                         where queue = $1 and state = 'queued' and run_at <= $2 \
                         order by priority desc, run_at asc, id asc \
                         limit $3 \
                         for update skip locked \
                     ) \
                     returning id, queue, payload::text as payload, state, priority, run_at, \
                               attempts, max_attempts, last_error \
                 ) \
                 select id, queue, payload::text as payload, state, priority, run_at, attempts, \
                        max_attempts, last_error \
                 from taken order by priority desc, run_at asc, id asc",
            )
            .bind(queue.as_str())
            .bind(now)
            // A `usize` limit is a `bigint` to Postgres, and the conversion can only fail for a
            // queue no process could hold: saturating keeps that impossible case out of the port's
            // error surface instead of adding a refusal nobody can reach.
            .bind(i64::try_from(limit).unwrap_or(i64::MAX))
            .fetch_all(&self.pool)
            .await
            .map_err(refused)?;
            rows.into_iter().map(JobRow::into_job).collect()
        })
    }

    fn complete<'a>(
        &'a self,
        worker: &'a WorkerId,
        id: &'a JobId,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let _ = worker;
            // The worker is not asked for a second time here either: the guard is the row's own
            // state, which is the only thing that can decide this once two workers disagree.
            self.settle(id, Change::Completed).await.map(|_| ())
        })
    }

    fn fail<'a>(
        &'a self,
        worker: &'a WorkerId,
        id: &'a JobId,
        reason: &'a str,
    ) -> BoxFuture<'a, Result<JobState, Error>> {
        Box::pin(async move {
            let _ = worker;
            self.settle(id, Change::Failed { reason }).await
        })
    }

    fn get<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<Option<Job>, Error>> {
        Box::pin(async move {
            let row: Option<JobRow> = sqlx::query_as(
                "select id, queue, payload::text as payload, state, priority, run_at, attempts, \
                        max_attempts, last_error \
                 from jobs where id = $1",
            )
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(refused)?;
            // A row that is not there is `Ok(None)` rather than an error: looking is allowed to
            // come up empty, and the engine is the layer that turns that into `Error::NotFound`
            // for the calls that owe the caller an answer about one specific job.
            row.map(JobRow::into_job).transpose()
        })
    }

    fn record<'a>(&'a self, event: &'a JobEvent) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            // One statement, so it needs no transaction of its own: the unique index decides who
            // wins when two callers append the same line at the same instant, and `do nothing` is
            // what makes the loser's write a no-op rather than an error it would have to translate.
            let mut connection = self.pool.acquire().await.map_err(refused)?;
            append(&mut connection, event).await
        })
    }

    fn events<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<Vec<JobEvent>, Error>> {
        Box::pin(async move {
            // The order is the port's promise, so it is written here rather than left to whatever
            // order the table hands rows back in: `at` first, then the two columns that settle a
            // tie the same way the in-memory backend settles it.
            let rows: Vec<EventRow> = sqlx::query_as(
                "select id, job_id, kind, attempt, detail, at from job_events where job_id = $1 \
                 order by at asc, kind asc, attempt asc",
            )
            .bind(id.as_uuid())
            .fetch_all(&self.pool)
            .await
            .map_err(refused)?;
            rows.into_iter().map(EventRow::into_event).collect()
        })
    }
}

/// The statement a settle's transaction starts with.
///
/// One line, and the step's whole isolation argument. At `repeatable read` a transaction's reads
/// and its writes belong to one snapshot, so a writer that commits in between is a *refusal*
/// (`40001`, [`Error::Conflict`]) rather than a `where` clause that quietly matches nothing. At the
/// default level the same write waits for the other transaction, re-reads the row it no longer
/// recognises, and either succeeds or finds nothing to do — with no way for this backend to tell
/// "somebody else settled it" from "the row was never running", and no way for the trail to record
/// which of the two happenings it was.
///
/// It is a `&'static str` because that is what the driver takes (see the note above the statements):
/// the isolation level of a transaction is a decision of this file, not a value a caller hands over.
const SETTLE: &str = "begin isolation level repeatable read";

/// The statement that records a success.
const COMPLETE: &str = "update jobs set state = 'succeeded', last_error = null \
                       where id = $1 and state = 'running' \
                       returning state, attempts";

/// The statement that records a failure: the row's own budget decides where the job goes, in the
/// same statement that writes the reason down.
const FAIL: &str = "update jobs set last_error = $2, \
                           state = case when attempts < max_attempts then 'queued' else 'failed' end \
                    where id = $1 and state = 'running' \
                    returning state, attempts";

/// What a settle does to the row it touches.
///
/// An enum rather than a flag because the two settles differ in three places at once — the
/// statement, the line they append, and the sentence a refusal is explained with — and a bool that
/// picks between three things is how the third one gets forgotten.
enum Change<'a> {
    /// The attempt finished the job.
    Completed,
    /// The attempt failed: the reason goes on the row, and the budget decides where the job goes.
    Failed {
        /// Why it failed, in the caller's words.
        reason: &'a str,
    },
}

impl Change<'_> {
    /// Move the row, or move nothing when there is no running row to move.
    ///
    /// The `where … and state = 'running'` is the promise rather than a guard, and the `returning`
    /// is what hands the settle the attempt number its line has to name: read the attempt in a
    /// second statement and the line can describe an attempt that had already moved on.
    async fn apply(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        id: &JobId,
    ) -> Result<Option<(String, i64)>, Error> {
        let row = match self {
            Change::Completed => sqlx::query_as(COMPLETE).bind(id.as_uuid()),
            Change::Failed { reason } => sqlx::query_as(FAIL).bind(id.as_uuid()).bind(*reason),
        }
        .fetch_optional(&mut **transaction)
        .await
        .map_err(refused)?;
        Ok(row)
    }

    /// The kind of line this change appends.
    fn kind(&self) -> EventKind {
        match self {
            Change::Completed => EventKind::Completed,
            Change::Failed { .. } => EventKind::AttemptFailed,
        }
    }

    /// What the line says, when there is something to say.
    fn detail(&self) -> Option<String> {
        match self {
            Change::Completed => None,
            Change::Failed { reason } => Some((*reason).to_owned()),
        }
    }

    /// Why a row that did not move could not be moved by *this* settle, for the refusal's sentence.
    fn refusal(&self) -> &'static str {
        match self {
            Change::Completed => "only the worker that claimed it can complete it",
            Change::Failed { .. } => {
                "a failure is only recorded against an attempt somebody actually started"
            }
        }
    }
}

impl PostgresStore {
    /// Settle one job: change the row, and write the line that describes the change, in one
    /// transaction.
    ///
    /// The two writes are the step. A settle that changed the row and then failed to append would
    /// leave a job nobody can explain; one that appended and then failed to change the row would
    /// leave a trail that lies about where the job is. Either way the disagreement is invisible
    /// until somebody has to reconstruct what happened, which is exactly when a queue's trail is
    /// the only evidence left — so the commit is one, and the append is not a second call that could
    /// be skipped.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when there is no such row, [`Error::Invalid`] when the row is not
    /// running, [`Error::Conflict`] when a concurrent writer made the database refuse the
    /// transaction, [`Error::Store`] for anything else the backend refused.
    async fn settle(&self, id: &JobId, change: Change<'_>) -> Result<JobState, Error> {
        let mut transaction = self.pool.begin_with(SETTLE).await.map_err(refused)?;
        let Some((state, attempts)) = change.apply(&mut transaction, id).await? else {
            // Nothing moved, and there are two different reasons for that — which is why the
            // refusal is a second question, asked through the transaction that just wrote, rather
            // than a message invented here.
            let unreadable = format!(
                "job {id} is {}, not running: {}",
                state_of(&mut transaction, id)
                    .await?
                    .as_deref()
                    .unwrap_or("gone"),
                change.refusal()
            );
            let refusal = match state_of(&mut transaction, id).await? {
                Some(_) => Error::Invalid { detail: unreadable },
                None => Error::NotFound { id: *id },
            };
            // Nothing was written (the update matched no row), so there is nothing to undo; the
            // rollback is what says so out loud, and it releases the snapshot and the locks the
            // transaction took the moment it began.
            transaction.rollback().await.map_err(refused)?;
            return Err(refusal);
        };
        append(
            &mut transaction,
            &JobEvent::new(
                *id,
                change.kind(),
                count(attempts, "attempts")?,
                change.detail(),
                Utc::now(),
            ),
        )
        .await?;
        // One commit for the row and the line: a reader sees both or neither.
        transaction.commit().await.map_err(refused)?;
        decode_state(&state)
    }
}

/// Append one line of the trail through whatever connection or transaction the caller holds.
///
/// `on conflict … do nothing` is where the idempotence of an append lives, and the unique index of
/// `migrations/0002_job_events.sql` is what makes it true: two transactions appending the same
/// `(job_id, kind, attempt)` cannot both win, and the one that loses writes nothing instead of
/// reporting an error the caller would have to translate back into "I already did this".
///
/// The answer is whether the line was new — `false` for a suppressed duplicate — because that is
/// what a caller that retried its own work wants to know.
async fn append(connection: &mut PgConnection, event: &JobEvent) -> Result<bool, Error> {
    let written = sqlx::query(
        "insert into job_events (id, job_id, kind, attempt, detail, at) \
         values ($1, $2, $3, $4, $5, $6) \
         on conflict (job_id, kind, attempt) do nothing",
    )
    .bind(event.id())
    .bind(event.job_id().as_uuid())
    .bind(event.kind().as_str())
    // `i64::from(u32)` is total, and the column is a `bigint` for the same reason `attempts` is:
    // the engine counts in `u32` and the table must be able to hold what the engine counts.
    .bind(i64::from(event.attempt()))
    .bind(event.detail())
    .bind(event.at())
    .execute(connection)
    .await
    .map_err(refused)?;
    Ok(written.rows_affected() > 0)
}

/// What the `state` column says about one job, or `None` when there is no such row.
///
/// Asked only after a write that changed nothing, to tell "this job is gone" from "this job is not
/// running" — the two refusals the port separates and the two a caller acts on differently. It is
/// asked *through the transaction that just wrote*, so the answer belongs to the same snapshot as
/// the refusal it explains.
async fn state_of(connection: &mut PgConnection, id: &JobId) -> Result<Option<String>, Error> {
    let row: Option<(String,)> = sqlx::query_as("select state from jobs where id = $1")
        .bind(id.as_uuid())
        .fetch_optional(connection)
        .await
        .map_err(refused)?;
    Ok(row.map(|(state,)| state))
}

/// One row of `jobs`, in the shape the driver decodes it.
///
/// Not quite the shape [`Job`] has, and the difference is not cosmetic: Postgres' `bigint` arrives
/// as `i64` while the engine counts in `u32`, the state is a text column, and the payload is JSON
/// that this crate parses itself. [`JobRow::into_job`] is the one place those three are reconciled,
/// so a row this backend wrote and cannot read back is reported where it can be seen rather than
/// travelling through the engine as a nonsense value.
#[derive(sqlx::FromRow)]
struct JobRow {
    /// Identity.
    id: Uuid,
    /// Queue name.
    queue: String,
    /// Payload, as text, parsed by `into_job`.
    payload: String,
    /// State, as text, decoded by `into_job`.
    state: String,
    /// Higher runs first.
    priority: i32,
    /// Not before this instant.
    run_at: DateTime<Utc>,
    /// Attempts started so far.
    attempts: i64,
    /// Attempts allowed in total.
    max_attempts: i64,
    /// Why the last attempt failed, when one did.
    last_error: Option<String>,
}

impl JobRow {
    /// The engine's row, or the reason the table holds something the engine cannot read.
    fn into_job(self) -> Result<Job, Error> {
        let queue =
            Queue::parse(&self.queue).map_err(|error| unreadable("queue", error.to_string()))?;
        let payload = serde_json::from_str(&self.payload)
            .map_err(|error| unreadable("payload", error.to_string()))?;
        Ok(Job {
            id: JobId::from_uuid(self.id),
            queue,
            payload,
            state: decode_state(&self.state)?,
            priority: self.priority,
            run_at: self.run_at,
            attempts: count(self.attempts, "attempts")?,
            max_attempts: count(self.max_attempts, "max_attempts")?,
            last_error: self.last_error,
        })
    }
}

/// One row of `job_events`, in the shape the driver decodes it.
///
/// The same kind of boundary as [`JobRow`], one table over: `bigint` is not `u32`, the kind is a
/// text column, and [`EventRow::into_event`] is the one place those are reconciled, so a row this
/// backend wrote and cannot read back is reported where it can be seen.
#[derive(sqlx::FromRow)]
struct EventRow {
    /// This line's own identity.
    id: Uuid,
    /// The job it is about.
    job_id: Uuid,
    /// What happened, as text.
    kind: String,
    /// Which attempt it happened to.
    attempt: i64,
    /// Why, when there is a why.
    detail: Option<String>,
    /// When the caller stamped it.
    at: DateTime<Utc>,
}

impl EventRow {
    /// The engine's line, or the reason this table holds something the engine cannot read.
    fn into_event(self) -> Result<JobEvent, Error> {
        Ok(JobEvent::stored(
            self.id,
            JobId::from_uuid(self.job_id),
            decode_kind(&self.kind)?,
            count(self.attempt, "attempt")?,
            self.detail,
            self.at,
        ))
    }
}

/// The `kind` column, as the enum the engine uses.
///
/// Written as a search over [`EventKind::as_str`] rather than a `match` on strings, so that the two
/// directions of the encoding cannot drift: adding a kind to the enum without teaching this function
/// about it is impossible, because there is nothing here to teach.
fn decode_kind(text: &str) -> Result<EventKind, Error> {
    [
        EventKind::Completed,
        EventKind::AttemptFailed,
        EventKind::Reclaimed,
    ]
    .into_iter()
    .find(|kind| kind.as_str() == text)
    .ok_or_else(|| {
        unreadable(
            "kind",
            format!("`{text}` is not an event this engine knows"),
        )
    })
}

/// The `state` column, as the enum the engine uses.
fn decode_state(text: &str) -> Result<JobState, Error> {
    match text {
        "queued" => Ok(JobState::Queued),
        "running" => Ok(JobState::Running),
        "succeeded" => Ok(JobState::Succeeded),
        "failed" => Ok(JobState::Failed),
        other => Err(unreadable(
            "state",
            format!("`{other}` is not a state this engine knows"),
        )),
    }
}

/// A count column, as the engine's unsigned count.
///
/// The table stores counts as `bigint`, so this is the one conversion between the two number types
/// the two sides of the port use, and it is the direction that can fail: a negative or absurd count
/// is not something this backend wrote, which makes it a store failure with a column name rather
/// than a caller's problem.
fn count(value: i64, column: &'static str) -> Result<u32, Error> {
    u32::try_from(value).map_err(|_| unreadable(column, format!("{value} is not a count")))
}

/// A driver failure, as the port wants it: whole, for the caller that wants the detail.
///
/// One class is split off, and the split is the subject of `jobq/3.6-transaction-isolation`: a
/// transaction the database refused because another writer got there first is the one refusal the
/// *same* call can turn around, so it becomes [`Error::Conflict`], and everything else — a
/// connection that is gone, a constraint that said no, a statement that does not parse — stays
/// [`Error::Store`]. The driver's own error travels inside either of them, so a caller that wants
/// the SQLSTATE can `downcast_ref` and read it.
fn refused(source: sqlx::Error) -> Error {
    if is_conflict(&source) {
        return Error::Conflict {
            source: Box::new(source),
        };
    }
    Error::Store {
        source: Box::new(source),
    }
}

/// Whether the driver's failure is the database refusing a transaction for concurrency.
///
/// PostgreSQL has two codes for it and this is both of them: `40001`, "could not serialize access"
/// — a transaction at `repeatable read` or `serializable` that lost a row to a commit it never saw —
/// and `40P01`, a deadlock the server broke by killing one of the two transactions. They are
/// *classes* rather than messages: matching the text would break the day PostgreSQL rewords it, and
/// treating anything wider as a conflict would retry failures that cannot change.
fn is_conflict(source: &sqlx::Error) -> bool {
    matches!(
        source
            .as_database_error()
            .and_then(|error| error.code().map(|code| code.into_owned()))
            .as_deref(),
        Some("40001" | "40P01")
    )
}

/// A row the engine cannot read back.
fn unreadable(column: &'static str, detail: String) -> Error {
    Error::Store {
        source: Box::new(Unreadable { column, detail }),
    }
}

/// Something in the `jobs` table that is not what this backend writes there.
///
/// A row this backend cannot read is a store failure and not a caller's mistake: the caller asked a
/// question the port answers, and what came back was a value the engine has no meaning for. It is
/// its own type so the message can name the column, and so a caller that wants to know whether the
/// problem is its own input can `downcast_ref` and see that it is not.
#[derive(Debug)]
struct Unreadable {
    /// Which column holds it.
    column: &'static str,
    /// What the engine made of the value.
    detail: String,
}

impl std::fmt::Display for Unreadable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the `jobs` table holds a value the engine cannot read in `{}`: {}",
            self.column, self.detail
        )
    }
}

impl std::error::Error for Unreadable {}
