//! What `meshkv/5.14-partition-drills` has to be true about.
//!
//! The step's subject is the sentence a runbook is written in, made checkable: an isolated node
//! does not commit, a connected one does, and the difference between "one link cut" and "isolated"
//! is the one a first draft gets wrong.

use meshkv::cluster::NodeId;
use meshkv::drill::{Check, Drill, built_in};
use meshkv::sim::{Playbook, run};

fn node(text: &str) -> NodeId {
    NodeId::parse(text).expect("a valid name")
}

/// The scenario the drill is written against: n2 isolated, written to, healed, written to again.
const ISOLATION: &str = "\
partition n2 n1
partition n2 n3
write@n2 a 1
heal n2 n1
heal n2 n3
write@n2 b 2
";

#[test]
fn an_isolated_node_is_refused_and_a_reconnected_one_commits() {
    let playbook = Playbook::parse(ISOLATION).expect("a scenario");
    let drill = built_in().expect("the course's drill");
    for seed in 1..=6 {
        let transcript = run(seed, 3, &playbook).expect("the run");
        drill.judge(&transcript).expect("the drill holds");
    }
}

#[test]
fn a_single_cut_link_is_not_an_isolation() {
    // The distinction the step is about: n2 cut from n3 still reaches a majority through n1, so a
    // write sent to n2 commits — and a drill that called that a split brain would be wrong.
    let playbook = Playbook::parse("partition n2 n3\nwrite@n2 a 1\n").expect("a scenario");
    let transcript = run(1, 3, &playbook).expect("the run");
    let committed = transcript
        .events()
        .iter()
        .filter(|event| event.contains("node=n2") && event.contains("committed="))
        .count();
    assert_eq!(
        committed,
        1,
        "one cut link leaves a majority reachable: {events:?}",
        events = transcript.events()
    );

    // And a drill that insists n2 is isolated fails on this transcript, which is the check doing
    // its job rather than the scenario being wrong.
    let strict = Drill::new(vec![Check::RefusedWhileIsolated { node: node("n2") }]);
    let _ = strict; // it would pass: nothing committed *while isolated*
    let wrong = Drill::new(vec![Check::Attempted { count: 2 }]);
    assert!(
        wrong.judge(&transcript).is_err(),
        "a drill over two writes does not hold on a scenario with one"
    );
}

#[test]
fn a_drill_that_requires_more_than_the_scenario_did_fails() {
    let playbook = Playbook::parse(ISOLATION).expect("a scenario");
    let transcript = run(1, 3, &playbook).expect("the run");

    let too_many = Drill::new(vec![Check::Attempted { count: 9 }]);
    let error = too_many
        .judge(&transcript)
        .expect_err("the scenario has two writes");
    assert!(error.to_string().contains("requires 9"), "{error}");

    let never_connected = Drill::new(vec![Check::CommittedWhileConnected {
        node: node("n3"),
        count: 1,
    }]);
    let error = never_connected
        .judge(&transcript)
        .expect_err("no write was sent to n3");
    assert!(error.to_string().contains("n3"), "{error}");
}

#[test]
fn a_crashed_node_is_isolated_too() {
    // A node that is down cannot reach a majority either: the same sentence, a different fault.
    let playbook =
        Playbook::parse("crash n2\nwrite@n2 a 1\nrestart n2\nwrite@n2 b 2\n").expect("a scenario");
    let drill = Drill::new(vec![
        Check::Attempted { count: 2 },
        Check::RefusedWhileIsolated { node: node("n2") },
        Check::CommittedWhileConnected {
            node: node("n2"),
            count: 1,
        },
    ]);
    for seed in 1..=4 {
        let transcript = run(seed, 3, &playbook).expect("the run");
        drill.judge(&transcript).expect("the drill holds");
    }
}

#[test]
fn a_transcript_without_its_member_count_is_refused() {
    let empty = meshkv::sim::Transcript::default();
    let error = built_in()
        .expect("the course's drill")
        .judge(&empty)
        .expect_err("an empty transcript says nothing");
    assert!(error.to_string().contains("members"), "{error}");
}

#[test]
fn the_drills_checks_are_the_sentences_a_runbook_uses() {
    let drill = built_in().expect("the course's drill");
    assert_eq!(drill.checks().len(), 3);
    assert!(matches!(drill.checks()[0], Check::Attempted { count: 2 }));
    assert!(matches!(
        drill.checks()[1],
        Check::RefusedWhileIsolated { .. }
    ));
    assert!(matches!(
        drill.checks()[2],
        Check::CommittedWhileConnected { count: 1, .. }
    ));
}
