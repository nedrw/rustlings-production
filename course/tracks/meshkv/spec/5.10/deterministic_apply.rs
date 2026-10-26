//! What `meshkv/5.10-deterministic-state-machine` has to be true about.
//!
//! The step's subject is one sentence — the state is a function of the entries and of nothing else
//! — and the cases below are the ways it stops being true: a command that only partly means
//! something, an apply out of order, an apply twice, a digest that depends on how the map was
//! built, and a compare-and-set that writes anyway.

use meshkv::error::Error;
use meshkv::kv::{Applied, Command, Kv};

fn set(key: &str, value: &[u8]) -> Vec<u8> {
    Command::Set {
        key: key.to_owned(),
        value: value.to_vec(),
    }
    .encode()
}

fn cas(key: &str, expect: &[u8], value: &[u8]) -> Vec<u8> {
    Command::Cas {
        key: key.to_owned(),
        expect: expect.to_vec(),
        value: value.to_vec(),
    }
    .encode()
}

fn delete(key: &str) -> Vec<u8> {
    Command::Delete {
        key: key.to_owned(),
    }
    .encode()
}

/// A state with three writes applied, at index 3.
fn populated() -> Kv {
    let mut kv = Kv::new();
    kv.apply(1, &set("a", b"1")).expect("applied");
    kv.apply(2, &set("b", b"2")).expect("applied");
    kv.apply(3, &set("c", b"3")).expect("applied");
    kv
}

#[test]
fn a_command_parses_and_round_trips() {
    for command in [
        set("a", b"1"),
        cas("a", b"1", b"2"),
        delete("a"),
        set("k", &[0x00, 0xff, 0x0a]),
    ] {
        let parsed = Command::parse(&command).expect("a command this build wrote");
        assert_eq!(
            parsed.encode(),
            command,
            "the wire form survived the round trip"
        );
    }

    for bad in [
        &b""[..],
        b"get a",
        b"set",
        b"set a",
        b"set a zz",
        b"set a 1 extra",
        b"cas a 1",
        b"delete",
        b"delete a b",
        b"set  \x01",
    ] {
        assert!(
            Command::parse(bad).is_err(),
            "`{}` is not a command",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn a_set_is_read_back() {
    let mut kv = Kv::new();
    let applied = kv.apply(1, &set("a", b"1")).expect("applied");
    assert_eq!(
        applied,
        Applied::Written {
            value: b"1".to_vec()
        }
    );
    assert_eq!(kv.get("a"), Some(&b"1"[..]));
    assert_eq!(kv.len(), 1);
    assert!(!kv.is_empty());
    assert_eq!(kv.applied_index(), 1);
    assert!(kv.get("nobody").is_none());
}

#[test]
fn a_compare_and_set_writes_only_when_the_value_matches() {
    let mut kv = populated();

    // The value matches: it is replaced.
    let applied = kv.apply(4, &cas("a", b"1", b"9")).expect("applied");
    assert_eq!(
        applied,
        Applied::Written {
            value: b"9".to_vec()
        }
    );
    assert_eq!(kv.get("a"), Some(&b"9"[..]));

    // It does not: nothing is written, and the answer says what is there.
    let applied = kv.apply(5, &cas("a", b"1", b"8")).expect("applied");
    assert_eq!(
        applied,
        Applied::Mismatch {
            holding: Some(b"9".to_vec())
        }
    );
    assert_eq!(
        kv.get("a"),
        Some(&b"9"[..]),
        "a failed compare-and-set changes nothing"
    );

    // A missing key does not match either.
    let applied = kv.apply(6, &cas("ghost", b"0", b"1")).expect("applied");
    assert_eq!(applied, Applied::Mismatch { holding: None });
    assert!(kv.get("ghost").is_none());
}

#[test]
fn a_delete_removes_and_reports_whether_it_did() {
    let mut kv = populated();
    assert_eq!(
        kv.apply(4, &delete("a")).expect("applied"),
        Applied::Removed
    );
    assert!(kv.get("a").is_none());
    assert_eq!(kv.len(), 2);
    assert_eq!(
        kv.apply(5, &delete("a")).expect("applied"),
        Applied::Absent,
        "deleting what is not there is a named answer, not an error"
    );
}

#[test]
fn a_state_machine_applies_a_prefix_and_refuses_a_gap_or_a_repeat() {
    let mut kv = populated();

    let gap = kv
        .apply(5, &set("d", b"4"))
        .expect_err("entry 5 cannot follow 3");
    assert!(gap.to_string().contains("in order"), "{gap}");

    let repeat = kv
        .apply(3, &set("c", b"3"))
        .expect_err("3 has already been applied");
    assert!(repeat.to_string().contains("in order"), "{repeat}");

    let backwards = kv.apply(1, &set("a", b"1")).expect_err("1 is behind");
    assert!(backwards.to_string().contains("in order"), "{backwards}");
    assert_eq!(kv.applied_index(), 3, "a refused apply changes nothing");
}

#[test]
fn two_machines_fed_the_same_entries_hold_the_same_state() {
    let entries = [
        set("b", b"2"),
        set("a", b"1"),
        cas("a", b"1", b"10"),
        delete("b"),
        set("c", b"3"),
    ];
    let mut first = Kv::new();
    let mut second = Kv::new();
    for (offset, entry) in entries.iter().enumerate() {
        let index = offset as u64 + 1;
        first.apply(index, entry).expect("applied");
        second.apply(index, entry).expect("applied");
    }
    assert_eq!(
        first.digest(),
        second.digest(),
        "the same entries, the same state"
    );
    assert_eq!(first, second);
}

#[test]
fn the_digest_does_not_depend_on_the_order_the_keys_were_inserted_in() {
    // Two machines, the same *set* of writes, but the log order differs. The maps hold the same
    // keys, so the digests agree — which is the property the map's ordering buys: the digest is a
    // function of the contents, not of the insertion history.
    let mut first = Kv::new();
    first.apply(1, &set("a", b"1")).expect("applied");
    first.apply(2, &set("b", b"2")).expect("applied");

    let mut second = Kv::new();
    second.apply(1, &set("b", b"2")).expect("applied");
    second.apply(2, &set("a", b"1")).expect("applied");

    assert_eq!(
        first.digest(),
        second.digest(),
        "the digest walks the keys in order, so the insertion order does not matter"
    );
}

#[test]
fn a_different_state_has_a_different_digest() {
    let mut first = populated();
    let mut second = populated();
    second.apply(4, &set("a", b"changed")).expect("applied");
    assert_ne!(first.digest(), second.digest());

    first.apply(4, &set("a", b"changed")).expect("applied");
    assert_eq!(
        first.digest(),
        second.digest(),
        "and the same state matches again"
    );
}

#[test]
fn a_payload_that_is_not_a_command_is_refused_without_touching_the_state() {
    let mut kv = populated();
    let before = kv.digest();
    let error = kv
        .apply(4, b"not a command")
        .expect_err("a payload that is not a command");
    assert!(matches!(error, Error::Protocol { .. }), "{error}");
    assert_eq!(kv.digest(), before);
    assert_eq!(kv.applied_index(), 3, "and the index did not move");
}
