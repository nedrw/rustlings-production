//! probe = "query/consistency-levels@apply"
//! title = "A level that cannot be honoured is refused, never quietly downgraded"
//! hint = "Parse the three levels strictly, answer a linearizable read only from a leader holding a lease and at the lease's index, serve a bounded-stale read only when the lag it can show is inside the bound, and refuse with the reason and the numbers."
//!
//! Step `meshkv/5.12-consistency-levels` gives the dial every replicated store ends up offering, and
//! the interesting part is not the enum: it is that **each level has to be refused when it cannot be
//! honoured**. A follower asked for a linearizable read must say so rather than answer from its own
//! state; a node asked for bounded-stale must be able to *show* its lag rather than assume it. A
//! level that is silently downgraded is worse than one that is refused, because the caller never
//! learns which promise it got — and a caller that cannot tell cannot reason about its own reads.
//!
//! Four decisions, and each one is a sentence a caller can rely on:
//!
//! 1. **the strong promise needs the leader and a confirmed lease.** No lease means no proof that
//!    this node is still the leader, and "probably still the leader" is exactly the class of answer
//!    the track keeps removing (`5.6`). The refusal names the leader, so the caller can ask there;
//! 2. **a bounded-stale read is served only when the lag is inside the bound, and the boundary is
//!    inclusive.** Three entries behind a bound of three is inside the promise; four is not. The
//!    refusal carries both numbers, so the caller can choose another node or a looser level;
//! 3. **a bound with no leader to measure against is unprovable.** A node that has heard of no
//!    commit index cannot show it is fresh enough, so it refuses rather than guesses;
//! 4. **the answer names the index it is correct at.** For a linearizable read that is the lease's
//!    index, not the newer index this node has applied: a caller told the newer one would believe a
//!    value committed after the round. For bounded and local reads it is the node's applied index,
//!    and a node ahead of the commit it knows about has a lag of zero rather than a negative one.
//!
//! ## What you write
//!
//! * [`Level::parse`] — the three levels, strictly: `linearizable`, `bounded-stale:<n>`, `local`, and
//!   a refusal for anything else, including a bound that is not a number of entries;
//! * [`answer`] — the whole policy as one pure function, with every fact it needs as an argument.
//!
//! [`Level::render`], [`Level::is_linearizable`], [`Answer`], [`Answer::is_served`] and [`View`] are
//! given. `View` is the step's six arguments gathered into one value, so a test cannot hand the
//! function the commit index where the applied one goes: the vocabulary is the interface, and the
//! arguments are its fields.
//!
//! ## What the tests ask
//!
//! Every level round-trips through [`Level::render`] and [`Level::parse`], `bounded-stale:0` is a
//! level and `bounded-stale:-1` is not, and eight strings that are not levels are refused. A follower
//! refuses a linearizable read and the refusal names the leader; a leader without a lease refuses one
//! and the refusal names the lease; a leader with a lease answers at the *lease's* index even though
//! it has applied a later one. A bounded-stale read is tested at its boundary from both sides: three
//! behind a bound of three is served at the node's own index, four behind is refused with the lag and
//! the bound in the reason, and zero behind a bound of zero is served. A bound with no commit index
//! known is refused. A node ahead of the commit it knows about is served, because its lag is zero. And
//! the same view that serves a local read cannot serve a linearizable one — the dial is not a hint.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no cluster, no leases and no time**: `answer` is given the five facts
//! as arguments, so there is no heartbeat, no clock to expire a lease, and no network to learn the
//! commit index from (`5.3`). What is re-checked is the arithmetic and the refusals — the policy — not
//! the mechanism that supplies the numbers. The lease itself is `meshkv/5.6-lease-reads`' subject, the
//! client that turns a refusal into a retry somewhere else is `api/client-sdk@apply`'s, and the
//! cluster-wide audit that compares the levels' promises with what the nodes hold is `5.22`'s.
//!
//! Missing on purpose: there is no clock in [`View`] at all, so the staleness here is counted in
//! entries rather than seconds; a real `bounded-stale` also has a time bound, and that is a second
//! question on top of this one. The dial is also per read rather than per route policy, which is a
//! decision a caller makes rather than one this function can make.

use std::fmt;

/// Why a level was not parsed, or a read was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// What was refused, phrased for the person who wrote the config or the query.
    reason: String,
}

impl Refusal {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    /// The reason, for a test that wants to read it rather than print it.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

/// How fresh a read has to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Correct as of a moment after the read began: the leader's lease.
    Linearizable,
    /// At most `max_lag` entries behind the cluster's commit.
    BoundedStale {
        /// The largest lag, in entries, this read accepts.
        max_lag: u64,
    },
    /// Whatever this node has applied, with no promise about staleness at all.
    Local,
}

impl Level {
    /// Parse `linearizable`, `bounded-stale:<entries>` or `local`.
    ///
    /// # Errors
    ///
    /// [`Refusal`] when the text is not one of the three, or when the bound is not a number of
    /// entries. A level a caller mistyped is refused rather than read as a weaker one.
    pub fn parse(text: &str) -> Result<Self, Refusal> {
        match text {
            "linearizable" => Ok(Self::Linearizable),
            "local" => Ok(Self::Local),
            other => {
                let Some(bound) = other.strip_prefix("bounded-stale:") else {
                    return Err(Refusal::new(format!(
                        "`{text}` is not a consistency level: `linearizable`, \
                         `bounded-stale:<n>` or `local`"
                    )));
                };
                let max_lag = bound
                    .parse::<u64>()
                    .map_err(|_| Refusal::new(format!("`{bound}` is not a number of entries")))?;
                Ok(Self::BoundedStale { max_lag })
            }
        }
    }

    /// The level as it is written, so a level that travelled through a config file is the level a
    /// caller gets.
    #[must_use]
    pub fn render(self) -> String {
        match self {
            Self::Linearizable => "linearizable".to_owned(),
            Self::BoundedStale { max_lag } => format!("bounded-stale:{max_lag}"),
            Self::Local => "local".to_owned(),
        }
    }

    /// `true` when this is the strongest level: the one that needs the leader.
    #[must_use]
    pub fn is_linearizable(self) -> bool {
        matches!(self, Self::Linearizable)
    }
}

/// What a node answers a read at a level with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The answer is correct at this index, under the promise the caller asked for.
    Served {
        /// The position the answer is correct at.
        index: u64,
    },
    /// The level cannot be honoured here, and this is what would have to change.
    Refused {
        /// Why, phrased for the caller that asked.
        reason: String,
    },
}

impl Answer {
    /// `true` when the read was answered.
    #[must_use]
    pub fn is_served(&self) -> bool {
        matches!(self, Self::Served { .. })
    }
}

/// One node's view when it is asked: five facts, and no clock among them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct View<'a> {
    /// Whether this node believes it is the leader.
    pub is_leader: bool,
    /// The index the leader's lease was confirmed at, if it holds one.
    pub lease_index: Option<u64>,
    /// How far this node has applied the log.
    pub applied: u64,
    /// The leader's commit index as this node last heard it; `None` when it has heard of no leader.
    pub commit: Option<u64>,
    /// The leader this node knows of, if any.
    pub leader: Option<&'a str>,
}

/// Answer a read at `level` from one node's view.
///
/// The function is pure: every fact it needs, including the lag, is an argument, so the boundary of a
/// level can be tested without a cluster.
#[must_use]
pub fn answer(level: Level, view: View<'_>) -> Answer {
    match level {
        Level::Linearizable => match (view.is_leader, view.lease_index) {
            (true, Some(index)) => Answer::Served { index },
            (true, None) => Answer::Refused {
                reason: "this node leads but has no lease: a linearizable read needs a confirmed \
                         round"
                    .to_owned(),
            },
            (false, _) => Answer::Refused {
                reason: match view.leader {
                    Some(leader) => format!(
                        "this node is a follower: a linearizable read has to be served by `{leader}`"
                    ),
                    None => "this node is a follower and knows no leader yet".to_owned(),
                },
            },
        },
        Level::BoundedStale { max_lag } => {
            let Some(commit) = view.commit else {
                return Answer::Refused {
                    reason:
                        "this node has heard of no leader, so it cannot show how far behind it \
                             is"
                        .to_owned(),
                };
            };
            if commit <= view.applied {
                // At or ahead of the commit it knows about: nothing to bound, and nothing negative
                // to call a lag.
                return Answer::Served {
                    index: view.applied,
                };
            }
            let lag = commit - view.applied;
            if lag <= max_lag {
                Answer::Served {
                    index: view.applied,
                }
            } else {
                Answer::Refused {
                    reason: format!(
                        "this node is {lag} entries behind and the level allows {max_lag}"
                    ),
                }
            }
        }
        Level::Local => Answer::Served {
            index: view.applied,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A follower that knows the leader's commit index, and one leader name.
    fn behind(applied: u64, commit: u64) -> View<'static> {
        View {
            is_leader: false,
            lease_index: None,
            applied,
            commit: Some(commit),
            leader: Some("n1"),
        }
    }

    #[test]
    fn a_level_parses_and_renders_and_anything_else_is_refused() {
        for text in [
            "linearizable",
            "bounded-stale:0",
            "bounded-stale:12",
            "local",
        ] {
            let level = Level::parse(text).expect("a level this build writes");
            assert_eq!(
                level.render(),
                text,
                "the wire form survives the round trip, so a level in a config file is the level a \
                 caller gets"
            );
        }
        assert_eq!(
            Level::parse("bounded-stale:0").expect("a level"),
            Level::BoundedStale { max_lag: 0 },
            "a bound of zero is a level: it accepts a node exactly at the commit and nothing \
             further behind"
        );

        for bad in [
            "",
            "strong",
            "bounded-stale",
            "bounded-stale:",
            "bounded-stale:x",
            "bounded-stale:-1",
            "Bounded-stale:3",
            "linearizable ",
        ] {
            assert!(
                Level::parse(bad).is_err(),
                "`{bad}` is not a level, and a build that read it as a weaker one would answer a \
                 promise the caller never asked for"
            );
        }
    }

    #[test]
    fn a_follower_refuses_a_linearizable_read_and_names_the_leader() {
        let view = View {
            is_leader: false,
            applied: 10,
            commit: Some(12),
            leader: Some("n1"),
            ..View::default()
        };
        match answer(Level::Linearizable, view) {
            Answer::Refused { reason } => assert!(
                reason.contains("n1"),
                "the refusal has to say where to ask next, or the caller can only give up: {reason}"
            ),
            Answer::Served { index } => panic!(
                "a follower has no lease to prove a linearizable read, and this one answered at \
                 {index}"
            ),
        }
    }

    #[test]
    fn a_leader_without_a_lease_refuses_a_linearizable_read() {
        let view = View {
            is_leader: true,
            lease_index: None,
            applied: 9,
            commit: Some(9),
            leader: None,
        };
        match answer(Level::Linearizable, view) {
            Answer::Refused { reason } => assert!(
                reason.contains("lease"),
                "the refusal has to say what is missing: a leader that has not confirmed a round \
                 is probably still the leader, and probably is not a promise: {reason}"
            ),
            Answer::Served { index } => panic!(
                "a leader without a lease answered a linearizable read at {index}: there is no \
                 proof that it is still the leader"
            ),
        }
    }

    #[test]
    fn a_leader_with_a_lease_answers_at_the_lease_index() {
        let view = View {
            is_leader: true,
            lease_index: Some(7),
            applied: 9,
            commit: Some(9),
            leader: Some("n1"),
        };
        assert_eq!(
            answer(Level::Linearizable, view),
            Answer::Served { index: 7 },
            "the answer is correct at the index the lease proved, not at the newer one this node \
             has applied: a caller told 9 would believe a value committed after the round"
        );
    }

    #[test]
    fn a_bounded_stale_read_at_the_bound_is_served_and_one_past_it_is_refused() {
        let level = Level::BoundedStale { max_lag: 3 };
        assert_eq!(
            answer(level, behind(9, 12)),
            Answer::Served { index: 9 },
            "exactly three behind a bound of three is inside the promise, and the index is the \
             node's own"
        );
        match answer(level, behind(8, 12)) {
            Answer::Refused { reason } => assert!(
                reason.contains('4') && reason.contains('3'),
                "the refusal carries the lag and the bound, so the caller can pick another node or \
                 a looser level: {reason}"
            ),
            Answer::Served { index } => panic!(
                "four entries behind exceeds a bound of three, and this node answered at {index}"
            ),
        }
        assert_eq!(
            answer(Level::BoundedStale { max_lag: 0 }, behind(12, 12)),
            Answer::Served { index: 12 },
            "zero behind a bound of zero is inside it: the boundary is inclusive on the side that \
             keeps the promise"
        );
    }

    #[test]
    fn a_bound_with_no_leader_to_measure_against_is_refused() {
        let view = View {
            applied: 4,
            commit: None,
            leader: None,
            ..View::default()
        };
        match answer(Level::BoundedStale { max_lag: 100 }, view) {
            Answer::Refused { reason } => assert!(
                reason.contains("no leader"),
                "the refusal says why: a lag nobody has heard a number for cannot be shown to be \
                 inside a bound: {reason}"
            ),
            Answer::Served { index } => panic!(
                "a node that has heard of no leader cannot prove it is fresh enough, and this one \
                 answered at {index}"
            ),
        }
    }

    #[test]
    fn a_node_ahead_of_the_commit_it_knows_about_is_not_negative_behind() {
        assert_eq!(
            answer(Level::BoundedStale { max_lag: 0 }, behind(12, 10)),
            Answer::Served { index: 12 },
            "a node ahead of the commit it last heard of has a lag of zero rather than a negative \
             one: a bound of zero still serves it"
        );
    }

    #[test]
    fn the_same_view_serves_a_local_read_and_cannot_serve_a_linearizable_one() {
        let view = View {
            is_leader: false,
            applied: 2,
            commit: None,
            leader: None,
            ..View::default()
        };
        assert_eq!(
            answer(Level::Local, view),
            Answer::Served { index: 2 },
            "a local read promises this node and nothing else, so a node cut off from the cluster \
             can always answer it"
        );
        assert!(
            !answer(Level::Linearizable, view).is_served(),
            "and the same node cannot answer the strongest level: the dial is not a hint, and a \
             level that was quietly downgraded would be a promise the caller never got"
        );
        assert!(Level::Linearizable.is_linearizable());
        assert!(!Level::Local.is_linearizable());
        assert!(
            !Level::BoundedStale { max_lag: 1_000 }.is_linearizable(),
            "no bound is the strong promise, however generous the number"
        );
    }
}
