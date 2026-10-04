-- The lease: who holds a job, and until when.
--
-- Step `jobq/3.7-leases-and-recovery`. This is the project's third migration, so it is `0003`.
--
-- Two columns, and the two things that read them:
--
--   * `lease_owner` — who holds the row, as the name the worker calls itself (the same string the
--     port takes in every settle, and the one an operator greps for in `pg_stat_activity`);
--   * `lease_expires_at` — the instant after which the row is presumed abandoned.
--
-- Both are `null` when nobody holds the job. A claimed row carries both, and the pair is what makes
-- every write in this project's write path conditional:
--
--   * a *settle* refuses a row whose `lease_owner` is not the worker that is settling it, which is
--     what stops a worker that was killed, declared dead and replaced from settling the attempt its
--     successor is running right now. (Both are named the same thing to the table: the row is the
--     only place that can decide it.)
--   * the *reclaimer* looks for `state = 'running' and lease_expires_at <= now()`, which is the one
--     query every worker runs at the start of every turn: recovering from a crash must not get more
--     expensive as the queue grows, so that predicate is what the index below covers.
--
-- The index is partial, like `jobs_claimable`: it covers the rows a reclaimer can see, not every row
-- the table has ever held.

alter table jobs add column lease_owner text;
alter table jobs add column lease_expires_at timestamptz;

create index jobs_expired_leases on jobs (lease_expires_at) where state = 'running';
