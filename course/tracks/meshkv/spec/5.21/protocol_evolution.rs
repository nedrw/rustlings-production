//! What `meshkv/5.21-protocol-evolution` has to be true about.
//!
//! The step's subject is two builds in one cluster. The cases below are the rule and the plan: a
//! version parses; a major cannot be mixed; a newer minor is refused by an older build; a plan that
//! crosses a major is not a rolling upgrade; and the plan's phases hold the new build back until the
//! last node.

use meshkv::cluster::NodeId;
use meshkv::protocol::{Greeting, Phase, Protocol, Upgrade, accept};

fn node(text: &str) -> NodeId {
    NodeId::parse(text).expect("a valid name")
}

fn version(text: &str) -> Protocol {
    Protocol::parse(text).expect("a version")
}

#[test]
fn a_version_parses_and_renders() {
    let one = version("1.2");
    assert_eq!(one.major, 1);
    assert_eq!(one.minor, 2);
    assert_eq!(one.render(), "1.2");
    for bad in ["", "1", "x.1", "1.y", "1.2.3"] {
        assert!(Protocol::parse(bad).is_err(), "`{bad}` is not a version");
    }
    assert!(
        version("1.10") > version("1.9"),
        "versions compare as numbers"
    );
}

#[test]
fn a_build_parses_every_minor_up_to_its_own_and_no_further() {
    assert!(version("1.3").can_parse(&version("1.1")));
    assert!(version("1.3").can_parse(&version("1.3")));
    assert!(
        !version("1.1").can_parse(&version("1.3")),
        "an older build cannot read a newer frame"
    );
    assert!(
        !version("1.3").can_parse(&version("2.0")),
        "and no major is mixed at all"
    );
}

#[test]
fn a_handshake_from_a_newer_minor_is_refused_by_an_older_build() {
    let local = version("1.1");
    let newer = Greeting::new(node("n2"), version("1.2"));
    let error = accept(local, &newer).expect_err("the older build cannot parse it");
    let message = error.to_string();
    assert!(
        message.contains("n2") && message.contains("1.2"),
        "{message}"
    );

    // The other direction works: the newer build parses the older one's frames.
    accept(version("1.2"), &Greeting::new(node("n1"), version("1.1")))
        .expect("a newer build reads an older one's frames");
}

#[test]
fn a_major_difference_is_refused_and_names_both_versions() {
    let error = accept(version("1.4"), &Greeting::new(node("n3"), version("2.0")))
        .expect_err("a major cannot be mixed in a running cluster");
    let message = error.to_string();
    assert!(
        message.contains("2.0") && message.contains("1.4"),
        "{message}"
    );
    assert!(message.contains("major"), "{message}");
}

#[test]
fn a_rolling_upgrade_holds_the_new_build_back_until_the_last_node() {
    let nodes = vec![node("n1"), node("n2"), node("n3")];
    let plan = Upgrade::new(&nodes, version("1.1"), version("1.2")).expect("a plan");
    let phases = plan.phases();
    assert_eq!(phases.len(), 3);
    assert_eq!(
        phases[0],
        Phase::Compat {
            speaking: version("1.1"),
            built: version("1.2"),
        },
        "the first upgraded node speaks the old minor while its peers are old"
    );
    assert_eq!(
        phases[2],
        Phase::Native {
            version: version("1.2")
        },
        "and the last step is where the cluster speaks the new one"
    );
}

#[test]
fn a_major_bump_is_not_a_rolling_upgrade() {
    let nodes = vec![node("n1"), node("n2")];
    let error = Upgrade::new(&nodes, version("1.9"), version("2.0"))
        .expect_err("a major bump is stop-the-world");
    assert!(error.to_string().contains("major"), "{error}");

    let backwards = Upgrade::new(&nodes, version("1.2"), version("1.1"))
        .expect_err("downgrading is not an upgrade");
    assert!(
        backwards.to_string().contains("not an upgrade"),
        "{backwards}"
    );

    let empty: Vec<NodeId> = Vec::new();
    assert!(Upgrade::new(&empty, version("1.1"), version("1.2")).is_err());
}
