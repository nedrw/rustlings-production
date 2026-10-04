//! probe = "db/atomic-claim@apply"
//! title = "One statement, disjoint batches"
//! hint = "Claim and stamp inside one critical section: a worker that reads the queue and then marks it hands the same row to somebody else."
//!
//! Step `relay/2.5-atomic-claim` is one statement — `SELECT … FOR UPDATE SKIP LOCKED` with the
//! claim stamped in the same breath — and the arithmetic is why: two workers that each read "the
//! next ten rows that nobody holds" read the *same* ten rows, and the overlap only shows up when
//! both of them have already sent something.
//!
//! The table here is a `BTreeMap` behind a `Mutex`, because a probe runs offline and in seconds.
//! The shape is the statement's:
//!
//! * `&mut self` is the transaction. Whoever holds the table is inside the statement, and the
//!   batch is decided and stamped before anybody else can look;
//! * the answer is the rows the worker now *holds* — not a list of ids to mark afterwards. That
//!   is the whole exercise: [`Deliveries::claim`] returns rows whose `claimed_by` is already this
//!   worker;
//! * a claim is the smallest unit of a lease. There is no expiry, no heartbeat and no reaper
//!   here — an abandoned claim is step `relay/2.15`'s problem, and pretending to re-check it
//!   would be a fake.
//!
//! ## What you write
//!
//! * [`Deliveries::claim`] — take up to `max` rows nobody holds, in id order, and write down the
//!   worker and the second on each of them. Fewer rows than `max` is an answer, not an error:
//!   the queue is allowed to run dry;
//! * [`Deliveries::release`] — give rows back, so a worker that stopped (or whose attempt failed)
//!   does not park them forever. The statement carries `AND claimed_by = $worker`, so a worker
//!   can only give back what it holds: freeing somebody else's row is not a release, it is the
//!   same read-then-write bug pointing the other way.
//!
//! ## What the tests ask
//!
//! Eight workers, one barrier, batches of seven over 200 rows: every row exactly once, and the
//! union of the eight answers is the whole table. `7` deliberately does not divide `200`, so the
//! last batch of every worker is a short one — that is where an off-by-one lives.
//!
//! The clock is a parameter. The real statement reads the database's `now()` in the same breath
//! as the claim, and a probe that waits for a wall clock is a probe nobody runs; what is re-checked
//! here is that the stamp and the claim are one write.

use std::collections::BTreeMap;

/// One row of the `deliveries` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The table's id, handed out once per row.
    pub id: u64,
    /// The subscriber or endpoint this delivery is for — what the worker serializes on.
    pub key: String,
    /// The worker holding this row right now, if one does.
    pub claimed_by: Option<String>,
    /// The second that worker took it.
    pub claimed_at: Option<u64>,
}

/// The queue a worker claims from.
#[derive(Debug)]
pub struct Deliveries {
    rows: BTreeMap<u64, Row>,
}

impl Deliveries {
    /// A queue with one unclaimed row per key, ids from 1 in the order given.
    #[must_use]
    pub fn new(keys: &[&str]) -> Self {
        let rows = keys
            .iter()
            .enumerate()
            .map(|(index, key)| {
                let row = Row {
                    id: index as u64 + 1,
                    key: (*key).to_owned(),
                    claimed_by: None,
                    claimed_at: None,
                };
                (row.id, row)
            })
            .collect();
        Self { rows }
    }

    /// Claim up to `max` rows nobody holds, oldest id first, and stamp them in the same breath.
    ///
    /// The rows that come back are the rows this worker now holds: the caller does not mark them
    /// afterwards, because a claim that is decided here and written there is the read-then-write
    /// the statement exists to replace. A worker that asks for more than is left gets what is
    /// left, and an empty answer means the queue ran dry rather than that something went wrong.
    #[must_use]
    pub fn claim(&mut self, worker: &str, max: usize, now: u64) -> Vec<Row> {
        todo!()
    }

    /// Give rows back: the worker stopped, or the attempt failed and the delivery is owed again.
    ///
    /// Releasing a row takes the claim off it, so the next [`Deliveries::claim`] can hand it to
    /// somebody else. Only the worker that holds a row can release it — a name that does not
    /// match leaves the row exactly as it was, because the statement this re-does carries
    /// `AND claimed_by = $worker`.
    pub fn release(&mut self, worker: &str, ids: &[u64]) {
        todo!()
    }

    /// The row, for the tests that read the table back.
    #[must_use]
    pub fn row(&self, id: u64) -> &Row {
        self.rows
            .get(&id)
            .unwrap_or_else(|| panic!("no row with id {id}"))
    }

    /// How many rows no worker holds: the backlog a claim of a bound size still has to chew
    /// through.
    #[must_use]
    pub fn free(&self) -> usize {
        self.rows
            .values()
            .filter(|row| row.claimed_by.is_none())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;

    /// How many rows the parallel test partitions, and how many workers do it.
    const ROWS: usize = 200;
    const WORKERS: usize = 8;
    /// What one worker asks for per claim. Not a divisor of `ROWS` on purpose.
    const BATCH: usize = 7;

    /// One claim of the shared table, the way a worker makes it: hold the table, answer, let go.
    fn claim(table: &Arc<Mutex<Deliveries>>, worker: &str, max: usize, now: u64) -> Vec<Row> {
        table
            .lock()
            .expect("the table is not poisoned")
            .claim(worker, max, now)
    }

    #[test]
    fn a_claim_takes_the_oldest_rows_and_stamps_them() {
        let mut table = Deliveries::new(&["ada", "grace", "ana", "linus", "edsger"]);

        let batch = table.claim("worker-1", 3, 100);
        assert_eq!(
            batch.iter().map(|row| row.id).collect::<Vec<u64>>(),
            vec![1, 2, 3],
            "a claim takes the oldest unclaimed rows: {batch:?}"
        );
        assert!(
            batch
                .iter()
                .all(|row| row.claimed_by.as_deref() == Some("worker-1")
                    && row.claimed_at == Some(100)),
            "the rows that came back are already stamped with the worker that claimed them: \
             {batch:?}"
        );
        for id in [1, 2, 3] {
            let row = table.row(id);
            assert_eq!(row.claimed_by.as_deref(), Some("worker-1"));
            assert_eq!(row.claimed_at, Some(100));
        }
        assert_eq!(table.free(), 2, "three of five rows are held now");

        // The rest is a short batch: asking for ten rows over a queue with two left is not an
        // error and must not invent eight more.
        let rest = table.claim("worker-1", 10, 200);
        assert_eq!(
            rest.iter().map(|row| row.id).collect::<Vec<u64>>(),
            vec![4, 5]
        );
        assert_eq!(table.free(), 0);

        // A batch size of zero is zero rows, not the default size.
        assert!(
            table.claim("worker-2", 0, 300).is_empty(),
            "`max` is a bound, and zero is a bound"
        );
    }

    #[test]
    fn a_claim_never_hands_over_a_row_somebody_holds() {
        let mut table = Deliveries::new(&["ada", "grace", "ana", "linus", "edsger"]);

        let first = table.claim("worker-1", 2, 100);
        assert_eq!(
            first.iter().map(|row| row.id).collect::<Vec<u64>>(),
            vec![1, 2]
        );

        let second = table.claim("worker-2", 2, 101);
        assert_eq!(
            second.iter().map(|row| row.id).collect::<Vec<u64>>(),
            vec![3, 4],
            "the second worker gets the rows the first one did not take: it skips what is held, \
             and it does not hand back a row that is: {second:?}"
        );
        assert_eq!(table.row(1).claimed_by.as_deref(), Some("worker-1"));
        assert_eq!(table.row(3).claimed_by.as_deref(), Some("worker-2"));

        // And once everything is held, a claim is empty rather than a second copy of the queue.
        assert_eq!(
            table.claim("worker-3", 10, 102).len(),
            1,
            "id 5 was still free"
        );
        assert!(
            table.claim("worker-4", 10, 103).is_empty(),
            "a fully claimed queue answers with nothing: handing the same row to a second worker is \
             how a delivery goes out twice"
        );
        assert_eq!(table.free(), 0);
    }

    #[test]
    fn eight_workers_claiming_in_parallel_partition_the_table() {
        let keys: Vec<String> = (1..=ROWS)
            .map(|index| format!("subscriber-{index}"))
            .collect();
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        let table = Arc::new(Mutex::new(Deliveries::new(&keys)));
        let barrier = Arc::new(Barrier::new(WORKERS));

        let mut handles = Vec::with_capacity(WORKERS);
        for index in 0..WORKERS {
            let table = Arc::clone(&table);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                let worker = format!("worker-{index}");
                // Everyone waits here, so the claims really do overlap: run them one after another
                // and even a read-then-write table would partition the queue.
                barrier.wait();
                let mut claimed = Vec::new();
                loop {
                    let batch = claim(&table, &worker, BATCH, 1_000);
                    if batch.is_empty() {
                        break;
                    }
                    claimed.extend(batch);
                }
                claimed
            }));
        }

        let claimed: Vec<Row> = handles
            .into_iter()
            .flat_map(|handle| {
                handle.join().unwrap_or_else(|_| {
                    panic!(
                        "a worker panicked while claiming: the table has to answer, not poison \
                            itself"
                    )
                })
            })
            .collect();

        assert_eq!(
            claimed.len(),
            ROWS,
            "the {WORKERS} workers claimed {} rows over a table of {ROWS} — every row leaves the \
             queue exactly once, so a longer union means two workers were handed the same row",
            claimed.len()
        );
        let mut ids: Vec<u64> = claimed.iter().map(|row| row.id).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            (1..=ROWS as u64).collect::<Vec<u64>>(),
            "the union of the batches is the whole table, with no row missing and none twice"
        );

        let table = table.lock().expect("the table is not poisoned");
        assert_eq!(table.free(), 0, "every row is held once the queue ran dry");
        for row in &claimed {
            assert_eq!(
                table.row(row.id).claimed_by,
                row.claimed_by,
                "the stamp in the table is the worker the claim was answered to"
            );
        }
    }

    #[test]
    fn a_released_row_is_owed_again() {
        let mut table = Deliveries::new(&["ada", "grace", "ana"]);

        let batch = table.claim("worker-1", 2, 100);
        assert_eq!(batch.len(), 2);
        let released: Vec<u64> = batch.iter().map(|row| row.id).collect();
        table.release("worker-1", &released);

        let row = table.row(1);
        assert_eq!(row.claimed_by, None, "the claim is off the row");
        assert_eq!(row.claimed_at, None, "and so is its stamp");
        assert_eq!(
            table.free(),
            3,
            "released rows are what the next claim takes"
        );

        // The next worker gets them, and it gets them first because they are the oldest free rows.
        let again = table.claim("worker-2", 3, 200);
        assert_eq!(
            again.iter().map(|row| row.id).collect::<Vec<u64>>(),
            vec![1, 2, 3],
            "a row that was released is claimable again: {again:?}"
        );
        assert_eq!(table.row(1).claimed_by.as_deref(), Some("worker-2"));
        assert_eq!(table.row(2).claimed_at, Some(200));

        // A release that does not name the holder is not a release: it is how one worker frees a
        // row another worker is in the middle of sending.
        table.release("worker-1", &[1, 2, 3]);
        assert_eq!(
            table.row(1).claimed_by.as_deref(),
            Some("worker-2"),
            "`worker-1` gave these rows back before `worker-2` took them"
        );
        assert_eq!(table.free(), 0);
    }

    #[test]
    fn an_empty_queue_answers_with_nothing() {
        let mut table = Deliveries::new(&[]);
        assert!(table.claim("worker-1", 5, 100).is_empty());
        assert_eq!(table.free(), 0);

        let mut table = Deliveries::new(&["ada"]);
        assert_eq!(table.claim("worker-1", 5, 100).len(), 1);
        assert!(
            table.claim("worker-2", 5, 200).is_empty(),
            "a claim that comes back empty is the queue saying \"nothing is owed\", and the \
             worker that got it already holds the row"
        );
        assert_eq!(
            table.row(1).claimed_at,
            Some(100),
            "the second claim did not restamp somebody else's row"
        );
    }
}
