//! probe = "dist/deterministic-state-machine@apply"
//! title = "The state is a function of the entries, and of nothing else"
//! hint = "Decode a payload only in the three shapes the state machine writes, apply only the entry that comes exactly next, let a compare-and-set that does not match write nothing, and fold the digest over the keys in order."
//!
//! Step `meshkv/5.10-deterministic-apply` is where the log becomes a store: nine steps moved entries
//! around, and this one says what an entry *means*. The sentence it has to make true is the one
//! replicated storage rests on — **the state is a function of the entries, and of nothing else** —
//! because a leader and a follower that applied the same entries must hold the same state, and a
//! follower replaying after a restart must reach the state the leader already has.
//!
//! Four decisions, and each one has a failure behind it:
//!
//! 1. **the payload is the command, and the parse is strict.** An entry is bytes (`5.3`); the state
//!    machine is the first place they are interpreted, so a shape it does not know is a refusal
//!    rather than a best guess — a guess is a place where two builds can differ;
//! 2. **a state machine applies a prefix.** `apply` takes the index and refuses anything that is not
//!    exactly the next one: a gap, a repeat and a step backwards all leave the state and the index
//!    alone. A machine that accepted a gap would be one whose state depends on which entries arrived;
//! 3. **a compare-and-set that does not match writes nothing**, and answers with what the key holds so
//!    that the caller can decide. Writing anyway would make it a `set` with extra words;
//! 4. **the digest is over contents, not history.** Two logs that wrote the same keys in different
//!    orders reach one state and must reach one digest, so the map is ordered and the digest walks it
//!    in that order — a hash map's iteration order is the classic way to break this sentence.
//!
//! ## What you write
//!
//! * [`Command::parse`] — the three shapes, values as hex, strict in both directions: nothing
//!   missing, nothing extra, no empty key;
//! * [`Kv::apply`] — the index check, the three commands, and the refusal that changes nothing;
//! * [`Kv::digest`] — the fold over the keys in order and their values.
//!
//! [`Command::encode`], the hex helpers, [`Applied`], [`Kv::new`], [`Kv::get`], [`Kv::len`],
//! [`Kv::is_empty`] and [`Kv::applied_index`] are given: the vocabulary is the interface, and
//! re-deriving the wire form here would be transcribing the step's spec rather than re-drilling its
//! habit.
//!
//! ## What the tests ask
//!
//! Every command round-trips through [`Command::encode`] and [`Command::parse`], and a payload that
//! is not one of the three shapes — an unknown verb, a missing word, a trailing word, an empty key, a
//! value that is not whole bytes of hex, bytes that are not text — is refused. A set is read back and
//! a delete says whether there was something to remove. A compare-and-set that matches replaces the
//! value; one that does not writes nothing and answers what is there, including for a key that is not
//! there. The index is the sharp edge: entry `n + 1` applies, and `n`, `n - 1` and `n + 2` are refused
//! without moving the state, the digest or the index — and `n + 1` still applies after a refusal. Two
//! machines fed the same entries hold the same state and the same digest; one value apart is two
//! digests, and undoing the difference brings the numbers back together. Two machines that reached
//! one map through two different orders have one digest. A payload that is not a command leaves the
//! state, the digest and the index exactly where they were, and the entry after it still applies.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no log and no cluster**: entries arrive as arguments rather than as a
//! committed prefix, there is no leader to send them (`5.7`) and no snapshot to carry the state
//! (`5.4`), so what is re-checked is the function — the index rule, the compare-and-set and the
//! digest — rather than the replication that makes the function matter. The client that drives a
//! real cluster is `api/client-sdk@apply`'s file, and the harness that runs this function under a
//! seed and a bad network is `test/deterministic-simulation@design`'s.
//!
//! Missing on purpose: the empty entry a leader commits when it wins, and the snapshot. The claim
//! that no clock is reachable is also smaller here than in the step: the step's point is that the
//! module has nowhere to put a clock, and here [`Kv::apply`]'s signature has nowhere either, which is
//! the same statement made small rather than proved.

use std::collections::BTreeMap;
use std::fmt;

/// Why a payload or an entry was not applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// What was refused, phrased for a person reading a log.
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

/// One write, as the log carries it: bytes that mean something only because the state machine says
/// so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Set a key.
    Set {
        /// The key.
        key: String,
        /// The value.
        value: Vec<u8>,
    },
    /// Set a key only if it currently holds `expect`.
    Cas {
        /// The key.
        key: String,
        /// The value the key must hold.
        expect: Vec<u8>,
        /// The value to write.
        value: Vec<u8>,
    },
    /// Remove a key.
    Delete {
        /// The key.
        key: String,
    },
}

impl Command {
    /// The command a payload spells: `set k v`, `cas k expect new`, `delete k`, values as hex.
    ///
    /// # Errors
    ///
    /// [`Refusal`] when the payload is not text, is not one of the three shapes, names no key, or
    /// carries a value that is not whole bytes of hex. A guess here is a place where two builds can
    /// read one entry differently.
    pub fn parse(payload: &[u8]) -> Result<Self, Refusal> {
        let text = std::str::from_utf8(payload)
            .map_err(|_| Refusal::new("a command is text: the payload is not UTF-8"))?;
        let mut words = text.split(' ');
        let wrong = |what: &str| Refusal::new(format!("`{text}` is not a command: {what}"));
        match words.next() {
            Some("set") => {
                let key = words.next().ok_or_else(|| wrong("it names no key"))?;
                let value = words.next().ok_or_else(|| wrong("it carries no value"))?;
                if words.next().is_some() {
                    return Err(wrong("it has trailing words"));
                }
                Ok(Self::Set {
                    key: checked_key(key)?,
                    value: unhex(value).ok_or_else(|| wrong("its value is not hex"))?,
                })
            }
            Some("cas") => {
                let key = words.next().ok_or_else(|| wrong("it names no key"))?;
                let expect = words.next().ok_or_else(|| wrong("it expects no value"))?;
                let value = words.next().ok_or_else(|| wrong("it carries no value"))?;
                if words.next().is_some() {
                    return Err(wrong("it has trailing words"));
                }
                Ok(Self::Cas {
                    key: checked_key(key)?,
                    expect: unhex(expect).ok_or_else(|| wrong("what it expects is not hex"))?,
                    value: unhex(value).ok_or_else(|| wrong("its value is not hex"))?,
                })
            }
            Some("delete") => {
                let key = words.next().ok_or_else(|| wrong("it names no key"))?;
                if words.next().is_some() {
                    return Err(wrong("it has trailing words"));
                }
                Ok(Self::Delete {
                    key: checked_key(key)?,
                })
            }
            _ => Err(wrong("it is not `set`, `cas` or `delete`")),
        }
    }

    /// The payload this command spells: the only thing the log carries.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Set { key, value } => format!("set {key} {}", hex(value)).into_bytes(),
            Self::Cas { key, expect, value } => {
                format!("cas {key} {} {}", hex(expect), hex(value)).into_bytes()
            }
            Self::Delete { key } => format!("delete {key}").into_bytes(),
        }
    }
}

/// What applying one entry did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The key now holds this value.
    Written {
        /// The value it holds.
        value: Vec<u8>,
    },
    /// The key was removed.
    Removed,
    /// The key was not there to remove, which is an answer about the log and not an error.
    Absent,
    /// A compare-and-set did not match: the state is unchanged, and this is what the key holds.
    Mismatch {
        /// The value the key holds now.
        holding: Option<Vec<u8>>,
    },
}

/// The state: an ordered map from keys to values, plus the last index applied.
///
/// Ordered, not hashed: [`Kv::digest`] walks the keys in order, so two nodes holding the same data
/// produce the same digest whatever order the entries arrived in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Kv {
    entries: BTreeMap<String, Vec<u8>>,
    applied: u64,
}

impl Kv {
    /// An empty state, at index zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The index of the last entry applied: `0` when the log has contributed nothing yet.
    #[must_use]
    pub fn applied_index(&self) -> u64 {
        self.applied
    }

    /// The value of `key`, if the state holds one.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&[u8]> {
        self.entries.get(key).map(Vec::as_slice)
    }

    /// How many keys the state holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when the state holds no key.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Apply the entry at `index`, and answer what it did.
    ///
    /// # Errors
    ///
    /// [`Refusal`] when `index` is not exactly the next one — a state machine applies a prefix of the
    /// log, so a gap, a repeat and a step backwards are refused rather than guessed at — and when the
    /// payload is not a command.
    pub fn apply(&mut self, index: u64, payload: &[u8]) -> Result<Applied, Refusal> {
        if index != self.applied + 1 {
            return Err(Refusal::new(format!(
                "entry {index} cannot be applied after {}: a state machine applies the log in order",
                self.applied
            )));
        }
        let command = Command::parse(payload)?;
        let applied = match command {
            Command::Set { key, value } => {
                self.entries.insert(key, value.clone());
                Applied::Written { value }
            }
            Command::Cas { key, expect, value } => match self.entries.get(&key) {
                Some(holding) if holding == &expect => {
                    self.entries.insert(key, value.clone());
                    Applied::Written { value }
                }
                holding => Applied::Mismatch {
                    holding: holding.cloned(),
                },
            },
            Command::Delete { key } => {
                if self.entries.remove(&key).is_some() {
                    Applied::Removed
                } else {
                    Applied::Absent
                }
            }
        };
        self.applied = index;
        Ok(applied)
    }

    /// A digest of the state: the keys in order and their values, as one string two replicas
    /// compare.
    ///
    /// Two nodes that applied the same entries hold the same map and answer the same string, and a
    /// node holding one value more answers a different one.
    #[must_use]
    pub fn digest(&self) -> String {
        let mut bytes = Vec::new();
        for (key, value) in &self.entries {
            bytes.extend_from_slice(key.as_bytes());
            bytes.push(b'=');
            bytes.extend_from_slice(hex(value).as_bytes());
            bytes.push(b'\n');
        }
        fold(&bytes)
    }
}

/// A key, checked: a command that names no key is not a command.
fn checked_key(text: &str) -> Result<String, Refusal> {
    if text.is_empty() {
        return Err(Refusal::new("a command names a non-empty key"));
    }
    Ok(text.to_owned())
}

/// Bytes as hex, two digits each.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Hex as bytes: whole bytes only, and `None` for anything else.
fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let raw = text.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len() / 2);
    for pair in raw.chunks(2) {
        let mut digits = pair.iter().map(|byte| (*byte as char).to_digit(16));
        let high = digits.next()??;
        let low = digits.next()??;
        bytes.push((high * 16 + low) as u8);
    }
    Some(bytes)
}

/// FNV-1a over `bytes`, as sixteen lower-case hex characters.
///
/// Given for the reason `release/artifacts@apply` gives the same one: a hand-rolled SHA-256 in a
/// course file teaches nothing while being wrong in ways nobody notices, and this digest's job is to
/// make two replicas comparable rather than to resist an attacker.
#[must_use]
pub fn fold(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The payload of a set, without spelling the wire form in every test.
    fn set(key: &str, value: &[u8]) -> Vec<u8> {
        Command::Set {
            key: key.to_owned(),
            value: value.to_vec(),
        }
        .encode()
    }

    /// The payload of a compare-and-set.
    fn cas(key: &str, expect: &[u8], value: &[u8]) -> Vec<u8> {
        Command::Cas {
            key: key.to_owned(),
            expect: expect.to_vec(),
            value: value.to_vec(),
        }
        .encode()
    }

    /// The payload of a delete.
    fn delete(key: &str) -> Vec<u8> {
        Command::Delete {
            key: key.to_owned(),
        }
        .encode()
    }

    /// A state with three sets applied, at index 3.
    fn populated() -> Kv {
        let mut kv = Kv::new();
        kv.apply(1, &set("a", b"1")).expect("entry 1 is the first");
        kv.apply(2, &set("b", b"2")).expect("entry 2 follows 1");
        kv.apply(3, &set("c", b"3")).expect("entry 3 follows 2");
        kv
    }

    #[test]
    fn a_command_round_trips_and_a_payload_that_is_not_one_is_refused() {
        for command in [
            set("a", b"1"),
            cas("a", b"1", b"2"),
            delete("a"),
            set("k", &[0x00, 0xff, 0x0a]),
        ] {
            let parsed = Command::parse(&command).expect("a payload this build wrote");
            assert_eq!(
                parsed.encode(),
                command,
                "the wire form survives the round trip, so two replicas encoding one command agree"
            );
        }
        for bad in [
            &b""[..],
            b"get a",
            b"set",
            b"set a",
            b"set a zz",
            b"set a 1 extra",
            b"set  11",
            b"cas a 1",
            b"cas a zz 1",
            b"delete",
            b"delete a b",
        ] {
            assert!(
                Command::parse(bad).is_err(),
                "`{}` is not one of the three shapes the state machine reads, and guessing at it \
                 would be a place where two builds read one entry differently",
                String::from_utf8_lossy(bad)
            );
        }
        assert!(
            Command::parse(&[0xff, 0xfe]).is_err(),
            "a payload that is not text cannot be a command: a build that replaced the bad bytes \
             would be reading a command nobody wrote"
        );
    }

    #[test]
    fn a_set_is_read_back_and_a_delete_says_whether_there_was_something_to_remove() {
        let mut kv = Kv::new();
        assert_eq!(kv.applied_index(), 0, "an empty state is at index zero");
        assert!(kv.is_empty());

        assert_eq!(
            kv.apply(1, &set("a", b"1")).expect("the first entry"),
            Applied::Written {
                value: b"1".to_vec()
            },
            "a set answers with the value it wrote"
        );
        assert_eq!(kv.get("a"), Some(&b"1"[..]));
        assert_eq!(kv.len(), 1);
        assert_eq!(kv.applied_index(), 1);
        assert!(
            kv.get("nobody").is_none(),
            "a key nobody wrote holds nothing: a missing key is not an empty value"
        );

        assert_eq!(
            kv.apply(2, &delete("a")).expect("the second entry"),
            Applied::Removed
        );
        assert!(kv.get("a").is_none());
        assert!(kv.is_empty());
        assert_eq!(
            kv.apply(3, &delete("a")).expect("an answer, not a refusal"),
            Applied::Absent,
            "deleting what is not there is a named answer: the entry was applied and the log says so"
        );
        assert_eq!(
            kv.applied_index(),
            3,
            "the index moves for a delete of nothing too, because the entry is in the log"
        );
    }

    #[test]
    fn a_compare_and_set_writes_only_when_the_value_matches() {
        let mut kv = populated();

        assert_eq!(
            kv.apply(4, &cas("a", b"1", b"9")).expect("entry 4"),
            Applied::Written {
                value: b"9".to_vec()
            }
        );
        assert_eq!(kv.get("a"), Some(&b"9"[..]));

        assert_eq!(
            kv.apply(5, &cas("a", b"1", b"8"))
                .expect("a mismatch is an answer"),
            Applied::Mismatch {
                holding: Some(b"9".to_vec())
            },
            "the mismatch carries what the key holds, so the caller can decide"
        );
        assert_eq!(
            kv.get("a"),
            Some(&b"9"[..]),
            "a compare-and-set that did not match wrote nothing, and `8` is nowhere in the state"
        );

        assert_eq!(
            kv.apply(6, &cas("ghost", b"0", b"1"))
                .expect("a missing key is a mismatch"),
            Applied::Mismatch { holding: None },
            "a key that is not there does not match either, and the answer says it is missing"
        );
        assert!(
            kv.get("ghost").is_none(),
            "a compare-and-set never creates the key it was checking"
        );
        assert_eq!(
            kv.applied_index(),
            6,
            "every one of them was an entry, matching or not"
        );
    }

    #[test]
    fn only_the_entry_that_comes_exactly_next_is_applied() {
        let mut kv = populated();
        let before = kv.digest();

        for index in [5, 3, 1, 0] {
            let refusal = kv
                .apply(index, &set("d", b"4"))
                .expect_err(&format!("entry {index} cannot follow 3"));
            assert!(
                refusal.to_string().contains("in order"),
                "a state machine applies a prefix, so a gap, a repeat and a step backwards are all \
                 the same refusal and it says so: {refusal}"
            );
            assert_eq!(
                kv.applied_index(),
                3,
                "a refused entry does not move the index, however wrong it was"
            );
            assert_eq!(
                kv.digest(),
                before,
                "and it does not touch the state: entry {index} wrote nothing"
            );
        }

        assert_eq!(
            kv.apply(4, &set("d", b"4")).expect("4 is exactly next"),
            Applied::Written {
                value: b"4".to_vec()
            },
            "the refusal of 5 did not consume 4: the next index is still the one the log commits"
        );
        assert_eq!(kv.applied_index(), 4);
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
            first.apply(index, entry).expect("applied on the first");
            second.apply(index, entry).expect("applied on the second");
        }

        assert_eq!(
            first.digest(),
            second.digest(),
            "the same entries in the same order are the same state, and the digest has to say so"
        );
        for key in ["a", "b", "c", "nobody"] {
            assert_eq!(
                first.get(key),
                second.get(key),
                "`{key}` holds one value on both machines, not one value each"
            );
        }

        second.apply(6, &set("c", b"4")).expect("applied");
        assert_ne!(
            first.digest(),
            second.digest(),
            "one value apart is two states: a digest that answered the same number would not be a \
             digest of the state"
        );
        first.apply(6, &set("c", b"4")).expect("applied");
        assert_eq!(
            first.digest(),
            second.digest(),
            "and undoing the difference brings the two numbers back together"
        );
    }

    #[test]
    fn the_digest_is_over_contents_and_not_over_the_history_that_produced_them() {
        let mut forwards = Kv::new();
        forwards.apply(1, &set("a", b"1")).expect("applied");
        forwards.apply(2, &set("b", b"2")).expect("applied");

        let mut backwards = Kv::new();
        backwards.apply(1, &set("b", b"2")).expect("applied");
        backwards.apply(2, &set("a", b"1")).expect("applied");

        assert_eq!(
            forwards.digest(),
            backwards.digest(),
            "two logs that reached one map through different orders are one state: the digest walks \
             the keys in order, not the order they arrived in"
        );
        assert_eq!(
            forwards.digest().len(),
            16,
            "the digest is sixteen hex characters, so two runs compare as one string"
        );
        assert!(
            forwards
                .digest()
                .chars()
                .all(|digit| digit.is_ascii_digit() || ('a'..='f').contains(&digit)),
            "lower-case hex, so a build that printed the same number twice compares equal: {}",
            forwards.digest()
        );
        assert_ne!(
            forwards.digest(),
            fold(b""),
            "a state with two keys in it is not the empty fold: the contents reached the hash"
        );
    }

    #[test]
    fn a_payload_that_is_not_a_command_leaves_the_state_and_the_index_alone() {
        let mut kv = populated();
        let before = kv.digest();

        assert!(
            kv.apply(4, b"not a command").is_err(),
            "a payload the state machine cannot read is a refusal, not an entry that did nothing"
        );
        assert_eq!(kv.digest(), before, "and the state itself is untouched");
        assert_eq!(
            kv.applied_index(),
            3,
            "a refused payload does not move the index either, so entry 4 is still the next one"
        );
        assert_eq!(
            kv.apply(4, &set("d", b"4"))
                .expect("the refusal was not an entry"),
            Applied::Written {
                value: b"4".to_vec()
            }
        );
    }
}
