//! probe = "db/port-conformance@apply"
//! title = "One suite, every backend"
//! hint = "The suite takes the port and a store, and it asserts the promises the port makes — never the order one backend happens to hand rows back in."
//!
//! Step `jobq/3.5-store-port-conformance` gives the engine a second place to keep jobs and, more
//! importantly, a way to know the two places are the same: **one conformance suite, run against
//! every implementation** (`conformance@2`, handoff-m2 §1.3). The suite is the contract; the
//! backends are the implementations; the engine only ever sees the port.
//!
//! What makes the suite worth writing is that it is *hard to write honestly*, and each of the
//! traps has an owner:
//!
//! * **a suite that asserts an order is a suite that only passes on one backend.** The port
//!   promises a *set* of due jobs for one worker, so a second backend is free to hand back the
//!   newest first. An assertion that "claim returns id 1, then id 2" is a test of the in-memory
//!   map, not of the port — and it is how a suite passes locally and fails against Postgres;
//! * **a suite that asserts nothing is decoration.** The way to know it still bites is to point it
//!   at a backend that *is* broken: one that hands the same row to two workers, or one that
//!   claims a job before its `run_at`. Both have to fail, and the tests below check that they do;
//! * **a claim hides the row.** `claim` and "mark it in my loop afterwards" is the same
//!   read-then-write bug [`Refused`] exists to prevent: what the suite can see from outside is
//!   that the second claimer does not get the row back.
//!
//! ## What you write
//!
//! * `impl Store for Memory` — the backend the course ships: the five methods, with the rules the
//!   port documents and nothing else;
//! * [`conformance`] — the suite. It is given a store, and it is the *only* thing in this file
//!   that decides whether a backend is a backend.
//!
//! [`Store`], [`Job`], [`State`] and [`Refused`] are given: the port is the fixed contract, and
//! the suite cannot be written against a port that moves.
//!
//! ## What the tests ask
//!
//! The suite runs against the course's own store, and then against a second backend written the
//! other way round (a `Vec` scanned from the end, so the newest due job comes first): both pass,
//! because neither backend is asserting anything about the other. Then the same suite runs against
//! two deliberately broken backends — one that never hides a claimed job and one that ignores
//! `run_at` — and it has to fail on both, because a suite that cannot fail is not evidence.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no Postgres and no container**: the second backend is a second
//! in-memory one that differs in the freedom the port grants (row order), which is the difference a
//! suite is most likely to trip over and the one a machine without a database can reproduce
//! exactly. The real gate re-runs the step's spec once per backend declared in `conformance.toml`,
//! with `KILN_BACKEND` choosing the store — that half is the project's, not this file's.
//!
//! Two more narrowings, stated rather than hidden: the real port returns `BoxFuture` (`jobq/3.1`),
//! and this one is the synchronous skeleton of the same five methods, because a probe has no
//! runtime to await on and a conformance suite is about behaviour rather than about the spelling of
//! a return type; and there is no transaction or rollback here, so "the write failed and nothing
//! changed" is not one of the promises the suite can check.

use std::collections::BTreeMap;

/// Where a job is in its life. The same four the engine uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Waiting to be claimed.
    Queued,
    /// Held by a worker.
    Running,
    /// Done, and it must never run again.
    Succeeded,
    /// Out of attempts, and it must not be claimed again.
    Failed,
}

/// Why a store refused a write. A refusal is an answer, not a panic: the suite drives these paths
/// on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The row is gone.
    Missing,
    /// The worker asking is not the one holding the row, or nobody holds it.
    NotHeld,
}

/// One row, as both the store and the suite read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// Identity, handed out once per row.
    pub id: u64,
    /// The queue it belongs to.
    pub queue: String,
    /// Where it is.
    pub state: State,
    /// The worker holding it, if one does.
    pub worker: Option<String>,
    /// Not before this instant.
    pub run_at: u64,
    /// Attempts started so far.
    pub attempts: u32,
    /// Attempts allowed in total.
    pub max_attempts: u32,
}

/// Where jobs live: the port, and the only thing a backend has to implement.
///
/// Every method here is a promise the suite is allowed to check. Anything a backend does beyond
/// these lines is its own business — a Postgres backend drives a connection, this file's backends
/// walk a collection — and a suite that can see the difference is a suite that will break the
/// first time somebody adds a third backend.
pub trait Store {
    /// Write a job down and hand back its identity.
    fn enqueue(&mut self, queue: &str, run_at: u64, max_attempts: u32) -> u64;

    /// Take up to `limit` jobs that are due at `now` and unheld, and mark them running for
    /// `worker`.
    ///
    /// The *set* is the promise: which rows come back, and in what order, is the backend's
    /// business. Two things are not up to the backend — a claim never hands over a row another
    /// worker holds, and when at least `limit` jobs are due and free the answer *is* `limit` of
    /// them, because a worker left idle next to a full queue is a queue that stopped.
    fn claim(&mut self, worker: &str, limit: usize, now: u64) -> Vec<Job>;

    /// Finish a job this worker holds.
    ///
    /// # Errors
    ///
    /// [`Refused::NotHeld`] when this worker does not hold the row.
    fn complete(&mut self, worker: &str, id: u64) -> Result<(), Refused>;

    /// Record a failed attempt and say where the job went: back to `State::Queued` while the
    /// budget lasts, to `State::Failed` when it does not.
    ///
    /// # Errors
    ///
    /// [`Refused::NotHeld`] when this worker does not hold the row.
    fn fail(&mut self, worker: &str, id: u64) -> Result<State, Refused>;

    /// Read a row, whoever holds it.
    fn get(&self, id: u64) -> Option<Job>;
}

/// The store this course ships first: one map, scanned in id order.
#[derive(Debug)]
pub struct Memory {
    rows: BTreeMap<u64, Job>,
    next_id: u64,
}

impl Memory {
    /// An empty store. Identity 0 is never handed out, so a missing id is never mistaken for one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rows: BTreeMap::new(),
            next_id: 1,
        }
    }
}

impl Default for Memory {
    fn default() -> Self {
        Self::new()
    }
}

impl Store for Memory {
    fn enqueue(&mut self, queue: &str, run_at: u64, max_attempts: u32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.rows.insert(
            id,
            Job {
                id,
                queue: queue.to_owned(),
                state: State::Queued,
                worker: None,
                run_at,
                attempts: 0,
                max_attempts,
            },
        );
        id
    }

    fn claim(&mut self, worker: &str, limit: usize, now: u64) -> Vec<Job> {
        // One pass over the map in id order, stamping as it goes: the rows that come back are the
        // rows this worker now holds, so nothing is "marked afterwards" by a caller that could be
        // interrupted between the two steps.
        let mut claimed = Vec::new();
        for row in self.rows.values_mut() {
            if claimed.len() == limit {
                break;
            }
            // `WHERE state = 'queued' AND run_at <= now`: a row somebody holds is skipped, and so
            // is one whose caller asked for a later instant.
            if row.state == State::Queued && row.run_at <= now {
                row.state = State::Running;
                row.worker = Some(worker.to_owned());
                row.attempts += 1;
                claimed.push(row.clone());
            }
        }
        claimed
    }

    fn complete(&mut self, worker: &str, id: u64) -> Result<(), Refused> {
        let Some(row) = self.rows.get_mut(&id) else {
            return Err(Refused::Missing);
        };
        // `AND worker = $worker`: finishing somebody else's row is not a completion, it is the
        // same read-then-write bug pointing the other way.
        if row.worker.as_deref() != Some(worker) {
            return Err(Refused::NotHeld);
        }
        row.state = State::Succeeded;
        row.worker = None;
        Ok(())
    }

    fn fail(&mut self, worker: &str, id: u64) -> Result<State, Refused> {
        let Some(row) = self.rows.get_mut(&id) else {
            return Err(Refused::Missing);
        };
        if row.worker.as_deref() != Some(worker) {
            return Err(Refused::NotHeld);
        }
        row.worker = None;
        // The attempt was counted when it was claimed, so the budget is already spent by the time
        // the failure arrives.
        row.state = if row.attempts < row.max_attempts {
            State::Queued
        } else {
            State::Failed
        };
        Ok(row.state)
    }

    fn get(&self, id: u64) -> Option<Job> {
        self.rows.get(&id).cloned()
    }
}

/// One promise a backend did not keep, named as the report has to name it.
///
/// The suite answers with the *first* promise it could not confirm rather than collecting them
/// all, because a backend that breaks one of these is usually breaking the next three for the
/// same reason, and the operator wants the cause rather than the symptom list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Broken {
    /// A job was claimed before its `run_at`.
    Waited {
        /// The job that was claimed early.
        id: u64,
    },
    /// A claim answered with fewer jobs than were due and free, leaving a worker idle next to a
    /// full queue.
    Starved {
        /// How many it did answer with.
        got: usize,
    },
    /// A claim answered with more jobs than it was asked for.
    OverLimit {
        /// How many it did answer with.
        got: usize,
    },
    /// A claimed row does not read back as held by the worker that claimed it: the claim was
    /// decided but not written down where the next claimer can see it.
    Unrecorded {
        /// The row.
        id: u64,
    },
    /// Two workers were handed the same job.
    Shared {
        /// The job both of them hold.
        id: u64,
    },
    /// The attempt a claim started was not counted on the row.
    Uncounted {
        /// The row.
        id: u64,
    },
    /// A write by a worker that does not hold the row was accepted.
    NotRefused {
        /// The row it was accepted on.
        id: u64,
    },
    /// The worker *does* hold the row, and its write was refused anyway.
    Refused {
        /// The row.
        id: u64,
    },
    /// A job that is finished — succeeded, or out of attempts — was handed out again.
    Revived {
        /// The row.
        id: u64,
    },
    /// A job that is owed again was not handed out at all.
    Lost {
        /// The row.
        id: u64,
    },
    /// A failure put the job somewhere other than the attempts left it.
    Misplaced {
        /// The row.
        id: u64,
        /// Where it went instead.
        state: State,
    },
}

/// The conformance suite: every promise [`Store`] makes, checked against one backend.
///
/// The store is handed over empty and is expected to be usable afterwards, so the suite cannot
/// assume identity values, insertion order or collection type — and neither may a backend. `Ok(())`
/// means every promise in this file held; `Err(broken)` names the first one that did not.
pub fn conformance<S: Store>(store: &mut S) -> Result<(), Broken> {
    // 1. `run_at` is a promise to the caller who scheduled the job, not a hint to the backend.
    let scheduled = store.enqueue("suite", 1_000, 2);
    if !store.claim("suite-early", 10, 999).is_empty() {
        return Err(Broken::Waited { id: scheduled });
    }

    // 2. A claim hands over the rows the worker now holds, and it hands over as many as it was
    //    asked for while there are that many due: three jobs are due by `now`, none is held.
    store.enqueue("suite", 0, 2);
    store.enqueue("suite", 0, 2);
    let held = store.claim("suite-w1", 2, 1_000);
    if held.len() > 2 {
        return Err(Broken::OverLimit { got: held.len() });
    }
    if held.len() < 2 {
        return Err(Broken::Starved { got: held.len() });
    }
    for row in &held {
        if row.state != State::Running || row.worker.as_deref() != Some("suite-w1") {
            return Err(Broken::Unrecorded { id: row.id });
        }
        if row.attempts != 1 {
            return Err(Broken::Uncounted { id: row.id });
        }
        // The answer and the store have to agree: a row the claimer holds is a row the next
        // claimer can see is held.
        match store.get(row.id) {
            Some(stored)
                if stored.state == State::Running
                    && stored.worker.as_deref() == Some("suite-w1") => {}
            _ => return Err(Broken::Unrecorded { id: row.id }),
        }
    }
    if !store.claim("suite-w2", 0, 1_000).is_empty() {
        return Err(Broken::OverLimit { got: 1 });
    }

    // 3. Nothing one worker holds is handed to another.
    let other = store.claim("suite-w2", 10, 1_000);
    for row in &other {
        if held.iter().any(|mine| mine.id == row.id) {
            return Err(Broken::Shared { id: row.id });
        }
    }

    // 4. A write is about the row *this* worker holds.
    let first = held[0].id;
    if store.complete("suite-w2", first).is_ok() || store.fail("suite-w2", first).is_ok() {
        return Err(Broken::NotRefused { id: first });
    }
    if store.get(first).map(|row| row.state) != Some(State::Running) {
        return Err(Broken::Unrecorded { id: first });
    }

    // 5. Success is terminal: the row stays readable and no worker is handed it again.
    if store.complete("suite-w1", first).is_err() {
        return Err(Broken::Refused { id: first });
    }
    if store.get(first).map(|row| row.state) != Some(State::Succeeded) {
        return Err(Broken::Unrecorded { id: first });
    }
    if store
        .claim("suite-w1", 10, u64::MAX)
        .iter()
        .any(|row| row.id == first)
    {
        return Err(Broken::Revived { id: first });
    }

    // 6. The attempt budget is spent by failing: one attempt left means the job is owed again,
    //    none means it is finished for good.
    let second = held[1].id;
    match store.fail("suite-w1", second) {
        Ok(State::Queued) => {}
        Ok(state) => return Err(Broken::Misplaced { id: second, state }),
        Err(_) => return Err(Broken::Refused { id: second }),
    }
    let retried = store
        .claim("suite-w1", 10, u64::MAX)
        .into_iter()
        .find(|row| row.id == second)
        .ok_or(Broken::Lost { id: second })?;
    if retried.attempts != 2 {
        return Err(Broken::Uncounted { id: second });
    }
    match store.fail("suite-w1", second) {
        Ok(State::Failed) => {}
        Ok(state) => return Err(Broken::Misplaced { id: second, state }),
        Err(_) => return Err(Broken::Refused { id: second }),
    }
    if store
        .claim("suite-w1", 10, u64::MAX)
        .iter()
        .any(|row| row.id == second)
    {
        return Err(Broken::Revived { id: second });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second backend, and the two broken ones, from one small core.
    ///
    /// `Vec` scanned from the end, which is the *other* legal answer to "which due jobs?" — the
    /// newest first. The two flags are the two ways to be wrong that the suite exists to catch:
    /// `hides: false` never marks a row as held (the same job goes to two workers), and
    /// `waits: false` claims a job before its `run_at`.
    #[derive(Debug)]
    struct VecStore {
        rows: Vec<Job>,
        next_id: u64,
        hides: bool,
        waits: bool,
    }

    impl VecStore {
        fn legal() -> Self {
            Self {
                rows: Vec::new(),
                next_id: 1,
                hides: true,
                waits: true,
            }
        }

        fn leaky() -> Self {
            Self {
                hides: false,
                ..Self::legal()
            }
        }

        fn blind() -> Self {
            Self {
                waits: false,
                ..Self::legal()
            }
        }

        fn find(&mut self, id: u64) -> Option<&mut Job> {
            self.rows.iter_mut().find(|row| row.id == id)
        }
    }

    impl Store for VecStore {
        fn enqueue(&mut self, queue: &str, run_at: u64, max_attempts: u32) -> u64 {
            let id = self.next_id;
            self.next_id += 1;
            self.rows.push(Job {
                id,
                queue: queue.to_owned(),
                state: State::Queued,
                worker: None,
                run_at,
                attempts: 0,
                max_attempts,
            });
            id
        }

        fn claim(&mut self, worker: &str, limit: usize, now: u64) -> Vec<Job> {
            let waits = self.waits;
            let hides = self.hides;
            let mut claimed = Vec::new();
            for row in self.rows.iter_mut().rev() {
                if claimed.len() == limit {
                    break;
                }
                let due = !waits || row.run_at <= now;
                let free = !hides || row.state == State::Queued;
                if due && free {
                    row.state = State::Running;
                    row.worker = Some(worker.to_owned());
                    row.attempts += 1;
                    claimed.push(row.clone());
                }
            }
            claimed
        }

        fn complete(&mut self, worker: &str, id: u64) -> Result<(), Refused> {
            let Some(row) = self.find(id) else {
                return Err(Refused::Missing);
            };
            if row.worker.as_deref() != Some(worker) {
                return Err(Refused::NotHeld);
            }
            row.state = State::Succeeded;
            row.worker = None;
            Ok(())
        }

        fn fail(&mut self, worker: &str, id: u64) -> Result<State, Refused> {
            let Some(row) = self.find(id) else {
                return Err(Refused::Missing);
            };
            if row.worker.as_deref() != Some(worker) {
                return Err(Refused::NotHeld);
            }
            row.worker = None;
            row.state = if row.attempts < row.max_attempts {
                State::Queued
            } else {
                State::Failed
            };
            Ok(row.state)
        }

        fn get(&self, id: u64) -> Option<Job> {
            self.rows.iter().find(|row| row.id == id).cloned()
        }
    }

    #[test]
    fn the_suite_passes_on_the_store_the_course_ships() {
        assert_eq!(
            conformance(&mut Memory::new()),
            Ok(()),
            "the suite has to accept the backend the course ships, or it is judging the wrong thing"
        );
    }

    #[test]
    fn the_suite_passes_on_a_backend_that_answers_in_another_order() {
        assert_eq!(
            conformance(&mut VecStore::legal()),
            Ok(()),
            "a second backend that hands back the newest due job first is legal: the port promises \
             a set, not an order — a suite that asserts an order only passes on one backend"
        );
    }

    #[test]
    fn the_suite_refuses_a_backend_that_hands_one_job_to_two_workers() {
        assert!(
            matches!(
                conformance(&mut VecStore::leaky()),
                Err(Broken::Shared { .. })
            ),
            "a store that does not hide a claimed row gives the same job to the next worker, which \
             is the bug the port exists to prevent: the suite has to catch it, and it has to say \
             which promise broke"
        );
    }

    #[test]
    fn the_suite_refuses_a_backend_that_claims_before_the_run_at() {
        assert!(
            matches!(
                conformance(&mut VecStore::blind()),
                Err(Broken::Waited { .. })
            ),
            "`run_at` is a promise to the caller who scheduled the job, not a hint to the backend"
        );
    }

    #[test]
    fn the_store_the_course_ships_keeps_its_own_promises() {
        let mut store = Memory::new();
        assert!(
            store.claim("w0", 5, 0).is_empty(),
            "an empty store has nothing to hand over"
        );

        // `run_at` is a promise to the caller who scheduled the job.
        let scheduled = store.enqueue("mail", 500, 2);
        assert!(
            store.claim("w1", 5, 100).is_empty(),
            "run_at is not a suggestion"
        );
        assert_eq!(
            store.get(scheduled).map(|row| row.state),
            Some(State::Queued),
            "an early claim left the row alone"
        );

        let first = store.enqueue("mail", 0, 2);
        let second = store.enqueue("mail", 0, 2);
        assert!(
            first < second,
            "identity is handed out once per row: {first} then {second}"
        );

        // A claim hands over rows the worker now holds, and hides them from everybody else.
        let held = store.claim("w1", 1, 100);
        assert_eq!(held.len(), 1, "`limit` is a bound: {held:?}");
        assert_eq!(
            held[0].id, first,
            "the store the course ships takes the oldest due job first"
        );
        assert_eq!(held[0].state, State::Running);
        assert_eq!(held[0].worker.as_deref(), Some("w1"));
        assert_eq!(held[0].attempts, 1, "claiming is starting an attempt");
        assert_eq!(
            store.get(first).and_then(|row| row.worker),
            Some("w1".to_owned()),
            "who holds a row is visible to the store's readers"
        );
        assert_eq!(
            store
                .claim("w2", 5, 100)
                .iter()
                .map(|row| row.id)
                .collect::<Vec<u64>>(),
            vec![second],
            "the second worker gets the rows the first one did not take"
        );
        assert!(store.claim("w3", 0, 100).is_empty(), "zero is a bound");
        assert_eq!(
            store.get(u64::MAX),
            None,
            "a missing id is `None`, not a refusal"
        );

        // A write by a worker that does not hold the row is refused, and changes nothing.
        assert_eq!(store.complete("w2", first), Err(Refused::NotHeld));
        assert_eq!(store.fail("w2", first), Err(Refused::NotHeld));
        assert_eq!(store.get(first).map(|row| row.state), Some(State::Running));

        // Success is terminal, and it is nobody's row afterwards.
        assert_eq!(store.complete("w1", first), Ok(()));
        assert_eq!(
            store.get(first).map(|row| row.state),
            Some(State::Succeeded)
        );
        assert_eq!(store.fail("w1", first), Err(Refused::NotHeld));
        let after = store.claim("w1", 5, 100_000);
        assert!(
            !after.iter().any(|row| row.id == first),
            "a job that succeeded must not be claimed again: {after:?}"
        );
        assert!(
            !after.iter().any(|row| row.id == second),
            "and the one the other worker holds is still theirs: {after:?}"
        );

        // The budget: this job gets two attempts, and the second failure is the last.
        assert_eq!(
            store.fail("w2", second),
            Ok(State::Queued),
            "an attempt is left, so the job is owed again"
        );
        assert_eq!(store.get(second).map(|row| row.attempts), Some(1));
        assert_eq!(
            store
                .claim("w1", 5, 100_000)
                .iter()
                .map(|row| row.id)
                .collect::<Vec<u64>>(),
            vec![second],
            "the released job is owed again, once its attempt budget allows another try"
        );
        assert_eq!(
            store.fail("w1", second),
            Ok(State::Failed),
            "the budget is two, so the second failure is the last"
        );
        let terminal = store.claim("w1", 5, 100_000);
        assert!(
            !terminal.iter().any(|row| row.id == second),
            "a failed job is terminal: {terminal:?}"
        );
        assert!(terminal.is_empty(), "nothing else was left: {terminal:?}");
    }
}
