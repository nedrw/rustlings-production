//! probe = "ops/rolling-upgrade@design"
//! title = "The schedule keeps a majority at every wave, or it is not a schedule"
//! hint = "Split the plan into waves, refuse a wave that takes more nodes down than the majority can spare, refuse a plan that starts below the majority, and let every wave but the last speak the old minor."
//!
//! Step `meshkv/5.21-protocol-evolution` teaches two slots. `dist/protocol-evolution@design` owns the
//! wire rule; this file owns the **schedule**, the operator's half — the same "plan checked before it
//! is run" that `meshkv/5.15`'s rolling restart established, now carrying two builds. A rolling
//! upgrade is the third time this track holds two truths at once (joint consensus, certificate
//! rotation, two builds), and the arithmetic is the same in a different unit:
//!
//! 1. **the plan is data and the order is the operator's.** A plan is the cluster, a list of waves,
//!    and the two versions; it is checked before anything is touched. The check does not care which
//!    node goes first; it cares that no instant leaves the cluster unable to decide.
//! 2. **a wave of `k` takes `k` nodes down at once, and a majority must stay up.** So `k` may never
//!    exceed `n − (⌊n/2⌋ + 1)`. In a five-node cluster two nodes may be down and three must remain;
//!    three in one wave is a plan that stops the cluster, and it is exactly the plan a spreadsheet
//!    makes look tidy. [`Plan::run`] refuses at the first wave that breaks it and names the wave and
//!    both counts.
//! 3. **two nodes cannot be rolled one at a time.** A majority of two is both, so taking one down is
//!    already no cluster. Three is the smallest cluster that can lose a node; [`Plan::quorum`] is
//!    `⌊n/2⌋ + 1` here exactly as in `perf/distributed-budget@design` and every other majority in
//!    this track.
//! 4. **the version only moves at the end.** Every wave but the last runs the new build in
//!    compatibility mode, speaking the old minor; the last wave is the move. A plan that upgrades
//!    every node at once is not a rolling upgrade — it is a stop-the-world restart with waves drawn
//!    on it. [`Plan::phases`] is the deploy log an operator reads.
//! 5. **a major bump is not a rolling upgrade, and neither is a plan that starts below its
//!    majority.** Two majors cannot be in one cluster; a wave cannot put back a majority that was
//!    not there when the plan started. Both refusals happen before the first node is touched, which
//!    is the whole value of a plan.
//!
//! ## What you write
//!
//! * [`Plan::new`] — the plan those waves describe, or the first reason they do not describe one:
//!   an empty cluster, no waves, a downgrade, a major crossing, a repeated, missing or foreign node,
//!   or a wave that upgrades nobody;
//! * [`Plan::quorum`] — how many nodes must be up for the cluster to decide;
//! * [`Plan::phases`] — the compatibility phase per wave, with the last one native;
//! * [`Plan::run`] — walk the waves from a given set of live nodes, or refuse at the wave that would
//!   leave fewer than a majority up.
//!
//! [`Protocol`], [`Phase`], [`Step`], [`Refusal`] and the [`NodeId`] alias are given. The phases and
//! the refusal are what a deploy log and a responder read; the wave is the unit the operator thinks
//! in, and it is the unit the check is expressed in. The cluster's members are a sorted list only in
//! the answer [`Refusal::Missing`] gives, because an operator fixes the first missing name.
//!
//! ## What the tests ask
//!
//! A three-node plan with one node per wave is accepted, has a quorum of two, and prints two
//! compatibility phases and one native. A plan that repeats a node, forgets one, invents one, has an
//! empty wave, has no wave, or has no cluster is refused with its own reason and the node or wave it
//! names; a one-wave plan is still accepted structurally, because running it is what the schedule
//! check is for. A major crossing is refused as a major, a downgrade and a same-version plan as
//! something that is not an upgrade, and `1.9 → 1.10` is accepted because versions are numbers. The
//! quorum is checked from one through seven nodes, including the even sizes. Two of five nodes down
//! is a wave that leaves three, and three of five is refused at wave zero; two nodes and one node can
//! never be rolled, while three is the smallest cluster that can. The phases are one per wave, all
//! compatibility but the last. And the same plan is safe when run with a node already down and
//! refused when it starts with only one of three up, which is the invariant read from both sides:
//! the plan's order is the operator's, the starting state is the schedule's.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no processes, no binaries and no health checks**. A node restarts
//! instantly, "up" is a list handed in, and a wave is assumed to finish before the next begins —
//! none of the four is true of a real cluster. The two builds are labels: this file never sends a
//! frame, and the compatibility mode that makes a mixed cluster safe is
//! `dist/protocol-evolution@design`'s rule, not this one's. Nothing here elects a leader, waits for a
//! node to catch up, or notices a node that dies during its own wave; those are `meshkv/5.15`'s
//! drills, and what every node ended up holding is `meshkv/5.22`'s audit.
//!
//! Missing on purpose: no downtime budget in seconds (a clock in a probe is a probe nobody runs),
//! no per-node readiness check, no rollback plan — a plan that has to be abandoned mid-way is a
//! different document — and no certificate or lease arithmetic, which are `meshkv/5.9`'s and
//! `meshkv/5.6`'s. What remains is what the step can honestly re-drill: the majority as a count, the
//! wave as the unit, the phases as the deploy log, and the refusal before the first node moves.

use std::collections::BTreeSet;
use std::fmt;

/// A node's name.
pub type NodeId = String;

/// The version of the wire protocol one build speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Protocol {
    /// The wire format. Two builds with different majors cannot talk at all.
    pub major: u32,
    /// The compatible revision inside a major.
    pub minor: u32,
}

impl Protocol {
    /// A protocol version.
    #[must_use]
    pub fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    /// The version as it is written: `major.minor`.
    #[must_use]
    pub fn render(self) -> String {
        format!("{}.{}", self.major, self.minor)
    }
}

/// What the nodes of one wave run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// A new build, held back to the old minor until the whole cluster is upgraded.
    Compat {
        /// The version it speaks on the wire.
        speaking: Protocol,
        /// The version it was built at.
        built: Protocol,
    },
    /// The last wave: the cluster now speaks the new minor.
    Native {
        /// The version the cluster speaks.
        version: Protocol,
    },
}

/// One wave after it ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The wave's position in the plan, from zero.
    pub wave: usize,
    /// The nodes it upgraded.
    pub nodes: Vec<NodeId>,
    /// How many nodes were up while the wave was down.
    pub up_during: usize,
}

/// Why a list of waves is not a plan, or a plan is not runnable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// There is no cluster.
    NoNodes,
    /// There is no wave.
    NoWaves,
    /// A wave upgrades nobody.
    EmptyWave {
        /// The wave's position.
        wave: usize,
    },
    /// A node appears twice in the plan.
    Repeated {
        /// The node named twice.
        node: NodeId,
    },
    /// A node in the plan is not a member.
    Stranger {
        /// The node that is not a member.
        node: NodeId,
    },
    /// A member does not appear in the plan.
    Missing {
        /// The first member with no wave.
        node: NodeId,
    },
    /// The target is not newer than the source.
    NotAnUpgrade {
        /// The version the cluster speaks.
        from: Protocol,
        /// The version the plan would make it speak.
        to: Protocol,
    },
    /// The plan crosses a major version.
    Major {
        /// The version the cluster speaks.
        from: Protocol,
        /// The version the plan would make it speak.
        to: Protocol,
    },
    /// The cluster is already below its majority before the first wave.
    StartsBelowMajority {
        /// How many nodes are up.
        up: usize,
        /// How many are needed.
        need: usize,
    },
    /// A wave would take the cluster below its majority.
    BelowMajority {
        /// The wave that would do it.
        wave: usize,
        /// How many nodes would be up while it runs.
        up: usize,
        /// How many are needed.
        need: usize,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoNodes => write!(
                formatter,
                "a plan needs a cluster, and this one has no nodes"
            ),
            Self::NoWaves => write!(formatter, "an upgrade with no wave upgrades nobody"),
            Self::EmptyWave { wave } => write!(
                formatter,
                "wave {wave} upgrades nobody: a step that changes nothing is a typo, not a step"
            ),
            Self::Repeated { node } => write!(
                formatter,
                "`{node}` is in the plan twice: a node is upgraded once, and a repeat hides a node \
                 that is not upgraded at all"
            ),
            Self::Stranger { node } => write!(
                formatter,
                "`{node}` is not a member of this cluster, so there is nothing to upgrade"
            ),
            Self::Missing { node } => write!(
                formatter,
                "`{node}` is a member with no wave: a partial upgrade leaves the cluster speaking two \
                 things the plan never accounted for"
            ),
            Self::NotAnUpgrade { from, to } => write!(
                formatter,
                "going from {} to {} is not an upgrade",
                from.render(),
                to.render()
            ),
            Self::Major { from, to } => write!(
                formatter,
                "going from {} to {} crosses a major version: that is a stop-the-world change, not a \
                 rolling upgrade",
                from.render(),
                to.render()
            ),
            Self::StartsBelowMajority { up, need } => write!(
                formatter,
                "the cluster starts with {up} node(s) up and needs {need}: a wave cannot put back a \
                 majority that was not there"
            ),
            Self::BelowMajority { wave, up, need } => write!(
                formatter,
                "wave {wave} would leave {up} node(s) up and the cluster needs {need}: restart one \
                 wave at a time"
            ),
        }
    }
}

/// A rolling upgrade: the cluster, the waves, and the two versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    members: Vec<NodeId>,
    waves: Vec<Vec<NodeId>>,
    from: Protocol,
    to: Protocol,
}

impl Plan {
    /// A plan that upgrades `members` in `waves` from `from` to `to`.
    ///
    /// # Errors
    ///
    /// One [`Refusal`] per rule: no cluster, no wave, an empty wave, a repeated node, a node that is
    /// not a member, a member with no wave, a target that is not newer, or a target in another major.
    pub fn new(
        members: &[NodeId],
        waves: &[Vec<NodeId>],
        from: Protocol,
        to: Protocol,
    ) -> Result<Self, Refusal> {
        todo!()
    }

    /// How many nodes have to be up for the cluster to decide: a majority of the members.
    #[must_use]
    pub fn quorum(&self) -> usize {
        todo!()
    }

    /// The waves, in the operator's order.
    #[must_use]
    pub fn waves(&self) -> &[Vec<NodeId>] {
        &self.waves
    }

    /// What each wave runs, in the plan's order.
    ///
    /// Every wave but the last runs the new build in compatibility mode: it speaks the old minor
    /// while its peers are old. The last wave is native, which is the instant the cluster moves.
    /// One phase per wave, in the same order, so a deploy log can be read beside the plan.
    #[must_use]
    pub fn phases(&self) -> Vec<Phase> {
        todo!()
    }

    /// Walk the plan from the nodes in `up`, one wave at a time, and answer what ran.
    ///
    /// A node that is already down does not reduce the cluster twice: taking it down again leaves
    /// the same nodes up, and the wave brings it back. A wave is checked before its nodes are brought
    /// back, because that is the instant the cluster is thinnest.
    ///
    /// # Errors
    ///
    /// [`Refusal::Stranger`] for a node in `up` that is not a member,
    /// [`Refusal::StartsBelowMajority`] when the starting set cannot decide, and
    /// [`Refusal::BelowMajority`] at the first wave that would leave fewer than a majority up.
    pub fn run(&self, up: &[&str]) -> Result<Vec<Step>, Refusal> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(names: &[&str]) -> Vec<NodeId> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn previous() -> Protocol {
        Protocol::new(1, 1)
    }

    fn next() -> Protocol {
        Protocol::new(1, 2)
    }

    fn three() -> Vec<NodeId> {
        nodes(&["n1", "n2", "n3"])
    }

    #[test]
    fn a_plan_that_names_every_node_once_in_a_minor_upgrade_is_accepted() {
        let members = three();
        let plan = Plan::new(
            &members,
            &[nodes(&["n1"]), nodes(&["n2"]), nodes(&["n3"])],
            previous(),
            next(),
        )
        .expect("three waves cover three nodes");

        assert_eq!(
            plan.quorum(),
            2,
            "three nodes decide with two: a majority, as everywhere in this track"
        );
        assert_eq!(
            plan.waves().len(),
            3,
            "the operator's three waves are the plan's three steps"
        );
        assert_eq!(
            plan.phases(),
            vec![
                Phase::Compat {
                    speaking: previous(),
                    built: next(),
                },
                Phase::Compat {
                    speaking: previous(),
                    built: next(),
                },
                Phase::Native { version: next() },
            ],
            "the first two waves hold the new build back and the last is the move"
        );
    }

    #[test]
    fn a_plan_that_repeats_forgets_or_invents_a_node_is_not_a_plan() {
        let members = three();
        let two_waves = [nodes(&["n1", "n2"]), nodes(&["n2", "n3"])];
        let error =
            Plan::new(&members, &two_waves, previous(), next()).expect_err("n2 is in two waves");
        assert_eq!(
            error,
            Refusal::Repeated {
                node: "n2".to_owned(),
            },
            "the node that appears twice is the one the refusal names"
        );

        let error = Plan::new(
            &members,
            &[nodes(&["n1"]), nodes(&["n2"])],
            previous(),
            next(),
        )
        .expect_err("n3 has no wave");
        assert_eq!(
            error,
            Refusal::Missing {
                node: "n3".to_owned(),
            },
            "the first member without a wave is the one an operator fixes"
        );

        let error = Plan::new(&members, &[nodes(&["n1", "n2", "n4"])], previous(), next())
            .expect_err("n4 is not in this cluster");
        assert_eq!(
            error,
            Refusal::Stranger {
                node: "n4".to_owned(),
            },
            "a node the cluster does not have cannot be upgraded by its plan"
        );

        let error = Plan::new(
            &members,
            &[nodes(&["n1", "n2", "n3"]), Vec::new()],
            previous(),
            next(),
        )
        .expect_err("an empty wave");
        assert_eq!(
            error,
            Refusal::EmptyWave { wave: 1 },
            "the wave that upgrades nobody is a typo, and its position is named"
        );

        assert_eq!(
            Plan::new(&members, &[], previous(), next()).err(),
            Some(Refusal::NoWaves),
            "no wave is an upgrade of nobody"
        );
        assert_eq!(
            Plan::new(&[], &[nodes(&["n1"])], previous(), next()).err(),
            Some(Refusal::NoNodes),
            "no cluster is no plan"
        );
    }

    #[test]
    fn a_major_bump_and_a_downgrade_are_not_rolling_upgrades() {
        let members = three();
        let waves = [nodes(&["n1", "n2", "n3"])];

        let error = Plan::new(&members, &waves, Protocol::new(1, 9), Protocol::new(2, 0))
            .expect_err("two majors cannot be in one cluster");
        assert_eq!(
            error,
            Refusal::Major {
                from: Protocol::new(1, 9),
                to: Protocol::new(2, 0),
            },
            "a major crossing is refused as a major, not as an ordinary upgrade"
        );
        assert!(
            error.to_string().contains("major"),
            "the reason says which number crossed: {error}"
        );

        assert_eq!(
            Plan::new(&members, &waves, Protocol::new(1, 2), Protocol::new(1, 1)).err(),
            Some(Refusal::NotAnUpgrade {
                from: Protocol::new(1, 2),
                to: Protocol::new(1, 1),
            }),
            "a downgrade is not an upgrade"
        );
        assert_eq!(
            Plan::new(&members, &waves, previous(), previous()).err(),
            Some(Refusal::NotAnUpgrade {
                from: previous(),
                to: previous(),
            }),
            "and neither is a plan to the version the cluster already speaks"
        );

        assert!(
            Plan::new(&members, &waves, Protocol::new(1, 9), Protocol::new(1, 10)).is_ok(),
            "1.10 is a later minor than 1.9, so moving to it is a rolling upgrade"
        );
    }

    #[test]
    fn the_quorum_is_a_majority_whatever_the_cluster_size() {
        for (count, quorum) in [(1, 1), (2, 2), (3, 2), (4, 3), (5, 3), (6, 4), (7, 4)] {
            let members: Vec<NodeId> = (1..=count).map(|index| format!("n{index}")).collect();
            let plan = Plan::new(&members, &[members.clone()], previous(), next())
                .expect("one wave that names every member is a description");
            assert_eq!(
                plan.quorum(),
                quorum,
                "{count} nodes decide with {quorum}: half rounded up, and never half"
            );
        }
    }

    #[test]
    fn a_wave_that_would_leave_less_than_a_majority_is_refused() {
        let five = nodes(&["n1", "n2", "n3", "n4", "n5"]);
        let plan = Plan::new(
            &five,
            &[nodes(&["n1", "n2"]), nodes(&["n3", "n4"]), nodes(&["n5"])],
            previous(),
            next(),
        )
        .expect("two waves of two and one of one");

        let steps = plan
            .run(&["n1", "n2", "n3", "n4", "n5"])
            .expect("two of five down still leaves three, which is the majority");
        assert_eq!(
            steps.iter().map(|step| step.up_during).collect::<Vec<_>>(),
            vec![3, 3, 4],
            "no wave leaves fewer than the majority up: a wave of two leaves three, and the last \
             wave of one leaves four"
        );
        assert_eq!(
            steps.iter().map(|step| step.wave).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "the steps come back in the operator's wave order"
        );

        let bold = Plan::new(
            &five,
            &[nodes(&["n1", "n2", "n3"]), nodes(&["n4", "n5"])],
            previous(),
            next(),
        )
        .expect("a wave of three is still a description of a plan");
        let error = bold
            .run(&["n1", "n2", "n3", "n4", "n5"])
            .expect_err("three of five down leaves only two");
        assert_eq!(
            error,
            Refusal::BelowMajority {
                wave: 0,
                up: 2,
                need: 3,
            },
            "the first wave that breaks the majority is the one named, with both counts"
        );
        assert!(
            error.to_string().contains("wave 0") && error.to_string().contains("needs 3"),
            "the reason is readable without the code: {error}"
        );
    }

    #[test]
    fn a_two_node_cluster_cannot_be_rolled_one_node_at_a_time() {
        let two = nodes(&["n1", "n2"]);
        let plan = Plan::new(&two, &[nodes(&["n1"]), nodes(&["n2"])], previous(), next())
            .expect("two waves of one node are a description");
        assert_eq!(plan.quorum(), 2, "a majority of two is both nodes");
        assert_eq!(
            plan.run(&["n1", "n2"])
                .expect_err("one of two down is not a cluster"),
            Refusal::BelowMajority {
                wave: 0,
                up: 1,
                need: 2,
            },
            "with two nodes there is no wave that leaves a majority"
        );

        let one = nodes(&["n1"]);
        let single = Plan::new(&one, &[nodes(&["n1"])], previous(), next())
            .expect("one node is a cluster, if a lonely one");
        assert_eq!(
            single.run(&["n1"]).expect_err("the only node is down"),
            Refusal::BelowMajority {
                wave: 0,
                up: 0,
                need: 1,
            },
            "a one-node upgrade stops the only node there is"
        );

        let plan = Plan::new(
            &three(),
            &[nodes(&["n1"]), nodes(&["n2"]), nodes(&["n3"])],
            previous(),
            next(),
        )
        .expect("three waves of one");
        assert!(
            plan.run(&["n1", "n2", "n3"]).is_ok(),
            "three is the smallest cluster that can lose a node and still decide"
        );
    }

    #[test]
    fn every_wave_but_the_last_runs_the_new_build_in_compatibility_mode() {
        let four = nodes(&["n1", "n2", "n3", "n4"]);
        let plan = Plan::new(
            &four,
            &[nodes(&["n1", "n2"]), nodes(&["n3", "n4"])],
            previous(),
            next(),
        )
        .expect("two waves of two");

        assert_eq!(
            plan.phases(),
            vec![
                Phase::Compat {
                    speaking: previous(),
                    built: next(),
                },
                Phase::Native { version: next() },
            ],
            "the first wave speaks the old minor although it is the new build; the last is the move"
        );

        let plan = Plan::new(
            &three(),
            &[nodes(&["n1"]), nodes(&["n2"]), nodes(&["n3"])],
            previous(),
            next(),
        )
        .expect("three waves of one");
        let phases = plan.phases();
        assert_eq!(
            phases.len(),
            3,
            "there is one phase per wave, because a deploy log has one line per step"
        );
        assert_eq!(
            phases[0],
            Phase::Compat {
                speaking: previous(),
                built: next(),
            },
            "the first wave is not the move"
        );
        assert_eq!(
            phases[2],
            Phase::Native { version: next() },
            "only the last wave is native"
        );
    }

    #[test]
    fn the_same_plan_is_safe_from_one_starting_state_and_refused_from_another() {
        let plan = Plan::new(
            &three(),
            &[nodes(&["n1"]), nodes(&["n2"]), nodes(&["n3"])],
            previous(),
            next(),
        )
        .expect("three waves of one");

        let steps = plan
            .run(&["n2", "n3"])
            .expect("n1 is already down, so restarting it first costs no majority");
        assert_eq!(
            steps[0].up_during, 2,
            "taking an already-down node down again leaves the other two up"
        );
        assert_eq!(
            steps.len(),
            3,
            "and every wave still runs, because none of them leaves fewer than two up"
        );

        assert_eq!(
            plan.run(&["n3"])
                .expect_err("one of three is not a majority"),
            Refusal::StartsBelowMajority { up: 1, need: 2 },
            "a plan that starts below its majority is refused before the first wave"
        );

        let reordered = Plan::new(
            &three(),
            &[nodes(&["n2"]), nodes(&["n1"]), nodes(&["n3"])],
            previous(),
            next(),
        )
        .expect("the order is the operator's");
        assert_eq!(
            reordered
                .run(&["n2", "n3"])
                .expect_err("n2 is up, and taking it down leaves only n3"),
            Refusal::BelowMajority {
                wave: 0,
                up: 1,
                need: 2,
            },
            "the same cluster and the same starting state are safe or not depending on the wave \
             that goes first"
        );
    }
}
