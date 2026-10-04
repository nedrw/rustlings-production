//! probe = "db/schema-online-evolution@apply"
//! title = "Expand, backfill, contract — and a read that works the whole way through"
//! hint = "Write both shapes while both exist, read the new one and fall back to the old, backfill in resumable batches, and refuse the contract while any row is unconverted."
//!
//! Step `jobq/3.23-schema-evolution` is the three-phase shape that lets a table change under a
//! running service: **expand** (add the new columns, write both, read either), **backfill** (convert
//! the old rows in batches, resumable), **contract** (drop the old column, once nothing needs it). A
//! probe cannot run Postgres, so the store is injected: the SQL a migration would send is the methods
//! of [`Store`], and the fake in the tests is a `BTreeMap`.
//!
//! Three things make it three phases rather than one `ALTER`, and each one is a rule:
//!
//! 1. **while both shapes exist, a read has to answer for either.** A row the backfill has converted
//!    has the new columns; a row it has not has only the old one. [`read`] prefers the new shape and
//!    falls back to converting the old one on the fly, so there is no window in which half the table
//!    is unreadable;
//! 2. **the backfill is batched and resumable.** A table with a billion rows is not converted in one
//!    transaction; [`backfill`] converts a batch, hands back where to resume, and answers "done"
//!    when it walks off the end. It is idempotent — a batch re-run from an earlier cursor converts
//!    nothing twice — because the row's own new value is what says whether it is done, not the
//!    cursor;
//! 3. **the contract refuses to run early.** Dropping the old column while any row still needs it
//!    reads `NULL` into live requests. [`contract`] counts what is left and refuses with the number,
//!    so the failure is a migration that stopped rather than a production incident that started.
//!
//! ## What you write
//!
//! * [`split`] — one legacy value into the new shape, or `None` when it cannot be converted;
//! * [`read`] — the dual read: the new shape, or the old value converted on the fly;
//! * [`expand_insert`] — the new code path's write: a row with both shapes at once;
//! * [`backfill`] — one resumable batch;
//! * [`contract`] — drop the old column, or refuse with what is left.
//!
//! [`Store`], [`Parts`], [`Batch`], [`NotReady`] and the fake in the tests are given. The SQL is
//! replaced by the trait; what is re-checked is the *shape* of the migration, not the database.
//!
//! ## What the tests ask
//!
//! `split` accepts one `@` with something on each side and refuses the rest, and a value it refuses
//! is a row the backfill leaves for a person. The dual read answers for a converted row and an
//! unconverted one alike, and the new shape wins when both are there. A full backfill over 200 rows
//! in batches of seven (not a divisor, so the last batch is short) converts every row once; a second
//! run converts nothing; a run resumed from a saved cursor picks up where it left off, and a run
//! restarted from the beginning skips what is already done. Then the contract: refused while a row
//! is unconverted, with the count, and refused forever for a row that cannot be converted; and after
//! a clean backfill it drops the column and the dual read still answers from the new shape.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no Postgres and no migration runner**: the store is a trait and the
//! fake is a map, so the DDL is a method that flips a flag and the `SELECT`s are method calls. What
//! is re-checked is the policy the three phases implement — when each write is safe, what a read may
//! assume, and when the contract may run — which is the part a learner has to design and defend.
//!
//! Missing on purpose: nothing here is concurrent (the real expand step is safe because the old and
//! new code both run during a rolling deploy, and that overlap is `ops/rolling-upgrade`'s file),
//! there is no transaction or lock (`db/transactions@design` owns the isolation), and the fake's
//! [`Store::drop_legacy`] is one statement for the whole table rather than a DDL lock a probe could
//! wait on.

/// The new shape: an address split into its two columns, the ones the expand step adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parts {
    /// The part before the `@`.
    pub local: String,
    /// The part after it.
    pub domain: String,
}

/// The table, as the migration sees it.
///
/// Injected: the real one is Postgres and a probe cannot run one, so the SQL the migration would
/// send is replaced by these methods and the fake in the tests is a map.
pub trait Store {
    /// Every row id, ascending: what a backfill walks.
    fn ids(&self) -> Vec<u64>;
    /// The legacy column's value, while the column exists, or `None` after [`Store::drop_legacy`].
    fn legacy(&self, id: u64) -> Option<String>;
    /// The new shape, once the row has been converted.
    fn parts(&self, id: u64) -> Option<Parts>;
    /// The new code path's write: one row, the legacy value and the new shape, in one statement.
    fn insert(&mut self, legacy: &str, parts: Option<Parts>) -> u64;
    /// The backfill's write: the new shape for one row. Idempotent.
    fn set_parts(&mut self, id: u64, parts: Parts);
    /// `ALTER TABLE … DROP COLUMN`, one statement for the whole table.
    fn drop_legacy(&mut self);
}

/// One legacy value into the new shape, or `None` when it cannot be converted.
///
/// Exactly one `@` with something on each side. A value with no domain cannot be split, and a
/// backfill that guessed one would be inventing data: `None` leaves the row for a person, and the
/// contract step refuses while any row is left that way.
#[must_use]
pub fn split(legacy: &str) -> Option<Parts> {
    todo!()
}

/// The dual read: the new shape if the row has one, else the legacy value converted on the fly.
///
/// This is the whole reason the expand step is separate from the backfill: while both shapes exist,
/// a read has to answer for a converted row and an unconverted one alike, or the migration has a
/// window in which half the table is unreadable. The new shape wins when both are there, because
/// once written it is what the backfill and every new write agree on.
#[must_use]
pub fn read(store: &dyn Store, id: u64) -> Option<Parts> {
    todo!()
}

/// The expand-phase write: a new row goes in with both shapes at once.
///
/// The new code path cannot write only the new columns, because an instance still running the old
/// code during a rolling deploy reads the legacy one; and it must not write only the legacy one, or
/// the backfill would have to convert a row that was just inserted. Returns the new row's id.
#[must_use]
pub fn expand_insert(store: &mut dyn Store, legacy: &str) -> u64 {
    todo!()
}

/// What one batch of the backfill did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Batch {
    /// Rows this batch converted.
    pub converted: usize,
    /// Where to resume, or `None` when the batch reached the end of the table — which is the signal
    /// that there is nothing left to convert.
    pub next: Option<u64>,
}

/// One batch of the backfill.
///
/// Converts at most `batch` rows that still need it, oldest first, starting strictly after `cursor`
/// (pass `None` to start at the beginning). Returns the cursor to resume from, or `None` when the
/// walk reached the end. A row already converted is skipped rather than counted, so a batch re-run
/// from an earlier cursor — an interrupted migration, a retry after a crash — converts nothing
/// twice: the row's own new value is what says whether it is done, not the cursor.
#[must_use]
pub fn backfill(store: &mut dyn Store, cursor: Option<u64>, batch: usize) -> Batch {
    todo!()
}

/// Why the contract step refused to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotReady {
    /// Rows that still have no new-shape value.
    pub unconverted: usize,
}

/// The contract step: drop the legacy column, once no row needs it.
///
/// # Errors
///
/// [`NotReady`] with the number of rows that are still unconverted. The refusal is the point: a
/// contract that runs early reads `NULL` into every request that still needs the old column, and the
/// failure shows up in production rather than in the migration.
pub fn contract(store: &mut dyn Store) -> Result<(), NotReady> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// One row of the table while the migration runs: the legacy column if it is still there, and
    /// the new shape if the backfill has written it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Row {
        legacy: Option<String>,
        parts: Option<Parts>,
    }

    /// The injected store: a map, because a probe runs offline and in seconds.
    #[derive(Debug, Default)]
    struct MemStore {
        rows: BTreeMap<u64, Row>,
        next: u64,
        legacy_dropped: bool,
    }

    impl MemStore {
        /// A table of unconverted rows, ids from 1 in the order given: the state the expand step
        /// finds the table in.
        fn seeded(values: &[&str]) -> Self {
            let mut store = Self {
                rows: BTreeMap::new(),
                next: 1,
                legacy_dropped: false,
            };
            for value in values {
                store.insert(value, None);
            }
            store
        }
    }

    impl Store for MemStore {
        fn ids(&self) -> Vec<u64> {
            self.rows.keys().copied().collect()
        }

        fn legacy(&self, id: u64) -> Option<String> {
            self.rows.get(&id).and_then(|row| row.legacy.clone())
        }

        fn parts(&self, id: u64) -> Option<Parts> {
            self.rows.get(&id).and_then(|row| row.parts.clone())
        }

        fn insert(&mut self, legacy: &str, parts: Option<Parts>) -> u64 {
            let id = self.next;
            self.next += 1;
            self.rows.insert(
                id,
                Row {
                    legacy: (!self.legacy_dropped).then(|| legacy.to_owned()),
                    parts,
                },
            );
            id
        }

        fn set_parts(&mut self, id: u64, parts: Parts) {
            if let Some(row) = self.rows.get_mut(&id) {
                row.parts = Some(parts);
            }
        }

        fn drop_legacy(&mut self) {
            self.legacy_dropped = true;
            for row in self.rows.values_mut() {
                row.legacy = None;
            }
        }
    }

    /// Run the backfill to completion from the beginning, the way a job would.
    fn backfill_all(store: &mut dyn Store, batch: usize) -> usize {
        let mut cursor = None;
        let mut converted = 0;
        loop {
            let step = backfill(store, cursor, batch);
            converted += step.converted;
            match step.next {
                Some(next) => cursor = Some(next),
                None => return converted,
            }
        }
    }

    #[test]
    fn a_value_is_split_on_its_one_at_sign_or_not_at_all() {
        assert_eq!(
            split("ada@example.com"),
            Some(Parts {
                local: "ada".to_owned(),
                domain: "example.com".to_owned(),
            })
        );
        assert_eq!(
            split("nobody"),
            None,
            "no domain to split on: the row is for a person, not a guess"
        );
        assert_eq!(
            split("@example.com"),
            None,
            "an empty local part is not one"
        );
        assert_eq!(split("ada@"), None, "and an empty domain is not one either");
        assert_eq!(split(""), None);
    }

    #[test]
    fn the_dual_read_answers_for_a_converted_row_and_an_unconverted_one() {
        let mut store = MemStore::seeded(&["ada@example.com", "grace@example.com"]);
        // Convert the first row by hand — the backfill's job, done here so the read can be seen
        // working across both shapes at once.
        store.set_parts(1, split("ada@example.com").expect("splittable"));

        assert_eq!(
            read(&store, 1),
            split("ada@example.com"),
            "a converted row reads from the new shape"
        );
        assert_eq!(
            read(&store, 2),
            split("grace@example.com"),
            "an unconverted row reads by converting the old column on the fly"
        );
        assert_eq!(
            read(&store, 9),
            None,
            "a row that is not there reads as nothing"
        );

        // When both are there and disagree, the new shape wins: it is what every new write agrees
        // on, and the old column is on its way out.
        store.set_parts(
            2,
            Parts {
                local: "x".to_owned(),
                domain: "y".to_owned(),
            },
        );
        assert_eq!(
            read(&store, 2),
            Some(Parts {
                local: "x".to_owned(),
                domain: "y".to_owned(),
            }),
            "the new shape is authoritative"
        );
    }

    #[test]
    fn the_expand_write_puts_both_shapes_in_one_row() {
        let mut store = MemStore::seeded(&[]);
        let id = expand_insert(&mut store, "ada@example.com");
        assert_eq!(
            store.legacy(id).as_deref(),
            Some("ada@example.com"),
            "the old column is written too, for the instance still running the old code"
        );
        assert_eq!(
            store.parts(id),
            split("ada@example.com"),
            "and the new shape is there, so the backfill does not have to convert a fresh row"
        );
        assert_eq!(
            read(&store, id),
            split("ada@example.com"),
            "a row written by the new code is readable at once"
        );

        // A value the new shape cannot hold still goes in with the legacy column: it is not dropped
        // on the floor, and the contract will refuse until a person deals with it.
        let broken = expand_insert(&mut store, "nobody");
        assert_eq!(store.legacy(broken).as_deref(), Some("nobody"));
        assert_eq!(store.parts(broken), None);
        assert_eq!(read(&store, broken), None);
    }

    #[test]
    fn a_batched_backfill_converts_every_row_once_and_then_nothing() {
        let values: Vec<String> = (1..=200)
            .map(|index| format!("user{index}@example.com"))
            .collect();
        let values: Vec<&str> = values.iter().map(String::as_str).collect();
        let mut store = MemStore::seeded(&values);

        // Batches of seven over 200 rows: seven does not divide 200, so the last batch is short —
        // where an off-by-one lives.
        assert_eq!(
            backfill_all(&mut store, 7),
            200,
            "every row was converted exactly once"
        );
        assert_eq!(store.parts(200), split("user200@example.com"));
        assert_eq!(store.parts(1), split("user1@example.com"));

        // A second full run converts nothing: the rows are done, and "done" is the row's new value
        // rather than the cursor.
        let step = backfill(&mut store, None, 7);
        assert_eq!(step.converted, 0, "the backfill is idempotent");
        assert_eq!(step.next, None, "and it walks off the end");
    }

    #[test]
    fn a_resumed_backfill_picks_up_where_it_left_off() {
        let mut store = MemStore::seeded(&["a@x", "b@x", "c@x", "d@x", "e@x", "f@x"]);

        // Convert the first two, then "crash": only the cursor is kept.
        let first = backfill(&mut store, None, 2);
        assert_eq!(first.converted, 2);
        assert!(store.parts(1).is_some() && store.parts(2).is_some());
        assert_eq!(store.parts(3), None, "the third row has not been reached");

        // Resume from the saved cursor. The rows already done are behind it and are not touched.
        let second = backfill(&mut store, first.next, 2);
        assert_eq!(second.converted, 2, "the next two rows");

        // A run restarted *from the beginning* — a caller that lost its cursor — still makes
        // progress, because a converted row is skipped rather than counted.
        let restarted = backfill(&mut store, None, 2);
        assert_eq!(
            restarted.converted, 2,
            "the two rows that were not converted yet"
        );
        let left = store
            .ids()
            .into_iter()
            .filter(|id| store.parts(*id).is_none())
            .count();
        assert_eq!(left, 0, "and now nothing is left");
    }

    #[test]
    fn the_contract_refuses_while_a_row_is_unconverted() {
        let mut store =
            MemStore::seeded(&["ada@example.com", "grace@example.com", "ana@example.com"]);
        assert_eq!(
            backfill(&mut store, None, 1).converted,
            1,
            "one row converted, two left"
        );

        assert_eq!(
            contract(&mut store),
            Err(NotReady { unconverted: 2 }),
            "two rows still need the old column, so dropping it would read NULL into their requests"
        );
        assert_eq!(
            store.legacy(1).as_deref(),
            Some("ada@example.com"),
            "and the refusal dropped nothing"
        );

        // Finish the backfill and the contract goes through.
        assert_eq!(
            backfill_all(&mut store, 2),
            2,
            "the two rows that were left"
        );
        assert_eq!(contract(&mut store), Ok(()));
        assert_eq!(store.legacy(1), None, "the column is gone");
        assert_eq!(
            read(&store, 1),
            split("ada@example.com"),
            "and the dual read still answers, from the new shape"
        );
    }

    #[test]
    fn a_row_the_backfill_cannot_convert_holds_the_contract_open() {
        let mut store = MemStore::seeded(&["ada@example.com", "nobody"]);
        assert_eq!(
            backfill_all(&mut store, 10),
            1,
            "only the row that could be split was"
        );

        assert!(store.parts(1).is_some(), "the address was converted");
        assert_eq!(
            store.parts(2),
            None,
            "the row that could not is left for a person"
        );
        assert_eq!(
            contract(&mut store),
            Err(NotReady { unconverted: 1 }),
            "a migration that guessed a domain would be inventing data, so it stops instead"
        );
        assert_eq!(store.legacy(2).as_deref(), Some("nobody"));
    }
}
