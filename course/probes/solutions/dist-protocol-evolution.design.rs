//! probe = "dist/protocol-evolution@design"
//! title = "A newer build holds back, an unknown frame is skipped, and a feature waits for every peer"
//! hint = "Let an older build read only its own minor and below, refuse a version it cannot parse before reading the frame, skip a kind it does not know, and name every peer holding a feature back."
//!
//! Step `meshkv/5.21-protocol-evolution` is the third "two truths at once" in this track: joint
//! consensus held two memberships, certificate rotation held two certificates, and a rolling upgrade
//! holds two **builds**. The other slot of the same step, `ops/rolling-upgrade@design`, owns the
//! schedule; this file owns the wire rule the schedule has to respect, and the feature that may not
//! be used until the schedule has finished. Five rules:
//!
//! 1. **two numbers, two jobs.** `major` is the wire format: two builds with different majors cannot
//!    talk, so a major bump is stop-the-world and never a rolling upgrade. `minor` is a compatible
//!    revision inside a major, and a build parses every minor up to its own and no further.
//!    [`Protocol::can_parse`] is the one rule both the handshake and the reader use.
//! 2. **the compatibility is asymmetric.** A newer build reads an older peer's frames; an older
//!    build cannot read a newer one's. So the newer build is the one that holds back, and a rule
//!    that assumed symmetry would leave every not-yet-upgraded node failing to parse what its
//!    upgraded neighbour sends.
//! 3. **the version is read before the kind, and the two answers differ.** A frame whose *version*
//!    this build cannot parse is refused: the framing itself may have changed, so there is no
//!    trustworthy length to skip. A frame whose version is readable but whose *kind* is unknown is
//!    skipped and counted — a newer peer is allowed to say something this build does not know, and
//!    closing the connection over it would make an additive change look like a partition.
//! 4. **a refusal names both versions and which number could not meet.** "Protocol mismatch" sends
//!    an operator to the wrong build; "peer `1.2`, this build `1.1`, minor" does not.
//! 5. **a feature may only be *used* once every peer can read it.** The local build having the
//!    feature is necessary and not sufficient: [`Feature::rollout`] answers [`Rollout::Unbuilt`] for
//!    the local side, and otherwise names the peers holding it back, sorted, so two deploy logs
//!    print the same line. That set is what the plan in `ops/rolling-upgrade@design` is waiting for.
//!
//! ## What you write
//!
//! * [`Protocol::can_parse`] — the asymmetric rule;
//! * [`Session::hello`] — a peer's version accepted, or a refusal that says which number failed;
//! * [`Session::receive`] — one frame delivered, skipped and counted, or refused before its kind is
//!   even looked at;
//! * [`Feature::rollout`] — whether a feature is usable, and if not, why: the local build or the
//!   named peers.
//!
//! [`Protocol`]'s parse/render/ordering, [`Frame`], [`known`] (the kinds this build knows),
//! [`Disposition`], [`Refusal`], [`Mismatch`], [`Rollout`] and [`Session`]'s accessors are given. The
//! frame's kind is a number rather than an enum on purpose: the set grows with every minor, and a
//! peer running a newer one has kinds this build cannot name. The version, the kind table and the
//! refusal are the vocabulary the real reader, the handshake and the page all share.
//!
//! ## What the tests ask
//!
//! `1.10` is a later minor than `1.9` — versions compare as numbers, not as text — and the parse
//! rule runs one way only. The newer session accepts an older peer while the older one refuses the
//! newer, naming both versions and saying the newer build has to hold back; its own version and an
//! older minor inside its major are accepted. A major difference is refused as a major before the
//! minor is even the question, and a newer minor is a minor refusal. A frame from a readable version
//! with kind 9 is skipped and counted, the highest known kind 3 is delivered, and the first unknown
//! kind 4 is skipped — the boundary is exactly the known set. A frame from a version this build
//! cannot parse is refused even when its kind is known, and a refusal is not counted as a skip, so
//! the two answers are not the same answer. A feature at `1.2` is usable with no peers and with every
//! peer at exactly `1.2`, held by the one peer at `1.1` or at a different major, and unbuilt when the
//! local build does not have it or is in another major. The last test gives the same peers in two
//! orders and gets the same sorted list, which is what makes the answer a deploy log rather than a
//! coincidence.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no socket, no bytes and no build**. The real reader has a length
//! prefix and skips an unknown frame by the number of bytes it declares; here the "skip" is the
//! decision that a readable version with an unknown kind is not an error, and the payload is absent,
//! so nothing can be misparsed by this file. The handshake is a method on a value, not a connection;
//! the versions are literals, not a `cargo` build; and the actual frames are `meshkv/5.21`'s
//! `protocol.rs`. What is re-checked is the *decision table* a reader and a deploy log are written
//! against, which is the part that decides whether an additive change is safe.
//!
//! Missing on purpose: no encoding or decoding, no payload validation, no connection state (a
//! frame after a refusal is not modelled), no major-version migration (a major bump is not a rolling
//! upgrade and has no plan here), and no ordering of features — a feature is a name and a version.
//! The schedule itself is `ops/rolling-upgrade@design`, the code-side page is `meshkv/5.21`, and
//! the same "two versions during one deploy" problem for a database is
//! `db/schema-online-evolution@apply`.

use std::fmt;

/// The version of the wire protocol one build speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Protocol {
    /// The wire format. Two builds with different majors cannot talk at all.
    pub major: u32,
    /// The compatible revision inside a major: a build parses every minor up to its own.
    pub minor: u32,
}

impl Protocol {
    /// A protocol version.
    #[must_use]
    pub fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    /// Parse `major.minor`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (major, minor) = text.split_once('.')?;
        Some(Self {
            major: major.parse().ok()?,
            minor: minor.parse().ok()?,
        })
    }

    /// The version as it is written: `major.minor`.
    #[must_use]
    pub fn render(self) -> String {
        format!("{}.{}", self.major, self.minor)
    }

    /// `true` when a build at `self` can *parse* what a build at `other` sends.
    ///
    /// A build understands every minor up to its own, and no further. The rule is asymmetric, and
    /// the asymmetry is the whole constraint of a rolling upgrade: the newer build is the one that
    /// has to hold back, because it is the one that could send something the older cannot read.
    #[must_use]
    pub fn can_parse(&self, other: &Self) -> bool {
        self.major == other.major && other.minor <= self.minor
    }
}

/// Which number could not meet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mismatch {
    /// The wire format: no rolling upgrade mixes two majors.
    Major,
    /// The revision: the newer build is sending past what this one parses.
    Minor,
}

/// A handshake or a frame that this build cannot accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal {
    /// What this build speaks.
    pub local: Protocol,
    /// What the peer speaks or wrote.
    pub peer: Protocol,
    /// Which number could not meet.
    pub mismatch: Mismatch,
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.mismatch {
            Mismatch::Major => write!(
                formatter,
                "the peer speaks protocol {} and this build speaks {}: a major version cannot be mixed \
                 in a running cluster",
                self.peer.render(),
                self.local.render()
            ),
            Mismatch::Minor => write!(
                formatter,
                "the peer sends protocol {} and this build parses up to {}: the newer build has to hold \
                 back until this one is upgraded",
                self.peer.render(),
                self.local.render()
            ),
        }
    }
}

/// One frame, as much of it as a reader needs to decide what to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// The protocol version the sender wrote it at.
    pub written: Protocol,
    /// The frame kind: a number, because a newer peer has kinds this build does not know.
    pub kind: u16,
}

/// The kinds this build knows, as `(kind, name)`.
pub const KNOWN_KINDS: [(u16, &str); 3] = [(1, "append"), (2, "vote"), (3, "replicate")];

/// The name of a known kind, or `None` for a kind this build does not know.
#[must_use]
pub fn known(kind: u16) -> Option<&'static str> {
    KNOWN_KINDS
        .iter()
        .find_map(|(number, name)| (*number == kind).then_some(*name))
}

/// What a reader did with a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// A kind this build knows, by name.
    Delivered(&'static str),
    /// A kind it does not know, skipped and counted.
    Ignored {
        /// The kind that was skipped.
        kind: u16,
    },
}

/// One build's view of one connection: what it speaks, and what it has skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    local: Protocol,
    ignored: u64,
}

impl Session {
    /// A session for a build at `local`.
    #[must_use]
    pub fn new(local: Protocol) -> Self {
        Self { local, ignored: 0 }
    }

    /// What this build speaks.
    #[must_use]
    pub fn local(&self) -> Protocol {
        self.local
    }

    /// How many frames have been skipped because their kind was unknown.
    #[must_use]
    pub fn ignored(&self) -> u64 {
        self.ignored
    }

    /// Check a peer's version before frames are exchanged.
    ///
    /// # Errors
    ///
    /// [`Mismatch::Major`] before [`Mismatch::Minor`]: two majors cannot be in one cluster, so that
    /// is the answer even when the minor would also fail.
    pub fn hello(&self, peer: Protocol) -> Result<(), Refusal> {
        if peer.major != self.local.major {
            return Err(Refusal {
                local: self.local,
                peer,
                mismatch: Mismatch::Major,
            });
        }
        if !self.local.can_parse(&peer) {
            return Err(Refusal {
                local: self.local,
                peer,
                mismatch: Mismatch::Minor,
            });
        }
        Ok(())
    }

    /// Read one frame: deliver a known kind, skip an unknown one, or refuse an unreadable version.
    ///
    /// The version is checked before the kind, because the version is what says whether the frame's
    /// framing can be trusted at all. A skip is counted; a refusal is not a skip — one is an
    /// additive change, the other is a peer this build cannot follow.
    ///
    /// # Errors
    ///
    /// The [`Refusal`] of [`Session::hello`] when the frame's version cannot be parsed.
    pub fn receive(&mut self, frame: &Frame) -> Result<Disposition, Refusal> {
        self.hello(frame.written)?;
        if let Some(name) = known(frame.kind) {
            Ok(Disposition::Delivered(name))
        } else {
            self.ignored += 1;
            Ok(Disposition::Ignored { kind: frame.kind })
        }
    }
}

/// A feature, and the protocol version whose frame carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feature {
    /// The name a deploy log and an ADR use.
    pub name: &'static str,
    /// The first version that can read the frame.
    pub since: Protocol,
}

/// Whether a feature may be used yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rollout {
    /// Every peer can read it, and so can this build: use it.
    Usable,
    /// This build does not have the feature at all.
    Unbuilt,
    /// These peers cannot read the frame, so the cluster holds the feature back.
    Held {
        /// The peers holding it back, sorted by name.
        by: Vec<String>,
    },
}

impl Feature {
    /// Whether this feature may be used by a build at `local` talking to `peers`.
    ///
    /// The local build must be able to read the feature's frame — otherwise the question is not
    /// about the cluster — and every peer must be able to read a frame written at
    /// [`Feature::since`]. A peer that cannot is named; the sort is what makes the answer stable
    /// across deploy logs that list the cluster in a different order.
    #[must_use]
    pub fn rollout(&self, local: Protocol, peers: &[(&str, Protocol)]) -> Rollout {
        if !local.can_parse(&self.since) {
            return Rollout::Unbuilt;
        }
        let mut by: Vec<String> = peers
            .iter()
            .filter(|(_, protocol)| !protocol.can_parse(&self.since))
            .map(|(name, _)| (*name).to_owned())
            .collect();
        if by.is_empty() {
            Rollout::Usable
        } else {
            by.sort();
            Rollout::Held { by }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Protocol {
        Protocol::parse(text).expect("a `major.minor` version")
    }

    #[test]
    fn the_minor_compares_as_a_number_and_the_rule_only_looks_one_way() {
        let one_ten = version("1.10");
        let one_nine = version("1.9");
        assert!(
            one_ten > one_nine,
            "1.10 is a later minor than 1.9: versions compare as numbers, not as text"
        );
        assert_eq!(
            (one_ten.major, one_ten.minor),
            (1, 10),
            "both numbers parse"
        );
        assert_eq!(one_ten.render(), "1.10", "and render back");

        assert!(
            one_ten.can_parse(&one_nine),
            "a newer build reads an older frame"
        );
        assert!(
            !one_nine.can_parse(&one_ten),
            "an older build cannot read a newer frame"
        );
        assert!(
            one_ten.can_parse(&one_ten),
            "and a build reads its own version"
        );
    }

    #[test]
    fn a_newer_build_reads_an_older_frame_and_an_older_build_refuses_a_newer_one() {
        let newer = Session::new(version("1.2"));
        assert_eq!(
            newer.hello(version("1.1")),
            Ok(()),
            "the newer build parses the older peer"
        );

        let older = Session::new(version("1.1"));
        let error = older
            .hello(version("1.2"))
            .expect_err("the older build cannot parse the newer peer");
        assert_eq!(
            error,
            Refusal {
                local: version("1.1"),
                peer: version("1.2"),
                mismatch: Mismatch::Minor,
            },
            "the refusal names which side failed and which number"
        );
        let message = error.to_string();
        assert!(
            message.contains("1.2") && message.contains("1.1"),
            "an operator reads both versions in the sentence: {message}"
        );
        assert!(
            message.contains("hold back"),
            "and reads which build has to move: {message}"
        );
    }

    #[test]
    fn a_major_difference_is_refused_before_the_minor_is_the_question() {
        let session = Session::new(version("1.9"));
        let error = session
            .hello(version("2.0"))
            .expect_err("a major cannot be mixed");
        assert_eq!(
            error.mismatch,
            Mismatch::Major,
            "the major is the first question, and the answer names it"
        );
        assert!(
            error.to_string().contains("major"),
            "the reason says which number failed: {error}"
        );

        assert_eq!(
            session.hello(version("1.99")),
            Err(Refusal {
                local: version("1.9"),
                peer: version("1.99"),
                mismatch: Mismatch::Minor,
            }),
            "a newer minor inside the same major is still a minor question"
        );
        assert_eq!(
            session.hello(version("1.0")),
            Ok(()),
            "an older minor inside the major is fine"
        );
        assert_eq!(
            session.hello(version("1.9")),
            Ok(()),
            "and so is the same version"
        );
    }

    #[test]
    fn a_frame_of_an_unknown_kind_is_ignored_once_its_version_is_readable() {
        let mut session = Session::new(version("1.2"));

        assert_eq!(
            session.receive(&Frame {
                written: version("1.2"),
                kind: 9,
            }),
            Ok(Disposition::Ignored { kind: 9 }),
            "a kind this build does not know is skipped, not misread"
        );
        assert_eq!(session.ignored(), 1, "and the skip is counted");

        assert_eq!(
            session.receive(&Frame {
                written: version("1.2"),
                kind: 3,
            }),
            Ok(Disposition::Delivered("replicate")),
            "the highest known kind is delivered"
        );
        assert_eq!(
            session.receive(&Frame {
                written: version("1.2"),
                kind: 4,
            }),
            Ok(Disposition::Ignored { kind: 4 }),
            "and the first kind past the table is skipped"
        );
        assert_eq!(
            session.receive(&Frame {
                written: version("1.1"),
                kind: 9,
            }),
            Ok(Disposition::Ignored { kind: 9 }),
            "an older frame is readable too, and its unknown kind is still just skipped"
        );
        assert_eq!(
            session.ignored(),
            3,
            "a delivered frame is not an ignore and a skip is always counted"
        );
    }

    #[test]
    fn a_frame_whose_version_cannot_be_read_is_refused_rather_than_skipped() {
        let mut session = Session::new(version("1.2"));

        let error = session
            .receive(&Frame {
                written: version("1.3"),
                kind: 1,
            })
            .expect_err("the framing of a newer minor is not readable");
        assert_eq!(
            error.mismatch,
            Mismatch::Minor,
            "a known kind inside a version this build cannot parse is still not a frame it can trust"
        );
        assert_eq!(
            session.ignored(),
            0,
            "a refusal is not a skip, so the counter does not move"
        );

        let error = session
            .receive(&Frame {
                written: version("2.0"),
                kind: 1,
            })
            .expect_err("a major cannot be mixed");
        assert_eq!(
            error.mismatch,
            Mismatch::Major,
            "and the major is the reason before the kind or the minor"
        );
    }

    #[test]
    fn a_feature_is_usable_only_when_every_peer_can_read_it() {
        let feature = Feature {
            name: "compare-and-swap",
            since: version("1.2"),
        };
        let local = version("1.2");

        assert_eq!(
            feature.rollout(local, &[]),
            Rollout::Usable,
            "a one-node cluster whose build has the feature uses it"
        );
        assert_eq!(
            feature.rollout(local, &[("n1", version("1.2")), ("n2", version("1.2"))]),
            Rollout::Usable,
            "every peer at exactly the feature's version is enough"
        );

        assert_eq!(
            feature.rollout(local, &[("n1", version("1.2")), ("n2", version("1.1"))]),
            Rollout::Held {
                by: vec!["n2".to_owned()],
            },
            "the peer that cannot read the frame is the one named"
        );
        assert_eq!(
            feature.rollout(local, &[("n3", version("2.0"))]),
            Rollout::Held {
                by: vec!["n3".to_owned()],
            },
            "and a peer in another major cannot read it either"
        );
    }

    #[test]
    fn a_build_that_does_not_have_the_feature_cannot_use_it_whatever_its_peers_say() {
        let feature = Feature {
            name: "compare-and-swap",
            since: version("1.2"),
        };

        assert_eq!(
            feature.rollout(version("1.1"), &[("n1", version("1.2"))]),
            Rollout::Unbuilt,
            "the local build lacks the frame, so the peers' versions cannot grant it"
        );
        assert_eq!(
            feature.rollout(version("2.0"), &[("n1", version("2.0"))]),
            Rollout::Unbuilt,
            "and another major does not have this feature at all"
        );
        assert_eq!(
            feature.rollout(version("1.3"), &[("n1", version("1.1"))]),
            Rollout::Held {
                by: vec!["n1".to_owned()],
            },
            "a newer local build still waits for the old peer"
        );
    }

    #[test]
    fn the_peers_holding_a_feature_back_are_named_in_one_order() {
        let feature = Feature {
            name: "compare-and-swap",
            since: version("1.2"),
        };
        let local = version("1.2");

        let one = feature.rollout(
            local,
            &[
                ("n2", version("1.1")),
                ("n1", version("1.0")),
                ("n3", version("1.2")),
            ],
        );
        let other = feature.rollout(
            local,
            &[
                ("n1", version("1.0")),
                ("n3", version("1.2")),
                ("n2", version("1.1")),
            ],
        );

        assert_eq!(
            one, other,
            "two deploy logs that list the same peers in a different order hold the same answer"
        );
        assert_eq!(
            one,
            Rollout::Held {
                by: vec!["n1".to_owned(), "n2".to_owned()],
            },
            "and the list is sorted, so two runs print the same line"
        );
    }
}
