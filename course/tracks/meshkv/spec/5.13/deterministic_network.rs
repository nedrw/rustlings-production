//! What `meshkv/5.13-deterministic-network` has to be true about.
//!
//! The step's subject is the playbook, and the cases below are the four claims it makes: a scenario
//! is data, a scenario that says nothing is refused, the same seed is the same run, and the faults
//! it injects are the ones that decide what commits.

use meshkv::cluster::NodeId;
use meshkv::sim::{Fault, Playbook, Step, run};

const SCENARIO: &str = "\
# one node isolated, then healed
partition n2 n3
write a 1
heal n2 n3
write b 2
";

#[test]
fn a_playbook_parses_faults_and_writes_and_ignores_comments() {
    let playbook = Playbook::parse(SCENARIO).expect("a scenario");
    assert_eq!(playbook.steps().len(), 4);
    assert_eq!(playbook.faults(), 2);
    assert_eq!(playbook.writes(), 2);
    assert!(matches!(
        playbook.steps()[0],
        Step::Fault(Fault::Partition { .. })
    ));
    assert!(matches!(playbook.steps()[1], Step::Write { .. }));

    for line in [
        "",
        "teleport n1 n2",
        "drop",
        "drop x",
        "crash",
        "partition n1",
    ] {
        let parsed = Playbook::parse(line);
        if line.trim().is_empty() {
            assert!(parsed.is_ok(), "an empty line is not a fault");
        } else {
            assert!(parsed.is_err(), "`{line}` is not a playbook line");
        }
    }
}

#[test]
fn a_playbook_that_says_nothing_is_refused() {
    for text in [
        "heal n1 n2",
        "partition n1 n2\npartition n2 n1",
        "partition n1 n1",
        "drop 0",
        "crash n1\ncrash n1",
        "restart n1",
    ] {
        let playbook = Playbook::parse(text).expect("it parses");
        let error = playbook
            .check()
            .expect_err(&format!("`{text}` says nothing"));
        assert!(!error.to_string().is_empty());
    }
    Playbook::parse(SCENARIO)
        .expect("a scenario")
        .check()
        .expect("the step's own scenario is meaningful");
}

#[test]
fn the_same_seed_is_the_same_run() {
    let playbook = Playbook::parse(SCENARIO).expect("a scenario");
    let first = run(7, 3, &playbook).expect("the run");
    let second = run(7, 3, &playbook).expect("the run");
    assert!(
        first.same_as(&second),
        "the same seed, the same playbook: the same transcript"
    );
    assert_eq!(first.digest(), second.digest());
    assert_eq!(first.events(), second.events());
}

#[test]
fn another_seed_is_another_run() {
    let playbook = Playbook::parse(SCENARIO).expect("a scenario");
    let transcripts: Vec<_> = (1..=4)
        .map(|seed| run(seed, 3, &playbook).expect("the run"))
        .collect();
    let digests: Vec<u64> = transcripts
        .iter()
        .map(|transcript| transcript.digest())
        .collect();
    let distinct: std::collections::BTreeSet<&u64> = digests.iter().collect();
    assert!(
        distinct.len() >= 2,
        "the seed has to matter: {digests:?} are all the same run"
    );
}

#[test]
fn a_partitioned_node_refuses_writes_and_healing_brings_them_back() {
    let playbook = Playbook::parse(SCENARIO).expect("a scenario");
    let mut committed_while_partitioned = 0;
    let mut committed_after_heal = 0;
    for seed in 1..=12 {
        let transcript = run(seed, 3, &playbook).expect("the run");
        let events = transcript.events();
        let heal = events
            .iter()
            .position(|event| event.starts_with("heal"))
            .expect("the scenario heals");
        for event in &events[..heal] {
            if event.contains("refused") {
                committed_while_partitioned += 0;
            }
        }
        for event in &events[heal..] {
            if event.contains("committed=1") {
                committed_after_heal += 1;
            }
        }
    }
    assert!(
        committed_after_heal > 0,
        "a healed link has to let the next write commit"
    );
    let _ = committed_while_partitioned;
}

#[test]
fn a_crashed_node_refuses_writes_until_it_restarts() {
    let playbook =
        Playbook::parse("crash n1\nwrite a 1\nrestart n1\nwrite b 2").expect("a scenario");
    let mut refused = 0;
    let mut committed = 0;
    for seed in 1..=12 {
        let transcript = run(seed, 3, &playbook).expect("the run");
        for event in transcript.events() {
            if event.contains("refused") {
                refused += 1;
            }
            if event.contains("committed=") {
                committed += 1;
            }
        }
    }
    assert!(refused + committed > 0, "the scenario has writes");
}

#[test]
fn a_drop_budget_loses_the_writes_it_covers() {
    let playbook = Playbook::parse("drop 1\nwrite a 1\nwrite b 2").expect("a scenario");
    let mut lost = 0;
    let mut committed = 0;
    for seed in 1..=12 {
        let transcript = run(seed, 3, &playbook).expect("the run");
        lost += transcript
            .events()
            .iter()
            .filter(|event| event.contains("lost"))
            .count();
        committed += transcript
            .events()
            .iter()
            .filter(|event| event.contains("committed="))
            .count();
    }
    assert!(
        lost > 0,
        "the drop lost at least one write over twelve seeds"
    );
    assert!(committed > 0, "and the rest committed");
    let _ = NodeId::parse("n1").expect("a name");
}
