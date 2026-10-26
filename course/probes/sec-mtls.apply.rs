//! probe = "sec/mtls@apply"
//! title = "A peer is the name its certificate proves, and a rotation holds two truths only while both verify"
//! hint = "Trust only your own issuers, keep now inside a half-open window, pin the certificate's subject to the claimed name, and present a rotation's new certificate only from the instant it is valid."
//!
//! Step `meshkv/5.9-mtls` is the step where a node's name stops being a claim and becomes a proof.
//! `5.1` let a node say its name in one line of text, and any process that can open a socket can say
//! it too. The fix is not "add TLS" as a checkbox: it is three decisions a handshake has to make,
//! plus one operation that is the cluster's own problem.
//!
//! 1. **the issuer has to be trusted.** A certificate is a statement by somebody, and the trust
//!    store is the list of somebodies this node believes. A well-formed certificate from a stranger
//!    is a stranger, and a store that believes nobody believes nobody.
//! 2. **the window has to contain now, and it is half-open.** The certificate is valid from
//!    `not_before` (inclusive) up to `not_after` (exclusive): at the expiry instant it is already
//!    invalid. The window is the mechanism by which a compromised key stops being useful, so a node
//!    that skips it has a trust store that only grows.
//! 3. **the name in the certificate has to be the name that was claimed.** A valid certificate for
//!    `n2` proves the peer is `n2`; it proves nothing about a peer claiming `n3`. A handshake that
//!    verifies the chain and ignores the claim has authenticated *somebody*, and in a cluster
//!    "somebody" is exactly what an attacker is.
//! 4. **the three refusals are three sentences.** An untrusted issuer, a window that has not opened
//!    and one that has closed have different fixes — a configuration, a clock, an operations
//!    problem — and "handshake failed" sends an operator to the wrong one. The issuer is checked
//!    first, so a stranger whose certificate has also expired is reported as a stranger.
//! 5. **a rotation holds two truths, on arithmetic.** A node presents the new certificate from the
//!    instant it is valid and not one tick earlier; it accepts a peer on either certificate while
//!    both verify; and the earliest safe retirement is the new certificate's start plus the
//!    distribution overlap. Retiring on nerves rather than arithmetic is an outage with a schedule.
//!
//! ## What you write
//!
//! * [`Certificate::new`] — a certificate is a name, an issuer and a non-empty window, or a refusal;
//! * [`verify`] — the three rules, in order, producing an [`Identity`] or naming which rule failed;
//! * [`authenticate`] — verify, **and** check that the certificate's subject is the claimed name;
//! * [`Rotation::new`] — a node's old and new certificate, which have to be for the same node;
//! * [`Rotation::presented`] — the certificate this node shows its peers at `now`;
//! * [`Rotation::earliest_safe_retirement`] — the instant the old certificate may be retired.
//!
//! [`TrustStore`], [`Certificate`] with its accessors and [`Certificate::covers`], [`Identity`],
//! [`Refusal`], [`Rotation::accepts`] and the getters are given. The vocabulary is the interface: a
//! probe that asked the learner to parse X.509 would be a parsing exercise, not this step. [`Refusal`]
//! is given with its four variants because the answer *is* the lesson — the four fixes are four
//! different people's jobs. `Identity` has a private field and no constructor outside [`verify`] and
//! [`authenticate`], so in this file "authenticated" is a fact about a type rather than a comment.
//!
//! ## What the tests ask
//!
//! A certificate's accessors read back what it was built with, its window includes its start and
//! excludes its end, and an empty, backwards, blank-subject or blank-issuer certificate is refused
//! at construction. A trusted certificate inside its window becomes an identity, at its first and
//! last valid instants too; one tick before the start it is `NotYetValid`, and at the expiry instant
//! it is `Expired`. A stranger is `NotTrusted`, and so is any certificate when the store is empty.
//! The three refusals are three distinct answers, and an expired stranger is reported as a stranger
//! because the issuer is checked first. A perfectly valid certificate for `n2` does not
//! authenticate a peer claiming `n3`, and the same certificate does authenticate `n2`. A rotation
//! into another name is refused with both names in the message. The old certificate is presented
//! until the new one's first valid instant and the new one from that instant on. A peer on the old
//! certificate is accepted through the overlap and refused at the instant it expires, while the new
//! one keeps working. And the earliest safe retirement is the new certificate's start plus the
//! overlap — zero overlap means the start itself.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, and **no TLS, no X.509, no signatures, no keys and no network**. There
//! is no cryptography in here and none is pretended: anybody can call [`Certificate::new`] with any
//! name, so a [`Certificate`] is a *claim* and nothing more. The proof that the peer holds the key
//! is the TLS library's job, and the three rules above are the decisions a handshake makes *after*
//! that proof has arrived — which is where a cluster still gets it wrong. [`Identity`] can be built
//! only by [`verify`] and [`authenticate`] within this file, so the newtype is honest about what it
//! checked and silent about what it did not.
//!
//! What is re-checked is exactly what a reviewer can audit without a certificate authority: the
//! half-open boundary, the name pin, the distinct refusals, and the rotation arithmetic that decides
//! whether renewing a certificate is a no-op or an outage. `sec/token-lifetime@apply` is the same
//! three rules for a bearer token, and the difference is worth knowing: a token is presented per
//! request while a certificate is presented per connection, which is why the overlap is a *cluster*
//! problem. `sec/secrets-management@design` is the same rotation question one layer up, where the
//! material itself changes.

/// Why a certificate is not a proof of who the peer is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The issuer is not one this node trusts.
    NotTrusted {
        /// The name the certificate is for.
        subject: String,
        /// The issuer this node does not believe.
        issuer: String,
    },
    /// The certificate's window has not started.
    NotYetValid {
        /// The name the certificate is for.
        subject: String,
        /// When it becomes valid.
        not_before: u64,
        /// The reading that was too early.
        now: u64,
    },
    /// The certificate's window has ended.
    Expired {
        /// The name the certificate is for.
        subject: String,
        /// When it stopped being valid.
        not_after: u64,
        /// The reading that was too late.
        now: u64,
    },
    /// The certificate verifies, but for a different name than the peer claimed.
    WrongSubject {
        /// The name the peer claimed.
        claimed: String,
        /// The name the certificate is for.
        subject: String,
    },
}

/// The issuers a node believes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustStore {
    roots: Vec<String>,
}

impl TrustStore {
    /// A store that believes nobody.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// A store that believes `roots`.
    #[must_use]
    pub fn of(roots: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            roots: roots.into_iter().map(Into::into).collect(),
        }
    }

    /// The issuers it believes.
    #[must_use]
    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    /// `true` when `issuer` is one of the roots.
    #[must_use]
    pub fn trusts(&self, issuer: &str) -> bool {
        self.roots.iter().any(|root| root == issuer)
    }
}

/// A certificate as it arrives: a name, an issuer, and the window it is good for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    subject: String,
    issuer: String,
    not_before: u64,
    not_after: u64,
}

impl Certificate {
    /// A certificate for `subject`, issued by `issuer`, valid from `not_before` until `not_after`.
    ///
    /// # Errors
    ///
    /// A reason when the subject or the issuer is blank, or when the window is empty: a
    /// certificate that is never valid can never be used, and one whose window is backwards is a
    /// mistake that would otherwise wait until the first handshake.
    pub fn new(
        subject: &str,
        issuer: &str,
        not_before: u64,
        not_after: u64,
    ) -> Result<Self, String> {
        todo!()
    }

    /// The name the certificate is for.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Who issued it.
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// When it becomes valid.
    #[must_use]
    pub fn not_before(&self) -> u64 {
        self.not_before
    }

    /// When it stops being valid.
    #[must_use]
    pub fn not_after(&self) -> u64 {
        self.not_after
    }

    /// `true` while `now` is inside the window — the window is half-open, so a certificate is
    /// already invalid at the instant it expires.
    #[must_use]
    pub fn covers(&self, now: u64) -> bool {
        self.not_before <= now && now < self.not_after
    }
}

/// A peer whose certificate was verified: the only thing the rest of the system should accept as
/// "who is calling".
///
/// There is no public constructor. An `Identity` comes out of [`verify`] and [`authenticate`] and
/// nowhere else, which is what makes "the peer is authenticated" a fact about the type rather than a
/// convention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    subject: String,
}

impl Identity {
    /// The name the certificate proved.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }
}

/// Verify `certificate` against `store` at `now`, and answer who the peer is.
///
/// The checks are in the order an operator would fix them: the issuer first, then the start of the
/// window, then its end.
///
/// # Errors
///
/// [`Refusal::NotTrusted`], [`Refusal::NotYetValid`] or [`Refusal::Expired`], each naming the
/// certificate and the fact that failed.
pub fn verify(
    certificate: &Certificate,
    store: &TrustStore,
    now: u64,
) -> Result<Identity, Refusal> {
    todo!()
}

/// Verify a certificate **and** that it belongs to the peer that claimed the name.
///
/// A handshake has two facts — the name in the greeting and the certificate that arrived with it —
/// and the only safe rule is that they are the same.
///
/// # Errors
///
/// Whatever [`verify`] refuses, and [`Refusal::WrongSubject`] when the certificate's subject is not
/// the claimed name.
pub fn authenticate(
    claimed: &str,
    certificate: &Certificate,
    store: &TrustStore,
    now: u64,
) -> Result<Identity, Refusal> {
    todo!()
}

/// A node's current and next certificate, so that a rotation does not have a gap.
///
/// Rotation is the one moment a cluster holds two truths at once: peers still present the old
/// certificate while the new one is distributed, and a node that accepts only one of them refuses
/// half the cluster for as long as the distribution takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rotation {
    current: Certificate,
    next: Certificate,
}

impl Rotation {
    /// A node's certificates: the one in use, and the one it is moving to.
    ///
    /// # Errors
    ///
    /// A reason when the two certificates are for different names: a node cannot rotate into
    /// somebody else's identity.
    pub fn new(current: Certificate, next: Certificate) -> Result<Self, String> {
        todo!()
    }

    /// The certificate in use before the rotation.
    #[must_use]
    pub fn current(&self) -> &Certificate {
        &self.current
    }

    /// The certificate the rotation is moving to.
    #[must_use]
    pub fn next(&self) -> &Certificate {
        &self.next
    }

    /// The certificate this node presents to its peers.
    ///
    /// The new one as soon as it is valid; the old one until then. Presenting the new one early is a
    /// handshake that fails against every peer the distribution has not reached yet.
    #[must_use]
    pub fn presented(&self, now: u64) -> &Certificate {
        todo!()
    }

    /// `true` when this node accepts a peer's certificate: either certificate while it verifies
    /// against the store, which is what makes the overlap have no gap.
    ///
    /// The acceptance question is about the *peer's* certificate, so what matters is the store, not
    /// this node's own rotation: a node accepts every peer whose certificate verifies, including one
    /// still on the old certificate. What the rotation bounds is how long that is true.
    #[must_use]
    pub fn accepts(&self, certificate: &Certificate, store: &TrustStore, now: u64) -> bool {
        verify(certificate, store, now).is_ok()
    }

    /// The instant after which the old certificate may be retired safely, given the overlap.
    ///
    /// The caller passes the overlap it chose — how long a distribution takes — and the answer is
    /// the moment the new certificate is valid plus that overlap. Retiring before it is how a
    /// rotation becomes an outage.
    #[must_use]
    pub fn earliest_safe_retirement(&self, overlap: u64) -> u64 {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one issuer the tests' cluster trusts.
    const CA: &str = "cluster-ca";

    fn store() -> TrustStore {
        TrustStore::of([CA])
    }

    fn cert(subject: &str, not_before: u64, not_after: u64) -> Certificate {
        Certificate::new(subject, CA, not_before, not_after).expect("a certificate")
    }

    #[test]
    fn a_certificate_is_a_name_an_issuer_and_a_half_open_window() {
        let certificate = cert("n1", 100, 200);
        assert_eq!(
            certificate.subject(),
            "n1",
            "the subject is the name the certificate is for"
        );
        assert_eq!(
            certificate.issuer(),
            CA,
            "the issuer is the somebody that vouches for it"
        );
        assert_eq!(certificate.not_before(), 100);
        assert_eq!(certificate.not_after(), 200);
        assert!(
            certificate.covers(100),
            "the window includes the instant it starts"
        );
        assert!(certificate.covers(199), "and every instant before the end");
        assert!(
            !certificate.covers(200),
            "the end instant is outside: a certificate is invalid at its expiry, not a tick later"
        );
        assert!(
            !certificate.covers(99),
            "and before the start it is not valid either"
        );

        let empty =
            Certificate::new("n1", CA, 200, 200).expect_err("a window has to contain an instant");
        assert!(
            empty.contains("window"),
            "the refusal names the empty window: {empty}"
        );
        let backwards =
            Certificate::new("n1", CA, 200, 100).expect_err("a backwards window is empty too");
        assert!(
            backwards.contains("window"),
            "and the refusal says so: {backwards}"
        );
        let anonymous_issuer =
            Certificate::new("n1", "  ", 100, 200).expect_err("an issuer is a name");
        assert!(
            anonymous_issuer.contains("issuer"),
            "the refusal names the missing issuer: {anonymous_issuer}"
        );
        let anonymous_subject =
            Certificate::new(" ", CA, 100, 200).expect_err("a subject is a name");
        assert!(
            anonymous_subject.contains("subject"),
            "the refusal names the missing subject: {anonymous_subject}"
        );
    }

    #[test]
    fn a_trusted_certificate_inside_its_window_becomes_an_identity() {
        let certificate = cert("n2", 100, 200);
        for now in [100, 150, 199] {
            let identity = verify(&certificate, &store(), now)
                .unwrap_or_else(|refused| panic!("{now} is inside the window: {refused:?}"));
            assert_eq!(
                identity.subject(),
                "n2",
                "the identity is the name the certificate is for"
            );
        }
        assert_eq!(
            verify(&certificate, &store(), 99),
            Err(Refusal::NotYetValid {
                subject: "n2".to_owned(),
                not_before: 100,
                now: 99,
            }),
            "one tick before the window opens, the certificate is not valid yet"
        );
        assert_eq!(
            verify(&certificate, &store(), 200),
            Err(Refusal::Expired {
                subject: "n2".to_owned(),
                not_after: 200,
                now: 200,
            }),
            "the expiry instant is already outside the window: the safe side of the comparison \
             refuses"
        );
    }

    #[test]
    fn a_certificate_from_a_stranger_is_a_stranger() {
        let stranger =
            Certificate::new("n2", "other-ca", 100, 200).expect("a well-formed certificate");
        assert_eq!(
            verify(&stranger, &store(), 150),
            Err(Refusal::NotTrusted {
                subject: "n2".to_owned(),
                issuer: "other-ca".to_owned(),
            }),
            "a certificate is only as good as the issuer this node believes, and a stranger is a \
             stranger however well-formed"
        );
        assert_eq!(
            verify(&cert("n2", 100, 200), &TrustStore::empty(), 150),
            Err(Refusal::NotTrusted {
                subject: "n2".to_owned(),
                issuer: CA.to_owned(),
            }),
            "a store that believes nobody believes nobody"
        );
    }

    #[test]
    fn untrusted_early_and_expired_are_three_different_answers() {
        let now = 150;
        let untrusted = verify(
            &Certificate::new("n2", "other-ca", 100, 200).expect("a certificate"),
            &store(),
            now,
        )
        .unwrap_err();
        let early = verify(&cert("n2", 200, 300), &store(), now).unwrap_err();
        let expired = verify(&cert("n2", 0, 100), &store(), now).unwrap_err();

        assert!(
            matches!(untrusted, Refusal::NotTrusted { .. }),
            "the wrong issuer is its own answer: {untrusted:?}"
        );
        assert!(
            matches!(early, Refusal::NotYetValid { .. }),
            "and a window that has not opened is its own: {early:?}"
        );
        assert!(
            matches!(expired, Refusal::Expired { .. }),
            "and so is one that has closed: {expired:?}"
        );
        assert_ne!(
            untrusted, early,
            "a wrong issuer is a configuration problem, an early certificate is a clock or \
             distribution problem"
        );
        assert_ne!(
            early, expired,
            "and an early certificate is not an expired one"
        );
        assert_ne!(untrusted, expired);

        // The issuer is checked before the window: a stranger whose certificate has also expired is
        // a stranger, and the operator has to fix the trust store, not the clock.
        let both = Certificate::new("n2", "other-ca", 0, 100).expect("a certificate");
        assert_eq!(
            verify(&both, &store(), now),
            Err(Refusal::NotTrusted {
                subject: "n2".to_owned(),
                issuer: "other-ca".to_owned(),
            }),
            "the issuer comes first: the refusal names the fix an operator has to make"
        );
    }

    #[test]
    fn a_certificate_for_somebody_else_does_not_authenticate_this_peer() {
        let certificate = cert("n2", 100, 200);
        assert_eq!(
            authenticate("n3", &certificate, &store(), 150),
            Err(Refusal::WrongSubject {
                claimed: "n3".to_owned(),
                subject: "n2".to_owned(),
            }),
            "a valid certificate for n2 proves nothing about a peer claiming n3: the name in the \
             greeting and the name in the certificate are one value or the handshake is refused"
        );
        let identity = authenticate("n2", &certificate, &store(), 150)
            .expect("the certificate and the claim agree");
        assert_eq!(identity.subject(), "n2");
    }

    #[test]
    fn a_rotation_into_another_name_is_refused() {
        let error = Rotation::new(cert("n1", 100, 200), cert("n2", 200, 300))
            .expect_err("a node cannot rotate into somebody else's identity");
        assert!(
            error.contains("n1") && error.contains("n2"),
            "the refusal names both names: {error}"
        );
    }

    #[test]
    fn a_rotation_presents_the_new_certificate_from_its_first_valid_instant() {
        let rotation =
            Rotation::new(cert("n1", 100, 400), cert("n1", 200, 500)).expect("a rotation");
        assert_eq!(rotation.current().not_before(), 100);
        assert_eq!(rotation.next().not_before(), 200);
        assert_eq!(
            rotation.presented(199).not_before(),
            100,
            "before the new certificate is valid, the old one is the one that verifies: presenting \
             the new one early fails against every peer the distribution has not reached"
        );
        assert_eq!(
            rotation.presented(200).not_before(),
            200,
            "the instant the new one is valid it is the one presented, not a tick later"
        );
        assert_eq!(
            rotation.presented(499).not_after(),
            500,
            "and it stays the one presented"
        );
    }

    #[test]
    fn a_rotation_accepts_a_peer_on_either_certificate_until_the_old_one_expires() {
        let rotation =
            Rotation::new(cert("n1", 100, 500), cert("n1", 200, 600)).expect("a rotation");
        let old_peer = cert("n2", 100, 500);
        let new_peer = cert("n2", 200, 600);

        assert!(
            rotation.accepts(&old_peer, &store(), 250),
            "during the overlap a peer still on the old certificate is accepted: that is what \
             makes the rotation have no gap"
        );
        assert!(
            rotation.accepts(&new_peer, &store(), 250),
            "and so is a peer already on the new one"
        );
        assert!(
            !rotation.accepts(&old_peer, &store(), 500),
            "the instant the old certificate expires it is refused, which is why the retirement \
             waits for the distribution"
        );
        assert!(
            rotation.accepts(&new_peer, &store(), 550),
            "while the new one has not expired"
        );
    }

    #[test]
    fn the_earliest_safe_retirement_is_the_new_start_plus_the_overlap() {
        let rotation = Rotation::new(cert("n1", 0, 100), cert("n1", 100, 400)).expect("a rotation");
        assert_eq!(
            rotation.earliest_safe_retirement(0),
            100,
            "with no distribution time, the old certificate may go the instant the new one is valid"
        );
        assert_eq!(
            rotation.earliest_safe_retirement(50),
            150,
            "retiring before the distribution finished is how a rotation becomes an outage"
        );
        assert!(
            rotation.earliest_safe_retirement(50) < rotation.next().not_after(),
            "and a sane overlap has to fit inside the new certificate's window"
        );
    }
}
