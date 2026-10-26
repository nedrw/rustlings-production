//! probe = "perf/distributed-budget@design"
//! title = "Adding a node buys fault tolerance, and the leader pays for it in counts"
//! hint = "Separate the quorum from the fan-out, count the acknowledgements the leader waits for, report the per-node share as exact parts per million, and refuse a printed run whose counts cannot be this model's."
//!
//! Step `meshkv/5.17-distributed-budgets` turns "does it scale?" into arithmetic a reader can check.
//! The shallower slot, `perf/bench-budget@apply` (`jobq/3.22`), is how a budget is written at all:
//! counters instead of seconds, a baseline the course owns. This file is the cluster's version of the
//! same decision, and it has three axes that only exist because there is more than one node:
//!
//! 1. **the quorum and the fan-out are different numbers.** A majority is `⌊n/2⌋ + 1`; the leader
//!    sends `n − 1` messages and waits for `quorum − 1` acknowledgements. A budget that used the
//!    sends as the wait overstates latency; one that used the wait as the sends understates the
//!    network. [`cost`] keeps both.
//! 2. **a count is the same on a busy machine as on an idle one.** This is why the step's gate reads
//!    counts and never wall-clock time, and why every number in this file is a `usize`: a budget in
//!    milliseconds fails on a loaded machine and passes on an empty one, which teaches the wrong
//!    lesson about which changes are free.
//! 3. **adding a node is not free.** The leader sends one more message per write per node, while a
//!    follower sends one per write whatever `n` is. [`Counts::per_node_ppm`] is the average: it
//!    *rises* toward one as the cluster grows, hiding the leader's `n − 1` behind a bigger divisor.
//!    The average and the leader's share answer different questions, and a budget needs both.
//! 4. **a run is only a run if its counts fit the model.** [`Counts::audit`] recomputes the shape
//!    from `nodes` and `writes` and refuses the first axis that disagrees. A gate that compares
//!    counts from two different clusters is a gate that stays green while the cost it measures moved
//!    to another machine.
//! 5. **the ratio is integer, so its boundary has one answer.** The per-node share is parts per
//!    million with no float: a threshold that arrived as `0.9999999` is a comparison between two
//!    machines, not between two runs.
//!
//! ## What you write
//!
//! * [`cost`] — one write and one linearizable read, for a cluster of `nodes`;
//! * [`workload`] — the same shape multiplied by a number of writes;
//! * [`Counts::per_node_ppm`] — the average messages per node, in millionths, with the integer
//!   division the tests can name;
//! * [`Counts::audit`] — the first axis on which a printed run cannot be this model's run.
//!
//! [`Cost`], [`Counts`], [`Axis`], [`Flaw`] and [`PPM`] are given. The fields are the three axes the
//! step puts on one page — quorum, fan-out, acknowledgements — plus the two hop counts, and the axis
//! names are what the gate's report prints; a probe that let you rename them would be checking a
//! different table.
//!
//! ## What the tests ask
//!
//! The quorum for one through seven nodes, including the even sizes where half the cluster plus the
//! leader is exactly the majority, and a cluster of no nodes, which has no quorum to budget for. A
//! write and a linearizable read each cost one hop whatever the size. The totals for five nodes and
//! a thousand writes are the step's printed numbers — four thousand messages, two thousand
//! acknowledgements, four thousand leader messages, a thousand per follower — and a workload of no
//! writes is a run of zeros rather than an error. The leader's messages are the followers' times the
//! peers and the total is the leader's, in both directions. The per-node average is exact at the
//! small sizes, rises with every added node, never reaches one, and is zero for a single node. And
//! an honest run audits clean while the same run with one axis altered is refused *at that axis*,
//! with both numbers in the reason; a run with two axes altered is refused at the first one the
//! audit reads.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no cluster, no network and no clock**. The counts are handed in;
//! nothing here sends a message or measures a node. The real step's harness runs a workload and
//! prints the counts the `bench-budget` gate compares, and the gate's budget file is the baseline
//! this arithmetic is supposed to predict. What is re-checked here is exactly the arithmetic and the
//! audit: two numbers that are different, an average that rises, and a measurement that cannot be
//! the model's.
//!
//! Missing on purpose: there is no read workload (the model's `read_hops` is here, but a workload of
//! reads is the step's harness), no batching, no retries, no CPU or memory cost — the average is a
//! ratio of messages, not a share of a machine — and nothing here compares a run to a *baseline*;
//! that comparison is `perf/bench-budget@operate`, while this file only decides whether the numbers
//! could have come from the shape at all. The predicted-versus-measured table is
//! `ops/capacity-cost@design`, and the quorum is the same `⌊n/2⌋ + 1` as `meshkv/5.15`'s rolling
//! restart and `ops/rolling-upgrade@design`'s waves.

use std::fmt;

/// How a fraction is spelled here: a millionth, so every boundary is an integer.
pub const PPM: u64 = 1_000_000;

/// What one cluster shape costs for one write and one linearizable read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cost {
    /// How many nodes the cluster has.
    pub nodes: usize,
    /// How many acknowledgements decide anything.
    pub quorum: usize,
    /// How many messages the leader sends for one write.
    pub messages: usize,
    /// How many acknowledgements the leader waits for.
    pub acks: usize,
    /// How many round trips a write takes before it is decided.
    pub write_hops: usize,
    /// How many round trips a linearizable read takes.
    pub read_hops: usize,
}

/// The counts a workload produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// How many nodes the cluster has.
    pub nodes: usize,
    /// How many writes the workload makes.
    pub writes: usize,
    /// Messages the cluster carries in total.
    pub messages: usize,
    /// Acknowledgements the leader waits for in total.
    pub acks: usize,
    /// Messages the leader sends in total — one per peer per write.
    pub leader_messages: usize,
    /// Messages one follower sends in total — one per write.
    pub follower_messages: usize,
    /// Round trips a write takes before it is decided.
    pub hops: usize,
}

/// One of the numbers a budget is made of, so a refusal can name the one that moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Messages the cluster carries.
    Messages,
    /// Acknowledgements the leader waits for.
    Acks,
    /// Messages the leader sends.
    LeaderMessages,
    /// Messages one follower sends.
    FollowerMessages,
    /// Round trips a write takes.
    Hops,
}

impl Axis {
    /// The name the gate's report prints.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            Self::Acks => "acks",
            Self::LeaderMessages => "leader messages",
            Self::FollowerMessages => "follower messages",
            Self::Hops => "hops",
        }
    }
}

/// Why a shape or a run cannot be a budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flaw {
    /// A cluster of no nodes has no quorum.
    NoNodes,
    /// A printed count is not one this shape can produce.
    NotARun {
        /// The axis that disagrees.
        axis: Axis,
        /// What the run printed.
        measured: usize,
        /// What the model says for this shape.
        expected: usize,
    },
}

impl fmt::Display for Flaw {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoNodes => write!(
                formatter,
                "a cluster of no nodes has no quorum to budget for"
            ),
            Self::NotARun {
                axis,
                measured,
                expected,
            } => write!(
                formatter,
                "the run reports {measured} {} and this shape produces {expected}: the counts cannot be \
                 one cluster's",
                axis.name()
            ),
        }
    }
}

/// The cost of a cluster of `nodes` nodes for one write and one linearizable read.
///
/// # Errors
///
/// [`Flaw::NoNodes`] when the cluster has no nodes: there is no majority to compute, and a budget
/// for a cluster that does not exist is a number nobody can use.
pub fn cost(nodes: usize) -> Result<Cost, Flaw> {
    if nodes == 0 {
        return Err(Flaw::NoNodes);
    }
    let quorum = nodes / 2 + 1;
    Ok(Cost {
        nodes,
        quorum,
        messages: nodes - 1,
        acks: quorum - 1,
        write_hops: 1,
        read_hops: 1,
    })
}

/// The counts `writes` writes produce on a cluster of `nodes` nodes.
///
/// # Errors
///
/// [`Flaw::NoNodes`] when the cluster has no nodes.
pub fn workload(nodes: usize, writes: usize) -> Result<Counts, Flaw> {
    let cost = cost(nodes)?;
    Ok(Counts {
        nodes,
        writes,
        messages: cost.messages * writes,
        acks: cost.acks * writes,
        leader_messages: cost.messages * writes,
        follower_messages: writes,
        hops: cost.write_hops,
    })
}

impl Counts {
    /// The average messages per node, in millionths: the number that rises as the cluster grows.
    ///
    /// The answer is `messages × 1_000_000 / nodes`, rounded down. That is `(n − 1) / n` for a one
    /// write workload — zero for a single node and never one — and it is the average that hides the
    /// leader's `n − 1`, which the caller has to read separately.
    ///
    /// # Errors
    ///
    /// [`Flaw::NoNodes`] when the run claims a cluster of no nodes.
    pub fn per_node_ppm(&self) -> Result<u64, Flaw> {
        if self.nodes == 0 {
            return Err(Flaw::NoNodes);
        }
        let messages = u64::try_from(self.messages).expect("a count fits in u64");
        let nodes = u64::try_from(self.nodes).expect("a count fits in u64");
        Ok(messages.saturating_mul(PPM) / nodes)
    }

    /// Check that these counts could be this shape's run, and name the first axis that could not.
    ///
    /// The audit recomputes the model from `nodes` and `writes`; it does not trust any of the other
    /// fields. The axes are read in the order of [`Axis`], because a run with two numbers wrong has
    /// one first thing to fix.
    ///
    /// # Errors
    ///
    /// [`Flaw::NotARun`] for the first axis whose printed count differs from the model's, and
    /// [`Flaw::NoNodes`] when the run claims no cluster.
    pub fn audit(&self) -> Result<(), Flaw> {
        if self.nodes == 0 {
            return Err(Flaw::NoNodes);
        }
        let expected = workload(self.nodes, self.writes)?;
        for (axis, measured, expected) in [
            (Axis::Messages, self.messages, expected.messages),
            (Axis::Acks, self.acks, expected.acks),
            (
                Axis::LeaderMessages,
                self.leader_messages,
                expected.leader_messages,
            ),
            (
                Axis::FollowerMessages,
                self.follower_messages,
                expected.follower_messages,
            ),
            (Axis::Hops, self.hops, expected.hops),
        ] {
            if measured != expected {
                return Err(Flaw::NotARun {
                    axis,
                    measured,
                    expected,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quorum_is_a_majority_and_the_fan_out_is_every_peer() {
        for (nodes, quorum) in [(1, 1), (2, 2), (3, 2), (4, 3), (5, 3), (6, 4), (7, 4)] {
            let cost = cost(nodes).expect("a cluster");
            assert_eq!(
                cost.quorum, quorum,
                "{nodes} nodes decide with {quorum} acknowledgements, itself included"
            );
            assert_eq!(
                cost.messages,
                nodes - 1,
                "the leader sends to every peer and nobody else sends write messages"
            );
            assert_eq!(
                cost.acks,
                quorum - 1,
                "what the leader waits for is the majority minus itself, never the whole fan-out"
            );
        }
        assert_eq!(
            cost(1).expect("a cluster").acks,
            0,
            "a single node needs nobody's acknowledgement"
        );
        assert_eq!(
            cost(0).expect_err("no nodes, no quorum"),
            Flaw::NoNodes,
            "a cluster of no nodes cannot be asked what a write costs"
        );
    }

    #[test]
    fn a_write_and_a_linearizable_read_each_cost_one_round_trip() {
        for nodes in 1..=9 {
            let cost = cost(nodes).expect("a cluster");
            assert_eq!(
                cost.write_hops, 1,
                "a write waits for one round trip whatever the size: {nodes} nodes"
            );
            assert_eq!(
                cost.read_hops, 1,
                "and a linearizable read waits for one too: {nodes} nodes"
            );
        }
    }

    #[test]
    fn the_totals_are_the_per_write_cost_times_the_writes() {
        let counts = workload(5, 1_000).expect("a workload");
        assert_eq!(
            counts,
            Counts {
                nodes: 5,
                writes: 1_000,
                messages: 4_000,
                acks: 2_000,
                leader_messages: 4_000,
                follower_messages: 1_000,
                hops: 1,
            },
            "five nodes, a thousand writes: four thousand sends, two thousand acknowledgements, \
             four thousand on the leader, a thousand per follower, one hop"
        );

        let idle = workload(5, 0).expect("no writes is a workload");
        assert_eq!(
            (idle.messages, idle.acks, idle.leader_messages),
            (0, 0, 0),
            "a workload of no writes costs nothing rather than being an error"
        );
        assert_eq!(
            idle.per_node_ppm().expect("a share"),
            0,
            "and no messages per node is no share"
        );
        assert_eq!(idle.audit(), Ok(()), "the empty run is still a run");
    }

    #[test]
    fn the_leader_does_more_than_a_follower_and_the_gap_is_the_fan_out() {
        for nodes in 2..=7 {
            let counts = workload(nodes, 100).expect("a workload");
            assert_eq!(
                counts.leader_messages,
                counts.follower_messages * (nodes - 1),
                "the leader sends one message per peer per write and a follower sends one per write"
            );
            assert_eq!(
                counts.leader_messages, counts.messages,
                "and those sends are the whole network: there is no other sender"
            );
        }
    }

    #[test]
    fn the_per_node_average_rises_towards_one_and_never_reaches_it() {
        assert_eq!(
            workload(1, 1)
                .expect("a workload")
                .per_node_ppm()
                .expect("a share"),
            0,
            "a single node sends nothing and receives nothing: its average is zero"
        );
        for (nodes, ppm) in [(2, 500_000), (3, 666_666), (4, 750_000), (7, 857_142)] {
            let counts = workload(nodes, 1).expect("a workload");
            assert_eq!(
                counts.per_node_ppm().expect("a share"),
                ppm,
                "{nodes} nodes put {ppm} millionths of a message on the average node"
            );
        }

        let mut previous = 0;
        for nodes in 2..=9 {
            let ppm = workload(nodes, 1)
                .expect("a workload")
                .per_node_ppm()
                .expect("a share");
            assert!(
                ppm > previous,
                "adding a node raises the average: {nodes} nodes give {ppm} against {previous}"
            );
            assert!(
                ppm < PPM,
                "and the average never reaches one message per node: {nodes} nodes give {ppm}"
            );
            previous = ppm;
        }
    }

    #[test]
    fn a_printed_run_that_cannot_be_the_models_run_is_refused_with_the_axis() {
        let honest = workload(5, 1_000).expect("a workload");

        let mut wrong_messages = honest;
        wrong_messages.messages += 1;
        let flaw = wrong_messages.audit().expect_err("one send too many");
        assert_eq!(
            flaw,
            Flaw::NotARun {
                axis: Axis::Messages,
                measured: 4_001,
                expected: 4_000,
            },
            "the network count has to be the fan-out times the writes"
        );

        let mut wrong_acks = honest;
        wrong_acks.acks += 1;
        assert_eq!(
            wrong_acks
                .audit()
                .expect_err("one acknowledgement too many"),
            Flaw::NotARun {
                axis: Axis::Acks,
                measured: 2_001,
                expected: 2_000,
            },
            "acknowledgements are the decision, and the decision cannot drift by one"
        );

        let mut wrong_leader = honest;
        wrong_leader.leader_messages -= 1;
        assert_eq!(
            wrong_leader
                .audit()
                .expect_err("a send the leader did not make"),
            Flaw::NotARun {
                axis: Axis::LeaderMessages,
                measured: 3_999,
                expected: 4_000,
            },
            "the leader is the only sender of write messages"
        );

        let mut wrong_follower = honest;
        wrong_follower.follower_messages += 1;
        assert_eq!(
            wrong_follower
                .audit()
                .expect_err("a follower sending twice"),
            Flaw::NotARun {
                axis: Axis::FollowerMessages,
                measured: 1_001,
                expected: 1_000,
            },
            "a follower sends one message per write, not two"
        );

        let mut wrong_hops = honest;
        wrong_hops.hops = 2;
        let flaw = wrong_hops
            .audit()
            .expect_err("two round trips where the model says one");
        assert_eq!(
            flaw,
            Flaw::NotARun {
                axis: Axis::Hops,
                measured: 2,
                expected: 1,
            },
            "a write is decided in one round trip, and two is a different protocol"
        );
        assert!(
            flaw.to_string().contains("hops") && flaw.to_string().contains("2"),
            "the reason names the axis and the number: {flaw}"
        );

        let mut two_wrong = honest;
        two_wrong.messages += 1;
        two_wrong.acks += 1;
        let flaw = two_wrong.audit().expect_err("two axes cannot be one run");
        assert!(
            matches!(
                flaw,
                Flaw::NotARun {
                    axis: Axis::Messages,
                    ..
                }
            ),
            "the first axis in the audit's order is the one to fix first: {flaw:?}"
        );
    }

    #[test]
    fn an_honest_run_audits_clean_and_a_run_of_no_nodes_is_refused() {
        for (nodes, writes) in [(1, 0), (1, 5), (2, 1), (3, 10), (5, 1_000), (7, 3)] {
            let counts = workload(nodes, writes).expect("a workload");
            assert_eq!(
                counts.audit(),
                Ok(()),
                "the model's own counts are the run it predicts: {nodes} nodes, {writes} writes"
            );
        }

        let no_cluster = Counts {
            nodes: 0,
            writes: 10,
            messages: 0,
            acks: 0,
            leader_messages: 0,
            follower_messages: 10,
            hops: 1,
        };
        assert_eq!(
            no_cluster.audit(),
            Err(Flaw::NoNodes),
            "a run that claims no cluster cannot be checked against a majority"
        );
        assert_eq!(
            no_cluster.per_node_ppm(),
            Err(Flaw::NoNodes),
            "and it has no per-node share to report"
        );
    }
}
