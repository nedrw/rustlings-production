//! probe = "dist/membership@apply"
//! title = "A change in flight is decided by both sets, and the entry that carries it is a log entry"
//! hint = "Require a majority of the old set and of the new one while joint, count only the targeted voters after the change commits, and refuse a second change while one is in flight."
//!
//! Step `meshkv/5.5-membership` is the window in which the cluster is two configurations at once.
//! This probe is that window as arithmetic: two voter sets, the majorities they require, and the
//! configuration entry that ends the change.
//!
//! Five rules, and every one of them has an outage behind it:
//!
//! 1. **a single switch has a window.** If the configuration could change in one step, the old and
//!    the new voters could each believe they are the cluster, and two non-overlapping majorities
//!    could decide two different things at one index. Joint consensus closes it by requiring a
//!    majority of the old set *and* of the new one while the change is in flight;
//! 2. **the configuration is a log entry.** It takes the next position like a write, so a change
//!    cannot overtake a write it must not lose. An entry that merely names the new voters is not a
//!    committed change — and an uncommitted one changes nothing;
//! 3. **the change ends when that entry commits.** While it is in flight every voter of either set
//!    is a peer, because the joining nodes must receive the entry they will vote on and the
//!    leaving ones must keep receiving it until it commits;
//! 4. **one change at a time.** Two configuration entries in flight are two windows, and the second
//!    can be decided by a set the first has not agreed on;
//! 5. **a node that left stops counting.** After the change commits, only the new voters decide; an
//!    acknowledgement from the departed node is not a vote, or a cluster could wait for ever on a
//!    machine that is powered off.
//!
//! ## What you write
//!
//! * [`Config::parse`] — `n1,n2,n3`, refusing an empty list, a nameless entry and a name twice;
//! * [`Membership::is_met`] — one majority when stable, both while joint, counting only named
//!   voters;
//! * [`Membership::voters`] — the union of both sets while joint, once each, targeted set first;
//! * [`Leader::begin`] — leader-only by construction here, appending the configuration entry and
//!   putting the cluster into the joint state, refusing a second change;
//! * [`Leader::on_ack`] — end the change only when both sets acknowledge it, and adopt the new
//!   configuration.
//!
//! [`Config`]'s accessors, [`Membership`]'s variants and `targeted`, the [`Entry`] type and the
//! [`Leader`]'s log are given, because they are the vocabulary of the protocol: a membership is
//! either stable or joint, and a configuration is a list of names, not a set of addresses.
//! [`encode`] and [`decode`] are given too: the marker that tells a configuration entry from a
//! write is `5.5`'s interface detail, and the learner's job is the decision, not the byte format.
//!
//! ## What the tests ask
//!
//! That a one-voter configuration has a majority of one and a duplicate voter is refused by name;
//! that a stable membership needs more than half — two of three decide, two of four do not; that a
//! joint membership refuses an old majority alone and a new majority alone, and decides on a
//! majority of both; that the voter union includes the joining and the leaving node exactly once;
//! that a second change is refused while one is in flight and possible after it commits; that the
//! change stays in flight until both sets acknowledge it; that the configuration takes a position
//! between the writes around it and decodes back to the target; and that a node that left appears
//! in no vote afterwards.
//!
//! ## Where this is honest, and where it is not
//!
//! There is no cluster here: no network, no follower, no `on_append`, no election, and no
//! per-index acknowledgement. [`Leader::on_ack`] is given the set of voters that hold the
//! configuration entry, not a set of match indices, so the probe re-checks the quorum arithmetic
//! and the order of the two phases rather than the replication that carries them. The state
//! machine that replicates the entry is `dist/consensus@apply`'s, and the names in a configuration
//! are the identities `dist/topology@apply` hands out. The habit is still worth re-drilling because
//! the quorum rule is where a membership change goes silently wrong: every node looks healthy, and
//! a write decided by the departing half is overwritten by the arriving one.

use std::fmt;

/// A position in the log: one-based, never reused.
pub type Index = u64;

/// Why a configuration or a change was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    fn invalid(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// The alphabet `meshkv/5.1` gave a node name, reused so a configuration accepts nothing a
/// topology would refuse.
fn check_name(text: &str) -> Result<(), Error> {
    if text.is_empty() {
        return Err(Error::invalid(
            "a configuration entry is at least one character — an empty name is a voter nobody can count",
        ));
    }
    if !text
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(Error::invalid(format!(
            "`{text}` is not a node name: names use ASCII letters, digits, `-` and `_`"
        )));
    }
    Ok(())
}

/// The voters of one configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    voters: Vec<String>,
}

impl Config {
    /// Parse `n1,n2,n3`.
    ///
    /// # Errors
    ///
    /// [`Error`] when the text is empty, when an entry is not a node name, or when a name appears
    /// twice — one node cannot be two voters.
    pub fn parse(text: &str) -> Result<Self, Error> {
        todo!()
    }

    /// The voters, in the order they were named.
    #[must_use]
    pub fn voters(&self) -> &[String] {
        &self.voters
    }

    /// How many voters it has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.voters.len()
    }

    /// `true` when it names no voter. Never true for a parsed one.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.voters.is_empty()
    }

    /// `true` when `id` is one of the voters.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.voters.iter().any(|voter| voter == id)
    }

    /// How many acknowledgements it takes to decide: more than half.
    #[must_use]
    pub fn majority(&self) -> usize {
        self.len() / 2 + 1
    }

    /// `true` when `acknowledgements` of these voters are enough.
    #[must_use]
    pub fn is_met(&self, acknowledgements: usize) -> bool {
        acknowledgements >= self.majority()
    }

    /// The canonical text, `n1,n2,n3`.
    #[must_use]
    pub fn render(&self) -> String {
        self.voters.join(",")
    }
}

/// The voters of a cluster, and whether a change is in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Membership {
    /// One configuration decides everything.
    Stable(Config),
    /// A change is in flight: a decision needs a majority of `old` **and** of `new`.
    Joint {
        /// The configuration the cluster is leaving.
        old: Config,
        /// The one it is moving to.
        new: Config,
    },
}

impl Membership {
    /// The configuration a stable membership is, or the new one a joint membership is moving to.
    #[must_use]
    pub fn targeted(&self) -> &Config {
        match self {
            Self::Stable(config) => config,
            Self::Joint { new, .. } => new,
        }
    }

    /// `true` while a change is in flight.
    #[must_use]
    pub fn is_joint(&self) -> bool {
        matches!(self, Self::Joint { .. })
    }

    /// `true` when the acknowledgements decide: a majority of the stable configuration, or a
    /// majority of **both** sets while a change is in flight.
    ///
    /// A name that is not a voter of the set being counted does not count toward that set: the
    /// quorum is of voters, not of messages.
    #[must_use]
    pub fn is_met(&self, acknowledged: &[&str]) -> bool {
        todo!()
    }

    /// Every voter of either configuration, once each.
    ///
    /// The targeted set comes first, so a node handed this list to open connections to starts with
    /// the cluster it is moving to.
    #[must_use]
    pub fn voters(&self) -> Vec<&str> {
        todo!()
    }
}

/// One entry of the leader's log: a write, or a configuration, as opaque bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Its position, one-based.
    pub index: Index,
    /// What it carries.
    pub payload: Vec<u8>,
}

/// A leader and the one configuration change it may have in flight.
#[derive(Debug, Clone)]
pub struct Leader {
    membership: Membership,
    pending: Option<Index>,
    entries: Vec<Entry>,
}

impl Leader {
    /// A leader whose cluster is on `config` and whose log is empty.
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            membership: Membership::Stable(config),
            pending: None,
            entries: Vec::new(),
        }
    }

    /// The configuration this leader is on, and whether a change is in flight.
    #[must_use]
    pub fn membership(&self) -> &Membership {
        &self.membership
    }

    /// Every peer of either set, itself included: the list a leader replicates to.
    #[must_use]
    pub fn voters(&self) -> Vec<&str> {
        self.membership.voters()
    }

    /// Append a write at the next position, and answer the position it took.
    pub fn propose(&mut self, payload: Vec<u8>) -> Index {
        let index = self.entries.len() as Index + 1;
        self.entries.push(Entry { index, payload });
        index
    }

    /// The entry at `index`, when the log holds it.
    #[must_use]
    pub fn entry(&self, index: Index) -> Option<&Entry> {
        let position = usize::try_from(index.checked_sub(1)?).ok()?;
        self.entries.get(position)
    }

    /// Begin a change to `target`: append the configuration entry and enter the joint state.
    ///
    /// # Errors
    ///
    /// [`Error`] when another change is already in flight — the second window has not agreed with
    /// the first, so it must wait until the first one commits.
    pub fn begin(&mut self, target: Config) -> Result<Index, Error> {
        todo!()
    }

    /// Record that the voters in `acknowledged` hold the configuration entry in flight.
    ///
    /// The change ends only when both sets are satisfied; then the new configuration is adopted
    /// and the leader is stable again. An acknowledgement while no change is pending, or one that
    /// does not satisfy both sets, finishes nothing and answers `false`.
    pub fn on_ack(&mut self, acknowledged: &[&str]) -> bool {
        todo!()
    }
}

/// The marker a configuration entry carries, so the log can tell it from a write.
const CONFIG_MARKER: &[u8] = b"\x01cfg ";

/// The payload of a configuration entry.
#[must_use]
pub fn encode(config: &Config) -> Vec<u8> {
    let mut payload = CONFIG_MARKER.to_vec();
    payload.extend_from_slice(config.render().as_bytes());
    payload
}

/// The configuration a payload carries, if it is a configuration entry.
#[must_use]
pub fn decode(payload: &[u8]) -> Option<Config> {
    let text = payload.strip_prefix(CONFIG_MARKER)?;
    let text = std::str::from_utf8(text).ok()?;
    Config::parse(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn config(text: &str) -> Config {
        Config::parse(text).expect("a valid configuration")
    }

    #[test]
    fn a_configuration_parses_and_refuses_an_empty_or_duplicated_list() {
        let three = config("n1,n2,n3");
        assert_eq!(
            three.render(),
            "n1,n2,n3",
            "the canonical text is the order the voters were named in"
        );
        assert_eq!(three.len(), 3);
        assert_eq!(three.majority(), 2, "a majority of three is two");

        let one = config("n1");
        assert_eq!(
            one.majority(),
            1,
            "a single voter is a majority of one: a one-node cluster decides by itself"
        );

        let duplicate = Config::parse("n1,n1").expect_err("a name cannot be two voters");
        let message = duplicate.to_string();
        assert!(
            message.contains("n1") && message.contains("twice"),
            "a duplicated voter is refused by name, because one node cannot cast two votes: \
             {message}"
        );

        for bad in ["", "n1,,n2", "n1,", "n1, n 2", "n1,n/2"] {
            let error = Config::parse(bad).expect_err(&format!("`{bad}` is not a configuration"));
            let message = error.to_string();
            assert!(
                message.contains("name") || message.contains("voter"),
                "the refusal of `{bad}` says what a configuration is made of: {message}"
            );
        }
    }

    #[test]
    fn a_stable_membership_decides_with_a_majority_of_its_voters() {
        let three = Membership::Stable(config("n1,n2,n3"));
        assert!(!three.is_met(&["n1"]), "one of three is not a majority");
        assert!(three.is_met(&["n1", "n2"]), "two of three decide");
        assert!(
            !three.is_met(&["n1", "n9"]),
            "a name that is not a voter does not count toward the majority"
        );

        let four = Membership::Stable(config("n1,n2,n3,n4"));
        assert!(
            !four.is_met(&["n1", "n2"]),
            "two of four is not a majority: more than half of four is three"
        );
        assert!(four.is_met(&["n1", "n2", "n3"]));
    }

    #[test]
    fn a_joint_membership_needs_a_majority_of_both_sets() {
        let joint = Membership::Joint {
            old: config("n1,n2,n3"),
            new: config("n1,n2,n3,n4,n5"),
        };
        assert!(
            !joint.is_met(&["n1", "n2"]),
            "a majority of the old set alone is not a decision: the arriving cluster has seen \
             nothing"
        );
        assert!(
            !joint.is_met(&["n1", "n4", "n5"]),
            "a majority of the new set alone is not a decision either: the departing set has \
             agreed to nothing"
        );
        assert!(
            joint.is_met(&["n1", "n2", "n4"]),
            "a majority of both decides: old n1,n2 and new n1,n2,n4"
        );
        assert!(
            joint.is_met(&["n1", "n2", "n3", "n4"]),
            "more than a majority of both is still enough"
        );
    }

    #[test]
    fn every_voter_of_either_set_is_a_peer_while_the_change_is_in_flight() {
        let joint = Membership::Joint {
            old: config("n1,n2,n3"),
            new: config("n1,n2"),
        };
        let voters = joint.voters();
        assert_eq!(
            voters,
            ["n1", "n2", "n3"],
            "the union is the targeted set plus the voters it is leaving, and a node that leaves \
             keeps receiving entries until the change commits"
        );
        let unique: BTreeSet<&str> = voters.iter().copied().collect();
        assert_eq!(
            unique.len(),
            voters.len(),
            "a voter named by both sets appears once: a peer list is a set"
        );
        assert!(
            !joint.is_met(&["n3"]),
            "the leaving node alone is not a majority of either set"
        );

        let growing = Membership::Joint {
            old: config("n1,n2"),
            new: config("n1,n2,n3"),
        };
        assert_eq!(
            growing.voters(),
            ["n1", "n2", "n3"],
            "a joining node is a peer immediately: it has to receive the entry it will vote on"
        );

        let stable = Membership::Stable(config("n1,n2"));
        assert_eq!(
            stable.voters(),
            ["n1", "n2"],
            "a stable membership counts its own voters and nobody else"
        );
    }

    #[test]
    fn a_second_change_is_refused_while_one_is_in_flight() {
        let mut leader = Leader::new(config("n1,n2,n3"));
        let first = leader
            .begin(config("n1,n2,n3,n4"))
            .expect("the leader may propose a change");
        assert!(
            leader.membership().is_joint(),
            "the change is in flight at once"
        );
        assert_eq!(
            first, 1,
            "the configuration entry is the first entry this leader writes"
        );

        let error = leader
            .begin(config("n1,n2"))
            .expect_err("one change at a time");
        assert!(
            error.to_string().contains("in flight"),
            "two changes in flight are two windows, and the second can be decided by a set the \
             first has not agreed on: {error}"
        );

        assert!(
            leader.on_ack(&["n1", "n2", "n4"]),
            "old n1,n2 and new n1,n2,n4 decide the first change"
        );
        assert!(!leader.membership().is_joint());
        leader
            .begin(config("n1,n2,n3"))
            .expect("after the first change commits, the next may begin");
    }

    #[test]
    fn a_change_ends_only_when_both_sets_acknowledge_it() {
        let mut leader = Leader::new(config("n1,n2,n3"));
        leader
            .begin(config("n1,n2,n3,n4,n5"))
            .expect("the leader may propose a change");

        assert!(
            !leader.on_ack(&["n1", "n2"]),
            "the old majority holds the entry and the new set has not: the change is not decided"
        );
        assert!(
            leader.membership().is_joint(),
            "so it stays in flight rather than half-applied"
        );
        assert_eq!(
            leader.membership().targeted().render(),
            "n1,n2,n3,n4,n5",
            "and the target is the set it is moving to"
        );

        assert!(
            !leader.on_ack(&["n1", "n4", "n5"]),
            "the new majority without the old is not a decision either"
        );
        assert!(
            leader.on_ack(&["n1", "n2", "n4"]),
            "a majority of both ends the change"
        );
        assert!(
            !leader.membership().is_joint(),
            "the committed configuration is the new one, alone"
        );
        assert_eq!(
            leader.membership().targeted().render(),
            "n1,n2,n3,n4,n5",
            "and the cluster is the new set"
        );
    }

    #[test]
    fn the_configuration_is_a_log_entry_between_the_writes() {
        let mut leader = Leader::new(config("n1,n2,n3"));
        let before = leader.propose(b"before".to_vec());
        let change = leader
            .begin(config("n1,n2,n3,n4"))
            .expect("the leader may propose a change");
        let after = leader.propose(b"after".to_vec());

        assert_eq!(
            (before, change, after),
            (1, 2, 3),
            "the configuration takes the next position, so it cannot overtake the write before it"
        );
        let entry = leader
            .entry(change)
            .expect("the configuration is in the log");
        assert_eq!(
            decode(&entry.payload)
                .expect("a configuration entry")
                .render(),
            "n1,n2,n3,n4",
            "the entry carries the configuration itself, not a side channel that could be \
             ordered separately"
        );
        assert_eq!(
            decode(&leader.entry(before).expect("a write").payload),
            None,
            "a write is not a configuration: the marker is what tells the two apart"
        );
    }

    #[test]
    fn a_node_that_left_stops_counting_once_the_change_commits() {
        let mut leader = Leader::new(config("n1,n2,n3"));
        leader.begin(config("n1,n2")).expect("n3 leaves");
        assert!(
            leader.on_ack(&["n1", "n2"]),
            "n1 and n2 are a majority of the old set and of the new one"
        );
        assert!(
            !leader.membership().is_joint(),
            "the change committed, so the new configuration is adopted"
        );
        assert_eq!(leader.membership().targeted().render(), "n1,n2");
        assert!(
            !leader.voters().contains(&"n3"),
            "the node that left is not a peer any more: it is not asked for anything"
        );
        assert!(
            !leader.membership().is_met(&["n1", "n3"]),
            "and its acknowledgement stops counting: a cluster that kept counting a powered-off \
             node could wait for ever"
        );
        assert!(
            leader.membership().is_met(&["n1", "n2"]),
            "the two voters decide the next write"
        );
    }
}
