-- The queue's own table: one row per job, and the state a worker moved it through.
--
-- Step `jobq/3.5-store-port-conformance`. This is the project's first migration: `mailer` and
-- `relay` number their own files, and neither counter has anything to do with this project's.
--
-- The columns are the ones the port carries and nothing a caller cannot see. Two decisions in here
-- are worth the paragraphs:
--
--   * `attempts` and `max_attempts` are `bigint`, so the engine's `u32` budget is stored without a
--     conversion that could fail (`Store::enqueue` takes a budget of any size, and a backend that
--     answered "too big for my column" would be a backend with an opinion the port does not have);
--   * the claim's predicate is `queue`, `state` and `run_at`, which is what the partial index below
--     is for. `claim` runs against a table that is mostly *not* due, on a statement that holds row
--     locks, so the index is what keeps a claim from scanning the finished work to find the queue.
--
-- The order the port promises (priority, then the longest due, then identity) is not an index
-- concern: `claim` sorts the rows it is about to stamp, which is a handful of rows, not the table.

create table jobs (
    id uuid primary key,
    queue text not null,
    payload jsonb not null,
    state text not null,
    priority integer not null,
    run_at timestamptz not null,
    attempts bigint not null,
    max_attempts bigint not null,
    last_error text
);

create index jobs_claimable on jobs (queue, run_at) where state = 'queued';
