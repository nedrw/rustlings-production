//! probe = "dist/linearizable-read@design"
//! title = "A read is answered only by the leader, only inside its lease, and only at the index the round proved"
//! hint = "Refuse a read without a round, answer at the commit index the round remembered, grant the lease only on a majority, and stop answering at its expiry or the moment you stop leading."
//!
//! Step `meshkv/5.6-lease-reads` is the step where a replicated log learns to answer questions
//! about *now*. A follower can be a hundred entries behind and still answer confidently; a leader
//! can have been replaced and not know it yet. The step's answer has two halves — a read index that
//! a majority confirmed, and a lease that lets the next reads skip that round — and this probe
//! re-drills the half that is an argument about time rather than a message type: when a leader may
//! answer, at what position, and for how long.
//!
//! Five rules a caller can rely on, and each one is a way a store answers a value that has
//! already been replaced:
//!
//! 1. **only the leader answers, and only after a round.** A leader without a round is *probably*
//!    still the leader, and "probably" is the class of answer this course keeps removing. A round
//!    in flight is not a proof either: the acknowledgements are.
//! 2. **the round proves a position, and the lease repeats that position.** The round remembers the
//!    commit index it saw; the lease answers at that index, even after a later write commits. A
//!    value committed after the round may not be on a majority, so claiming it would be claiming a
//!    decision nobody proved. A new round is how the leader learns about the newer position.
//! 3. **a majority includes the leader, and one short is not a majority.** In three nodes the
//!    leader plus one acknowledgement decides; in five it is the leader plus two. The proof is a
//!    set of voters, not a count of hopeful replies.
//! 4. **the lease is half-open, and a served round is spent.** It is live while
//!    `now < expires_at` and over at the expiry instant. When a round is answered, its read index
//!    is used up: finishing the same round again without new heartbeats is not a new proof, and
//!    must not extend the window.
//! 5. **the lease dies with the leadership, and it has to be shorter than the election timeout.**
//!    A node that steps down forgets its lease and its read index in the same breath as its role.
//!    A lease as long as the timeout is already too long, because an election can start at the
//!    instant it would end; `is_safe_lease` is that sentence, and it is strict.
//!
//! ## What you write
//!
//! * [`is_safe_lease`] — the relationship between a lease and the election timeout it is derived
//!   from;
//! * [`Node::confirm_read`] — start a round: remember the commit index, answer the peers to
//!   heartbeat;
//! * [`Node::complete_read`] — finish a round: a majority grants a lease at the remembered index,
//!   anything less grants nothing;
//! * [`Node::serve_read`] — the answer a caller gets: served at the lease's index, redirected to the
//!   leader, or told a round is needed;
//! * [`Node::step_down`] — losing the leadership, and with it the lease and the read index.
//!
//! [`Node::leader`], [`Node::follower`], [`Node::commit_to`], the accessors, [`Lease`],
//! [`ReadAnswer`], [`RoundError`] and the two constants are given. The vocabulary is the interface:
//! a probe that made the learner invent a clock abstraction would be re-teaching `5.6` rather than
//! re-checking it. [`Lease::is_valid`] is given too, because the half-open comparison belongs to
//! the type; what the exercise decides is whether the read path uses it.
//!
//! ## What the tests ask
//!
//! A follower answers `NotLeader`, with the name it knows or with none, and cannot start a round. A
//! leader with no round answers `NeedsQuorum`, and so does one whose round is still in flight. A
//! round remembers the commit index; a majority answers at it and the lease then answers the same
//! position even after a newer commit. One acknowledgement short of a majority is refused in a
//! three-node cluster and in a five-node one, and the tests state the arithmetic both times. The
//! last millisecond of the lease answers and the expiry instant does not. A lease answers the
//! position its round proved, and a *new* round sees the newer commit, which is what makes the
//! first answer a proof rather than a cache. Stepping down drops the lease and the read index and
//! turns a served read into a refusal. A served round cannot be finished a second time to extend
//! its lease. And `is_safe_lease` is true one millisecond below the timeout, false at it, and false
//! above it.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, and **none of the machinery that makes a read linearizable**: there
//! is no log, no term on a message, no vote, no heartbeat sent anywhere, no network and no daemon.
//! The acknowledgements arrive as a slice of names handed to [`Node::complete_read`], and "a
//! majority" is arithmetic over that slice rather than a statement about who could have won an
//! election. The thing the real step proves — a majority that acknowledged this leader intersects
//! any majority that could elect another, so no other term can have committed past the read index —
//! is assumed here, not re-derived.
//!
//! What is re-checked is the part a reviewer has to be able to check: the relationship between two
//! durations, the instant the lease ends, the position it freezes, and the fact that it cannot
//! outlive the role that earned it. Those are exactly the places a lease implementation goes wrong,
//! and a lease longer than the election timeout is a stale read with extra steps however correct
//! the log is. `time/clock-injection@apply` is the habit that makes all of this testable — every
//! answer here is about the `now` it was given — and `dist/client-routing@apply` is the next
//! question: a client that has been told where the leader is still has to send the request
//! somewhere, and retry it when the answer does not arrive.

/// A position in the replicated log.
pub type Index = u64;

/// A Raft term. Leadership is per term, and a lease that outlived its term answers for a term that
/// has ended.
pub type Term = u64;

/// How long a leader may answer reads from its own memory after a majority acknowledged it.
pub const LEASE_MILLIS: u64 = 400;

/// The election timeout this course's lease is derived from.
pub const ELECTION_TIMEOUT_MILLIS: u64 = 1_000;

/// `true` when `millis` is short enough to be a lease against an election timeout of `timeout`.
///
/// Equal is already too long: an election can start at the instant the lease would end, so the
/// comparison has to be strict.
#[must_use]
pub fn is_safe_lease(millis: u64, timeout: u64) -> bool {
    todo!()
}

/// A window in which a leader may answer reads locally, and the position those answers are correct
/// at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    expires_at: u64,
    index: Index,
}

impl Lease {
    /// A lease that began at `now`, lasts `millis`, and is correct at `index`.
    #[must_use]
    pub fn new(now: u64, millis: u64, index: Index) -> Self {
        Self {
            expires_at: now.saturating_add(millis),
            index,
        }
    }

    /// The instant it expires, on the clock it was taken on.
    #[must_use]
    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }

    /// The position the reads it covers are correct at.
    #[must_use]
    pub fn index(&self) -> Index {
        self.index
    }

    /// `true` while `now` is before the expiry: a lease that expires *at* an instant is already
    /// over at that instant, which is the safe side of the comparison.
    #[must_use]
    pub fn is_valid(&self, now: u64) -> bool {
        now < self.expires_at
    }
}

/// What a node answers a read with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadAnswer {
    /// The value at this index is linearizable: nothing committed after it can exist yet.
    Served {
        /// The position the answer is correct at.
        index: Index,
    },
    /// This node is not the leader, and this is who it believes is.
    NotLeader {
        /// The leader this node knows of, when it knows one.
        leader: Option<String>,
    },
    /// This node is the leader and has no valid lease: a read needs a confirmed round.
    NeedsQuorum,
}

impl ReadAnswer {
    /// `true` when the read was answered.
    #[must_use]
    pub fn is_served(&self) -> bool {
        matches!(self, Self::Served { .. })
    }
}

/// Why a read round could not be started or finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoundError {
    /// This node is not the leader, and this is the leader it knows.
    NotLeader {
        /// The leader this node knows of, when it knows one.
        leader: Option<String>,
    },
}

/// What a node is, and who it believes leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// This node leads the cluster in its term.
    Leader,
    /// This node follows, and names the leader it has heard from, if it has heard from one.
    Follower {
        /// The leader this node believes is leading, when it knows one.
        leader: Option<String>,
    },
}

/// One node's read memory: what it is, what its last round proved, and what it may still answer.
#[derive(Debug)]
pub struct Node {
    name: String,
    peers: Vec<String>,
    role: Role,
    term: Term,
    commit_index: Index,
    read_index: Option<Index>,
    lease: Option<Lease>,
}

impl Node {
    /// A leader of `peers` and itself, in term 1, with the log committed through `committed`.
    #[must_use]
    pub fn leader(name: &str, peers: &[&str], committed: Index) -> Self {
        Self {
            name: name.to_owned(),
            peers: peers.iter().map(|peer| (*peer).to_owned()).collect(),
            role: Role::Leader,
            term: 1,
            commit_index: committed,
            read_index: None,
            lease: None,
        }
    }

    /// A follower that has heard from `leader`, if it has heard from one.
    #[must_use]
    pub fn follower(name: &str, leader: Option<&str>) -> Self {
        Self {
            name: name.to_owned(),
            peers: Vec::new(),
            role: Role::Follower {
                leader: leader.map(|leader| leader.to_owned()),
            },
            term: 1,
            commit_index: 0,
            read_index: None,
            lease: None,
        }
    }

    /// The term this node is in.
    #[must_use]
    pub fn term(&self) -> Term {
        self.term
    }

    /// The position the last round remembered, if it remembered one.
    #[must_use]
    pub fn read_index(&self) -> Option<Index> {
        self.read_index
    }

    /// The lease this node holds, if it holds one.
    #[must_use]
    pub fn lease(&self) -> Option<Lease> {
        self.lease
    }

    /// Record that the log is committed through `index`, as a write that reached a majority does.
    pub fn commit_to(&mut self, index: Index) {
        self.commit_index = index;
    }

    /// The leader this node believes in, if it believes in one.
    fn believed_leader(&self) -> Option<String> {
        match &self.role {
            Role::Leader => Some(self.name.clone()),
            Role::Follower { leader } => leader.clone(),
        }
    }

    /// Begin a read round: remember the commit index, and answer the peers to heartbeat.
    ///
    /// The round proves nothing until it is finished by [`Node::complete_read`]; a leader that has
    /// only started one still may not answer.
    ///
    /// # Errors
    ///
    /// [`RoundError::NotLeader`] when this node is not the leader: a follower has nothing to
    /// confirm.
    pub fn confirm_read(&mut self) -> Result<Vec<String>, RoundError> {
        todo!()
    }

    /// Finish a round with the peers that acknowledged its heartbeat, at `now`.
    ///
    /// A majority of the cluster, the leader included, grants a lease that starts at `now` and
    /// covers the position the round remembered. A round that does not find a majority grants
    /// nothing. A served round is spent: finishing it again without a new round is not a new proof,
    /// and must not extend the window. Finishing a round that was never started — or one that was
    /// already answered — answers [`ReadAnswer::NeedsQuorum`]: there is nothing to prove.
    ///
    /// # Errors
    ///
    /// [`RoundError::NotLeader`] when this node is not the leader.
    pub fn complete_read(
        &mut self,
        acknowledged: &[String],
        now: u64,
    ) -> Result<ReadAnswer, RoundError> {
        todo!()
    }

    /// Answer a read at `now` from this node's own memory, if it may.
    ///
    /// A follower answers [`ReadAnswer::NotLeader`] with the leader it knows; a leader answers
    /// [`ReadAnswer::Served`] at its lease's index while the lease is valid, and
    /// [`ReadAnswer::NeedsQuorum`] otherwise.
    #[must_use]
    pub fn serve_read(&self, now: u64) -> ReadAnswer {
        todo!()
    }

    /// Stop leading in `term`: the role, the lease and the read index change in the same breath.
    ///
    /// A newer term is not a pause in leadership: it is the end of it. Keeping either the lease or
    /// the read index would let this node answer a read for a term that has already ended.
    pub fn step_down(&mut self, term: Term) {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A three-node leader: n1 leads n2 and n3, and the log is committed through index 7.
    fn leader() -> Node {
        Node::leader("n1", &["n2", "n3"], 7)
    }

    /// The peers that acknowledged a heartbeat, as [`Node::complete_read`] takes them.
    fn acked(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_follower_does_not_answer_a_read_and_says_who_does() {
        let heard_from = Node::follower("n2", Some("n1"));
        assert_eq!(
            heard_from.serve_read(0),
            ReadAnswer::NotLeader {
                leader: Some("n1".to_owned()),
            },
            "a follower cannot know how far behind it is: it answers with the leader's name, and \
             the client asks there"
        );

        let mut on_its_own = Node::follower("n2", None);
        assert_eq!(
            on_its_own.serve_read(0),
            ReadAnswer::NotLeader { leader: None },
            "a follower that has heard from nobody says exactly that, instead of inventing a name"
        );
        assert_eq!(
            on_its_own.confirm_read(),
            Err(RoundError::NotLeader { leader: None }),
            "and a follower cannot start a round: it has no leadership to confirm"
        );
    }

    #[test]
    fn a_leader_without_a_round_asks_for_one() {
        let node = leader();
        assert_eq!(
            node.serve_read(0),
            ReadAnswer::NeedsQuorum,
            "being the leader is not a proof that this node still is one: a read needs a round first"
        );
        assert!(
            node.lease().is_none(),
            "and with no round there is no lease to answer from"
        );
    }

    #[test]
    fn a_round_confirms_leadership_and_answers_at_the_commit_index() {
        let mut node = leader();
        assert_eq!(
            node.confirm_read(),
            Ok(acked(&["n2", "n3"])),
            "a round is a heartbeat to every peer, and that is what it sends"
        );
        assert_eq!(
            node.read_index(),
            Some(7),
            "the round remembers the commit index it saw before the round"
        );
        assert!(
            node.lease().is_none(),
            "a round in flight proves nothing yet: the proof is the acknowledgements"
        );

        assert_eq!(
            node.complete_read(&acked(&["n2"]), 1_000),
            Ok(ReadAnswer::Served { index: 7 }),
            "the leader plus one peer is a majority of three, and the answer names the position it \
             is correct at"
        );
        assert_eq!(
            node.serve_read(1_100),
            ReadAnswer::Served { index: 7 },
            "and the lease just taken answers the next read at that same position"
        );
        assert_eq!(
            node.read_index(),
            None,
            "a served round is spent: its proof moved into the lease"
        );
    }

    #[test]
    fn a_majority_is_the_leader_plus_enough_peers_and_one_short_is_not() {
        let mut three = leader();
        three.confirm_read().expect("the leader starts a round");
        assert_eq!(
            three.complete_read(&acked(&[]), 1_000),
            Ok(ReadAnswer::NeedsQuorum),
            "the leader alone is one of three, which is not a majority"
        );
        assert!(
            three.lease().is_none(),
            "a round that did not confirm leadership grants no window"
        );
        assert_eq!(
            three.serve_read(1_000),
            ReadAnswer::NeedsQuorum,
            "and the node still may not answer"
        );

        let mut five = Node::leader("n1", &["n2", "n3", "n4", "n5"], 7);
        five.confirm_read().expect("the leader starts a round");
        assert_eq!(
            five.complete_read(&acked(&["n2"]), 1_000),
            Ok(ReadAnswer::NeedsQuorum),
            "the leader plus one peer is two of five: one short of a majority"
        );
        five.confirm_read().expect("the leader asks again");
        assert_eq!(
            five.complete_read(&acked(&["n2", "n3"]), 1_000),
            Ok(ReadAnswer::Served { index: 7 }),
            "the leader plus two peers is three of five, which is a majority"
        );
    }

    #[test]
    fn the_lease_ends_at_its_expiry_and_not_one_millisecond_later() {
        let mut node = leader();
        node.confirm_read().expect("a round");
        node.complete_read(&acked(&["n2"]), 1_000)
            .expect("a majority");

        assert_eq!(
            node.serve_read(1_000 + LEASE_MILLIS - 1),
            ReadAnswer::Served { index: 7 },
            "the last millisecond of the window still answers"
        );
        assert_eq!(
            node.serve_read(1_000 + LEASE_MILLIS),
            ReadAnswer::NeedsQuorum,
            "at exactly the expiry the lease is over: live means `now < expires_at`"
        );
        assert_eq!(
            node.serve_read(1_000 + LEASE_MILLIS + 1),
            ReadAnswer::NeedsQuorum,
            "and it does not come back: a read after the window needs a new round"
        );
    }

    #[test]
    fn stepping_down_drops_the_lease_and_the_read_index() {
        let mut node = leader();
        node.confirm_read().expect("a round");
        node.complete_read(&acked(&["n2"]), 1_000)
            .expect("a majority");
        assert!(
            node.serve_read(1_100).is_served(),
            "the fixture has a lease to lose, or the rest of this test proves nothing"
        );

        node.step_down(9);
        assert_eq!(node.term(), 9);
        assert_eq!(
            node.read_index(),
            None,
            "a read index is a claim about a term that has ended"
        );
        assert!(
            node.lease().is_none(),
            "and a promise about being the leader cannot outlive the leadership"
        );
        assert_eq!(
            node.serve_read(1_100),
            ReadAnswer::NotLeader { leader: None },
            "so a read is refused rather than answered from a term that ended"
        );
    }

    #[test]
    fn the_lease_answers_the_position_the_round_proved_and_not_a_later_commit() {
        let mut node = Node::leader("n1", &["n2", "n3"], 4);
        node.confirm_read().expect("a round");
        node.complete_read(&acked(&["n2"]), 1_000)
            .expect("a majority");

        node.commit_to(9);
        assert_eq!(
            node.serve_read(1_100),
            ReadAnswer::Served { index: 4 },
            "the lease answers the position the round proved: a value committed after it may not be \
             on a majority yet"
        );

        node.confirm_read().expect("a new round");
        assert_eq!(
            node.complete_read(&acked(&["n2"]), 1_200),
            Ok(ReadAnswer::Served { index: 9 }),
            "a new round does see the newer commit index, which is what makes the first answer a \
             proof rather than a cache"
        );
    }

    #[test]
    fn a_round_is_answered_once_and_a_second_answer_proves_nothing() {
        let mut node = leader();
        node.confirm_read().expect("a round");
        node.complete_read(&acked(&["n2"]), 1_000)
            .expect("a majority");

        assert_eq!(
            node.complete_read(&acked(&["n2", "n3"]), 1_390),
            Ok(ReadAnswer::NeedsQuorum),
            "the round was spent when it was answered: finishing it again without new heartbeats is \
             not a new proof"
        );
        assert_eq!(
            node.serve_read(1_000 + LEASE_MILLIS),
            ReadAnswer::NeedsQuorum,
            "and the lease still ends where the round that earned it said it would, not 390 ms later"
        );
    }

    #[test]
    fn a_lease_has_to_be_shorter_than_the_election_timeout() {
        assert!(
            is_safe_lease(LEASE_MILLIS, ELECTION_TIMEOUT_MILLIS),
            "the course's constant is the sentence: 400 ms is short enough for a 1 000 ms timeout"
        );
        assert!(
            is_safe_lease(ELECTION_TIMEOUT_MILLIS - 1, ELECTION_TIMEOUT_MILLIS),
            "one millisecond shorter is still shorter"
        );
        assert!(
            !is_safe_lease(ELECTION_TIMEOUT_MILLIS, ELECTION_TIMEOUT_MILLIS),
            "a lease as long as the timeout is already too long: an election can start at the \
             instant the lease would end"
        );
        assert!(
            !is_safe_lease(ELECTION_TIMEOUT_MILLIS + 1, ELECTION_TIMEOUT_MILLIS),
            "and a lease longer than the timeout is a stale read with extra steps"
        );
        assert!(
            is_safe_lease(0, ELECTION_TIMEOUT_MILLIS),
            "a zero lease is safe and useless: every read asks for a round"
        );
    }
}
