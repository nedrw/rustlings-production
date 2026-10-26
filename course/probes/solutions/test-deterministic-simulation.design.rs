//! probe = "test/deterministic-simulation@design"
//! title = "The schedule is an input: a seed, a playbook, and a digest for the run"
//! hint = "Refuse a playbook whose faults cannot change anything, then run it: apply each fault where it stands, send a write to a node that can still reach a majority, and deliver the queued copies in the order the seed picks."
//!
//! Step `meshkv/5.13-deterministic-network` is where the cluster's *testing* changes shape. Up to
//! here a failure was something a harness did to a value; this step makes the failures themselves
//! data — a **playbook** of faults and writes — and then makes the run a function of `(seed,
//! playbook)`. Two properties fall out and both are the point: the same seed is the same run, byte for
//! byte, so a bug found by a seed is a bug somebody else can reproduce; and another seed is another
//! run, because one interleaving tests one interleaving.
//!
//! Four decisions, and each one is what separates a simulation from an anecdote:
//!
//! 1. **the failure is an input.** A scenario is lines of text — a cut link, a heal, a drop budget, a
//!    crash — and the scenario's *order* is the scenario: a crash after the last write is a different
//!    test from a crash before it;
//! 2. **a playbook that says nothing is refused.** `partition n1 n1`, a heal of a link that is not
//!    partitioned, a drop of zero, two crashes with no restart: each is a step that cannot change
//!    anything, and a scenario built from those proves nothing. The check runs before the run;
//! 3. **the seed decides the order, and nothing else does.** Which node an unnamed write reaches and
//!    the order a write's copies are delivered in both come out of the seed, so the same seed replays
//!    the run and a different seed is a different one. No clock, no path, no address reaches the
//!    transcript;
//! 4. **the digest identifies the transcript.** Every event is a name, a number or a verdict, folded
//!    into one hash as it is recorded, so "the same run" is one comparison — and a change to the
//!    simulator that moves the run moves the digest.
//!
//! ## What you write
//!
//! * [`Playbook::check`] — refuse the scenarios whose faults cannot change anything, before anything
//!   runs;
//! * [`run`] — the scheduler: the cluster's faults, one write per step, a majority to decide who can
//!   serve one, and a delivery queue whose order the seed picks.
//!
//! [`Fault`], [`Step`] and [`Playbook::parse`] are given, so the playbook is data rather than string
//! parsing; [`Event`], [`Transcript`], [`Rng`] and [`Refusal`] are given, so the transcript's grammar
//! and the seed's arithmetic are vocabulary rather than homework. That leaves the two things this
//! probe is actually about: which steps are meaningful, and which order events happen in.
//!
//! ## What the tests ask
//!
//! Every no-op scenario is refused and the tests' own scenario is not. The same seed, the same size
//! and the same playbook are one transcript — the same events and the same digest — and seeds 1 to 4
//! are not all the same run, because a harness that ignores the seed passes three times and proves
//! nothing. A node cut from both peers cannot reach a majority, so a write sent to it is refused on
//! every seed, and the same write after the heals commits. The majority is tested at its boundary
//! from both sides: one cut peer out of three still commits, two cut peers do not, and a one-node
//! cluster always does. Every peer of a committed write is handed exactly one message, delivered or
//! lost, and a drop budget loses exactly the deliveries it covers and no more. A crashed node refuses
//! writes until its restart, and the transcript says so in the scenario's order. A write that names a
//! node the cluster does not have, and a cluster of no nodes, are refused before the run.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no threads, no sockets and no sleeping**: time does not pass here, a
//! delivery is a `VecDeque::remove` and not a packet, and the "cluster" is a majority count rather
//! than a log. The step's harness is `turmoil` at the socket level with a virtual clock (`5.3`'s
//! instrument), and a real run applies real entries to the state machine of
//! `dist/deterministic-state-machine@apply`. What is re-checked is the shape that survives every
//! simulator: a scenario is data, a seed is the whole schedule, a digest is what "the same run" means,
//! and a no-op scenario is refused rather than trusted.
//!
//! Missing on purpose: there is no state machine and no consensus here, so nothing converges and no
//! write is ever followed by a second one that depends on it — a committed write is recorded and its
//! copies are delivered, and that is all. The faults are also coarser than `turmoil`'s: no latency, no
//! reordering, no message duplication, and a partition is between two nodes rather than a network
//! split. Each of those is a knob on the shape this file checks.

use std::collections::{BTreeSet, VecDeque};
use std::fmt;

/// Where a zero seed is nudged to, once: all three shifts of zero are zero.
const AWAY_FROM_ZERO: u64 = 0x9E37_79B9_7F4A_7C15;

/// The star: odd, and therefore invertible, so a nonzero state never answers zero.
const MULTIPLIER: u64 = 0x2545_F491_4F6C_DD1D;

/// The generator a run is seeded with: xorshift64\*.
///
/// Given, because it is the track's own arithmetic and this probe is about where the seed is *spent*
/// rather than about the arithmetic. What matters here is the property a run rests on: one seed in,
/// one sequence out, on every machine, for ever — which is why `rand` is the wrong crate for a
/// simulation even though it is a good one.
#[derive(Debug, Clone)]
pub struct Rng {
    /// The whole state.
    state: u64,
}

impl Rng {
    /// A generator seeded with `seed`. **Zero is a legal seed**: the one state xorshift cannot leave
    /// is nudged off zero once, here, where the correction is visible.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { AWAY_FROM_ZERO } else { seed },
        }
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        let mut state = self.state;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.state = state;
        state.wrapping_mul(MULTIPLIER)
    }

    /// A number in `0..bound`, and `0` when there is nothing to draw from.
    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        (self.next_u64() % bound as u64) as usize
    }
}

/// Why a scenario, a fault or a run was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// What was refused, phrased for the person who wrote the playbook.
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

/// A node number as the playbook writes it: `n` and the number, one-based.
///
/// # Errors
///
/// [`Refusal`] when the text is not `n` followed by a positive number.
fn parse_node(text: &str) -> Result<usize, Refusal> {
    let digits = text.strip_prefix('n').ok_or_else(|| {
        Refusal::new(format!(
            "`{text}` is not a node: a node is `n` and a number"
        ))
    })?;
    let number = digits.parse::<usize>().map_err(|_| {
        Refusal::new(format!(
            "`{text}` is not a node: a node is `n` and a number"
        ))
    })?;
    if number == 0 {
        return Err(Refusal::new(
            "`n0` is not a node: the nodes are `n1` to the cluster's size",
        ));
    }
    Ok(number)
}

/// Two node numbers in a fixed order, so `partition n2 n1` and `partition n1 n2` are one link.
fn link(a: usize, b: usize) -> (usize, usize) {
    if a <= b { (a, b) } else { (b, a) }
}

/// One fault a playbook can inject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// Nothing gets between the two nodes, in either direction.
    Partition {
        /// One end.
        a: usize,
        /// The other end.
        b: usize,
    },
    /// The link between the two works again.
    Heal {
        /// One end.
        a: usize,
        /// The other end.
        b: usize,
    },
    /// The next `count` deliveries are lost, in delivery order.
    Drop {
        /// How many to lose.
        count: usize,
    },
    /// A node stops answering until its restart.
    Crash {
        /// Which one.
        node: usize,
    },
    /// A node answers again.
    Restart {
        /// Which one.
        node: usize,
    },
}

impl Fault {
    /// Parse one line: `partition <a> <b>`, `heal <a> <b>`, `drop <n>`, `crash <a>`, `restart <a>`.
    ///
    /// # Errors
    ///
    /// [`Refusal`] when the line is not one of the five, when it has a missing or extra word, or when
    /// a node is not `n` and a positive number.
    pub fn parse(line: &str) -> Result<Self, Refusal> {
        let mut words = line.split_whitespace();
        let wrong = || Refusal::new(format!("`{line}` is not a fault this playbook knows"));
        let fault = match words.next() {
            Some("partition") => Self::Partition {
                a: parse_node(words.next().ok_or_else(wrong)?)?,
                b: parse_node(words.next().ok_or_else(wrong)?)?,
            },
            Some("heal") => Self::Heal {
                a: parse_node(words.next().ok_or_else(wrong)?)?,
                b: parse_node(words.next().ok_or_else(wrong)?)?,
            },
            Some("drop") => {
                let count = words.next().ok_or_else(wrong)?;
                let count = count.parse::<usize>().map_err(|_| {
                    Refusal::new(format!("`{count}` is not a number of deliveries"))
                })?;
                Self::Drop { count }
            }
            Some("crash") => Self::Crash {
                node: parse_node(words.next().ok_or_else(wrong)?)?,
            },
            Some("restart") => Self::Restart {
                node: parse_node(words.next().ok_or_else(wrong)?)?,
            },
            _ => return Err(wrong()),
        };
        if words.next().is_some() {
            return Err(Refusal::new(format!("`{line}` has a word too many")));
        }
        Ok(fault)
    }

    /// The line this fault is written as.
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Self::Partition { a, b } => format!("partition n{a} n{b}"),
            Self::Heal { a, b } => format!("heal n{a} n{b}"),
            Self::Drop { count } => format!("drop {count}"),
            Self::Crash { node } => format!("crash n{node}"),
            Self::Restart { node } => format!("restart n{node}"),
        }
    }
}

/// One step of a scenario: a fault, or a write the client attempts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Inject this fault where the scenario put it.
    Fault(Fault),
    /// Attempt this write, at the node it names or at one the seed draws.
    Write {
        /// The key.
        key: String,
        /// The value, as text: the wire form is not this probe's subject.
        value: String,
        /// The node the scenario sends it to, when it names one.
        at: Option<usize>,
    },
}

/// A scenario: the steps in order, and nothing else.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Playbook {
    steps: Vec<Step>,
}

impl Playbook {
    /// Read a scenario: one step per line, `#` starts a comment, blank lines are ignored.
    ///
    /// A line is a fault (`partition n1 n2`, `heal n1 n2`, `drop 2`, `crash n1`, `restart n1`) or a
    /// write (`write <key> <value>`, `write@<node> <key> <value>`).
    ///
    /// # Errors
    ///
    /// [`Refusal`] for a line that is neither.
    pub fn parse(text: &str) -> Result<Self, Refusal> {
        let mut steps = Vec::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix("write@") {
                let Some((at, rest)) = rest.split_once(' ') else {
                    return Err(Refusal::new(format!(
                        "`{line}` names no node: expected `write@n1 key value`"
                    )));
                };
                let Some((key, value)) = rest.split_once(' ') else {
                    return Err(Refusal::new(format!(
                        "`{line}` is not a write: expected `write@n1 key value`"
                    )));
                };
                steps.push(Step::Write {
                    key: key.to_owned(),
                    value: value.to_owned(),
                    at: Some(parse_node(at)?),
                });
                continue;
            }
            if let Some(rest) = line.strip_prefix("write ") {
                let Some((key, value)) = rest.split_once(' ') else {
                    return Err(Refusal::new(format!(
                        "`{line}` is not a write: expected `write key value`"
                    )));
                };
                steps.push(Step::Write {
                    key: key.to_owned(),
                    value: value.to_owned(),
                    at: None,
                });
                continue;
            }
            steps.push(Step::Fault(Fault::parse(line)?));
        }
        Ok(Self { steps })
    }

    /// The steps, in order: the scenario's order is the scenario.
    #[must_use]
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// Whether the scenario says anything: a fault that cannot change anything is a step whose author
    /// believed it did.
    ///
    /// # Errors
    ///
    /// [`Refusal`] when the playbook has no steps at all, or when one of its faults is a no-op: a
    /// node partitioned from itself, a pair partitioned twice, a heal of a link that is not
    /// partitioned, a drop of zero, a crash of a crashed node, or a restart of a node that is up.
    pub fn check(&self) -> Result<(), Refusal> {
        if self.steps.is_empty() {
            return Err(Refusal::new("a playbook with no steps says nothing"));
        }
        let mut partitioned: BTreeSet<(usize, usize)> = BTreeSet::new();
        let mut crashed: BTreeSet<usize> = BTreeSet::new();
        for step in &self.steps {
            let Step::Fault(fault) = step else {
                continue;
            };
            match fault {
                Fault::Partition { a, b } => {
                    if a == b {
                        return Err(Refusal::new(format!(
                            "`n{a}` cannot be partitioned from itself"
                        )));
                    }
                    if !partitioned.insert(link(*a, *b)) {
                        return Err(Refusal::new(format!(
                            "`n{a}` and `n{b}` are partitioned twice, and the second one cannot \
                             change anything"
                        )));
                    }
                }
                Fault::Heal { a, b } => {
                    if !partitioned.remove(&link(*a, *b)) {
                        return Err(Refusal::new(format!(
                            "`n{a}` and `n{b}` are not partitioned, so there is nothing to heal"
                        )));
                    }
                }
                Fault::Drop { count } => {
                    if *count == 0 {
                        return Err(Refusal::new(
                            "a drop of zero deliveries is a fault that changes nothing",
                        ));
                    }
                }
                Fault::Crash { node } => {
                    if !crashed.insert(*node) {
                        return Err(Refusal::new(format!(
                            "`n{node}` is already crashed, so a second crash changes nothing"
                        )));
                    }
                }
                Fault::Restart { node } => {
                    if !crashed.remove(node) {
                        return Err(Refusal::new(format!(
                            "`n{node}` is not crashed, so there is nothing to restart"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

/// One thing that happened, as the transcript records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The size of the cluster, before anything happened.
    Members {
        /// How many nodes the run was built with.
        size: usize,
    },
    /// A fault, where the scenario put it.
    Fault(Fault),
    /// A client write reached a node that could serve it.
    Committed {
        /// The node it reached.
        node: usize,
        /// The key it wrote.
        key: String,
        /// The value it wrote.
        value: String,
    },
    /// A client write reached a node that could not serve it: crashed, or cut from a majority.
    Refused {
        /// The node it reached.
        node: usize,
        /// The key it tried to write.
        key: String,
    },
    /// A copy of a write was handed to a peer, in this order.
    Delivered {
        /// Who took the write.
        from: usize,
        /// Who it was broadcast to.
        to: usize,
        /// The key it carries.
        key: String,
    },
    /// A copy of a write never arrived: the link was cut, or the drop budget took it.
    Lost {
        /// Who sent it.
        from: usize,
        /// Who never got it.
        to: usize,
        /// The key it carries.
        key: String,
    },
}

impl Event {
    /// The line this event is written as: names, numbers and verdicts, never a clock or a path.
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Self::Members { size } => format!("members={size}"),
            Self::Fault(fault) => format!("fault {}", fault.render()),
            Self::Committed { node, key, value } => {
                format!("committed node={node} key={key} value={value}")
            }
            Self::Refused { node, key } => format!("refused node={node} key={key}"),
            Self::Delivered { from, to, key } => format!("delivered from={from} to={to} key={key}"),
            Self::Lost { from, to, key } => format!("lost from={from} to={to} key={key}"),
        }
    }
}

/// What a run happened: the events in order, and the one number two runs are compared by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transcript {
    events: Vec<Event>,
    digest: u64,
}

impl Transcript {
    /// A transcript that opens with the size of the cluster it ran on, so two sizes are two runs.
    #[must_use]
    pub fn opening(size: usize) -> Self {
        let mut transcript = Self::default();
        transcript.record(Event::Members { size });
        transcript
    }

    /// Record one event, folding it into the digest as it goes.
    ///
    /// FNV-1a 64, over the rendered event and a newline: the same constant `release/artifacts@apply`
    /// uses for the same reason, and enough here because the digest's job is to make two runs
    /// comparable rather than to resist an attacker.
    pub fn record(&mut self, event: Event) {
        let line = event.render();
        for byte in line.as_bytes().iter().chain(b"\n") {
            self.digest ^= u64::from(*byte);
            self.digest = self.digest.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.events.push(event);
    }

    /// The events, in order.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// The digest of the events.
    #[must_use]
    pub fn digest(&self) -> u64 {
        self.digest
    }

    /// `true` when two runs are the same run: one digest, one event list.
    #[must_use]
    pub fn same_as(&self, other: &Transcript) -> bool {
        self.digest == other.digest && self.events == other.events
    }
}

/// Run `playbook` on a cluster of `size` nodes, and answer what happened.
///
/// Node numbers are `1..=size`, the ones the playbook writes. The run is a function of `(seed, size,
/// playbook)` and of nothing else:
///
/// * it opens with [`Transcript::opening`], and a fault is recorded where it stands and takes effect
///   for the steps after it;
/// * a write goes to the node it names, or to the node the seed draws when it names none. It is
///   committed only if that node can still reach a majority — itself plus every peer that is neither
///   crashed nor partitioned from it — and otherwise it is refused and no message is sent;
/// * a committed write is broadcast: one message per peer, lost at once when the peer is crashed or
///   the link is partitioned, and queued otherwise. The queue is drained before the next step, each
///   delivery drawn out of it by the seed, so the order a write's copies arrive in is the seed's
///   decision rather than the kind's;
/// * while the drop budget is positive, a delivery spends one unit of it and is lost instead.
///
/// # Errors
///
/// [`Refusal`] when the cluster has no nodes, when the playbook does not make sense
/// ([`Playbook::check`]), or when a write names a node the cluster does not have.
pub fn run(seed: u64, size: usize, playbook: &Playbook) -> Result<Transcript, Refusal> {
    if size == 0 {
        return Err(Refusal::new("a cluster of no nodes cannot run a playbook"));
    }
    playbook.check()?;
    for step in playbook.steps() {
        if let Step::Write { at: Some(node), .. } = step {
            if *node > size {
                return Err(Refusal::new(format!(
                    "`n{node}` is not one of the {size} nodes this run has"
                )));
            }
        }
    }

    let majority = size / 2 + 1;
    let mut transcript = Transcript::opening(size);
    let mut partitioned: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut crashed: BTreeSet<usize> = BTreeSet::new();
    let mut budget = 0usize;
    let mut rng = Rng::new(seed);

    for step in playbook.steps() {
        match step {
            Step::Fault(fault) => {
                match fault {
                    Fault::Partition { a, b } => {
                        partitioned.insert(link(*a, *b));
                    }
                    Fault::Heal { a, b } => {
                        partitioned.remove(&link(*a, *b));
                    }
                    Fault::Drop { count } => budget += count,
                    Fault::Crash { node } => {
                        crashed.insert(*node);
                    }
                    Fault::Restart { node } => {
                        crashed.remove(node);
                    }
                }
                transcript.record(Event::Fault(fault.clone()));
            }
            Step::Write { key, value, at } => {
                // The node the write reaches: the one the scenario names, or the seed's draw. Which
                // node an unnamed write lands on is part of the schedule, so it comes from the seed
                // like the delivery order does.
                let target = match at {
                    Some(node) => *node,
                    None => 1 + rng.below(size),
                };
                let reachable = if crashed.contains(&target) {
                    0
                } else {
                    1 + (1..=size)
                        .filter(|peer| {
                            *peer != target
                                && !crashed.contains(peer)
                                && !partitioned.contains(&link(target, *peer))
                        })
                        .count()
                };
                if reachable < majority {
                    transcript.record(Event::Refused {
                        node: target,
                        key: key.clone(),
                    });
                    continue;
                }

                transcript.record(Event::Committed {
                    node: target,
                    key: key.clone(),
                    value: value.clone(),
                });
                let mut queue: VecDeque<(usize, usize, String)> = VecDeque::new();
                for peer in 1..=size {
                    if peer == target {
                        continue;
                    }
                    if crashed.contains(&peer) || partitioned.contains(&link(target, peer)) {
                        transcript.record(Event::Lost {
                            from: target,
                            to: peer,
                            key: key.clone(),
                        });
                    } else {
                        queue.push_back((target, peer, key.clone()));
                    }
                }

                // The queue is the network, and the seed is its scheduler: each delivery is drawn out
                // of what is still in flight.
                while !queue.is_empty() {
                    let index = rng.below(queue.len());
                    let (from, to, key) = queue
                        .remove(index)
                        .expect("the draw is inside the queue's length");
                    if budget > 0 {
                        budget -= 1;
                        transcript.record(Event::Lost { from, to, key });
                    } else {
                        transcript.record(Event::Delivered { from, to, key });
                    }
                }
            }
        }
    }
    Ok(transcript)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many nodes the tests' runs are built with.
    const SIZE: usize = 3;

    /// The scenario the tests run: a node cut from both peers, a write refused at it, the heals, a
    /// write that commits, and two writes the seed places.
    fn scenario() -> Playbook {
        Playbook::parse(
            "\
# a node cut from both peers cannot serve a write; after the heals it can
partition n2 n1
partition n2 n3
write@n2 a 1
heal n2 n1
heal n2 n3
write@n2 b 2
write c 3
write d 4
",
        )
        .expect("the tests' scenario parses")
    }

    /// `true` when `node` committed a write of `key`.
    fn committed(transcript: &Transcript, node: usize, key: &str) -> bool {
        transcript.events().iter().any(|event| {
            matches!(event, Event::Committed { node: n, key: k, .. } if *n == node && k == key)
        })
    }

    /// `true` when `node` refused a write of `key`.
    fn refused(transcript: &Transcript, node: usize, key: &str) -> bool {
        transcript.events().iter().any(
            |event| matches!(event, Event::Refused { node: n, key: k } if *n == node && k == key),
        )
    }

    /// How many messages of `key` ended up in this column.
    fn count(transcript: &Transcript, key: &str, delivered: bool) -> usize {
        transcript
            .events()
            .iter()
            .filter(|event| match event {
                Event::Delivered { key: k, .. } => delivered && k == key,
                Event::Lost { key: k, .. } => !delivered && k == key,
                _ => false,
            })
            .count()
    }

    #[test]
    fn a_playbook_that_cannot_change_anything_is_refused() {
        for text in [
            "",
            "heal n2 n1",
            "partition n1 n2\npartition n2 n1",
            "partition n1 n1",
            "drop 0",
            "crash n1\ncrash n1",
            "restart n1",
            "partition n1 n2\npartition n1 n2",
        ] {
            let playbook = Playbook::parse(text).expect("the text parses");
            assert!(
                playbook.check().is_err(),
                "`{text}` is a scenario whose steps cannot change anything, and a scenario built \
                 from steps that cannot change anything proves nothing"
            );
        }
        scenario()
            .check()
            .expect("the tests' scenario is meaningful: every fault in it changes what can commit");
    }

    #[test]
    fn the_same_seed_is_the_same_run_and_another_seed_is_another() {
        let playbook = scenario();
        let first = run(1, SIZE, &playbook).expect("the scenario runs");
        let again = run(1, SIZE, &playbook).expect("the same seed runs again");
        assert_eq!(
            first.events(),
            again.events(),
            "the seed, the size and the playbook are the whole input: two runs of seed 1 differ in \
             their events, so something else reached the schedule"
        );
        assert!(
            first.same_as(&again),
            "and two runs with the same events are one run: the digest has to agree too"
        );
        assert_eq!(first.digest(), again.digest());

        let digests: Vec<u64> = (1..=4)
            .map(|seed| {
                run(seed, SIZE, &playbook)
                    .expect("every seed runs the same scenario")
                    .digest()
            })
            .collect();
        let distinct: BTreeSet<u64> = digests.iter().copied().collect();
        assert!(
            distinct.len() >= 2,
            "the seed has to reach the schedule: seeds 1 to 4 produced {digests:?}, which are all \
             one run, and three identical transcripts prove nothing"
        );
    }

    #[test]
    fn an_isolated_node_refuses_writes_and_healing_brings_them_back() {
        for seed in 1..=4 {
            let transcript = run(seed, SIZE, &scenario()).expect("the scenario runs");
            assert!(
                refused(&transcript, 2, "a"),
                "a node cut from both peers reaches nobody but itself, and one of three is not a \
                 majority, so the write sent to it is refused under every seed"
            );
            assert!(
                committed(&transcript, 2, "b"),
                "after the heals the same node reaches a majority, and the write it is sent \
                 commits"
            );
            assert_eq!(
                count(&transcript, "a", true),
                0,
                "a refused write sent no copies: the peers were never told about `a`"
            );
        }
    }

    #[test]
    fn the_majority_is_what_decides_and_the_boundary_is_exact() {
        // Three nodes, one cut peer: two of three is still a majority.
        let one_cut = Playbook::parse("partition n2 n1\nwrite@n2 a 1").expect("it parses");
        let transcript = run(1, SIZE, &one_cut).expect("it runs");
        assert!(
            committed(&transcript, 2, "a"),
            "a node cut from one peer still reaches two of three, which is a majority: the write \
             it is sent commits"
        );
        assert_eq!(
            count(&transcript, "a", false),
            1,
            "and exactly the cut link's copy is lost, not both"
        );

        // The same cluster with the second cut is the scenario's first write, refused above.
        let both =
            Playbook::parse("partition n2 n1\npartition n2 n3\nwrite@n2 a 1").expect("it parses");
        assert!(
            refused(&run(1, SIZE, &both).expect("it runs"), 2, "a"),
            "cut from both peers it reaches one of three, which is not a majority: the same write \
             is now refused"
        );

        // One node is a majority of one, with no peers to ask.
        let alone = Playbook::parse("write@n1 a 1").expect("it parses");
        let transcript = run(1, 1, &alone).expect("a one-node cluster runs");
        assert!(
            committed(&transcript, 1, "a"),
            "the only node of a one-node cluster is a majority of one, so its write commits"
        );
        assert_eq!(
            count(&transcript, "a", true) + count(&transcript, "a", false),
            0,
            "and a cluster of one has no peer to broadcast to"
        );
    }

    #[test]
    fn every_peer_of_a_committed_write_is_handed_exactly_one_message() {
        let plain = Playbook::parse("write a 1\nwrite b 2").expect("it parses");
        let transcript = run(1, SIZE, &plain).expect("it runs");
        for key in ["a", "b"] {
            assert_eq!(
                count(&transcript, key, true),
                SIZE - 1,
                "every peer of the node that committed `{key}` was handed one copy: a broadcast to \
                 `{SIZE} - 1` peers is `{SIZE} - 1` messages"
            );
            assert_eq!(
                count(&transcript, key, false),
                0,
                "no link was cut and no drop was ordered, so no copy of `{key}` was lost"
            );
        }

        let cut = Playbook::parse("partition n1 n2\nwrite@n1 a 1").expect("it parses");
        let transcript = run(1, SIZE, &cut).expect("it runs");
        assert_eq!(
            count(&transcript, "a", true),
            SIZE - 2,
            "with one cut link the copies that reach a peer are the peers that are not cut"
        );
        assert_eq!(
            count(&transcript, "a", false),
            1,
            "and the cut peer's copy is in the other column: nothing was delivered to nobody"
        );
    }

    #[test]
    fn a_drop_budget_loses_exactly_the_deliveries_it_covers() {
        // Two peers, two writes: four copies. A drop of two takes the first two, in delivery order,
        // and stops — which with three nodes means both copies of the first write and neither of the
        // second.
        let playbook = Playbook::parse("drop 2\nwrite@n1 a 1\nwrite@n1 b 2").expect("it parses");
        let transcript = run(1, SIZE, &playbook).expect("it runs");
        assert!(
            committed(&transcript, 1, "a") && committed(&transcript, 1, "b"),
            "the drop budget is about messages and not about writes: both writes reached a node \
             that could serve them"
        );
        assert_eq!(
            count(&transcript, "a", false),
            2,
            "the budget covered both copies of `a`: one message is not two and two is not three"
        );
        assert_eq!(
            count(&transcript, "a", true),
            0,
            "and so none of `a` arrived"
        );
        assert_eq!(
            count(&transcript, "b", false),
            0,
            "the budget was spent by then, so no copy of `b` was taken"
        );
        assert_eq!(
            count(&transcript, "b", true),
            2,
            "and both copies of `b` arrived"
        );
    }

    #[test]
    fn a_crashed_node_refuses_writes_until_it_restarts_and_the_order_is_the_scenario() {
        let playbook =
            Playbook::parse("crash n1\nwrite@n1 a 1\nrestart n1\nwrite@n1 b 2").expect("it parses");
        let transcript = run(1, SIZE, &playbook).expect("it runs");
        assert!(
            refused(&transcript, 1, "a"),
            "a crashed node cannot answer, so the write sent to it is refused rather than queued \
             for after the restart"
        );
        assert!(
            committed(&transcript, 1, "b"),
            "the restart is a step in the timeline: after it the same node commits"
        );

        let refused_at = transcript
            .events()
            .iter()
            .position(|event| matches!(event, Event::Refused { key, .. } if key == "a"))
            .expect("the refusal is in the transcript");
        let committed_at = transcript
            .events()
            .iter()
            .position(|event| matches!(event, Event::Committed { key, .. } if key == "b"))
            .expect("the commit is in the transcript");
        assert!(
            refused_at < committed_at,
            "the playbook's order is the run's order: faults are between the writes, not around \
             them"
        );
    }

    #[test]
    fn a_run_that_cannot_be_what_it_says_is_refused_before_it_starts() {
        let unknown = Playbook::parse("write@n7 a 1").expect("the text parses");
        assert!(
            run(1, SIZE, &unknown).is_err(),
            "`n7` is not a node of a three-node cluster: running the scenario would record a write \
             nobody can read"
        );
        assert!(
            run(1, 0, &scenario()).is_err(),
            "a cluster of no nodes cannot reach a majority of anything, and it is refused rather \
             than run to an empty transcript"
        );
        let empty = Playbook::parse("# nothing\n").expect("it parses");
        assert!(
            run(1, SIZE, &empty).is_err(),
            "a scenario with no steps is the no-op the check exists to refuse, and the run asks the \
             check first"
        );
    }
}
