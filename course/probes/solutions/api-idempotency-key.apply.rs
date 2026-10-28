//! probe = "api/idempotency-key@apply"
//! title = "The same key twice is one effect"
//! hint = "A key says “this request, once”: same key and same payload replays the recorded answer, same key with another payload is a conflict, no key means no promise."
//!
//! Step `relay/2.2-idempotent-ingest` makes an event acceptable once however many times a client
//! posts it — a mobile client that retries after a timeout cannot know whether the first attempt
//! arrived. The key is the client's promise that two requests are one; the server's job is to keep
//! that promise without turning a confused client's *second, different* request into a silent
//! replay of the first.
//!
//! The store here is a vector of records, because a probe runs offline and in seconds. The shape is
//! the table's: the key is unique, the payload is what it was first recorded with, and the answer
//! is what that first request produced.
//!
//! ## What you write
//!
//! [`submit`] — what this request gets, given the store:
//!
//! * no key: [`Outcome::Fresh`], and **nothing is recorded** — an unkeyed request has made no
//!   promise, so there is nothing to remember it by;
//! * a key seen before with the same payload: [`Outcome::Replay`] with the recorded answer, and the
//!   store is unchanged;
//! * a key seen before with a different payload: [`Outcome::Conflict`], and the store is unchanged
//!   (the first request's answer stays the one that key replays);
//! * a key never seen: [`Outcome::Fresh`], and `(key, payload, answer)` is recorded.
//!
//! ## What the tests ask
//!
//! A first request; the same request twice (one record, a replay); a different payload under the
//! same key (conflict, and the original still replays); unkeyed requests repeated (two effects,
//! nothing recorded); two different keys; and a replay after other keys were recorded.

/// What one request produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The request was handled now.
    Fresh(String),
    /// The request was handled before; this is the recorded answer.
    Replay(String),
    /// The key is in use by a different request.
    Conflict,
}

/// The idempotency table.
#[derive(Debug, Default)]
pub struct Store {
    entries: Vec<(String, String, String)>,
}

impl Store {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many requests are recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// What `key` replays, if it is known.
    #[must_use]
    pub fn recorded(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(recorded, _, _)| recorded == key)
            .map(|(_, _, answer)| answer.as_str())
    }
}

/// Handle one request against the store.
pub fn submit(store: &mut Store, key: Option<&str>, payload: &str, answer: &str) -> Outcome {
    // An unkeyed request made no promise, so there is nothing to remember it by — and nothing to
    // compare a second one against.
    let Some(key) = key else {
        return Outcome::Fresh(answer.to_owned());
    };
    match store
        .entries
        .iter()
        .find(|(recorded, _, _)| recorded == key)
    {
        Some((_, recorded_payload, recorded_answer)) if recorded_payload == payload => {
            Outcome::Replay(recorded_answer.clone())
        }
        // The same key naming a different request is the client's mistake; replaying the first
        // answer would tell them the wrong thing happened.
        Some(_) => Outcome::Conflict,
        None => {
            store
                .entries
                .push((key.to_owned(), payload.to_owned(), answer.to_owned()));
            Outcome::Fresh(answer.to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_keyed_request_is_handled_now_and_recorded() {
        let mut store = Store::new();
        assert_eq!(
            submit(&mut store, Some("k1"), "event-1", "202 created"),
            Outcome::Fresh("202 created".to_owned())
        );
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn the_same_key_and_payload_is_a_replay_of_the_first_answer() {
        let mut store = Store::new();
        submit(&mut store, Some("k1"), "event-1", "202 created");
        assert_eq!(
            submit(&mut store, Some("k1"), "event-1", "202 created again"),
            Outcome::Replay("202 created".to_owned())
        );
        assert_eq!(store.len(), 1, "a replay is not a second record");
    }

    #[test]
    fn the_same_key_with_another_payload_is_a_conflict() {
        let mut store = Store::new();
        submit(&mut store, Some("k1"), "event-1", "202 created");
        assert_eq!(
            submit(&mut store, Some("k1"), "event-2", "202 other"),
            Outcome::Conflict
        );
        assert_eq!(store.len(), 1);
        assert_eq!(store.recorded("k1"), Some("202 created"));
    }

    #[test]
    fn an_unkeyed_request_is_never_deduplicated_and_never_recorded() {
        let mut store = Store::new();
        assert_eq!(
            submit(&mut store, None, "event-1", "202 a"),
            Outcome::Fresh("202 a".to_owned())
        );
        assert_eq!(
            submit(&mut store, None, "event-1", "202 b"),
            Outcome::Fresh("202 b".to_owned())
        );
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn two_keys_are_two_records() {
        let mut store = Store::new();
        submit(&mut store, Some("k1"), "event-1", "a");
        submit(&mut store, Some("k2"), "event-1", "b");
        assert_eq!(store.len(), 2);
        assert_eq!(store.recorded("k1"), Some("a"));
        assert_eq!(store.recorded("k2"), Some("b"));
    }

    #[test]
    fn a_replay_still_works_after_other_keys_were_recorded() {
        let mut store = Store::new();
        submit(&mut store, Some("k1"), "event-1", "first");
        submit(&mut store, Some("k2"), "event-2", "second");
        assert_eq!(
            submit(&mut store, Some("k1"), "event-1", "ignored"),
            Outcome::Replay("first".to_owned())
        );
        assert_eq!(store.len(), 2);
    }
}
