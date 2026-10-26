//! What `meshkv/5.9-mtls` has to be true about.
//!
//! The step's subject is the difference between a peer that *claims* a name and a peer that
//! *proves* one. The cases below are the ways the proof goes wrong: an untrusted issuer, a window
//! that has not started, one that has ended, a certificate for somebody else, a rotation with a
//! gap, and an old certificate retired too early.

use meshkv::cluster::NodeId;
use meshkv::error::Error;
use meshkv::security::{Certificate, Identity, Rotation, TrustStore, authenticate, verify};

fn id(text: &str) -> NodeId {
    NodeId::parse(text).expect("a valid name")
}

fn store() -> TrustStore {
    TrustStore::of(["cluster-ca"])
}

fn cert(subject: &str, not_before: u64, not_after: u64) -> Certificate {
    Certificate::new(id(subject), "cluster-ca", not_before, not_after).expect("a certificate")
}

#[test]
fn a_certificate_names_its_subject_its_issuer_and_its_window() {
    let certificate = cert("n1", 100, 200);
    assert_eq!(certificate.subject().as_str(), "n1");
    assert_eq!(certificate.issuer(), "cluster-ca");
    assert_eq!(certificate.not_before(), 100);
    assert_eq!(certificate.not_after(), 200);
    assert!(certificate.covers(100), "the window includes its start");
    assert!(certificate.covers(199));
    assert!(!certificate.covers(200), "and excludes its end");

    for (not_before, not_after) in [(200, 200), (200, 100)] {
        assert!(
            Certificate::new(id("n1"), "cluster-ca", not_before, not_after).is_err(),
            "a window from {not_before} to {not_after} is empty"
        );
    }
    assert!(
        Certificate::new(id("n1"), "  ", 0, 10).is_err(),
        "a certificate names its issuer"
    );
}

#[test]
fn a_certificate_from_a_trusted_issuer_inside_its_window_is_an_identity() {
    let identity: Identity = verify(&cert("n2", 100, 200), &store(), 150).expect("it verifies");
    assert_eq!(identity.subject().as_str(), "n2");
}

#[test]
fn a_certificate_from_an_issuer_the_node_does_not_trust_is_refused() {
    let stranger = Certificate::new(id("n2"), "other-ca", 100, 200).expect("a certificate");
    let error = verify(&stranger, &store(), 150).expect_err("a stranger is a stranger");
    let message = error.to_string();
    assert!(message.contains("other-ca"), "{message}");
    assert!(message.contains("trust"), "{message}");

    let empty = TrustStore::empty();
    assert!(
        verify(&cert("n2", 100, 200), &empty, 150).is_err(),
        "a store that believes nobody believes nobody"
    );
}

#[test]
fn a_certificate_is_refused_before_its_window_and_after_it() {
    let certificate = cert("n2", 100, 200);
    let early = verify(&certificate, &store(), 99).expect_err("not valid yet");
    assert!(early.to_string().contains("not valid until"), "{early}");
    let late = verify(&certificate, &store(), 200).expect_err("already expired");
    assert!(late.to_string().contains("expired"), "{late}");
}

#[test]
fn a_certificate_that_verifies_for_somebody_else_does_not_authenticate_this_peer() {
    // The peer claims n3 and presents n2's (perfectly valid) certificate.
    let error = authenticate(&id("n3"), &cert("n2", 100, 200), &store(), 150)
        .expect_err("a valid certificate for somebody else proves nothing about this peer");
    let message = error.to_string();
    assert!(
        message.contains("n3") && message.contains("n2"),
        "{message}"
    );

    // The same certificate does authenticate the peer it belongs to.
    let identity = authenticate(&id("n2"), &cert("n2", 100, 200), &store(), 150)
        .expect("the certificate and the claim agree");
    assert_eq!(identity.subject().as_str(), "n2");
}

#[test]
fn a_rotation_presents_the_certificate_that_is_valid_now() {
    let rotation = Rotation::new(cert("n1", 100, 200), cert("n1", 200, 300)).expect("a rotation");

    assert_eq!(
        rotation.presented(150).not_before(),
        100,
        "before the new certificate is valid, the old one is presented"
    );
    assert_eq!(
        rotation.presented(200).not_before(),
        200,
        "the moment the new one is valid, it is the one presented"
    );
    assert_eq!(rotation.presented(299).not_before(), 200);
}

#[test]
fn a_rotation_into_another_name_is_refused() {
    let error = Rotation::new(cert("n1", 100, 200), cert("n2", 200, 300))
        .expect_err("a node cannot rotate into somebody else's identity");
    assert!(
        error.to_string().contains("n1") && error.to_string().contains("n2"),
        "{error}"
    );
}

#[test]
fn a_node_accepts_a_peer_still_on_the_old_certificate_until_the_overlap_ends() {
    let rotation = Rotation::new(cert("n1", 100, 500), cert("n1", 200, 600)).expect("a rotation");

    // During the overlap both certificates are valid, and both are accepted.
    assert!(
        rotation.accepts(&cert("n2", 100, 500), &store(), 250),
        "the old one"
    );
    assert!(
        rotation.accepts(&cert("n2", 200, 600), &store(), 250),
        "and the new one"
    );

    // The instant the old certificate expires, it is refused — which is why the retirement has to
    // wait for every peer to have the new one.
    assert!(!rotation.accepts(&cert("n2", 100, 500), &store(), 500));
    assert!(rotation.accepts(&cert("n2", 200, 600), &store(), 550));
}

#[test]
fn the_earliest_safe_retirement_is_the_new_certificate_plus_the_overlap() {
    let rotation = Rotation::new(cert("n1", 0, 100), cert("n1", 100, 400)).expect("a rotation");
    assert_eq!(
        rotation.earliest_safe_retirement(50),
        150,
        "retiring before the distribution finished is how a rotation becomes an outage"
    );
    assert!(
        rotation.earliest_safe_retirement(50) < rotation.next().not_after(),
        "and the overlap has to fit inside the new certificate's window"
    );
}

#[test]
fn the_identity_type_is_the_only_way_to_carry_a_verified_peer() {
    // There is no constructor: an `Identity` comes out of `verify` or `authenticate` and nowhere
    // else. This case is a reminder in executable form — the compile error is the check.
    let identity = verify(&cert("n2", 0, 10), &store(), 5).expect("it verifies");
    assert_eq!(identity.subject().as_str(), "n2");

    // A refused certificate produces no identity at all: the `Result` is the boundary.
    let refused: Result<Identity, Error> = verify(&cert("n2", 100, 200), &store(), 0);
    assert!(refused.is_err());
}
