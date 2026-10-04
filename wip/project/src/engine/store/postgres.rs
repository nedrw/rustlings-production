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
//! The schema this file talks to is `migrations/0001_jobs.sql`, and the port's five methods are the
//! only place the column names appear: no other file in the crate knows that jobs live in a table.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

use crate::engine::domain::{Job, JobId, JobState, Queue, SubmitRequest, WorkerId};
use crate::engine::error::Error;
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
            // The `and state = 'running'` is the promise rather than a guard: a job nobody is
            // running is a job whose outcome nobody gets to record, and a write that guessed
            // would overwrite whatever the row was waiting for with "succeeded".
            let written = sqlx::query(
                "update jobs set state = 'succeeded', last_error = null \
                                       where id = $1 and state = 'running'",
            )
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(refused)?;
            if written.rows_affected() > 0 {
                return Ok(());
            }
            // Nothing changed, and there are two different reasons for that — which is why the
            // refusal is a second question rather than a message invented here.
            Err(match self.state_of(id).await? {
                None => Error::NotFound { id: *id },
                Some(state) => Error::Invalid {
                    detail: format!(
                        "job {id} is {state}, not running: only the worker that claimed it can \
                         complete it"
                    ),
                },
            })
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
            // Where a failed job goes is decided by the row's own budget, in the same statement
            // that records the reason: a caller cannot be handed "queued" by a backend that then
            // fails to write it, and the attempt count the in-memory backend keeps is the same
            // count this `case` reads.
            let row: Option<(String,)> = sqlx::query_as(
                "update jobs set last_error = $2, \
                     state = case when attempts < max_attempts then 'queued' else 'failed' end \
                 where id = $1 and state = 'running' \
                 returning state",
            )
            .bind(id.as_uuid())
            .bind(reason)
            .fetch_optional(&self.pool)
            .await
            .map_err(refused)?;
            let Some((state,)) = row else {
                return Err(match self.state_of(id).await? {
                    None => Error::NotFound { id: *id },
                    Some(state) => Error::Invalid {
                        detail: format!(
                            "job {id} is {state}, not running: a failure is only recorded against \
                             an attempt somebody actually started"
                        ),
                    },
                });
            };
            decode_state(&state)
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
}

impl PostgresStore {
    /// What the `state` column says about one job, or `None` when there is no such row.
    ///
    /// Asked only after a write that changed nothing, to tell "this job is gone" from "this job is
    /// not running" — the two refusals the port separates and the two a caller acts on
    /// differently.
    async fn state_of(&self, id: &JobId) -> Result<Option<String>, Error> {
        let row: Option<(String,)> = sqlx::query_as("select state from jobs where id = $1")
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(refused)?;
        Ok(row.map(|(state,)| state))
    }
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
fn refused(source: sqlx::Error) -> Error {
    Error::Store {
        source: Box::new(source),
    }
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
