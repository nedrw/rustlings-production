//! probe = "ops/dlq-tooling@operate"
//! title = "The 2am pass: page the dead letters, act once, and leave a trail that resumes"
//! hint = "A pass acts on a page, refuses what the page no longer allows, writes one line per act, and the next operator continues from the log alone."
//!
//! The `apply` half of this slot is the port's contract (`inspect`, `replay`, `discard` and the trail
//! that says who did). This file is the *operate* half: the procedure somebody runs at 2am against a
//! queue that keeps moving, and the three things an incident teaches about it.
//!
//! * **a pass is a page, not a queue.** The operator reads one page of rows, acts on them, and stops.
//!   The next pass starts where the last one ended. A pass that walked the whole queue is a pass
//!   nobody can stop, resume, or hand to a second person;
//! * **the page is stale by the time you act.** Between the read and the act, another operator or a
//!   worker may already have moved a row. Every order is checked against the row in hand, and the
//!   classes are answered in a fixed order — a discard with no reason, then an id this pass already
//!   acted on, then an id that is not on the page, then a row that is not dead any more. The order is
//!   part of the answer: a script that retried its own command must see `AlreadyActed` (its own
//!   business) rather than `NotDead` (the queue's). A refusal leaves the rows and the trail exactly
//!   where they were;
//! * **the trail is the handover.** After a pass, a *different* operator has to answer "what did the
//!   last one do, and where do I continue" from the log alone: one line per act, in order, with the
//!   actor, the reason on the irreversible one, and the page it belonged to.
//!
//! ## What you write
//!
//! * [`Door::pass`] — one page of orders, acted on or refused, one trail line per act;
//! * [`Door::resume`] — the page the next operator starts from, and the ids already acted on;
//! * [`passes`] — how many passes draining `pending` rows takes at a given page size.
//!
//! [`Row`], [`Order`], [`Action`], [`Refused`], [`Line`], [`Pass`], [`Handover`] and [`Door::new`] are
//! given: the row's shape and the classes are the project's, and a probe that let you invent them
//! would be checking a different tool.
//!
//! ## What the tests ask
//!
//! A pass acts on each order in the page once and writes one line per act, in order. The same id
//! twice in one pass is refused as `AlreadyActed` — a script that retried its own command must not put
//! the work back twice. A discard of a row that is not dead any more, and a replay of a row that is
//! already replayed, are refused with their own class. An id that is not in the page is `Unknown`.
//! Every refusal leaves the rows and the log untouched: the operator's next move is a call, not a
//! rollback. A pass that stops at its page size names the page to continue from, and the next pass acts
//! on the rows the first one did not touch. And draining 25 rows at 10 a page is three passes, not two
//! and not a loop with no bound.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no live queue**: the rows are a `Vec` here and a query there, and the
//! page is a slice rather than a `limit` with a cursor. What a probe can check is the procedure's
//! shape — one line per act, refusals by class, a resumable page, a bounded drain — and that is
//! exactly what the operator's half of this slot is a claim about. Also missing on purpose: no
//! permissions (who *may* replay is `sec/authz-tenant-isolation@operate`), no concurrency (two
//! operators on one queue is a store problem, and `jobq/3.6`'s transactions are where it is solved),
//! and no confirmation prompt (a tool that asks a question a script cannot answer is a tool that
//! hangs in a runbook).

/// A row as one page of the operator's screen shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Identity.
    pub id: u64,
    /// The queue it died in.
    pub queue: String,
    /// How many attempts it used.
    pub attempts: u32,
    /// What the budget was.
    pub max_attempts: u32,
    /// Whether it has already been put back once.
    pub replayed: bool,
}

impl Row {
    /// A dead row: the budget is spent and nobody has put it back yet.
    #[must_use]
    pub fn dead(id: u64, queue: &str) -> Self {
        Self {
            id,
            queue: queue.to_owned(),
            attempts: 3,
            max_attempts: 3,
            replayed: false,
        }
    }

    /// `true` while the row is one the door may act on.
    #[must_use]
    pub fn is_dead(&self) -> bool {
        !self.replayed && self.attempts >= self.max_attempts
    }
}

/// What an operator asked the door for. The actor is part of the order, not of the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Order {
    /// Put one job back in its queue.
    Replay { id: u64, actor: String },
    /// Throw one job away, with the reason it is being thrown away.
    Discard {
        id: u64,
        actor: String,
        reason: String,
    },
}

impl Order {
    /// The row this order is about.
    #[must_use]
    pub fn id(&self) -> u64 {
        match self {
            Order::Replay { id, .. } | Order::Discard { id, .. } => *id,
        }
    }

    /// `replay` or `discard`, the two words a trail line carries.
    #[must_use]
    pub fn action(&self) -> Action {
        match self {
            Order::Replay { .. } => Action::Replay,
            Order::Discard { .. } => Action::Discard,
        }
    }
}

/// The verb of one act.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Put it back.
    Replay,
    /// Throw it away.
    Discard,
}

impl Action {
    /// The word a trail line and a report print.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Replay => "replay",
            Action::Discard => "discard",
        }
    }
}

/// Why an order did not happen. A script branches on the class, so each one is its own answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// A discard with no reason. The only irreversible act needs one, and it is checked first.
    NoReason,
    /// The row is not dead any more: something replayed it, or it never was dead.
    NotDead,
    /// This pass already acted on that id. A retried command must not act twice.
    AlreadyActed,
    /// The id is not on this page. A typo in a loop has to be learnable.
    Unknown,
}

impl Refused {
    /// The word a report prints for this class.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Refused::NoReason => "no-reason",
            Refused::NotDead => "not-dead",
            Refused::AlreadyActed => "already-acted",
            Refused::Unknown => "unknown",
        }
    }
}

/// One line of the trail: what happened, to what, by whom, when, and on which page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The row it is about.
    pub id: u64,
    /// What was done.
    pub action: Action,
    /// Who said so.
    pub actor: String,
    /// Why, on the act that cannot be undone.
    pub reason: Option<String>,
    /// When, on the clock the caller passes in.
    pub at: u64,
    /// The page the act belonged to, so a handover can be read in order.
    pub page: usize,
}

/// What one pass did, in the order it did it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pass {
    /// The ids this pass acted on.
    pub acted: Vec<u64>,
    /// The orders it refused, with their class.
    pub refused: Vec<(u64, Refused)>,
    /// The page the *next* pass starts from.
    pub next: usize,
}

/// What a second operator needs to continue: where to start, and what is already spoken for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handover {
    /// The page to read next.
    pub page: usize,
    /// Every id the log already accounts for, in the order they were acted on.
    pub done: Vec<u64>,
}

/// The door, over one page at a time.
///
/// The rows are sorted by id and never reordered: a page is a window on that order, so the operator's
/// cursor is an index into the rows and the handover is a number.
pub struct Door {
    /// Every row the queue has, in id order.
    rows: Vec<Row>,
    /// How many rows one pass reads.
    page: usize,
    /// What has happened so far, oldest first.
    log: Vec<Line>,
}

impl Door {
    /// A door over `rows`, reading `page` of them per pass.
    #[must_use]
    pub fn new(mut rows: Vec<Row>, page: usize) -> Self {
        rows.sort_by_key(|row| row.id);
        Self {
            rows,
            page: page.max(1),
            log: Vec::new(),
        }
    }

    /// The trail so far.
    #[must_use]
    pub fn log(&self) -> &[Line] {
        &self.log
    }

    /// Act on the orders the page at `page` allows, and answer what happened.
    ///
    /// `page` is what [`Handover`] hands over, so a second operator continues with the same call.
    #[must_use]
    pub fn pass(&mut self, page: usize, orders: &[Order], at: u64) -> Pass {
        let _ = (page, orders, at);
        todo!(
            "ops/dlq-tooling@operate: page the rows, act once each, refuse by class, one line per act"
        )
    }

    /// Where the next operator starts, and what the log already accounts for.
    #[must_use]
    pub fn resume(&self) -> Handover {
        todo!("ops/dlq-tooling@operate: the next page and the ids the log already covers")
    }

    /// The row at `id`, when it is on the page at `page`.
    fn on_page(&self, page: usize, id: u64) -> Option<&Row> {
        let start = page * self.page;
        self.rows
            .get(start..(start + self.page).min(self.rows.len()))?
            .iter()
            .find(|row| row.id == id)
    }
}

/// How many passes draining `pending` rows takes at `page` rows a pass.
///
/// A drain loop needs a bound, and the bound is arithmetic rather than a condition somebody hopes
/// will become false (the runbook's `until the queue is empty` is how a pass hangs at 2am).
#[must_use]
pub fn passes(pending: usize, page: usize) -> usize {
    let _ = (pending, page);
    todo!("ops/dlq-tooling@operate: how many pages a drain of `pending` rows takes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqs(ids: &[u64]) -> Vec<Row> {
        ids.iter().map(|id| Row::dead(*id, "mail")).collect()
    }

    /// One line per act, in the order the orders were given.
    #[test]
    fn a_pass_acts_once_per_order_and_the_trail_says_so() {
        let mut door = Door::new(sqs(&[1, 2, 3]), 10);
        let pass = door.pass(
            0,
            &[
                Order::Replay {
                    id: 2,
                    actor: "ada".to_owned(),
                },
                Order::Discard {
                    id: 3,
                    actor: "ada".to_owned(),
                    reason: "payload is a test fixture".to_owned(),
                },
            ],
            1_700_000_000,
        );
        assert_eq!(pass.acted, [2, 3], "both orders were actionable");
        assert!(
            pass.refused.is_empty(),
            "nothing was refused: {:?}",
            pass.refused
        );
        let log = door.log();
        assert_eq!(
            log.len(),
            2,
            "one line per act, and no line for a look: {log:?}"
        );
        assert_eq!(log[0].id, 2);
        assert_eq!(log[0].action, Action::Replay);
        assert_eq!(log[0].actor, "ada");
        assert_eq!(
            log[0].at, 1_700_000_000,
            "the trail carries the instant the caller passed in: an operator's line without a time \
             is not evidence"
        );
        assert_eq!(
            log[0].reason, None,
            "a replay is reversible and needs no reason"
        );
        assert_eq!(log[1].action, Action::Discard);
        assert_eq!(
            log[1].reason.as_deref(),
            Some("payload is a test fixture"),
            "the irreversible act carries its reason into the trail"
        );
    }

    /// The same id twice in one pass is the retry a script makes, and it must not act twice.
    #[test]
    fn the_same_id_twice_in_one_pass_is_refused_the_second_time() {
        let mut door = Door::new(sqs(&[7]), 10);
        let pass = door.pass(
            0,
            &[
                Order::Replay {
                    id: 7,
                    actor: "ada".to_owned(),
                },
                Order::Replay {
                    id: 7,
                    actor: "ada".to_owned(),
                },
            ],
            1_700_000_000,
        );
        assert_eq!(pass.acted, [7]);
        assert_eq!(pass.refused, [(7, Refused::AlreadyActed)]);
        assert_eq!(door.log().len(), 1, "one act, one line: {:?}", door.log());
    }

    /// Refusals are classes a script branches on, and none of them touches anything.
    #[test]
    fn every_refusal_has_its_class_and_leaves_the_rows_and_the_trail_alone() {
        let mut door = Door::new(sqs(&[1, 2]), 10);
        let pass = door.pass(
            0,
            &[
                Order::Discard {
                    id: 1,
                    actor: "ada".to_owned(),
                    reason: "   ".to_owned(),
                },
                Order::Replay {
                    id: 99,
                    actor: "ada".to_owned(),
                },
            ],
            1_700_000_000,
        );
        assert_eq!(pass.acted, [] as [u64; 0], "nothing happened");
        assert_eq!(
            pass.refused,
            [(1, Refused::NoReason), (99, Refused::Unknown)],
            "each class is its own answer, and the unknown id is reported rather than ignored"
        );
        assert!(
            door.log().is_empty(),
            "a refusal writes nothing: {:?}",
            door.log()
        );
    }

    /// A row something else already moved is refused as `NotDead`.
    #[test]
    fn a_row_that_is_not_dead_any_more_is_refused() {
        let mut replayed = Row::dead(4, "mail");
        replayed.replayed = true;
        let mut door = Door::new(vec![replayed], 10);
        let pass = door.pass(
            0,
            &[Order::Discard {
                id: 4,
                actor: "ada".to_owned(),
                reason: "nobody wants it".to_owned(),
            }],
            1_700_000_000,
        );
        assert_eq!(pass.refused, [(4, Refused::NotDead)]);
        assert!(door.log().is_empty());
    }

    /// A pass is a page: the next operator continues where it stopped.
    #[test]
    fn a_pass_is_a_page_and_the_next_one_continues_from_the_handover() {
        let mut door = Door::new(sqs(&[1, 2, 3, 4, 5]), 2);
        let first = door.pass(
            0,
            &[Order::Replay {
                id: 1,
                actor: "ada".to_owned(),
            }],
            1_700_000_000,
        );
        assert_eq!(first.acted, [1]);
        assert_eq!(first.next, 1, "the next pass reads the second page");

        let handover = door.resume();
        assert_eq!(handover.page, 1);
        assert_eq!(
            handover.done,
            [1],
            "the log is what the next operator reads"
        );

        let second = door.pass(
            handover.page,
            &[Order::Discard {
                id: 4,
                actor: "bob".to_owned(),
                reason: "fixture".to_owned(),
            }],
            1_700_000_100,
        );
        assert_eq!(
            second.acted,
            [4],
            "the second page holds 3 and 4, so the order for 4 is actionable"
        );
        assert_eq!(door.log().len(), 2, "two acts, two lines: {:?}", door.log());
    }

    /// Draining has a size, and the size is arithmetic rather than a hope.
    #[test]
    fn a_drain_is_bounded_by_its_pages() {
        assert_eq!(passes(25, 10), 3, "three pages: 10, 10, 5");
        assert_eq!(
            passes(20, 10),
            2,
            "a page that is exactly full is two passes"
        );
        assert_eq!(passes(0, 10), 0, "nothing left is no pass at all");
        assert_eq!(
            passes(5, 0),
            passes(5, 1),
            "a page size of zero is one row a pass, not a loop that never ends"
        );
    }
}
