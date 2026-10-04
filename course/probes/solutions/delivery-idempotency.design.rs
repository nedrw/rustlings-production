//! probe = "delivery/idempotency@design"
//! title = "The deduplication window is a decision: for how long, and what happens at its edge"
//! hint = "Remember each key for a window from its first request, answer a repeat inside it with the first answer, and let exactly `t + window` be the instant the key is new again."
//!
//! Step `jobq/3.12-dedup-window` is the question `delivery/idempotency@apply` leaves open. That
//! file makes one delivery idempotent with a claim, a send and a record; this one is about the
//! *window*: how long the key is remembered, what a caller may assume after it, and what the store
//! has to keep to enforce it. A probe cannot run the service's database, so the store is a map and
//! what is injected is the **clock and the call site's work**: [`Dedup::accept`] is given a reading
//! and a closure, and its answer says whether the work ran.
//!
//! Three decisions, and each is a sentence a caller can rely on:
//!
//! 1. **the window is measured from the first request, and a duplicate does not extend it.** A key
//!    retried every second for an hour is remembered for one window, not for the hour: the guarantee
//!    is "the same intent, once", and a window a caller could extend by retrying would be a window
//!    the caller controls;
//! 2. **inside the window a repeat gets the *first* answer.** The store keeps the answer the first
//!    request produced, so a retry is neither a second execution nor an error — it is the same
//!    result, which is the only thing a caller can act on;
//! 3. **at exactly `t + window` the key is new again.** The window is half-open: an entry written at
//!    `t` is live while `now < t + window` and gone at `t + window`. This is the boundary the tests
//!    live on, because an implementation off by one request either deduplicates a legitimate repeat
//!    or executes a duplicate.
//!
//! ## What you write
//!
//! * [`Dedup::accept`] — run the work or answer from the store, and remember the answer;
//! * [`Dedup::held`] — how many keys the store is holding inside the window at a reading: the memory
//!   the guarantee costs;
//! * [`Dedup::purge`] — forget the keys whose window has passed, and say how many: housekeeping, not
//!   correctness.
//!
//! [`Dedup::new`], [`Dedup::entry`], [`Entry`] and [`Outcome`] are given. The work is a closure so
//! that the test can count how many times it ran, which is the only honest way to see a duplicate:
//! "the row is still there" is not the same fact as "the work did not run".
//!
//! ## What the tests ask
//!
//! A first request runs and a repeat does not, and the repeat's answer is the first one even when
//! the work would produce a different one now. One second before `t + window` is a duplicate and
//! one second after is fresh, and *at* `t + window` it is fresh — the window is half-open. A
//! duplicate in the middle of the window does not push the end out. `held` counts the keys inside
//! the window and `purge` reclaims the ones outside it, and a store that is never purged still
//! answers correctly, because the row's own timestamp is what says it is dead. A window of zero is
//! no deduplication at all.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no database and no clock**: the readings are parameters (a probe
//! that sleeps is a probe nobody runs) and the store is a map, so what is re-checked is the *policy*
//! — the boundary, the answer a repeat gets and the memory it costs — rather than the unique index
//! that would enforce it. The `INSERT … ON CONFLICT` that makes the window safe under concurrent
//! workers is `db/unique-as-concurrency@design`'s file, and the claim/send/record state machine is
//! `delivery/idempotency@apply`'s.
//!
//! Missing on purpose: nothing here is concurrent ([`Dedup::accept`] is `&mut self`, one caller at a
//! time), the window is the same for every key rather than a per-route policy, and the store keeps
//! the answer in memory rather than in a table with its own retention — the shape of the guarantee
//! is what is being re-checked, and each of those is a knob on top of it.

use std::collections::BTreeMap;

/// What happened to one keyed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The key was not remembered: the work ran, and this is its answer.
    Fresh(String),
    /// The key was still remembered: the work did **not** run, and this is the first answer.
    Duplicate(String),
}

/// One remembered key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// When the first request with this key was accepted.
    pub first_s: u64,
    /// The answer the first request produced, kept so that a repeat can have it too.
    pub answer: String,
}

/// The deduplication store: a window, and the keys inside it.
#[derive(Debug)]
pub struct Dedup {
    /// How long a key is remembered, from the first request that used it.
    window_s: u64,
    /// What is remembered.
    entries: BTreeMap<String, Entry>,
}

impl Dedup {
    /// A store that remembers a key for `window_s` seconds from the first request.
    #[must_use]
    pub fn new(window_s: u64) -> Self {
        Self {
            window_s,
            entries: BTreeMap::new(),
        }
    }

    /// Offer a request with this key at `now_s`.
    ///
    /// If the key is still remembered, nothing runs and the first answer comes back; otherwise the
    /// work runs, its answer is remembered, and the caller is told it was fresh. The window is
    /// stamped from the *first* request, so a duplicate never pushes the end out. A remembered key
    /// whose window has passed is treated as absent whether or not [`Dedup::purge`] has run.
    pub fn accept(&mut self, key: &str, now_s: u64, work: impl FnOnce() -> String) -> Outcome {
        if let Some(entry) = self.entries.get(key) {
            if now_s < entry.first_s.saturating_add(self.window_s) {
                return Outcome::Duplicate(entry.answer.clone());
            }
        }
        // Fresh, or a remembered key past its window: run the work and stamp a new window from now.
        // The work is called after the check, so a duplicate never runs it — that ordering is the
        // whole guarantee.
        let answer = work();
        self.entries.insert(
            key.to_owned(),
            Entry {
                first_s: now_s,
                answer: answer.clone(),
            },
        );
        Outcome::Fresh(answer)
    }

    /// How many keys the store is holding inside the window at `now_s`: the memory the guarantee
    /// costs, and the number an operator watches when the table is growing.
    #[must_use]
    pub fn held(&self, now_s: u64) -> usize {
        self.entries
            .values()
            .filter(|entry| now_s < entry.first_s.saturating_add(self.window_s))
            .count()
    }

    /// Forget every key whose window has passed as of `now_s`, and answer how many were forgotten.
    ///
    /// Housekeeping, not correctness: [`Dedup::accept`] already treats a dead key as absent. This is
    /// what keeps the table from growing forever.
    pub fn purge(&mut self, now_s: u64) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|_, entry| now_s < entry.first_s.saturating_add(self.window_s));
        before - self.entries.len()
    }

    /// The entry for a key, live or not, for the tests that read the table back.
    #[must_use]
    pub fn entry(&self, key: &str) -> Option<&Entry> {
        self.entries.get(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// The work a request does: record that it ran, and answer with something that changes each
    /// time, so that "the same answer came back" is a visible fact and not a coincidence.
    fn work(calls: &Cell<u32>) -> impl FnOnce() -> String + '_ {
        move || {
            calls.set(calls.get() + 1);
            format!("answer-{}", calls.get())
        }
    }

    #[test]
    fn a_first_request_runs_and_a_repeat_does_not() {
        let calls = Cell::new(0);
        let mut store = Dedup::new(60);

        assert_eq!(
            store.accept("k", 1_000, work(&calls)),
            Outcome::Fresh("answer-1".to_owned()),
            "the first request with a key runs the work"
        );
        assert_eq!(
            store.accept("k", 1_030, work(&calls)),
            Outcome::Duplicate("answer-1".to_owned()),
            "a repeat inside the window does not run it again, and gets the first answer"
        );
        assert_eq!(calls.get(), 1, "the work ran exactly once");
        assert_eq!(
            store.entry("k").expect("remembered").first_s,
            1_000,
            "the window is stamped from the first request"
        );
    }

    #[test]
    fn the_window_is_half_open_and_the_boundary_is_exact() {
        let calls = Cell::new(0);
        let mut store = Dedup::new(60);
        assert_eq!(
            store.accept("k", 1_000, work(&calls)),
            Outcome::Fresh("answer-1".to_owned())
        );

        assert_eq!(
            store.accept("k", 1_059, work(&calls)),
            Outcome::Duplicate("answer-1".to_owned()),
            "one second before the window closes, the key is still remembered"
        );
        assert_eq!(
            store.accept("k", 1_060, work(&calls)),
            Outcome::Fresh("answer-2".to_owned()),
            "at exactly t + window the key is new again: live means `now < t + window`"
        );
        assert_eq!(
            store.accept("k", 1_061, work(&calls)),
            Outcome::Duplicate("answer-2".to_owned()),
            "and the new window is stamped from the request that reopened it"
        );
        assert_eq!(calls.get(), 2, "the work ran once per window");
    }

    #[test]
    fn a_duplicate_does_not_push_the_window_out() {
        let calls = Cell::new(0);
        let mut store = Dedup::new(60);
        store.accept("k", 1_000, work(&calls));

        // Repeated every thirty seconds: the last-seen time is always recent, so a window that
        // followed the last request would never close.
        for at in [1_030, 1_059] {
            assert_eq!(
                store.accept("k", at, work(&calls)),
                Outcome::Duplicate("answer-1".to_owned()),
                "still the same intent at {at}"
            );
        }
        assert_eq!(
            store.accept("k", 1_060, work(&calls)),
            Outcome::Fresh("answer-2".to_owned()),
            "the window is from the first request, so it closes on schedule whatever the retries did"
        );
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn the_store_holds_the_keys_inside_the_window_and_purges_the_rest() {
        let calls = Cell::new(0);
        let mut store = Dedup::new(60);
        for (index, key) in ["a", "b", "c"].iter().enumerate() {
            store.accept(key, 1_000 + index as u64 * 10, work(&calls));
        }

        assert_eq!(store.held(1_050), 3, "all three are inside their windows");
        assert_eq!(
            store.held(1_065),
            2,
            "\"a\" (window ends at 1 060) has passed and the other two have not"
        );
        assert_eq!(
            store.held(1_070),
            1,
            "\"b\" (1 070) is at its boundary and \"c\" (1 080) is not"
        );

        assert_eq!(store.purge(1_070), 2, "two keys are forgotten");
        assert_eq!(store.entry("a"), None, "the dead ones are gone");
        assert!(store.entry("c").is_some(), "the live one is kept");
        assert_eq!(store.held(1_070), 1);
    }

    #[test]
    fn a_store_that_is_never_purged_still_answers_correctly() {
        let calls = Cell::new(0);
        let mut store = Dedup::new(60);
        store.accept("k", 1_000, work(&calls));

        assert_eq!(
            store.accept("k", 5_000, work(&calls)),
            Outcome::Fresh("answer-2".to_owned()),
            "the row's own timestamp says it is dead, whether or not housekeeping has run: a purge \
             that is late is memory, not a wrong answer"
        );
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn a_window_of_zero_is_no_deduplication_at_all() {
        let calls = Cell::new(0);
        let mut store = Dedup::new(0);
        assert!(matches!(
            store.accept("k", 1_000, work(&calls)),
            Outcome::Fresh(_)
        ));
        assert!(
            matches!(store.accept("k", 1_000, work(&calls)), Outcome::Fresh(_)),
            "even a repeat in the same second is fresh when the window is zero"
        );
        assert_eq!(calls.get(), 2, "the work ran both times");
        assert_eq!(store.held(1_000), 0, "and nothing is ever held");
    }
}
