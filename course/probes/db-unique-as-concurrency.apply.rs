//! probe = "db/unique-as-concurrency@apply"
//! title = "The constraint decides, not the caller"
//! hint = "Check and insert inside one critical section: a read followed by a write is the race the unique constraint exists to close."
//!
//! Step `mailer/1.11-signup` is about arithmetic, not style. Two callers sign up the same address
//! at the same moment; both `SELECT`s find nothing; both `INSERT`s go ahead — and one of them
//! fails against `UNIQUE (email)`, in production, in front of the person signing up. The only
//! thing that can decide a race between two callers is the thing they both talk to, and the
//! statement this probe re-does is `INSERT … ON CONFLICT DO NOTHING RETURNING id`: the winner gets
//! a row back, the loser gets nothing, and neither of them read anything first.
//!
//! The table here is a `Vec` behind a `Mutex`, because a probe runs offline and in seconds. The
//! shape is the database's: whoever holds the guard is the transaction, and **one guard has to
//! cover both the check and the insert**. Two guard acquisitions are the bug this exercise exists
//! to make visible.
//!
//! ## What you write
//!
//! * [`Signups::subscribe`] — insert the address, or hand back the fact that somebody was already
//!   there. New rows get the next id, and the answer a caller gets is [`Outcome::Inserted`] exactly
//!   once per address, [`Outcome::Taken`] for every other call;
//! * [`Signups::rows`] — what the table holds, in the order the rows were inserted;
//! * [`Outcome::status`] — `201` for the caller that inserted the row, `409` for everyone else.
//!
//! ## What the tests ask
//!
//! Eight threads, one barrier, one address: exactly one `Inserted`, seven `Taken`, one row, no
//! panic — and then the same race over addresses that differ, where every caller has to win.
//!
//! The id matters too: it is handed out once per row, so the id the winner was answered with is the
//! id the table holds.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// A row of the `subscribers` table, as far as this probe reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscriber {
    /// The table's id, handed out once per row.
    pub id: u64,
    /// The address, already normalised by a parser (step `mailer/1.10-domain-types`).
    pub email: String,
}

/// What one call to [`Signups::subscribe`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// This call inserted the row.
    Inserted {
        /// The id the table gave it.
        id: u64,
    },
    /// Somebody had already subscribed this address: nothing was inserted, and nothing is known
    /// about their row that the `INSERT … ON CONFLICT DO NOTHING` that answers this actually returns.
    Taken,
}

impl Outcome {
    /// The status this answer becomes: the winner has a row to show, everyone else is told.
    #[must_use]
    pub fn status(&self) -> u16 {
        todo!()
    }
}

/// The table and the constraint that decides who wins.
#[derive(Debug)]
pub struct Signups {
    rows: Mutex<Vec<Subscriber>>,
    next_id: AtomicU64,
}

impl Default for Signups {
    fn default() -> Self {
        Self::new()
    }
}

impl Signups {
    /// An empty table: the ids start at 1.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rows: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Subscribe an address, or find out that somebody was faster.
    ///
    /// Answers [`Outcome::Inserted`] for the call that put the row in the table and
    /// [`Outcome::Taken`] for every other call — including the ones that arrive at the same moment
    /// as the winner. It takes `&self` rather than `&mut self` on purpose: two callers of one
    /// service share one table, so the decision cannot live in the borrow checker.
    pub fn subscribe(&self, email: &str) -> Outcome {
        todo!()
    }

    /// The rows, in the order they were inserted.
    #[must_use]
    pub fn rows(&self) -> Vec<Subscriber> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::thread;

    /// How many callers race for one address. Eight is enough to lose the window between a read and
    /// a write on any machine, and small enough that a failure names something.
    const CALLERS: usize = 8;

    /// How many addresses the race is repeated over: one `Barrier` round per address, so a table
    /// that wins most races still has to win all of them.
    const ROUNDS: usize = 32;

    /// Race `CALLERS` threads for one address and answer what each of them was told.
    fn race(table: &Arc<Signups>, email: &str) -> Vec<Outcome> {
        let barrier = Arc::new(Barrier::new(CALLERS));
        let mut handles = Vec::with_capacity(CALLERS);
        for _ in 0..CALLERS {
            let table = Arc::clone(table);
            let barrier = Arc::clone(&barrier);
            let email = email.to_owned();
            handles.push(thread::spawn(move || {
                // Everyone waits here, so the calls really do overlap: a test that ran them one
                // after another would pass against a table with no constraint at all.
                barrier.wait();
                table.subscribe(&email)
            }));
        }
        handles
            .into_iter()
            .map(|handle| {
                handle.join().unwrap_or_else(|_| {
                    panic!(
                        "a caller panicked while subscribing: the table has to answer, not \
                            poison itself"
                    )
                })
            })
            .collect()
    }

    #[test]
    fn a_second_subscribe_for_one_address_finds_the_row_that_is_already_there() {
        let table = Signups::new();

        let first = table.subscribe("ada@example.com");
        assert_eq!(
            first,
            Outcome::Inserted { id: 1 },
            "the first caller inserts the row"
        );
        let second = table.subscribe("ada@example.com");
        assert_eq!(
            second,
            Outcome::Taken,
            "the second caller adds nothing: the address is already subscribed"
        );

        let rows = table.rows();
        assert_eq!(
            rows,
            vec![Subscriber {
                id: 1,
                email: "ada@example.com".to_owned(),
            }],
            "one row for one address, and no half-written one either: {rows:?}"
        );

        // A different address is a different row, with an id of its own.
        assert_eq!(
            table.subscribe("grace@example.com"),
            Outcome::Inserted { id: 2 },
            "the constraint is per address, not per table"
        );
        assert_eq!(table.rows().len(), 2);
    }

    #[test]
    fn eight_threads_racing_for_one_address_leave_exactly_one_row() {
        let table = Arc::new(Signups::new());
        let outcomes = race(&table, "ada@example.com");

        let winners: Vec<&Outcome> = outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Outcome::Inserted { .. }))
            .collect();
        assert_eq!(
            winners.len(),
            1,
            "exactly one of {CALLERS} simultaneous signups inserts the row — the ones that lost \
             are told, not left holding a duplicate: {outcomes:?}"
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, Outcome::Taken))
                .count(),
            CALLERS - 1,
            "every other caller gets the same answer: {outcomes:?}"
        );

        let rows = table.rows();
        assert_eq!(rows.len(), 1, "one address, one row: {rows:?}");
        let Outcome::Inserted { id } = winners[0] else {
            unreachable!("filtered above")
        };
        assert_eq!(
            rows[0].id, *id,
            "the id the winner was answered with is the id the table holds"
        );
    }

    #[test]
    fn racing_addresses_that_differ_all_get_their_own_row() {
        let table = Arc::new(Signups::new());
        let barrier = Arc::new(Barrier::new(CALLERS));
        let mut handles = Vec::with_capacity(CALLERS);
        for caller in 0..CALLERS {
            let table = Arc::clone(&table);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                table.subscribe(&format!("caller-{caller}@example.com"))
            }));
        }
        let outcomes: Vec<Outcome> = handles
            .into_iter()
            .map(|handle| handle.join().expect("a caller panicked while subscribing"))
            .collect();

        assert!(
            outcomes
                .iter()
                .all(|outcome| matches!(outcome, Outcome::Inserted { .. })),
            "addresses that differ do not compete: all of them insert: {outcomes:?}"
        );
        let rows = table.rows();
        assert_eq!(rows.len(), CALLERS, "one row per address: {rows:?}");

        let mut ids: Vec<u64> = rows.iter().map(|row| row.id).collect();
        ids.sort_unstable();
        let expected: Vec<u64> = (1..=CALLERS as u64).collect();
        assert_eq!(ids, expected, "every row gets an id of its own: {rows:?}");
    }

    #[test]
    fn the_race_is_decided_by_the_table_over_and_over() {
        let table = Arc::new(Signups::new());
        for round in 0..ROUNDS {
            let email = format!("round-{round}@example.com");
            let outcomes = race(&table, &email);
            assert_eq!(
                outcomes
                    .iter()
                    .filter(|outcome| matches!(outcome, Outcome::Inserted { .. }))
                    .count(),
                1,
                "round {round} for `{email}` did not have exactly one winner: {outcomes:?}"
            );
        }
        assert_eq!(
            table.rows().len(),
            ROUNDS,
            "{ROUNDS} addresses, {ROUNDS} rows — a race that is won twice is how a table ends up \
             with two spellings of one subscriber"
        );
    }

    #[test]
    fn the_outcome_table_answers_201_for_the_winner_and_409_for_everyone_else() {
        assert_eq!(
            Outcome::Inserted { id: 7 }.status(),
            201,
            "the caller that inserted the row is told what it created"
        );
        assert_eq!(
            Outcome::Taken.status(),
            409,
            "an address that is taken is not a server error: it is a decision the caller has to \
             hear"
        );
    }
}
