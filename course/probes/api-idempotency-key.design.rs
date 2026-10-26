//! probe = "api/idempotency-key@design"
//! title = "The key belongs to the session, so a restart can still recognise the retry"
//! hint = "Refuse a session with no id, derive each key from the session id and the request bytes so a restart rebuilds it, and answer a repeat with the index the first application took."
//!
//! Step `meshkv/5.8-idempotent-sessions` is the step where the *client* becomes part of the
//! protocol. `5.7` taught it to find the leader; this one is about the answer that never arrives. A
//! timeout is ambiguous — the write may have committed, or the request may never have reached the
//! leader — so the client's only safe move is to ask again, and the second ask must not be a second
//! write.
//!
//! The mechanism has two halves, and the half this probe re-drills is the one a client library owns:
//!
//! 1. **the caller never writes the key.** The caller names a session and a request; the SDK derives
//!    the key. A key a caller chooses is a key a caller can reuse for a different write, and the
//!    store would swallow the second write as a duplicate of the first.
//! 2. **the key is a pure function of the session id and the request bytes.** No UUID per attempt,
//!    no process-local counter, no timestamp: a retry after a restart has to rebuild the *same* key
//!    from the same two inputs, or the new leader sees a new write. Two different pairs must never
//!    collide either, including the pair where the id and the request run together.
//! 3. **the session is an identity, and an anonymous one cannot be recognised.** A session with no
//!    id has nothing to resume; refusing it when it is built is cheaper than discovering it at the
//!    first retry.
//! 4. **a duplicate is answered with the index the first application took.** "Made no change" is not
//!    an answer: a caller whose write returned index 41 and retried has to get 41 back, not a fresh
//!    number and not a shrug. That is why the second application's closure must not even run.
//! 5. **the memory is the table, not the process, and only a successful write enters it.** A new
//!    leader that rebuilt the table replays the session; a leader that rebuilt from nothing applies
//!    the write again, which is why the table has to be durable. And a write the state machine
//!    refused is not a write, so its key stays free for the retry that succeeds.
//!
//! ## What you write
//!
//! * [`Session::new`] — a session with an id, or a refusal when there is no id to resume;
//! * [`Session::key_for`] — the key one intended write carries, derived from the session and the
//!   request bytes;
//! * [`Ledger::apply`] — apply a keyed write, or answer a key the table remembers with the index it
//!   took the first time.
//!
//! [`Key`], [`Outcome`], [`Answers`] and [`Ledger::new`] are given. The vocabulary is the interface:
//! the key is an opaque string on purpose — the store compares it and never parses it, so the
//! encoding is the learner's, and the tests only ask that it is a function of the two inputs and
//! cannot collide. [`Answers`] is the step's remembered answers, minus the window: the window is a
//! separate decision with its own probe, and this file takes the table as durable so that the
//! *contract* can be the subject. [`Outcome`] is the step's, with one deliberate difference: the
//! duplicate carries the index as well as being a duplicate, because "the original result" is
//! exactly what a retry has to return.
//!
//! ## What the tests ask
//!
//! A blank session id is refused, and a named session is not. The same id built twice — the restart
//! case — produces the same key for the same request, and a different id produces a different one.
//! One byte more at the end of a request is a different write, and the seam between the id and the
//! request is a real boundary: `ab` + `c` is not `a` + `bc`. A retry is answered with the first
//! index while the closure that applies the write is not called a second time. A new leader built
//! over the same table replays the session and answers the original index; a new leader built over
//! an empty table applies the write again, which is the cost the table exists to pay. A second
//! request in the same session is a second write, not a duplicate of the first. A refused write
//! leaves its key free, so the retry can be the write that succeeds. And five retries of one write
//! add exactly one entry to the table, not six.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, and **none of the machinery the step runs on**: no log, no leader, no
//! network, no failure detector and no concurrency. A "restart" is a new [`Ledger`] built over a
//! cloned [`Answers`], not a process boundary; the table is a map, not a table with its own
//! durability and retention; and the window that bounds the memory is deliberately absent. What is
//! re-checked is the contract a client library has to keep — a derived, deterministic, collision-free
//! key and a duplicate that returns the original result — because those are the parts an SDK can get
//! wrong without any of the distributed machinery in the room. The random UUID generated per attempt
//! is the bug this probe exists to catch, and it is invisible to every test that only checks "the
//! write did not run twice" on one leader.
//!
//! `delivery/idempotency@design` is the other half of the same idea on the delivery side, and its
//! subject is the window; `delivery/idempotency@apply` is claim, send, record.
//! `dist/client-routing@apply` is where the retry goes before it gets here: the client that was
//! redirected is the client that has to carry the key.

use std::collections::BTreeMap;

/// The key one intended write carries. It is opaque to the store: compared, never parsed.
pub type Key = String;

/// The index a write took in the replicated log.
pub type Index = u64;

/// A client's identity across retries, and the source of the keys they carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    id: String,
}

impl Session {
    /// A session with the id the SDK generated.
    ///
    /// # Errors
    ///
    /// A reason when the id has no content: an anonymous session cannot be recognised on a retry,
    /// which is the only thing a session is for.
    pub fn new(id: &str) -> Result<Self, String> {
        todo!()
    }

    /// The session's id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The key one intended write carries.
    ///
    /// A pure function of the session's id and the request bytes: the same request in the same
    /// session always produces the same key, on this process and on any other, and two different
    /// pairs of inputs never produce the same one.
    #[must_use]
    pub fn key_for(&self, request: &[u8]) -> Key {
        todo!()
    }
}

/// What the leader did with a keyed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The write was applied now, at this index.
    Applied {
        /// The index the write took.
        index: Index,
    },
    /// The key was seen before: the write was **not** applied again, and this is the index its
    /// first application took.
    Duplicate {
        /// The index the first application took — the answer the caller never got.
        index: Index,
    },
}

/// The answers the leader remembers: the table that survives a restart.
#[derive(Debug, Clone, Default)]
pub struct Answers {
    applied: BTreeMap<Key, Index>,
}

impl Answers {
    /// A table that remembers nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many keys the table remembers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.applied.len()
    }

    /// `true` when the table remembers nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.applied.is_empty()
    }

    /// The index this key's write took, if the table remembers the key.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<Index> {
        self.applied.get(key).copied()
    }

    /// Remember that `key`'s write took `index`.
    pub fn remember(&mut self, key: &str, index: Index) {
        self.applied.insert(key.to_owned(), index);
    }
}

/// One leader's memory of applied keys.
#[derive(Debug)]
pub struct Ledger {
    answers: Answers,
}

impl Ledger {
    /// A leader that remembers `answers` — after a restart, the table rebuilt from the log.
    #[must_use]
    pub fn new(answers: Answers) -> Self {
        Self { answers }
    }

    /// The table this leader remembers.
    #[must_use]
    pub fn answers(&self) -> &Answers {
        &self.answers
    }

    /// Apply the write `key` names, or answer with the index its first application took.
    ///
    /// The closure is what applying a *new* write does. It is not called for a key the table
    /// remembers, which is the whole guarantee: a duplicate is answered, not executed.
    ///
    /// # Errors
    ///
    /// Whatever the closure refused with. A refused write is not a write: it leaves no answer, so
    /// the key stays free for the retry that succeeds.
    pub fn apply(
        &mut self,
        key: &Key,
        apply: impl FnOnce() -> Result<Index, String>,
    ) -> Result<Outcome, String> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The named session every test resumes.
    fn session() -> Session {
        Session::new("s-1").expect("a named session")
    }

    #[test]
    fn an_anonymous_session_is_refused_and_a_named_one_is_not() {
        for blank in ["", " ", "\t "] {
            let error = Session::new(blank).expect_err("a session with no id cannot be resumed");
            assert!(
                error.contains("session"),
                "the refusal says what is missing, and it is the session: {error}"
            );
        }
        assert_eq!(
            session().id(),
            "s-1",
            "a named session keeps the id it was built with, because that id is what a retry is \
             recognised by"
        );
    }

    #[test]
    fn a_session_resumed_by_its_id_rebuilds_the_same_key() {
        let first = session();
        let resumed = Session::new("s-1").expect("the same id resumes the same session");
        assert_eq!(resumed.id(), "s-1");
        assert_eq!(
            first.key_for(b"set a 1"),
            resumed.key_for(b"set a 1"),
            "the key is derived from the id and the request: nothing about the process that built \
             it can be in it, or a restart turns every retry into a new write"
        );

        let other = Session::new("s-2").expect("another session");
        assert_ne!(
            first.key_for(b"set a 1"),
            other.key_for(b"set a 1"),
            "two sessions asking for the same request are two writes, not one"
        );
    }

    #[test]
    fn a_key_covers_the_session_and_every_byte_of_the_request() {
        let session = session();
        let key = session.key_for(b"set a 1");
        assert_eq!(
            session.key_for(b"set a 1"),
            key,
            "a retry is the same request, and the same request is the same key"
        );
        assert_ne!(
            key,
            session.key_for(b"set a 2"),
            "a different request is a different intended write"
        );

        assert_ne!(
            key,
            session.key_for(b"set a 1 "),
            "one byte at the end is enough to make it a different write: a key that ignores the \
             last byte answers the wrong request"
        );
        assert_ne!(
            key,
            session.key_for(b"set a 10"),
            "and a request that merely starts with the same bytes is not the same request"
        );

        let short = Session::new("ab").expect("a session").key_for(b"c");
        let longer = Session::new("a").expect("a session").key_for(b"bc");
        assert_ne!(
            short, longer,
            "the session id and the request are two fields, not one string: `ab` + `c` and `a` + \
             `bc` are different writes, and a key that joins them without a boundary says otherwise"
        );
    }

    #[test]
    fn a_retry_is_answered_with_the_index_the_first_application_took() {
        let mut ledger = Ledger::new(Answers::new());
        let key = session().key_for(b"set a 1");
        let mut applied = 0;

        assert_eq!(
            ledger
                .apply(&key, || {
                    applied += 1;
                    Ok(41)
                })
                .expect("the write is applied"),
            Outcome::Applied { index: 41 },
            "the first application answers with the index its write took"
        );
        assert_eq!(
            ledger
                .apply(&key, || {
                    applied += 1;
                    Ok(99)
                })
                .expect("the retry is answered, not refused"),
            Outcome::Duplicate { index: 41 },
            "the duplicate carries the original index: a fresh 99 would tell the caller a second \
             write happened, and 'no change' would tell it nothing"
        );
        assert_eq!(
            applied, 1,
            "the closure that applies the write did not run a second time, whatever it would have \
             answered"
        );
    }

    #[test]
    fn a_restart_replays_the_table_and_not_the_process_memory() {
        let key = session().key_for(b"set a 1");
        let mut before = Ledger::new(Answers::new());
        assert_eq!(
            before.apply(&key, || Ok(41)).expect("applied"),
            Outcome::Applied { index: 41 }
        );

        // The process is gone; the table is what it left behind.
        let mut after_restart = Ledger::new(before.answers().clone());
        let resumed = Session::new("s-1").expect("the id is the session");
        assert_eq!(
            after_restart
                .apply(&resumed.key_for(b"set a 1"), || Ok(99))
                .expect("the retry is answered"),
            Outcome::Duplicate { index: 41 },
            "a resumed session asks with the same key, so the new leader recognises the write and \
             answers the index the first application took"
        );

        // The other direction: a table that was lost recognises nothing.
        let mut lost = Ledger::new(Answers::new());
        assert_eq!(
            lost.apply(&key, || Ok(99)).expect("applied again"),
            Outcome::Applied { index: 99 },
            "the memory is the table, not the process: a leader that rebuilt from nothing applies \
             the write again, which is why the table has to be durable"
        );
    }

    #[test]
    fn a_second_request_in_the_same_session_is_not_swallowed_as_a_duplicate() {
        let session = session();
        let first = session.key_for(b"set a 1");
        let second = session.key_for(b"set a 2");
        assert_ne!(
            first, second,
            "the key has to depend on the request as well as the session"
        );

        let mut ledger = Ledger::new(Answers::new());
        let mut applied = 0;
        assert_eq!(
            ledger
                .apply(&first, || {
                    applied += 1;
                    Ok(41)
                })
                .expect("the first write"),
            Outcome::Applied { index: 41 }
        );
        assert_eq!(
            ledger
                .apply(&second, || {
                    applied += 1;
                    Ok(42)
                })
                .expect("the second write"),
            Outcome::Applied { index: 42 },
            "a different request in the same session is a different write: answering it as a \
             duplicate would silently drop it"
        );
        assert_eq!(applied, 2, "both writes were applied, once each");
    }

    #[test]
    fn a_refused_write_leaves_the_key_free() {
        let key = session().key_for(b"set a 1");
        let mut ledger = Ledger::new(Answers::new());

        assert_eq!(
            ledger.apply(&key, || Err("the value is not an integer".to_owned())),
            Err("the value is not an integer".to_owned()),
            "the state machine's refusal is carried through untouched"
        );
        assert!(
            ledger.answers().get(&key).is_none(),
            "a refused write is not a write: remembering it would answer the retry with a \
             failure that never applied"
        );
        assert_eq!(
            ledger.apply(&key, || Ok(41)).expect("the retry succeeds"),
            Outcome::Applied { index: 41 },
            "so the key is still free, and the retry can be the write that succeeds"
        );
    }

    #[test]
    fn retries_do_not_grow_the_table() {
        let session = session();
        let key = session.key_for(b"set a 1");
        let mut ledger = Ledger::new(Answers::new());
        let mut applied = 0;

        for attempt in 0..6 {
            let outcome = ledger
                .apply(&key, || {
                    applied += 1;
                    Ok(41)
                })
                .expect("applied once and answered five times");
            if attempt == 0 {
                assert_eq!(outcome, Outcome::Applied { index: 41 });
            } else {
                assert_eq!(
                    outcome,
                    Outcome::Duplicate { index: 41 },
                    "attempt {attempt} is the same write, not a sixth one"
                );
            }
        }
        assert_eq!(
            ledger.answers().len(),
            1,
            "a retry is answered from the table; it does not add an entry"
        );
        assert_eq!(applied, 1, "and only the first attempt ran the closure");

        let other = session.key_for(b"set a 2");
        let _ = ledger
            .apply(&other, || Ok(42))
            .expect("a new intended write");
        assert_eq!(
            ledger.answers().len(),
            2,
            "a new intended write adds exactly one entry, however many times it is retried"
        );
    }
}
