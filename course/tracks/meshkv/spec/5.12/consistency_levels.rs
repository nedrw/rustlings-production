//! What `meshkv/5.12-consistency-levels` has to be true about.
//!
//! The step's subject is the dial, and the cases below are the two ways a dial goes wrong: a level
//! that is silently downgraded (the caller thinks it got a promise it did not), and a level that is
//! refused for a reason the caller cannot act on.

use meshkv::cluster::NodeId;
use meshkv::consistency::{Answer, Level, answer};

fn id(text: &str) -> NodeId {
    NodeId::parse(text).expect("a valid name")
}

#[test]
fn a_level_parses_and_renders() {
    assert_eq!(
        Level::parse("linearizable").expect("a level"),
        Level::Linearizable
    );
    assert_eq!(Level::parse("local").expect("a level"), Level::Local);
    assert_eq!(
        Level::parse("bounded-stale:12").expect("a level"),
        Level::BoundedStale { max_lag: 12 }
    );
    for text in [
        "",
        "strong",
        "bounded-stale",
        "bounded-stale:x",
        "bounded-stale:-1",
    ] {
        assert!(Level::parse(text).is_err(), "`{text}` is not a level");
    }
    for level in [
        Level::Linearizable,
        Level::BoundedStale { max_lag: 3 },
        Level::Local,
    ] {
        assert_eq!(Level::parse(&level.render()).expect("round trip"), level);
    }
    assert!(Level::Linearizable.is_linearizable());
    assert!(!Level::Local.is_linearizable());
}

#[test]
fn a_follower_refuses_a_linearizable_read_and_names_the_leader() {
    let served = answer(
        Level::Linearizable,
        false,
        None,
        10,
        Some(12),
        Some(&id("n1")),
    );
    match served {
        Answer::Refused { reason } => assert!(reason.contains("n1"), "{reason}"),
        Answer::Served { index } => {
            panic!("a follower cannot serve a linearizable read, and this one answered at {index}")
        }
    }
}

#[test]
fn a_leader_without_a_lease_refuses_a_linearizable_read() {
    let served = answer(Level::Linearizable, true, None, 10, Some(10), None);
    match served {
        Answer::Refused { reason } => assert!(reason.contains("lease"), "{reason}"),
        Answer::Served { index } => {
            panic!("a leader without a lease answered at {index}")
        }
    }
}

#[test]
fn a_leader_with_a_lease_serves_at_the_lease_index() {
    let served = answer(Level::Linearizable, true, Some(7), 9, Some(9), None);
    assert_eq!(
        served,
        Answer::Served { index: 7 },
        "the index is the one the lease proved, not the newer applied one"
    );
}

#[test]
fn a_bounded_stale_read_is_served_only_within_the_bound() {
    // Three behind, and the level allows three: served, at the node's own index.
    let served = answer(
        Level::BoundedStale { max_lag: 3 },
        false,
        None,
        9,
        Some(12),
        Some(&id("n1")),
    );
    assert_eq!(served, Answer::Served { index: 9 });

    // Four behind, and the level allows three: refused, and the refusal carries both numbers so
    // the caller can choose another node or a looser level.
    let served = answer(
        Level::BoundedStale { max_lag: 3 },
        false,
        None,
        8,
        Some(12),
        Some(&id("n1")),
    );
    match served {
        Answer::Refused { reason } => {
            assert!(reason.contains('4') && reason.contains('3'), "{reason}");
        }
        Answer::Served { index } => {
            panic!("four entries behind exceeded a bound of three, and answered at {index}")
        }
    }
}

#[test]
fn a_bounded_stale_read_that_has_heard_of_no_leader_is_refused() {
    // The node cannot show its lag, so it cannot honour a bound: the honest answer is a refusal.
    let served = answer(
        Level::BoundedStale { max_lag: 100 },
        false,
        None,
        4,
        None,
        None,
    );
    match served {
        Answer::Refused { reason } => assert!(reason.contains("no leader"), "{reason}"),
        Answer::Served { index } => {
            panic!("a bound with no leader to measure against answered at {index}")
        }
    }
}

#[test]
fn a_local_read_is_always_served_from_the_node() {
    // However far behind, and with no leader known: a local read promises only "this node".
    let served = answer(Level::Local, false, None, 2, None, None);
    assert_eq!(served, Answer::Served { index: 2 });
    assert!(served.is_served());
}

#[test]
fn a_node_ahead_of_the_commit_it_knows_about_is_not_behind() {
    // Applied 12, commit 10 as far as this node knows: the lag is zero, not negative.
    let served = answer(
        Level::BoundedStale { max_lag: 0 },
        false,
        None,
        12,
        Some(10),
        Some(&id("n1")),
    );
    assert_eq!(served, Answer::Served { index: 12 });
}
