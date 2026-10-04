//! probe = "db/atomic-claim@design"
//! title = "A lease, a fence, and the worker that came back too late"
//! hint = "The claim stamps a deadline and hands out a monotonic fence; a renewal only works while the lease is live; a write from a worker whose lease is gone is refused — the fence is what makes a late worker harmless."
//!
//! Step `jobq/3.7-leases-and-recovery` is where the claim stops being one statement and becomes a
//! *lease*: a claim that expires, that a worker has to keep alive, and that the queue takes back when
//! nobody is holding it any more. `db/atomic-claim@apply` re-checks the statement (one critical
//! section, disjoint batches); this slot is about everything that follows from letting a claim *end*.
//!
//! Three consequences, and the third is the one that bites in production:
//!
//! 1. **a lease is a bet on how long work takes.** Too short, and a slow worker's job is handed to
//!    somebody else while it is still running — the same job, twice. Too long, and a worker that
//!    crashed leaves its job invisible for the whole lease. The number is the recovery latency, and
//!    that is the whole tradeoff: `reap` gives a row back exactly at `claimed_at + lease`, no earlier
//!    and no later;
//! 2. **a renewal is a question, not a statement.** A worker that asks for more time *after* its
//!    lease has run out must be told that it lost the job — an unconditional extension keeps alive a
//!    claim the queue has already taken back, which is how a healthy-looking worker runs work nobody
//!    is waiting for;
//! 3. **the fence is what makes the late write harmless.** The unrecoverable version of this bug is
//!    subtle: worker A's lease expires, the job goes back, worker B claims it and starts, and *then*
//!    A finishes and writes "succeeded". The row says done; B's work is lost; nobody can tell. A
//!    monotonically increasing fence handed out with each claim makes A's write a value the queue
//!    can recognise as stale and refuse.
//!
//! ## What you write
//!
//! * [`Queue::claim`] — up to `max` rows with no live lease, stamped with a deadline and a fence;
//! * [`Queue::renew`] — more time, or `Err(Lost::Expired)`; a renewal counts from the reading it is
//!   given, never from the old deadline;
//! * [`Queue::reap`] — give back every row whose lease has run out, and say how many that was;
//! * [`Queue::complete`] — finish a row, but only for the claim that is *current*: anybody else's
//!   write is [`Lost::Stale`], and a write from an expired lease is [`Lost::Expired`].
//!
//! [`Row`], [`Claim`], [`Claimed`], [`Lost`] and the table itself are given: the row's shape is the
//! migration's, and a probe cannot run a migration.
//!
//! ## What the tests ask
//!
//! A claim hides the row from everybody and hands out a fence. `reap` gives the row back at exactly
//! the deadline and does not touch a live lease; a second `reap` recovers nothing. A renewal just
//! before the deadline extends from that reading; one after it is refused and does not resurrect
//! anything. Then the scenario this slot exists for: the lease expires, another worker claims the
//! job, and the first worker's completion is refused as stale — and the row is left exactly as the
//! current holder has it. A worker that quietly stops renewing loses the job and finds out when it
//! next asks.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no Postgres and no clock**: the table is a map, the readings are
//! parameters (as everywhere in this course, a probe that sleeps is a probe nobody runs), and the
//! reaper is a sweep over the rows rather than `UPDATE … WHERE lease_expires_at < now()`. What is
//! re-checked is the *policy* the SQL would implement — when a row comes back, what a renewal may
//! do, and which writes are refused — which is the part a learner has to design and defend.
//!
//! Missing on purpose: nothing here is observable (counters and dashboards for reaped rows are
//! `obs/metrics-slo@design`), the lease is not tuned against real handler timings (`relay/2.15`
//! argues that number), and there is no `SKIP LOCKED` — that is `db/atomic-claim@apply`'s file, and
//! this one deliberately starts where that one stops.

use std::collections::BTreeMap;

/// One row of the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The table's id.
    pub id: u64,
    /// What the work is for.
    pub key: String,
    /// The live claim, if there is one.
    pub claim: Option<Claim>,
}

/// A claim as the row stores it: who, since when, for how long, and under which fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// The worker holding it.
    pub worker: String,
    /// When it was taken or last renewed, as the caller's clock read it.
    pub at_ms: i64,
    /// How long the lease runs from `at_ms`.
    pub lease_ms: u64,
    /// The fence: handed out with this claim, and the value a write has to quote.
    pub fence: u64,
}

impl Claim {
    /// The instant this lease runs out.
    #[must_use]
    pub fn deadline_ms(&self) -> i64 {
        self.at_ms
            .saturating_add(i64::try_from(self.lease_ms).unwrap_or(i64::MAX))
    }

    /// `true` when the lease is over at `now_ms`.
    #[must_use]
    pub fn expired(&self, now_ms: i64) -> bool {
        now_ms >= self.deadline_ms()
    }
}

/// What a worker holds after a claim: the row, and the fence it must quote to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claimed {
    /// The row.
    pub id: u64,
    /// The fence this claim was given.
    pub fence: u64,
}

/// Why a write was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lost {
    /// The lease had run out when the worker asked. The worker has lost the job and has to stop.
    Expired {
        /// The fence the worker was quoting.
        fence: u64,
    },
    /// The row is held under another fence: somebody claimed it after this worker's lease expired.
    /// The worker's write is about a claim that no longer exists.
    Stale {
        /// The fence the worker was quoting.
        fence: u64,
        /// The fence the row is under now.
        current: u64,
    },
    /// Nobody holds the row at all.
    NotHeld {
        /// The fence the worker was quoting.
        fence: u64,
    },
}

/// The queue.
#[derive(Debug)]
pub struct Queue {
    rows: BTreeMap<u64, Row>,
    fence: u64,
}

impl Queue {
    /// A queue with one unclaimed row per key, ids from 1.
    #[must_use]
    pub fn new(keys: &[&str]) -> Self {
        let rows = keys
            .iter()
            .enumerate()
            .map(|(index, key)| {
                let id = index as u64 + 1;
                (
                    id,
                    Row {
                        id,
                        key: (*key).to_owned(),
                        claim: None,
                    },
                )
            })
            .collect();
        Self { rows, fence: 0 }
    }

    /// Take up to `max` rows with no live lease, and stamp each with a deadline and a fresh fence.
    ///
    /// A row whose lease has run out but which nobody has reaped yet is claimable: the deadline is
    /// the authority, not the reaper's promptness.
    #[must_use]
    pub fn claim(&mut self, worker: &str, max: usize, now_ms: i64, lease_ms: u64) -> Vec<Claimed> {
        let mut claimed = Vec::new();
        let ids: Vec<u64> = self.rows.keys().copied().collect();
        for id in ids {
            if claimed.len() == max {
                break;
            }
            let free = match &self.rows[&id].claim {
                Some(claim) => claim.expired(now_ms),
                None => true,
            };
            if !free {
                continue;
            }
            self.fence += 1;
            let fence = self.fence;
            let row = self.rows.get_mut(&id).expect("the row was just read");
            row.claim = Some(Claim {
                worker: worker.to_owned(),
                at_ms: now_ms,
                lease_ms,
                fence,
            });
            claimed.push(Claimed { id, fence });
        }
        claimed
    }

    /// Extend the lease this worker holds under this fence, and answer with the new deadline.
    ///
    /// From *now*, not from the old deadline: a renewal is a fresh window, and a worker that renews
    /// late does not get paid for the time it missed. The question the caller has to ask first is
    /// this method's whole subject — a lease that has already run out cannot be extended, because the
    /// queue may have given the row to somebody else in the meantime.
    ///
    /// # Errors
    ///
    /// [`Lost::Expired`] when the lease is over, [`Lost::NotHeld`] or [`Lost::Stale`] when this
    /// worker is not the one holding the row.
    pub fn renew(
        &mut self,
        id: u64,
        worker: &str,
        fence: u64,
        now_ms: i64,
        lease_ms: u64,
    ) -> Result<i64, Lost> {
        let row = self.rows.get_mut(&id).ok_or(Lost::NotHeld { fence })?;
        let current = row.claim.as_ref().ok_or(Lost::NotHeld { fence })?;
        if current.fence != fence || current.worker != worker {
            return Err(Lost::Stale {
                fence,
                current: current.fence,
            });
        }
        if current.expired(now_ms) {
            return Err(Lost::Expired { fence });
        }
        let claim = row.claim.as_mut().expect("the claim was just read");
        claim.at_ms = now_ms;
        claim.lease_ms = lease_ms;
        Ok(claim.deadline_ms())
    }

    /// Give back every row whose lease has run out, and answer how many that was.
    ///
    /// Idempotent, and it counts rather than hides: the number of rows a sweep recovered is the
    /// metric an operator watches when they suspect workers are dying.
    pub fn reap(&mut self, now_ms: i64) -> usize {
        let mut recovered = 0;
        for row in self.rows.values_mut() {
            let expired = row
                .claim
                .as_ref()
                .is_some_and(|claim| claim.expired(now_ms));
            if expired {
                row.claim = None;
                recovered += 1;
            }
        }
        recovered
    }

    /// Finish the row this worker holds under this fence.
    ///
    /// The write is about the claim, not about the row: a worker whose lease has run out is refused
    /// even if nobody has re-claimed the row yet, and a worker whose row went back into the queue and
    /// was claimed by somebody else is refused as stale. This is the check that stops a late
    /// completion from marking somebody else's work as done.
    ///
    /// # Errors
    ///
    /// [`Lost::Expired`], [`Lost::Stale`] or [`Lost::NotHeld`], in that order of specificity.
    pub fn complete(&mut self, id: u64, worker: &str, fence: u64, now_ms: i64) -> Result<(), Lost> {
        let row = self.rows.get_mut(&id).ok_or(Lost::NotHeld { fence })?;
        let current = row.claim.as_ref().ok_or(Lost::NotHeld { fence })?;
        if current.fence != fence || current.worker != worker {
            return Err(Lost::Stale {
                fence,
                current: current.fence,
            });
        }
        if current.expired(now_ms) {
            return Err(Lost::Expired { fence });
        }
        row.claim = None;
        Ok(())
    }

    /// The row, for the tests that read the table back.
    #[must_use]
    pub fn row(&self, id: u64) -> &Row {
        self.rows
            .get(&id)
            .unwrap_or_else(|| panic!("no row with id {id}"))
    }

    /// How many rows have no live lease at this reading: the backlog a claim can still take.
    #[must_use]
    pub fn free(&self, now_ms: i64) -> usize {
        self.rows
            .values()
            .filter(|row| row.claim.as_ref().is_none_or(|claim| claim.expired(now_ms)))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lease of thirty seconds, and a reading that is not zero so an off-by-one is visible.
    const LEASE_MS: u64 = 30_000;
    const START: i64 = 1_760_000_000_000;

    fn queue() -> Queue {
        Queue::new(&["ada", "grace", "ana"])
    }

    #[test]
    fn a_claim_hides_the_row_and_hands_out_a_fence() {
        let mut queue = queue();
        let batch = queue.claim("worker-1", 2, START, LEASE_MS);
        assert_eq!(
            batch.iter().map(|claim| claim.id).collect::<Vec<u64>>(),
            vec![1, 2],
            "oldest first"
        );
        assert_ne!(
            batch[0].fence, batch[1].fence,
            "every claim gets its own fence: two rows under one number cannot be told apart later"
        );
        assert_eq!(
            queue
                .row(1)
                .claim
                .as_ref()
                .map(|claim| claim.worker.as_str()),
            Some("worker-1")
        );
        assert_eq!(
            queue.row(1).claim.as_ref().map(Claim::deadline_ms),
            Some(START + LEASE_MS as i64)
        );

        // Nothing that is held is handed over, and the free count agrees: three rows, two held.
        assert_eq!(queue.free(START), 1);
        let second = queue.claim("worker-2", 5, START, LEASE_MS);
        assert_eq!(
            second.iter().map(|claim| claim.id).collect::<Vec<u64>>(),
            vec![3],
            "a second worker gets the free row, and there is exactly one"
        );
        assert!(
            queue.claim("worker-3", 5, START, LEASE_MS).is_empty(),
            "everything is held now, and a claim that comes back empty is the queue saying so"
        );
        assert_eq!(queue.free(START), 0);
    }

    #[test]
    fn the_reaper_gives_a_row_back_at_exactly_the_deadline() {
        let mut queue = queue();
        assert_eq!(
            queue.claim("worker-1", 1, START, LEASE_MS).len(),
            1,
            "one row goes out on a lease"
        );

        assert_eq!(
            queue.reap(START + LEASE_MS as i64 - 1),
            0,
            "one millisecond early is a live lease, and reaping it would hand the same job to two \
             workers"
        );
        assert_eq!(
            queue.free(START + LEASE_MS as i64 - 1),
            2,
            "one row is held, two are free"
        );
        assert_eq!(
            queue.reap(START + LEASE_MS as i64),
            1,
            "at the deadline the row is owed again, and that instant is the recovery latency"
        );
        assert_eq!(
            queue.reap(START + LEASE_MS as i64 + 60_000),
            0,
            "a second sweep recovers nothing: the reaper is a sweep, not a counter"
        );
        assert!(queue.row(1).claim.is_none());
    }

    #[test]
    fn a_renewal_only_works_while_the_lease_is_live() {
        let mut queue = queue();
        let claimed = queue.claim("worker-1", 1, START, LEASE_MS)[0];

        // Renewing just before the deadline extends from the reading, not from the old deadline.
        let nearly = START + LEASE_MS as i64 - 1;
        let deadline = queue
            .renew(claimed.id, "worker-1", claimed.fence, nearly, LEASE_MS)
            .expect("the lease is live");
        assert_eq!(
            deadline,
            nearly + LEASE_MS as i64,
            "a fresh window from now: a late renewal does not get paid for the time it missed"
        );
        assert_eq!(queue.free(nearly + LEASE_MS as i64 - 1), 2);

        // Asking after the deadline is refused, and the refusal is the worker's signal to stop.
        let late = nearly + LEASE_MS as i64 + 1;
        assert_eq!(
            queue.renew(claimed.id, "worker-1", claimed.fence, late, LEASE_MS),
            Err(Lost::Expired {
                fence: claimed.fence
            }),
            "the queue may have given the row away by now, so this is not a renewal to grant"
        );
        assert_eq!(
            queue.row(claimed.id).claim.as_ref().map(Claim::deadline_ms),
            Some(deadline),
            "and the refused renewal did not quietly extend anything"
        );

        // A worker that is not the holder cannot renew somebody else's lease in either direction.
        assert_eq!(
            queue.renew(claimed.id, "worker-2", claimed.fence, nearly, LEASE_MS),
            Err(Lost::Stale {
                fence: claimed.fence,
                current: claimed.fence
            }),
            "the fence matches but the worker does not: the pair is the identity"
        );
    }

    #[test]
    fn a_worker_whose_lease_expired_cannot_finish_somebody_elses_work() {
        let mut queue = queue();
        let first = queue.claim("worker-1", 1, START, LEASE_MS)[0];

        // The lease expires while worker-1 is still busy, or asleep, or dead.
        let after = START + LEASE_MS as i64 + 1;
        assert_eq!(queue.reap(after), 1);
        let second = queue.claim("worker-2", 1, after, LEASE_MS);
        assert_eq!(second.len(), 1, "the row is owed again");
        assert_eq!(second[0].id, first.id, "it is the same row");
        assert_ne!(
            second[0].fence, first.fence,
            "and it is a different claim, which is what the fence is for"
        );

        // Worker-1 wakes up and reports success. Without the fence this is the bug: the row says
        // done and worker-2's work is lost with no trace.
        assert_eq!(
            queue.complete(first.id, "worker-1", first.fence, after + 1),
            Err(Lost::Stale {
                fence: first.fence,
                current: second[0].fence
            }),
            "a write from the previous claim is stale, and the refusal names both fences"
        );
        assert!(
            queue.row(first.id).claim.is_some(),
            "and the row is exactly where the current holder has it"
        );

        // The current holder finishes it, which is the write that counts.
        assert_eq!(
            queue.complete(first.id, "worker-2", second[0].fence, after + 2),
            Ok(())
        );
        assert!(queue.row(first.id).claim.is_none());
        assert_eq!(
            queue.complete(first.id, "worker-2", second[0].fence, after + 3),
            Err(Lost::NotHeld {
                fence: second[0].fence
            }),
            "and finishing it twice is not idempotent: the second call is about a claim that ended"
        );
    }

    #[test]
    fn a_write_from_an_expired_lease_is_refused_even_before_the_row_is_reaped() {
        let mut queue = queue();
        let claimed = queue.claim("worker-1", 1, START, LEASE_MS)[0];
        let after = START + LEASE_MS as i64;

        // Nobody has reaped it: the row still reads as held, and the write is still not allowed.
        assert_eq!(
            queue.complete(claimed.id, "worker-1", claimed.fence, after),
            Err(Lost::Expired {
                fence: claimed.fence
            }),
            "the deadline is the authority, not the reaper's promptness"
        );
        assert_eq!(
            queue.renew(claimed.id, "worker-1", claimed.fence, after, LEASE_MS),
            Err(Lost::Expired {
                fence: claimed.fence
            })
        );
        assert_eq!(
            queue.renew(claimed.id, "worker-1", claimed.fence, after - 1, LEASE_MS),
            Ok(after - 1 + LEASE_MS as i64),
            "one millisecond earlier is a live lease with a different answer"
        );
    }

    #[test]
    fn a_worker_that_stops_renewing_loses_the_job_at_the_deadline() {
        let mut queue = queue();
        let claimed = queue.claim("worker-1", 3, START, LEASE_MS);
        assert_eq!(queue.free(START), 0, "all three rows are held");

        // A worker at two renewals per lease: as long as it keeps asking, it keeps the rows, and
        // the deadline moves with it.
        let mut now = START;
        for _ in 0..3 {
            now += LEASE_MS as i64 / 2;
            for held in &claimed {
                queue
                    .renew(held.id, "worker-1", held.fence, now, LEASE_MS)
                    .expect("the lease is live while the worker keeps asking");
            }
        }
        assert_eq!(
            queue.free(now),
            0,
            "renewal is what keeps a healthy worker holding"
        );

        // Then it stops. Half a lease later it is still fine; a lease later it is not, and every
        // row is owed again.
        assert_eq!(queue.reap(now + LEASE_MS as i64 / 2), 0);
        assert_eq!(
            queue.reap(now + LEASE_MS as i64),
            3,
            "the whole batch comes back at the deadline: a worker that stopped is a crash, whatever \
             it looks like in a log"
        );
        assert_eq!(queue.free(now + LEASE_MS as i64), 3);
        for held in &claimed {
            assert_eq!(
                queue.renew(
                    held.id,
                    "worker-1",
                    held.fence,
                    now + LEASE_MS as i64,
                    LEASE_MS
                ),
                Err(Lost::NotHeld { fence: held.fence }),
                "and it finds out by asking, not by writing: after the sweep the row has no holder \
                 at all, and a claim that was never reaped answers `Expired` instead — either way \
                 the worker is told, and neither answer is silence"
            );
        }
    }

    #[test]
    fn an_empty_queue_and_a_bound_of_zero_are_not_errors() {
        let mut empty = Queue::new(&[]);
        assert!(empty.claim("worker-1", 5, START, LEASE_MS).is_empty());
        assert_eq!(empty.reap(START), 0);
        assert_eq!(empty.free(START), 0);

        let mut queue = queue();
        assert!(
            queue.claim("worker-1", 0, START, LEASE_MS).is_empty(),
            "zero is a bound"
        );
        assert_eq!(queue.free(START), 3, "and nothing was taken");
        assert_eq!(
            queue.claim("worker-1", 10, START, LEASE_MS).len(),
            3,
            "asking for more than there is gets what there is"
        );
        assert_eq!(queue.free(START), 0);
        assert!(queue.claim("worker-2", 1, START, LEASE_MS).is_empty());
    }
}
