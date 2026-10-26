//! probe = "dist/consensus@apply"
//! title = "A vote goes to a caught-up candidate once, and only the leader's own term commits"
//! hint = "Compare terms before log positions, spend a vote once per term, replace a divergent suffix, and count the leader only once when a majority decides."
//!
//! Step `meshkv/5.3-teaching-raft` makes `5.2`'s four rules move. This probe is the voter's half
//! of that protocol as a value: no socket, no timeout, no simulator — a term, a log, and the two
//! rules that decide a vote and a commit.
//!
//! Four rules, and someone has already paid for each of them:
//!
//! 1. **a majority is more than half the cluster, and the leader counts once.** A candidate's own
//!    vote counts, so a candidate of three needs one more vote, not two; a leader that counted
//!    itself twice would commit with fewer than a majority and lose a decided write;
//! 2. **a vote is granted only to a candidate that is at least as up to date, and only once per
//!    term.** The term is compared before the index: a log with more entries from an older term is
//!    *behind* one with fewer entries from a newer term, and a vote spent on one candidate cannot
//!    be spent on another in the same term;
//! 3. **a refusal is an answer.** A message from an older term is refused with this node's term
//!    before its content is read, so a stale candidate learns it is behind instead of waiting for
//!    ever; a message from a newer term moves the voter to that term and forgets the old vote;
//! 4. **only an entry of the leader's current term may commit.** A majority holding an inherited
//!    entry is not a decision: the leader writes one empty entry of its own term, and the old
//!    entries commit with it. A follower's divergent suffix is replaced from the first mismatch,
//!    never merged.
//!
//! ## What you write
//!
//! * [`at_least_as_up_to_date`] — the term-first comparison a vote is decided by;
//! * [`Voter::hear_vote_request`] — adopt a newer term, refuse an older one, spend the vote once;
//! * [`Voter::hear_append`] — refuse an older term and a predecessor that does not match, keep a
//!   matching prefix, replace a divergent suffix, and clamp the leader's commit to this log;
//! * [`advance_commit`] — the highest position a majority holds whose entry is the leader's own.
//!
//! [`Term`], [`LogPosition`], [`Entry`] and [`Log`] are given, because the vocabulary is the
//! interface: a position is a term and an index, and "at least as up to date" is a comparison of
//! two positions rather than of two lengths. The same shapes are what `dist/snapshot@design`
//! compacts and what `dist/membership@apply` decides with.
//!
//! ## What the tests ask
//!
//! That a candidate one position behind is refused and an equal one is granted; that the term wins
//! before the index in both directions; that a vote is spent once and a retry by the same candidate
//! is the same vote; that a newer term moves a voter forward and clears its vote while an older one
//! is answered with the voter's own term and writes nothing; that a follower replaces only the
//! divergent suffix and refuses an append whose predecessor it does not hold; and that an inherited
//! entry stays uncommitted even with a majority, while the leader's own entry commits at the
//! majority boundary.
//!
//! ## Where this is honest, and where it is not
//!
//! This is not the step's node: there is no election timeout, no candidate or leader role, no
//! `campaign`, no replication loop and no `turmoil` schedule. [`Voter::hear_vote_request`] is a
//! voter answering a message, not a daemon driven by a clock, so "a node that has not heard from a
//! leader campaigns" is not re-checked here. The habit is still worth re-drilling because the vote
//! and the commit rule are the two places a consensus bug is silent: the code runs, the cluster
//! elects someone, and a decided write disappears under a later leader. The network that would
//! carry these values is `meshkv/5.3`'s harness; the values themselves are what these tests pin.

/// A position in the log: one-based, monotonic, never reused.
pub type Index = u64;

/// How many elections the cluster has been through. A term only moves up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Term(u64);

impl Term {
    /// Term zero, where a brand-new node starts.
    #[must_use]
    pub fn zero() -> Self {
        Self(0)
    }

    /// A term as it is written on the wire.
    #[must_use]
    pub fn new(number: u64) -> Self {
        Self(number)
    }

    /// The number.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// A position the log can name: a term and an index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogPosition {
    /// The term of the entry at this position.
    pub term: Term,
    /// The position itself, one-based.
    pub index: Index,
}

impl LogPosition {
    /// The position before the first entry: term zero, index zero.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            term: Term::zero(),
            index: 0,
        }
    }
}

/// One write, as the log keeps it: the term that proposed it and where it sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The term of the leader that proposed it.
    pub term: Term,
    /// Its position, one-based.
    pub index: Index,
}

impl Entry {
    /// An entry proposed in `term` at `index`.
    #[must_use]
    pub fn new(term: Term, index: Index) -> Self {
        Self { term, index }
    }
}

/// The append-only log every voter keeps, as a value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Log {
    entries: Vec<Entry>,
}

impl Log {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A log that already holds `entries`, whose positions are their place in the list plus one.
    #[must_use]
    pub fn from_entries(entries: Vec<Entry>) -> Self {
        Self { entries }
    }

    /// How many entries it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when it holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The position of the last entry, or zero for an empty log.
    #[must_use]
    pub fn last_index(&self) -> Index {
        self.entries.len() as Index
    }

    /// The last entry as a position a vote request carries.
    #[must_use]
    pub fn last_position(&self) -> LogPosition {
        self.entries
            .last()
            .map_or(LogPosition::empty(), |entry| LogPosition {
                term: entry.term,
                index: entry.index,
            })
    }

    /// The entry at `index`, one-based.
    #[must_use]
    pub fn at(&self, index: Index) -> Option<&Entry> {
        let position = index.checked_sub(1)?;
        let position = usize::try_from(position).ok()?;
        self.entries.get(position)
    }

    /// `true` when the entry the leader named as `prev` is the one this log holds.
    ///
    /// Position zero is always held: it is the position before the first entry, which is the one
    /// thing every log agrees about.
    #[must_use]
    pub fn holds(&self, prev: LogPosition) -> bool {
        prev.index == 0
            || self
                .at(prev.index)
                .is_some_and(|entry| entry.term == prev.term)
    }

    /// Drop everything from `index` onwards, and answer how many entries went.
    pub fn truncate_from(&mut self, index: Index) -> usize {
        let position = index.saturating_sub(1);
        let Ok(position) = usize::try_from(position) else {
            return 0;
        };
        if position >= self.entries.len() {
            return 0;
        }
        let dropped = self.entries.len() - position;
        self.entries.truncate(position);
        dropped
    }

    /// Append an entry the caller has already shown belongs at the next position.
    pub fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
    }
}

/// `true` when `candidate`'s last position is at least as up to date as `mine`.
///
/// The term is compared first. A log with more entries from an older term is behind one with fewer
/// entries from a newer term, because the newer term has already overwritten it somewhere the
/// length does not show.
#[must_use]
pub fn at_least_as_up_to_date(candidate: LogPosition, mine: LogPosition) -> bool {
    todo!()
}

/// A voter's answer to a vote request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vote {
    /// The answering voter's term.
    pub term: Term,
    /// Whether the vote was granted.
    pub granted: bool,
}

/// A follower's answer to an append.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendAck {
    /// The answering follower's term.
    pub term: Term,
    /// Whether the entries were accepted.
    pub success: bool,
    /// How far the follower's log reaches after the append.
    pub match_index: Index,
}

/// One node's voter state: what it believes, and what it answers.
#[derive(Debug, Clone)]
pub struct Voter {
    term: Term,
    voted_for: Option<String>,
    log: Log,
    commit: Index,
    leader: Option<String>,
}

impl Voter {
    /// A fresh voter: term zero, an empty log, no vote given.
    #[must_use]
    pub fn new() -> Self {
        Self {
            term: Term::zero(),
            voted_for: None,
            log: Log::new(),
            commit: 0,
            leader: None,
        }
    }

    /// A voter that has been running: it is in `term` and already holds `entries`.
    #[must_use]
    pub fn holding(term: Term, entries: Vec<Entry>) -> Self {
        Self {
            term,
            voted_for: None,
            log: Log::from_entries(entries),
            commit: 0,
            leader: None,
        }
    }

    /// The term this voter is in.
    #[must_use]
    pub fn term(&self) -> Term {
        self.term
    }

    /// The log this voter keeps.
    #[must_use]
    pub fn log(&self) -> &Log {
        &self.log
    }

    /// How far this voter's log is committed.
    #[must_use]
    pub fn commit(&self) -> Index {
        self.commit
    }

    /// The candidate this voter spent its vote on, in the current term.
    #[must_use]
    pub fn voted_for(&self) -> Option<&str> {
        self.voted_for.as_deref()
    }

    /// The leader this voter last heard from, when it has heard one.
    #[must_use]
    pub fn leader(&self) -> Option<&str> {
        self.leader.as_deref()
    }

    /// Answer a request for a vote from `candidate`, whose log ends at `last`.
    ///
    /// A request from a newer term is adopted first; one from an older term is refused with this
    /// voter's term before the candidate's log is read. Within the term the vote is granted only to
    /// a caught-up candidate, and only if this voter has not spent it on somebody else.
    pub fn hear_vote_request(&mut self, candidate: &str, term: Term, last: LogPosition) -> Vote {
        todo!()
    }

    /// Take a leader's entries, or refuse them and say how far this log reaches.
    ///
    /// An append whose `prev` this log does not hold is refused: appending it would leave a gap.
    /// Otherwise the matching prefix is kept and the suffix from the first mismatch is replaced,
    /// never merged. The leader's commit index is applied no further than this log reaches.
    pub fn hear_append(
        &mut self,
        term: Term,
        leader: &str,
        prev: LogPosition,
        entries: &[Entry],
        leader_commit: Index,
    ) -> AppendAck {
        todo!()
    }

    /// Move to a newer term as a follower: the vote and the leader are forgotten.
    fn step_down(&mut self, term: Term) {
        self.term = term;
        self.voted_for = None;
        self.leader = None;
    }
}

/// The largest position a majority holds whose entry is of the leader's own term.
///
/// `matched[i]` is how far node `i`'s log reaches, the leader included, so the leader is counted
/// once — a leader that counted itself twice would commit with fewer than a majority. The scan
/// starts at the end of the log and never moves the commit index backwards. A position whose entry
/// is from an older term is skipped even when every node holds it: an inherited entry can still be
/// overwritten, and only an entry of the current term is a decision.
#[must_use]
pub fn advance_commit(
    commit: Index,
    log: &Log,
    current_term: Term,
    matched: &[Index],
    cluster_size: usize,
) -> Index {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(number: u64) -> Term {
        Term::new(number)
    }

    fn entry(term: u64, index: u64) -> Entry {
        Entry::new(t(term), index)
    }

    fn position(term: u64, index: u64) -> LogPosition {
        LogPosition {
            term: t(term),
            index,
        }
    }

    fn voter(term: u64, entries: &[(u64, u64)]) -> Voter {
        Voter::holding(
            t(term),
            entries
                .iter()
                .map(|(term, index)| entry(*term, *index))
                .collect(),
        )
    }

    #[test]
    fn a_candidate_that_is_behind_is_refused_and_a_caught_up_one_is_granted() {
        let mut learner = voter(1, &[(1, 1)]);
        let behind = learner.hear_vote_request("n3", t(2), position(0, 0));
        assert_eq!(
            behind.term,
            t(2),
            "the refusal carries the voter's term, so a stale candidate can see it is behind"
        );
        assert!(
            !behind.granted,
            "a candidate with an empty log would overwrite the entry this voter holds"
        );

        let caught_up = learner.hear_vote_request("n3", t(2), position(1, 1));
        assert!(
            caught_up.granted,
            "a candidate whose last position is this voter's last position is caught up"
        );

        let mut one_behind = voter(1, &[(1, 2)]);
        let refused = one_behind.hear_vote_request("n3", t(1), position(1, 1));
        assert!(
            !refused.granted,
            "one entry behind is behind: the rule is `at least as up to date`, not `almost`"
        );
    }

    #[test]
    fn the_term_is_compared_before_the_position() {
        assert!(
            !at_least_as_up_to_date(position(1, 9), position(2, 1)),
            "a log with more entries from an older term is behind one with fewer entries from a \
             newer term"
        );
        assert!(
            at_least_as_up_to_date(position(2, 1), position(1, 9)),
            "and the same comparison run the other way agrees: the newer term is ahead"
        );
        assert!(
            at_least_as_up_to_date(position(2, 7), position(2, 7)),
            "equal positions are caught up: a candidate holding the same log may be elected"
        );
        assert!(
            !at_least_as_up_to_date(position(2, 6), position(2, 7)),
            "within one term the index still decides, and one position short is short"
        );
    }

    #[test]
    fn a_vote_is_spent_once_per_term_and_a_retry_is_the_same_vote() {
        let mut learner = voter(1, &[(1, 1)]);
        assert!(
            learner
                .hear_vote_request("n2", t(2), position(1, 1))
                .granted,
            "the first caught-up candidate gets the vote"
        );
        assert!(
            learner
                .hear_vote_request("n2", t(2), position(1, 1))
                .granted,
            "the same candidate asking again is a retry, not a second vote"
        );
        assert!(
            !learner
                .hear_vote_request("n3", t(2), position(1, 1))
                .granted,
            "a second candidate in the same term is refused: one node cannot be two votes"
        );
        assert_eq!(
            learner.voted_for(),
            Some("n2"),
            "and the vote is still n2's"
        );
    }

    #[test]
    fn a_message_from_a_newer_term_moves_the_voter_forward_and_forgets_its_vote() {
        let mut learner = voter(1, &[]);
        assert!(
            learner
                .hear_vote_request("n2", t(1), LogPosition::empty())
                .granted
        );
        assert_eq!(learner.voted_for(), Some("n2"));

        let ack = learner.hear_append(t(4), "n1", LogPosition::empty(), &[], 0);
        assert!(
            ack.success,
            "an append from a newer term is accepted, not refused for its term"
        );
        assert_eq!(
            learner.term(),
            t(4),
            "a newer term moves this voter forward"
        );
        assert_eq!(
            learner.voted_for(),
            None,
            "and the vote given in the old term is forgotten: it belonged to another election"
        );
        assert_eq!(
            learner.leader(),
            Some("n1"),
            "the append also says who leads"
        );
    }

    #[test]
    fn a_message_from_an_older_term_is_refused_with_the_voters_own_term() {
        let mut learner = voter(3, &[(3, 1), (3, 2)]);
        let vote = learner.hear_vote_request("n1", t(2), position(2, 5));
        assert!(
            !vote.granted,
            "a request from an older term is stale before its log is even read"
        );
        assert_eq!(
            vote.term,
            t(3),
            "the answer carries the voter's term, so the stale candidate can catch up"
        );

        let ack = learner.hear_append(t(2), "n1", LogPosition::empty(), &[entry(2, 3)], 3);
        assert!(!ack.success, "an append from an older term must not write");
        assert_eq!(ack.term, t(3), "and it is answered with this voter's term");
        assert_eq!(learner.log().len(), 2, "the stale append changed nothing");
        assert_eq!(
            learner.leader(),
            None,
            "and a stale leader is not believed either"
        );
    }

    #[test]
    fn a_follower_takes_the_leaders_entries_and_answers_how_far_it_reaches() {
        let mut learner = Voter::new();
        let ack = learner.hear_append(
            t(1),
            "n1",
            LogPosition::empty(),
            &[entry(1, 1), entry(1, 2)],
            0,
        );
        assert!(
            ack.success,
            "an append whose predecessor matches is accepted"
        );
        assert_eq!(
            ack.match_index, 2,
            "the answer is how far the log now reaches, not how many entries arrived"
        );
        assert_eq!(learner.log().len(), 2);
        assert_eq!(
            learner.commit(),
            0,
            "nothing is committed until the leader says so"
        );

        let ack = learner.hear_append(t(1), "n1", position(1, 2), &[], 99);
        assert!(ack.success);
        assert_eq!(
            learner.commit(),
            2,
            "a commit index past the follower's own log is clamped: it never commits an entry it \
             does not hold"
        );
    }

    #[test]
    fn a_divergent_suffix_is_replaced_and_a_matching_prefix_is_kept() {
        let mut learner = voter(1, &[(1, 1), (1, 2), (1, 3)]);
        let ack = learner.hear_append(t(1), "n1", position(1, 1), &[entry(1, 2)], 0);
        assert!(ack.success);
        assert_eq!(
            learner.log().len(),
            3,
            "an entry the follower already holds is not appended twice"
        );
        assert_eq!(
            learner.log().at(2).map(|entry| entry.term),
            Some(t(1)),
            "the matching prefix is kept as it was"
        );

        let ack = learner.hear_append(t(2), "n1", position(1, 1), &[entry(2, 2), entry(2, 3)], 0);
        assert!(
            ack.success,
            "the leader's entries replace the divergent suffix"
        );
        assert_eq!(learner.log().len(), 3);
        assert_eq!(
            learner.log().at(2).map(|entry| entry.term),
            Some(t(2)),
            "the first mismatching entry is replaced, not merged"
        );
        assert_eq!(
            learner.log().at(3).map(|entry| entry.term),
            Some(t(2)),
            "and the rest of the leader's suffix with it"
        );

        let ack = learner.hear_append(t(2), "n1", position(1, 9), &[entry(2, 4)], 0);
        assert!(
            !ack.success,
            "an append whose predecessor the follower does not hold is refused rather than \
             leaving a gap"
        );
        assert_eq!(learner.log().len(), 3, "and the log is untouched");
    }

    #[test]
    fn only_an_entry_of_the_leaders_own_term_commits_even_with_a_majority() {
        let log = Log::from_entries(vec![entry(1, 1), entry(2, 2), entry(2, 3)]);
        assert_eq!(
            advance_commit(0, &log, t(2), &[3, 3, 1], 3),
            3,
            "a majority of the current-term entry at 3 commits it, and the inherited entry at 1 \
             comes with it"
        );
        assert_eq!(
            advance_commit(0, &log, t(2), &[1, 1, 1], 3),
            0,
            "a majority holding only the inherited term-1 entry commits nothing: an inherited \
             entry can still be overwritten"
        );
        assert_eq!(
            advance_commit(0, &log, t(2), &[3, 2, 1], 3),
            2,
            "the highest current-term entry a majority holds is 2, not the tail"
        );
        assert_eq!(
            advance_commit(0, &log, t(2), &[3, 1, 1], 4),
            0,
            "two of four is not a majority: a cluster of four needs three"
        );
        assert_eq!(
            advance_commit(3, &log, t(2), &[3, 3, 3], 3),
            3,
            "a commit index never moves backwards"
        );
    }
}
