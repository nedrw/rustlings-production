-- The trail: one row per settle, and the constraint that makes an append idempotent.
--
-- Step `jobq/3.6-transaction-isolation`. This is the project's second migration, so it is `0002`
-- (`0001` wrote `jobs`); `mailer` and `relay` number their own files, and neither counter has
-- anything to do with this project's.
--
-- Three decisions are in this file rather than in a sentence somewhere:
--
--   * **a line's identity is `(job_id, kind, attempt)`.** The same settle appended twice — a
--     transaction that was retried after the client lost the answer, a tool that replays its own
--     work — has to be the *same* line, and that promise is kept by the database rather than by an
--     application that reads before it writes. An application check ("is there a line like this
--     already?") is a window two writers can both pass through; a unique index is a decision the
--     database makes once, for everybody, while holding the row. The append that rides along with a
--     settle therefore writes `on conflict (job_id, kind, attempt) do nothing`, and a second append
--     is a no-op instead of a second row.
--   * **`attempt` is `bigint`**, like `jobs.attempts`: the engine counts in `u32`, and a column that
--     could not hold what the row next to it holds would make "the trail disagrees with the job" a
--     schema problem instead of a bug.
--   * **the foreign key cascades.** A line about a job nobody kept is not evidence, it is a leak;
--     whether a *purge* (`jobq/3.16-dlq-tooling`) wants to keep the trail of what it removed is that
--     step's decision, and it can change this then — with a migration, which is the point of writing
--     the schema down.
--
-- The index that reads one job's trail is `(job_id, at)`: an operator asks "what happened to this
-- one", in order, and the later steps that read the whole trail (`jobq/3.17-error-budget-alerts`
-- counts by kind over a window) can add their own index when they know which query they run.

create table job_events (
    id uuid primary key,
    job_id uuid not null references jobs (id) on delete cascade,
    kind text not null,
    attempt bigint not null,
    detail text,
    at timestamptz not null
);

create unique index job_events_once_per_attempt on job_events (job_id, kind, attempt);

create index job_events_of_a_job on job_events (job_id, at);
