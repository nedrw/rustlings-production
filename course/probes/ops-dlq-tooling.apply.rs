//! probe = "ops/dlq-tooling@apply"
//! title = "Inspect, replay, discard — and the trail that says who did"
//! hint = "Reading is not an action; a replay of a job that is already replayed changes nothing and says so; a discard without a reason is refused before anything moves."
//!
//! Step `jobq/3.16-dlq-tooling` is the operator's half of the dead-letter queue. `relay/2.10`
//! decided *when* a job is dead; this step is about the tool somebody points at a production
//! incident at 2am, and the three things it has to get right are all about not making the incident
//! worse:
//!
//! * **reading is not an action.** [`Dlq::inspect`] takes `&self`, so the compiler is the first half
//!   of the rule, and the second half is that looking never writes a line to the audit trail. An
//!   inspect that logs is an inspect whose log an auditor cannot use to tell who *did* something;
//! * **a replay is idempotent, and an idempotent no-op is not a second run.** Replaying a job that
//!   was already replayed answers "already" rather than replaying it again. That is what lets a
//!   script retry its own command after a timeout without putting the job back in the queue twice,
//!   and it is why the second call leaves **no** audit line: nothing happened, so there is nothing
//!   to record. The report is where "I asked and nothing happened" belongs;
//! * **a discard has a reason or it does not happen.** Dropping a job is the one irreversible act in
//!   this file: the work is thrown away and nobody can reconstruct why from the job. A tool that
//!   accepts `--reason ""` produces an audit trail that cannot answer the only question it exists
//!   for, so the refusal comes *before* anything moves.
//!
//! ## What you write
//!
//! * [`Dlq::inspect`] — the rows that match a filter, and nothing else;
//! * [`Dlq::replay`] — put jobs back in the queue, and report what happened to each id;
//! * [`Dlq::discard`] — throw jobs away, with a reason that is written down.
//!
//! [`Dlq`]'s storage, [`Dead`], [`Outcome`], [`Audit`], [`Action`], [`Report`], [`Filter`],
//! [`Refused`] and the two writing helpers (`new`, `bury`) are given: the row's shape is the
//! engine's, and a probe that let you invent it would be checking a different tool.
//!
//! ## What the tests ask
//!
//! Inspecting twice leaves the trail empty and the rows untouched. A filter selects by queue, by
//! state and by both. One replay moves one row and leaves exactly one line naming the actor and the
//! instant; the same replay again answers "already" and writes nothing, which is the idempotence a
//! script needs. Ids that are not there are *reported*, not ignored, so a caller with a typo in a
//! loop learns it. A discard with a blank reason is refused before anything changes and a discard
//! with a reason removes the row — and leaves the trail as the only record that it existed.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no CLI and no database**: this is the library the command is a thin
//! shell around, and the real step's spec drives `jobqc dlq …` against a live store. The audit trail
//! is a `Vec` here and a table there, which is the one difference that matters for the "works while
//! the worker is running" half of the step: with a table, the row may move between the read and the
//! write, and the real tool does the check and the write in one transaction. What a probe can check
//! is the *contract* the tool has to keep — no line for a look, exactly one line per change, and a
//! reason on the irreversible one.
//!
//! Also missing on purpose: no confirmation prompt (the step's rule is that the CLI never asks a
//! question a script cannot answer — `--reason` is how a human and a script both say why), no
//! bulk-selection grammar (`--queue` filters are the CLI's, not the library's), and no permissions:
//! who may replay is `sec/authz-tenant-isolation@operate`'s business, and the actor's *name* is all
//! this file records.

use std::collections::BTreeMap;

/// Where a dead job has got to, as far as the operator's tool is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Dead, and nobody has touched it yet.
    Waiting,
    /// Back in the queue: the tool moved it, and it may be running right now.
    Replayed,
}

/// A dead job, as the operator sees it: enough to decide, and no payload.
///
/// The payload is deliberately absent. An operator deciding whether to replay an invoice does not
/// need the customer's data on their screen, and a tool that prints it is a tool that puts it in a
/// terminal scrollback and a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dead {
    /// Identity.
    pub id: u64,
    /// The queue it died in.
    pub queue: String,
    /// How many attempts it used.
    pub attempts: u32,
    /// Why the last one failed, in the handler's words.
    pub last_error: String,
    /// Whether it has been replayed already.
    pub state: Outcome,
}

/// What an operator did to one job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Put it back in the queue.
    Replay,
    /// Threw it away.
    Discard,
}

/// One line of the trail: who did what, to which job, when, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audit {
    /// The job it was done to.
    pub job: u64,
    /// What was done.
    pub action: Action,
    /// Who ran the command.
    pub actor: String,
    /// When, as the caller's clock read it.
    pub at_ms: i64,
    /// Why. Required for a discard, optional for a replay.
    pub reason: Option<String>,
}

/// What one command did, id by id.
///
/// Every id the caller named appears in exactly one of these lists: a tool that silently ignores
/// what it did not act on is a tool whose caller cannot tell a typo from a success.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Ids it acted on, in the order they were named.
    pub acted: Vec<u64>,
    /// Ids that were already in the state the command wanted, so nothing was done.
    pub already: Vec<u64>,
    /// Ids that are not in the dead-letter queue at all.
    pub missing: Vec<u64>,
}

/// Which rows to look at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Only this queue, when one is named.
    pub queue: Option<String>,
    /// Only this state, when one is named.
    pub state: Option<Outcome>,
}

/// Why a command was refused before it did anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// A discard without a reason. Nothing was discarded.
    ReasonRequired {
        /// The job whose discard was refused.
        job: u64,
    },
}

/// The dead letters, and the trail of what has been done to them.
#[derive(Debug, Default)]
pub struct Dlq {
    rows: BTreeMap<u64, Dead>,
    trail: Vec<Audit>,
}

impl Dlq {
    /// An empty dead-letter queue with an empty trail.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Put a dead job in, the way the worker does when it gives up.
    pub fn bury(&mut self, dead: Dead) {
        self.rows.insert(dead.id, dead);
    }

    /// The rows that match `filter`, in id order.
    ///
    /// `&self` is half of the reason this is safe to run during an incident; the other half is that
    /// it writes nothing: looking is not an action.
    #[must_use]
    pub fn inspect(&self, filter: &Filter) -> Vec<&Dead> {
        self.rows
            .values()
            .filter(|row| {
                filter
                    .queue
                    .as_ref()
                    .is_none_or(|queue| &row.queue == queue)
                    && filter.state.is_none_or(|state| row.state == state)
            })
            .collect()
    }

    /// Put these jobs back in the queue.
    ///
    /// A job that has already been replayed is `already`, and **no line is written for it**: the
    /// trail records changes, and re-running the same command changed nothing. A job that is not in
    /// the queue at all is `missing` — the honest answer for an id that was never dead or was
    /// discarded by somebody else in the meantime.
    ///
    /// # Errors
    ///
    /// Nothing today: replay is the reversible one. The signature has a `Result` so a caller written
    /// against it does not have to change when a refusal is added.
    pub fn replay(&mut self, ids: &[u64], actor: &str, at_ms: i64) -> Result<Report, Refused> {
        todo!()
    }

    /// Throw these jobs away, with a reason.
    ///
    /// The reason is checked for **every** id before anything moves: a command that discarded two
    /// jobs and then refused the third would leave a trail that cannot be explained. A job that is
    /// not there is `missing`, which is also the answer for one that was already discarded — the
    /// row is gone, and the trail is what is left of it.
    ///
    /// # Errors
    ///
    /// [`Refused::ReasonRequired`] when the reason is empty or only whitespace, naming the first id
    /// whose discard was refused.
    pub fn discard(
        &mut self,
        ids: &[u64],
        actor: &str,
        at_ms: i64,
        reason: &str,
    ) -> Result<Report, Refused> {
        todo!()
    }

    /// The trail, oldest first.
    #[must_use]
    pub fn trail(&self) -> &[Audit] {
        &self.trail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dead(id: u64, queue: &str) -> Dead {
        Dead {
            id,
            queue: queue.to_owned(),
            attempts: 3,
            last_error: "connection reset".to_owned(),
            state: Outcome::Waiting,
        }
    }

    /// Three dead jobs in two queues, one of them already replayed.
    fn dlq() -> Dlq {
        let mut dlq = Dlq::new();
        dlq.bury(dead(1, "mail"));
        dlq.bury(dead(2, "mail"));
        dlq.bury(dead(3, "reports"));
        dlq.replay(&[3], "ada", 1_000).expect("a replay is allowed");
        dlq
    }

    #[test]
    fn reading_is_not_an_action() {
        let dlq = dlq();
        let before = dlq.trail().len();

        let all = dlq.inspect(&Filter::default());
        assert_eq!(
            all.iter().map(|row| row.id).collect::<Vec<u64>>(),
            vec![1, 2, 3],
            "the default filter is every row, in id order"
        );
        let only_reports = dlq.inspect(&Filter {
            queue: Some("reports".to_owned()),
            state: None,
        });
        assert_eq!(only_reports.len(), 1);
        assert_eq!(only_reports[0].id, 3);
        let replayed = dlq.inspect(&Filter {
            queue: None,
            state: Some(Outcome::Replayed),
        });
        assert_eq!(
            replayed.iter().map(|row| row.id).collect::<Vec<u64>>(),
            vec![3]
        );
        let both = dlq.inspect(&Filter {
            queue: Some("mail".to_owned()),
            state: Some(Outcome::Replayed),
        });
        assert!(both.is_empty(), "a filter is an AND: {both:?}");

        assert_eq!(
            dlq.trail().len(),
            before,
            "an inspect that writes a line is an inspect whose log cannot tell who did something"
        );
    }

    #[test]
    fn one_replay_moves_one_row_and_leaves_one_line() {
        let mut dlq = dlq();
        let lint = dlq.trail().len();

        let report = dlq
            .replay(&[1], "ada", 1_760_000_000_000)
            .expect("a replay is allowed");
        assert_eq!(
            report,
            Report {
                acted: vec![1],
                already: Vec::new(),
                missing: Vec::new()
            }
        );
        assert_eq!(
            dlq.inspect(&Filter::default())
                .iter()
                .find(|row| row.id == 1)
                .map(|row| row.state),
            Some(Outcome::Replayed),
            "the row says it is back in the queue"
        );
        assert_eq!(dlq.trail().len(), lint + 1);
        let line = dlq.trail().last().expect("one line was written");
        assert_eq!(line.job, 1);
        assert_eq!(line.action, Action::Replay);
        assert_eq!(
            line.actor, "ada",
            "the trail is about people, not about ids"
        );
        assert_eq!(line.at_ms, 1_760_000_000_000);
        assert_eq!(
            line.reason, None,
            "a replay is reversible and does not need a reason; the field is there for the caller \
             that wants to leave one"
        );
    }

    #[test]
    fn replaying_the_same_job_again_is_not_a_second_run() {
        let mut dlq = Dlq::new();
        dlq.bury(dead(1, "mail"));

        let first = dlq.replay(&[1], "ada", 1).expect("a replay is allowed");
        assert_eq!(first.acted, vec![1]);
        let lines = dlq.trail().len();

        // The same command again: a script that timed out and retried has to be safe, and a job
        // that goes back into the queue twice is the incident getting worse.
        let second = dlq.replay(&[1], "ada", 2).expect("a replay is allowed");
        assert_eq!(
            second,
            Report {
                acted: Vec::new(),
                already: vec![1],
                missing: Vec::new()
            },
            "the report says nothing happened to it"
        );
        assert_eq!(
            dlq.trail().len(),
            lines,
            "and the trail agrees: the audit records changes, and there was none"
        );
        assert_eq!(
            dlq.inspect(&Filter::default())[0].state,
            Outcome::Replayed,
            "the row is exactly where the first replay put it"
        );
    }

    #[test]
    fn an_id_that_is_not_there_is_reported_rather_than_ignored() {
        let mut dlq = dlq();
        let lines = dlq.trail().len();

        let report = dlq
            .replay(&[9, 1, 9], "ada", 5)
            .expect("a replay is allowed");
        assert_eq!(
            report,
            Report {
                acted: vec![1],
                already: Vec::new(),
                missing: vec![9, 9]
            },
            "a caller looping over ids from a log has to learn that its list is wrong, and it is \
             told per id rather than with a count"
        );
        assert_eq!(
            dlq.trail().len(),
            lines + 1,
            "only the real one left a line"
        );
    }

    #[test]
    fn a_discard_without_a_reason_is_refused_before_anything_moves() {
        let mut dlq = dlq();
        let lines = dlq.trail().len();

        assert_eq!(
            dlq.discard(&[1, 2], "ada", 5, "   "),
            Err(Refused::ReasonRequired { job: 1 }),
            "the reason is checked before the first id, or half the command would have run"
        );
        assert_eq!(dlq.trail().len(), lines, "the trail is untouched");
        assert_eq!(
            dlq.inspect(&Filter::default()).len(),
            3,
            "and so is every row: a refused discard discards nothing"
        );

        // With a reason it happens, and the reason is what the trail keeps.
        let report = dlq
            .discard(&[1, 2], "ada", 6, " customer asked us to stop ")
            .expect("a discard with a reason is allowed");
        assert_eq!(report.acted, vec![1, 2]);
        assert_eq!(
            dlq.inspect(&Filter::default())
                .iter()
                .map(|row| row.id)
                .collect::<Vec<u64>>(),
            vec![3],
            "a discarded job is not in the queue any more"
        );
        for line in dlq
            .trail()
            .iter()
            .filter(|line| line.action == Action::Discard)
        {
            assert_eq!(
                line.reason.as_deref(),
                Some("customer asked us to stop"),
                "the reason is stored trimmed: whitespace is not documentation"
            );
            assert_eq!(line.actor, "ada");
            assert_eq!(line.at_ms, 6);
        }
        assert_eq!(
            dlq.trail()
                .iter()
                .filter(|line| line.action == Action::Discard)
                .count(),
            2,
            "one line per job: the same command over two jobs is two changes"
        );
    }

    #[test]
    fn the_trail_is_what_is_left_of_a_discarded_job() {
        let mut dlq = dlq();
        dlq.discard(&[2], "grace", 10, "duplicate of job 1")
            .expect("a discard with a reason is allowed");

        assert!(
            dlq.inspect(&Filter::default())
                .iter()
                .all(|row| row.id != 2),
            "the row is gone from the tool"
        );
        let line = dlq
            .trail()
            .iter()
            .find(|line| line.job == 2)
            .expect("and the trail is the only record that it existed");
        assert_eq!(line.action, Action::Discard);
        assert_eq!(line.actor, "grace");
        assert_eq!(line.reason.as_deref(), Some("duplicate of job 1"));

        // Asking for it again says it is gone, which is the same answer a caller gets for an id it
        // made up — and the trail is what tells the two apart.
        assert_eq!(
            dlq.replay(&[2], "grace", 11).expect("a replay is allowed"),
            Report {
                acted: Vec::new(),
                already: Vec::new(),
                missing: vec![2]
            }
        );
    }

    #[test]
    fn the_trail_reads_in_the_order_things_happened() {
        let mut dlq = dlq();
        dlq.replay(&[1], "ada", 100).expect("a replay is allowed");
        dlq.discard(&[2], "grace", 200, "not ours")
            .expect("a discard with a reason is allowed");
        dlq.replay(&[1], "ada", 300).expect("a replay is allowed");

        let trail: Vec<(u64, Action, &str, i64)> = dlq
            .trail()
            .iter()
            .map(|line| (line.job, line.action, line.actor.as_str(), line.at_ms))
            .collect();
        assert_eq!(
            trail,
            vec![
                (3, Action::Replay, "ada", 1_000),
                (1, Action::Replay, "ada", 100),
                (2, Action::Discard, "grace", 200),
            ],
            "oldest first, and the no-op replay at 300 wrote nothing: an audit trail is the list of \
             things that happened, not of commands that were run"
        );
    }
}
