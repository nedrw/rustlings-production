//! probe = "sec/token-lifetime@apply"
//! title = "Expired, replayed, tampered: three answers"
//! hint = "Existence first, then replay, then the window — and an answer describes the state the row is in."
//!
//! Step `mailer/1.12-token` made a confirmation link three claims at once: nobody can guess it,
//! it stops working after its window, and it confirms the row once. Each way that goes wrong has
//! its own sentence, because a support queue is people asking which one happened: a token nobody
//! was given is `404`, an expired one is `410`, and one that already worked is `409`.
//!
//! This probe re-does the store behind that route, with the clock injected: the real service reads
//! the database's `now()`, and a probe that sleeps for a day to test an expiry is a probe nobody
//! runs. The token's *entropy* is not part of this exercise — that is `getrandom` in step 1.12,
//! and a probe with no dependencies cannot re-check it honestly. What is re-checked here is
//! everything the store does with the value it is handed.
//!
//! ## The rules
//!
//! * [`Confirmations::register`] is what signup does with the token it generated: the row starts
//!   pending, with `expires_at = now + window`. A confirmation-token column is `UNIQUE`, so the
//!   store may assume no two registrations share a token.
//! * [`Confirmations::confirm`] answers the subscriber's address, or why not:
//!   1. a token nobody registered — or one character off a registered one — is [`Refusal::Unknown`],
//!      and nothing changes;
//!   2. a registered token that already confirmed somebody is [`Refusal::Replayed`], naming the
//!      address, **and the stamp does not move** — two clicks of one link must not move
//!      `confirmed_at` to the time of the second click;
//!   3. a registered, pending token whose deadline has passed (`now >= expires_at`) is
//!      [`Refusal::Expired`], and the subscriber is still pending;
//!   4. otherwise the token is spent: the answer is the address, and `confirmed_at` is `now`.
//! * the order matters, and it is decided by what the row *is*, not by the clock: a token that was
//!   used and whose window has since passed is [`Refusal::Replayed`], because that is the state it
//!   is in;
//! * the window is exclusive at the top: a token is usable while `now < expires_at`;
//! * the store never reads the system clock — every answer is about the `now` it was given.
//!
//! The statuses (`404`, `410`, `409`) are [`Refusal::status`]'s, and they are three different
//! numbers on purpose.

use std::collections::BTreeMap;

/// Why a confirmation was refused. The address is in the replay case only: a token is a secret, so
/// no refusal carries one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Nobody was handed this token: it was never issued, or a character of it changed.
    Unknown,
    /// The token is this service's, and its window has passed. Its subscriber is still pending.
    Expired,
    /// The token worked once already. The address is not waiting for anything.
    Replayed {
        /// The address that was confirmed by the first click.
        subscriber: String,
    },
}

impl Refusal {
    /// The status this failure answers with: `404`, `410` and `409`, because a person who clicked
    /// an old link is not a person who typed a broken one and neither is somebody probing.
    #[must_use]
    pub fn status(&self) -> u16 {
        todo!()
    }
}

/// One token, as it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The address this token was issued for.
    pub subscriber: String,
    /// The second the token stops working.
    pub expires_at: u64,
    /// `None` until this token confirmed its subscriber, then the second that happened.
    pub confirmed_at: Option<u64>,
}

/// The tokens this service handed out, keyed by the token's text.
#[derive(Debug)]
pub struct Confirmations {
    window: u64,
    rows: BTreeMap<String, Row>,
}

impl Confirmations {
    /// A store whose tokens are good for `window` seconds.
    #[must_use]
    pub fn new(window: u64) -> Self {
        Self {
            window,
            rows: BTreeMap::new(),
        }
    }

    /// Record a token as issued now, for `subscriber`.
    pub fn register(&mut self, token: &str, subscriber: &str, now: u64) {
        todo!()
    }

    /// Confirm with a token: the address it was issued for, or why it cannot be used.
    ///
    /// # Errors
    ///
    /// [`Refusal::Unknown`], [`Refusal::Expired`] or [`Refusal::Replayed`] — the three states a
    /// row can be in, which are three different sentences for three different mistakes.
    pub fn confirm(&mut self, token: &str, now: u64) -> Result<String, Refusal> {
        todo!()
    }

    /// When this subscriber confirmed, if they have.
    #[must_use]
    pub fn confirmed_at(&self, subscriber: &str) -> Option<u64> {
        todo!()
    }

    /// The stored row for a token, for the tests that read the table back.
    #[must_use]
    pub fn row(&self, token: &str) -> Option<&Row> {
        self.rows.get(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A day, as step `mailer/1.12-token`'s migration stamps it.
    const DAY: u64 = 24 * 60 * 60;

    #[test]
    fn a_token_confirms_its_subscriber_once() {
        let mut store = Confirmations::new(DAY);
        store.register("aaa", "ada@example.com", 1_000);
        store.register("bbb", "grace@example.com", 1_000);

        assert_eq!(
            store.confirm("aaa", 1_001),
            Ok("ada@example.com".to_owned()),
            "a token confirms the subscriber it was issued for"
        );
        assert_eq!(store.confirmed_at("ada@example.com"), Some(1_001));

        // The second click: told, and the stamp stays where the first click put it.
        assert_eq!(
            store.confirm("aaa", 1_002),
            Err(Refusal::Replayed {
                subscriber: "ada@example.com".to_owned(),
            }),
            "a link that worked once answers `409`, not `200`"
        );
        assert_eq!(
            store.confirmed_at("ada@example.com"),
            Some(1_001),
            "a replayed click must not move `confirmed_at` to a time nobody clicked anything"
        );

        // Somebody else's token is untouched by any of that.
        assert_eq!(
            store.confirm("bbb", 1_003),
            Ok("grace@example.com".to_owned())
        );
        assert_eq!(store.confirmed_at("grace@example.com"), Some(1_003));
    }

    #[test]
    fn a_window_that_has_passed_is_expired_and_leaves_the_subscriber_pending() {
        let mut store = Confirmations::new(60);
        store.register("inside", "ada@example.com", 1_000);
        store.register("edge", "grace@example.com", 1_000);

        // The last usable second, and the first one that is not: the window is exclusive at the top.
        assert_eq!(
            store.confirm("inside", 1_059),
            Ok("ada@example.com".to_owned()),
            "a token is good until its deadline"
        );
        assert_eq!(
            store.confirm("edge", 1_060),
            Err(Refusal::Expired),
            "the deadline is the second it stops working, not the second after"
        );
        assert_eq!(
            store.confirmed_at("grace@example.com"),
            None,
            "an expired token confirms nobody: the row is still pending"
        );
        assert_eq!(
            store.confirm("edge", 9_999),
            Err(Refusal::Expired),
            "the answer keeps describing the row, however long the link sits in an inbox"
        );

        // The subscriber is still waiting, so the service can issue a fresh link for them.
        store.register("fresh", "grace@example.com", 9_999);
        assert_eq!(
            store.confirm("fresh", 10_000),
            Ok("grace@example.com".to_owned())
        );
        assert_eq!(store.confirmed_at("grace@example.com"), Some(10_000));
    }

    #[test]
    fn a_token_one_character_off_is_unknown_just_like_one_nobody_got() {
        let mut store = Confirmations::new(DAY);
        store.register("aaaaaaaaaaaaaaaa", "ada@example.com", 1_000);

        let tampered = store.confirm("aaaaaaaaaaaaaaab", 1_001);
        let never_issued = store.confirm("zzzzzzzzzzzzzzzz", 1_002);
        assert_eq!(
            tampered,
            Err(Refusal::Unknown),
            "a changed character is not a link"
        );
        assert_eq!(
            tampered, never_issued,
            "a token somebody edited and a token nobody was given have to be the same answer: \
             telling them apart is an oracle for guessing tokens"
        );

        // Neither attempt touched the row, so the real link still works.
        assert_eq!(store.confirmed_at("ada@example.com"), None);
        assert_eq!(
            store.confirm("aaaaaaaaaaaaaaaa", 1_003),
            Ok("ada@example.com".to_owned())
        );
    }

    #[test]
    fn the_three_failures_are_three_answers() {
        let mut store = Confirmations::new(60);
        store.register("used", "ada@example.com", 1_000);
        store.register("old", "grace@example.com", 1_000);
        store.register("live", "ana@example.com", 1_000);

        // The first click works; the second one is the replay.
        assert_eq!(
            store.confirm("used", 1_010),
            Ok("ada@example.com".to_owned()),
            "the fixture click has to work, or the replay below is not a replay"
        );
        let replayed = store.confirm("used", 1_011).unwrap_err();
        let expired = store.confirm("old", 2_000).unwrap_err();
        let unknown = store.confirm("forged", 1_010).unwrap_err();

        assert_eq!(
            replayed,
            Refusal::Replayed {
                subscriber: "ada@example.com".to_owned(),
            }
        );
        assert_eq!(expired, Refusal::Expired);
        assert_eq!(unknown, Refusal::Unknown);

        // `403`-style answers instead of "not found" would confirm that the id exists, and one
        // status for all three leaves support with nothing to say.
        assert_eq!(unknown.status(), 404, "nobody has that token");
        assert_eq!(expired.status(), 410, "that link is gone");
        assert_eq!(replayed.status(), 409, "that link worked once");

        // A token that was used and whose window has since passed is still a replay: the answer
        // describes the state the row is in, not the clock.
        assert_eq!(
            store.confirm("used", 999_999),
            Err(Refusal::Replayed {
                subscriber: "ada@example.com".to_owned(),
            }),
            "a used token is history, not an expiry"
        );

        // And a live one still works, which is what makes the three above failures.
        assert_eq!(
            store.confirm("live", 1_010),
            Ok("ana@example.com".to_owned())
        );
    }
}
