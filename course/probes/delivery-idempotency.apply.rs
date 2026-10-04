//! probe = "delivery/idempotency@apply"
//! title = "Claim, send, record"
//! hint = "The claim is a write, the stamp means the mail server took it, and a failed send gives the claim back."
//!
//! Step `mailer/1.15-idempotent-delivery` is three decisions, and every one of them is a way to
//! send the same confirmation email twice:
//!
//! 1. **the key is the intent, not the message.** What must not happen twice is *telling this
//!    subscriber about their confirmation*, so the state hangs off the subscriber — not a hash of
//!    the body, and not the token, which is exactly the thing a rotation changes;
//! 2. **the claim is a conditional write, and its answer is the truth.** `claim` does not read a row
//!    and then decide: it takes the claim for one caller and tells every other caller there is
//!    nothing to do;
//! 3. **the record is written after the send, and given back when it fails.**
//!    [`Deliveries::mark_sent`] means a mail server took the message;
//!    [`Deliveries::release`] puts the row back so the message stays owed instead of being stuck as
//!    "somebody is on it" forever — and the attempt count lives in the row, so "we gave up on this
//!    one" is a fact somebody can query rather than a line in a log that scrolled away.
//!
//! The sender is injected: a probe has no mail server, and the interesting failure is the one that
//! happens *between* two database writes.
//!
//! ## What you write
//!
//! * [`Deliveries::claim`] — take the confirmation for one subscriber if it is still owed to
//!   anybody, and count the attempt. `None` is not a failure: the message was sent, or another
//!   worker is on it right now;
//! * [`Deliveries::mark_sent`], [`Deliveries::release`], [`Deliveries::rotate`] — the three writes
//!   the worker and the signup route make;
//! * [`attempt`] — one attempt of the worker: claim, send **once**, record or release.
//!
//! [#Deliveries::row] is given, so the tests can read the table back; it is what a person queries
//! when they want to know how hard the service tried.
//!
//! The one hole this step does not fix, said out loud in its own prose: a worker that dies between
//! the claim and the send leaves a row claimed and unsent. What this probe re-checks is that no
//! number of retries, restarts or concurrent workers can send a confirmation twice.

use std::collections::BTreeMap;

/// The message a worker sends: built from the row at the moment somebody claimed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The address.
    pub to: String,
    /// The body, with the confirmation link in it.
    pub body: String,
}

/// The confirmation message of step `mailer/1.13-email-port`, built from the row.
#[must_use]
pub fn confirmation_message(email: &str, token: &str) -> Message {
    Message {
        to: email.to_owned(),
        body: format!("confirm your address with {token}"),
    }
}

/// The mail server, injected. `Ok` means a mail server took the message — nothing weaker counts.
pub trait Sender {
    /// Hand one message over.
    ///
    /// # Errors
    ///
    /// Whatever the mail server said. A failure is not fatal: the message is still owed.
    fn send(&mut self, message: &Message) -> Result<(), String>;
}

/// What one attempt of the worker did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attempt {
    /// Claimed, sent, and the row now says the message went out.
    Delivered,
    /// Nothing to do: already sent, or claimed by another worker right now.
    NothingToDo,
    /// The send failed and the claim went back. The count is what the row holds, not what this
    /// process remembers.
    Failed {
        /// How many times this subscriber's confirmation has been offered to a mail server.
        attempts: u32,
    },
}

/// The delivery columns of one row, as step 1.15's migration added them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The address, already normalised.
    pub email: String,
    /// The confirmation token the message carries.
    pub token: String,
    /// Whether the subscriber has confirmed (step `mailer/1.12-token`).
    pub confirmed: bool,
    /// When a worker took this confirmation, if one holds it right now.
    pub claimed_at: Option<u64>,
    /// When a mail server took the message. `None` means it is still owed.
    pub sent_at: Option<u64>,
    /// How many times the message was offered to a mail server.
    pub attempts: u32,
}

/// The rows a worker claims from: one per subscriber, keyed by address.
#[derive(Debug)]
pub struct Deliveries {
    rows: BTreeMap<String, Row>,
}

impl Deliveries {
    /// The rows a signup wrote: one pending subscriber per `(email, token)`.
    #[must_use]
    pub fn new(subscribers: &[(&str, &str)]) -> Self {
        let rows = subscribers
            .iter()
            .map(|(email, token)| {
                (
                    (*email).to_owned(),
                    Row {
                        email: (*email).to_owned(),
                        token: (*token).to_owned(),
                        confirmed: false,
                        claimed_at: None,
                        sent_at: None,
                        attempts: 0,
                    },
                )
            })
            .collect();
        Self { rows }
    }

    /// Take the confirmation for this subscriber, if it is still owed to anybody.
    ///
    /// A claim *is* an attempt — the worker sends once per claim — so the count of tries lives in
    /// the row and survives a restart. The message is rebuilt from the row, which is why the token
    /// never has to travel in a job.
    #[must_use]
    pub fn claim(&mut self, email: &str, now: u64) -> Option<Message> {
        todo!()
    }

    /// The mail server took the message: the stamp means that and nothing weaker, and the claim is
    /// cleared with it.
    pub fn mark_sent(&mut self, email: &str, now: u64) {
        todo!()
    }

    /// The send failed: the claim goes back, so the message stays owed and somebody can claim it
    /// again. Without this, one failed send makes the row unclaimable forever.
    pub fn release(&mut self, email: &str) {
        todo!()
    }

    /// A new token for the same subscriber: a rotated token is a different *message* and the same
    /// intent.
    pub fn rotate(&mut self, email: &str, token: &str) {
        todo!()
    }

    /// The row, for the tests that read the table back.
    #[must_use]
    pub fn row(&self, email: &str) -> &Row {
        self.rows
            .get(email)
            .unwrap_or_else(|| panic!("no row for `{email}`"))
    }

    /// Move a subscriber to confirmed, the way the confirmation route does.
    pub fn confirm(&mut self, email: &str) {
        if let Some(row) = self.rows.get_mut(email) {
            row.confirmed = true;
        }
    }
}

/// One attempt of the worker: claim, send once, record or release.
pub fn attempt(store: &mut Deliveries, sender: &mut dyn Sender, email: &str, now: u64) -> Attempt {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mail server that answers with whatever the test decided, and remembers what it was
    /// offered.
    #[derive(Debug, Default)]
    struct FakeSender {
        /// Every message that was handed over successfully.
        delivered: Vec<Message>,
        /// Every message the send was *tried* for while the sender was refusing them.
        refused: Vec<Message>,
        /// Whether the mail server is currently down.
        down: bool,
    }

    impl Sender for FakeSender {
        fn send(&mut self, message: &Message) -> Result<(), String> {
            if self.down {
                self.refused.push(message.clone());
                return Err("the mail server is not answering".to_owned());
            }
            self.delivered.push(message.clone());
            Ok(())
        }
    }

    #[test]
    fn one_attempt_sends_once_and_the_row_records_that_it_did() {
        let mut store = Deliveries::new(&[("ada@example.com", "tok-1")]);
        let mut sender = FakeSender::default();

        assert_eq!(
            attempt(&mut store, &mut sender, "ada@example.com", 100),
            Attempt::Delivered,
            "a pending subscriber's confirmation is owed, so the first attempt delivers it"
        );
        assert_eq!(
            sender.delivered,
            vec![confirmation_message("ada@example.com", "tok-1")],
            "the message is built from the row, and exactly one message went out"
        );
        let row = store.row("ada@example.com");
        assert_eq!(
            row.sent_at,
            Some(100),
            "the stamp means a mail server took it"
        );
        assert_eq!(
            row.claimed_at, None,
            "the claim is finished, not left behind"
        );
        assert_eq!(row.attempts, 1, "a claim is an attempt");

        // The second job for this subscriber — a retry, a requeue, a second signup — sends nothing.
        assert_eq!(
            attempt(&mut store, &mut sender, "ada@example.com", 200),
            Attempt::NothingToDo,
            "the row already says the message went out"
        );
        assert_eq!(
            sender.delivered.len(),
            1,
            "a mail server cannot help here: deduplicating is this service's job"
        );

        // Nobody sends a confirmation to a subscriber who already confirmed either.
        let mut store = Deliveries::new(&[("grace@example.com", "tok-2")]);
        store.confirm("grace@example.com");
        assert_eq!(
            attempt(
                &mut store,
                &mut FakeSender::default(),
                "grace@example.com",
                300
            ),
            Attempt::NothingToDo,
            "a confirmation for a confirmed subscriber is not owed by anybody"
        );
    }

    #[test]
    fn a_claim_is_a_write_so_a_second_worker_is_told_there_is_nothing_to_do() {
        let mut store = Deliveries::new(&[("ada@example.com", "tok-1")]);

        let first = store.claim("ada@example.com", 100);
        assert!(first.is_some(), "the first worker takes the message");
        assert_eq!(
            store.row("ada@example.com").claimed_at,
            Some(100),
            "the claim is written down, not remembered by the worker"
        );
        assert_eq!(
            store.claim("ada@example.com", 101),
            None,
            "while somebody holds the claim there is nothing to do: a second worker that read the \
             row and decided for itself is how one confirmation goes out twice"
        );
        assert_eq!(
            store.row("ada@example.com").attempts,
            1,
            "only a claim is an attempt"
        );

        store.mark_sent("ada@example.com", 102);
        assert_eq!(
            store.claim("ada@example.com", 103),
            None,
            "and once the stamp is there, nobody claims it again"
        );
    }

    #[test]
    fn a_failed_send_leaves_the_message_owed() {
        let mut store = Deliveries::new(&[("ada@example.com", "tok-1")]);
        let mut sender = FakeSender {
            down: true,
            ..FakeSender::default()
        };

        assert_eq!(
            attempt(&mut store, &mut sender, "ada@example.com", 100),
            Attempt::Failed { attempts: 1 },
            "a send that failed is not a delivery"
        );
        let row = store.row("ada@example.com");
        assert_eq!(
            row.sent_at, None,
            "the stamp is not a guess about the future"
        );
        assert_eq!(
            row.claimed_at, None,
            "the claim went back, or one failed send makes this subscriber unsendable forever"
        );

        // The mail server comes back, and the message is still owed — so the next attempt delivers
        // it, and the count shows there was one before.
        sender.down = false;
        assert_eq!(
            attempt(&mut store, &mut sender, "ada@example.com", 200),
            Attempt::Delivered
        );
        assert_eq!(
            sender.delivered.len(),
            1,
            "exactly one message reached the mail server: {:?}",
            sender.delivered
        );
        assert_eq!(sender.refused.len(), 1, "and one try was refused");
        assert_eq!(store.row("ada@example.com").attempts, 2);
    }

    #[test]
    fn giving_up_is_a_fact_the_row_holds() {
        let mut store = Deliveries::new(&[("ada@example.com", "tok-1")]);
        let mut sender = FakeSender {
            down: true,
            ..FakeSender::default()
        };

        for tries in 1..=3 {
            assert_eq!(
                attempt(&mut store, &mut sender, "ada@example.com", u64::from(tries)),
                Attempt::Failed { attempts: tries },
                "the count is in the row, so it survives the worker that counted it"
            );
        }
        let row = store.row("ada@example.com");
        assert_eq!(
            row.attempts, 3,
            "three offers to a mail server, three attempts"
        );
        assert_eq!(row.sent_at, None);
        assert_eq!(
            row.claimed_at, None,
            "a row that gave up is still claimable"
        );
    }

    #[test]
    fn a_rotated_token_is_a_new_message_for_the_same_intent() {
        let mut store = Deliveries::new(&[("ada@example.com", "tok-1")]);
        let mut sender = FakeSender {
            down: true,
            ..FakeSender::default()
        };

        assert_eq!(
            attempt(&mut store, &mut sender, "ada@example.com", 100),
            Attempt::Failed { attempts: 1 }
        );
        // A new link is issued: the message changes, the intent does not.
        store.rotate("ada@example.com", "tok-2");
        sender.down = false;
        assert_eq!(
            attempt(&mut store, &mut sender, "ada@example.com", 200),
            Attempt::Delivered
        );
        assert_eq!(
            sender.delivered,
            vec![confirmation_message("ada@example.com", "tok-2")],
            "the message is rebuilt from the row at the moment it is claimed: the new link is what \
             goes out, and the old one is not sent at all"
        );

        // And a rotation *after* the send is not a reason to tell the subscriber twice: the key is
        // the subscriber, not the token.
        store.rotate("ada@example.com", "tok-3");
        assert_eq!(
            attempt(&mut store, &mut sender, "ada@example.com", 300),
            Attempt::NothingToDo,
            "deduplicating on the message is how a rotated token produces a second confirmation"
        );
        assert_eq!(
            sender.delivered.len(),
            1,
            "one subscriber, one confirmation"
        );
    }
}
