//! probe = "delivery/dlq@apply"
//! title = "Two ways to die, and a way back"
//! hint = "Give up on the first 4xx, give up when the attempts run out, and keep both out of the queue but queryable."
//!
//! Step `relay/2.10-dlq` is what a retry loop does when it stops: the row leaves the queue, and it
//! leaves it for one of two reasons that mean completely different things to the person looking at
//! it.
//!
//! * **a 4xx is a promise about the future.** The target said the request itself is wrong — that
//!   mailbox does not exist, that payload is not what the endpoint accepts. Attempt three is the
//!   same request meeting the same answer, so the delivery is [`State::Rejected`] on the first
//!   one. A 5xx is the opposite: the endpoint is unwell, not the request, so it is worth another
//!   attempt, right up to the budget;
//! * **running out of attempts is not a 4xx** either. [`State::Exhausted`] says "we tried, five
//!   times" — which an operator can act on by fixing the target and replaying, whereas a rejection
//!   is usually a data problem nobody can replay away;
//! * **out of the queue is not the same as gone.** Both terminal states stay queryable
//!   ([`Queue::dead`]), and [`Queue::replay`] puts one back.
//!
//! The third thing this probe insists on, because it is the bug the DLQ exists to prevent: a dead
//! row is never handed to a worker again. A poison pill that keeps coming back is a queue that
//! stops, and a key that is quietly starving while the worker retries the same row forever is the
//! worst version of it — the subscriber notices before the dashboard does.
//!
//! ## What you write
//!
//! * [`Queue::claim`] — the next delivery that is still owed, **never** a dead one;
//! * [`Queue::failed`] — the target's answer sorted into "never work" and "worth another try",
//!   with the budget the queue was built with deciding the latter;
//! * [`Queue::succeeded`] — it went out, which is a third way to leave the queue and *not* a way
//!   into the dead-letter set;
//! * [`Queue::dead`] and [`Queue::replay`] — what an operator queries and what they do about it.
//!
//! ## Where this is honest, and where it is not
//!
//! There is no HTTP here: the status is the number the caller reports, and a probe that stands up a
//! server to produce a 404 would be testing the server. What is missing on purpose: the operator's
//! tooling around the DLQ (that is `ops/dlq-tooling@apply`, a different capability and a different
//! step), the database these rows would live in, and any notion of *why* a replay happened — the
//! row keeps the counters, and who replayed it and when is an audit trail, not a delivery.
//!
//! A replayed delivery keeps its attempts, so it gets exactly one more: replay puts a message back
//! in front of a target that may have been fixed since, and an operator who wants a second full
//! budget is changing the budget — a decision with a number in it, not a button.

use std::collections::BTreeMap;

/// The three places a delivery can be, plus the one it can stay in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Still owed: a worker may claim it.
    Retryable,
    /// It went out. Not retryable, and deliberately not dead either.
    Delivered,
    /// The target answered that this request will never work: a 4xx.
    Rejected {
        /// The status the target answered with.
        status: u16,
    },
    /// The attempts ran out (`relay/2.8`'s `max_attempts`). The target may be fine; we stopped
    /// asking.
    Exhausted {
        /// How many times it was handed to a worker before we gave up.
        attempts: u32,
    },
}

/// One delivery, as the queue holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The queue's id.
    pub id: u64,
    /// The subscriber this delivery is for — the key a poison pill must not block.
    pub key: String,
    /// What the target would be sent.
    pub body: String,
    /// How many times this row was handed to a worker.
    pub attempts: u32,
    /// How many times somebody put it back after giving up on it.
    pub replays: u32,
    /// Where it is.
    pub state: State,
}

/// Why a replay was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No delivery with that id.
    Unknown,
    /// It is in the queue already, or it went out. Replay is for what we gave up on.
    NotDead,
}

/// The deliveries a worker claims from, and the ones we gave up on.
#[derive(Debug)]
pub struct Queue {
    rows: BTreeMap<u64, Row>,
    /// How many attempts a delivery gets before it is out of budget.
    max_attempts: u32,
}

impl Queue {
    /// A queue holding one delivery per `(key, body)`, ids from 1 in the order given.
    #[must_use]
    pub fn new(jobs: &[(&str, &str)], max_attempts: u32) -> Self {
        let rows = jobs
            .iter()
            .enumerate()
            .map(|(index, (key, body))| {
                let row = Row {
                    id: index as u64 + 1,
                    key: (*key).to_owned(),
                    body: (*body).to_owned(),
                    attempts: 0,
                    replays: 0,
                    state: State::Retryable,
                };
                (row.id, row)
            })
            .collect();
        Self { rows, max_attempts }
    }

    /// Hand a worker the oldest delivery that is still owed, and count the attempt.
    ///
    /// `None` means nothing is owed right now: every row has been delivered, or given up on, or is
    /// being worked on by somebody else. A row this queue gave up on is never handed out again —
    /// that is the whole reason it left.
    #[must_use]
    pub fn claim(&mut self) -> Option<Row> {
        // `WHERE state = 'retryable' ORDER BY id LIMIT 1`, in one pass: what we gave up on is not
        // a candidate, which is what keeps a poison pill from being handed out forever.
        let row = self
            .rows
            .values_mut()
            .find(|row| row.state == State::Retryable)?;
        // A claim is an attempt, and it is written down here rather than counted by the worker that
        // is about to be restarted.
        row.attempts += 1;
        Some(row.clone())
    }

    /// The target answered with this status: sort it into "never work" and "worth another try".
    ///
    /// The 4xx range is the target saying the *request* is wrong, so that is terminal on the first
    /// answer. Anything else is the endpoint being unwell, and it stays [`State::Retryable`] until
    /// the attempts reach the budget this queue was built with.
    pub fn failed(&mut self, id: u64, status: u16) {
        // Read the budget before the row is borrowed: they are two fields of one struct, and the
        // decision needs both.
        let max_attempts = self.max_attempts;
        let Some(row) = self.rows.get_mut(&id) else {
            return;
        };

        row.state = if (400..500).contains(&status) {
            // The target says the request itself will never work. Attempt three is the same request
            // meeting the same answer, so this is terminal on the first one.
            State::Rejected { status }
        } else if row.attempts >= max_attempts {
            // The endpoint may be perfectly fine; we simply stopped asking.
            State::Exhausted {
                attempts: row.attempts,
            }
        } else {
            State::Retryable
        };
    }

    /// It went out: out of the queue, and not into the dead-letter set.
    pub fn succeeded(&mut self, id: u64) {
        if let Some(row) = self.rows.get_mut(&id) {
            // Out of the queue, and out of the retry policy — but not into the dead-letter set: an
            // operator looking for things to replay is not looking for this one.
            row.state = State::Delivered;
        }
    }

    /// What we gave up on, oldest first, with the reason — what an operator queries.
    #[must_use]
    pub fn dead(&self) -> Vec<Row> {
        self.rows
            .values()
            .filter(|row| matches!(row.state, State::Rejected { .. } | State::Exhausted { .. }))
            .cloned()
            .collect()
    }

    /// Put a delivery we gave up on back in the queue, keeping its history.
    ///
    /// # Errors
    ///
    /// [`Refusal::Unknown`] for an id that is not there, [`Refusal::NotDead`] for one that is still
    /// owed or already went out.
    pub fn replay(&mut self, id: u64) -> Result<(), Refusal> {
        let Some(row) = self.rows.get_mut(&id) else {
            return Err(Refusal::Unknown);
        };
        if matches!(row.state, State::Retryable | State::Delivered) {
            // Nothing to undo: it is in the queue, or it already happened.
            return Err(Refusal::NotDead);
        }

        // The history stays: how many attempts it took to give up, and that somebody has put this
        // one back before. Only the state changes, so the replayed delivery costs its one attempt
        // from where it stopped rather than from zero.
        row.replays += 1;
        row.state = State::Retryable;
        Ok(())
    }

    /// The row, for the tests that read the table back.
    #[must_use]
    pub fn row(&self, id: u64) -> &Row {
        self.rows
            .get(&id)
            .unwrap_or_else(|| panic!("no delivery with id {id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One claim where the test does not care which delivery it was — the driver step of a test
    /// about what happens *after* the attempt.
    fn claim_one(queue: &mut Queue) -> Row {
        queue.claim().expect("a delivery is owed")
    }

    #[test]
    fn a_rejection_is_terminal_on_the_first_answer() {
        let mut queue = Queue::new(&[("ada", "a"), ("ada", "b")], 5);

        let claimed = queue.claim().expect("the queue has a delivery owed");
        assert_eq!(claimed.id, 1);
        assert_eq!(claimed.attempts, 1, "a claim is an attempt");
        queue.failed(1, 404);

        assert_eq!(
            queue.row(1).state,
            State::Rejected { status: 404 },
            "a mailbox that does not exist will not exist after two more attempts either"
        );
        assert_eq!(
            queue.claim().map(|row| row.id),
            Some(2),
            "the queue moved on to the next delivery rather than retrying the rejected one"
        );
        assert_eq!(
            queue.dead().len(),
            1,
            "and what we gave up on is still queryable"
        );
    }

    #[test]
    fn a_server_error_is_retryable_until_the_attempts_run_out() {
        let mut queue = Queue::new(&[("ada", "a")], 3);

        for attempt in 1..=3 {
            let claimed = queue.claim().expect("a 5xx keeps the delivery owed");
            assert_eq!(claimed.attempts, attempt, "each claim is one more attempt");
            queue.failed(1, 503);
        }

        assert_eq!(
            queue.row(1).state,
            State::Exhausted { attempts: 3 },
            "the failure that spends the last attempt is the one that gives up"
        );
        assert_eq!(
            queue.claim(),
            None,
            "and after that the delivery is not handed to anybody: a worker that keeps claiming it \
             is the loop the dead-letter queue replaces"
        );
        assert_eq!(
            queue.row(1).attempts,
            3,
            "the count is what the row says it is"
        );
        assert_eq!(
            queue.dead(),
            vec![queue.row(1).clone()],
            "out of the queue, still in the table"
        );

        // Another row is unaffected by any of it.
        let mut queue = Queue::new(&[("ada", "a"), ("grace", "b")], 3);
        claim_one(&mut queue);
        queue.failed(1, 500);
        assert_eq!(
            queue.row(1).state,
            State::Retryable,
            "one 5xx out of three attempts is not the end"
        );
        assert_eq!(queue.claim().map(|row| row.id), Some(1));
    }

    #[test]
    fn both_ways_out_of_the_queue_are_queryable_and_delivered_is_neither() {
        let mut queue = Queue::new(
            &[("ada", "rejected"), ("ada", "gone"), ("ada", "exhausted")],
            2,
        );

        claim_one(&mut queue);
        queue.failed(1, 422);

        claim_one(&mut queue);
        queue.succeeded(2);

        for _ in 0..2 {
            claim_one(&mut queue);
            queue.failed(3, 500);
        }

        assert_eq!(queue.row(1).state, State::Rejected { status: 422 });
        assert_eq!(queue.row(2).state, State::Delivered);
        assert_eq!(queue.row(3).state, State::Exhausted { attempts: 2 });
        assert_eq!(queue.claim(), None, "nothing is owed any more");

        let dead: Vec<(u64, State)> = queue.dead().iter().map(|row| (row.id, row.state)).collect();
        assert_eq!(
            dead,
            vec![
                (1, State::Rejected { status: 422 }),
                (3, State::Exhausted { attempts: 2 })
            ],
            "the dead-letter set is the two ways of giving up, oldest first, with the reason \
             attached"
        );
        assert!(
            queue.dead().iter().all(|row| row.id != 2),
            "a delivery that went out is not in the dead-letter set: an operator who replays \
             everything the queue is not holding re-sends it"
        );
    }

    #[test]
    fn a_poison_pill_does_not_hold_up_the_rest_of_the_queue() {
        let jobs = [
            ("ada", "poison"),
            ("ada", "for ada"),
            ("grace", "poison"),
            ("grace", "for grace"),
            ("ana", "poison"),
            ("ana", "for ana"),
        ];
        let mut queue = Queue::new(&jobs, 5);

        let mut claims = 0;
        let mut delivered = Vec::new();
        while let Some(row) = queue.claim() {
            claims += 1;
            assert!(
                claims <= jobs.len() * 2,
                "the queue handed out {claims} deliveries over {} rows: one we gave up on came back",
                jobs.len()
            );
            if row.body == "poison" {
                queue.failed(row.id, 400);
            } else {
                delivered.push(row.id);
                queue.succeeded(row.id);
            }
        }

        assert_eq!(claims, jobs.len(), "every delivery was handed out once");
        assert_eq!(
            delivered,
            vec![2, 4, 6],
            "the deliveries that were fine went through, in queue order"
        );
        assert_eq!(
            queue.dead().len(),
            3,
            "the three poison pills are the only ones we gave up on"
        );
        assert!(
            queue
                .dead()
                .iter()
                .all(|row| row.state == State::Rejected { status: 400 }),
            "and they are out of the queue for the reason the target gave: {:?}",
            queue.dead()
        );
        for id in [2, 4, 6] {
            assert_eq!(
                queue.row(id).attempts,
                1,
                "a delivery to another key did not pay for the poison pill's retries"
            );
        }
    }

    #[test]
    fn a_replay_puts_one_back_without_losing_its_history() {
        let mut queue = Queue::new(&[("ada", "a")], 2);
        for _ in 0..2 {
            claim_one(&mut queue);
            queue.failed(1, 500);
        }
        assert_eq!(queue.row(1).state, State::Exhausted { attempts: 2 });

        queue
            .replay(1)
            .expect("an exhausted delivery can be replayed");
        let row = queue.row(1);
        assert_eq!(row.state, State::Retryable, "it is owed again");
        assert_eq!(
            row.attempts, 2,
            "and the attempts it took are still on the row"
        );
        assert_eq!(
            row.replays, 1,
            "along with the fact that somebody put it back"
        );
        assert!(
            queue.dead().is_empty(),
            "a replayed delivery is not in the dead-letter set any more"
        );

        // A row that is still in the queue is not something to replay: there is nothing to undo.
        assert_eq!(queue.replay(1), Err(Refusal::NotDead));
        claim_one(&mut queue);
        assert_eq!(
            queue.row(1).attempts,
            3,
            "the replayed delivery gets the one more attempt replay is worth"
        );
        queue.failed(1, 500);
        assert_eq!(
            queue.row(1).state,
            State::Exhausted { attempts: 3 },
            "and the same answer gives up on it again, with the history intact"
        );
        assert_eq!(queue.dead().len(), 1);
        assert_eq!(
            queue.replay(99),
            Err(Refusal::Unknown),
            "there is no such row"
        );

        queue.replay(1).expect("the second replay works too");
        assert_eq!(queue.row(1).replays, 2);
        assert_eq!(queue.row(1).attempts, 3);
    }
}
