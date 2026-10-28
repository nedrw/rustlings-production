//! probe = "cache/single-flight@apply"
//! title = "The first asker fills, the rest wait"
//! hint = "Register the key exactly once: a waiter that also registers is a second filler, and a per-key marker that is global is a single-threaded cache."
//!
//! Step `cache/6.3-single-flight` collapses the askers that arrive on one expired key: the first
//! fills, everybody else waits for that fill. This probe is the bookkeeping, with the fill itself
//! left out — the decision is the part that goes wrong when the code is buried in async plumbing.
//!
//! ## What you write
//!
//! [`Claims::begin`] — [`Claim::Fill`] and mark the key when nobody holds it; [`Claim::Wait`] when
//! somebody does, without marking it again. [`Claims::release`] is given.
//!
//! ## What the tests ask
//!
//! First fills and second waits; a release frees the key; releasing a key nobody holds is `false`;
//! two keys are independent; and ten waiters leave one claim standing.

use std::collections::BTreeSet;

/// What an asker should do about a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// Nobody holds the key: fill it.
    Fill,
    /// Somebody holds it: wait for their result.
    Wait,
}

/// The keys somebody is filling right now.
#[derive(Debug, Default)]
pub struct Claims {
    held: BTreeSet<String>,
}

impl Claims {
    /// No claims.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many keys are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// `true` when no key is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// Ask to fill `key`.
    pub fn begin(&mut self, key: &str) -> Claim {
        // `insert` answers whether the key was new — that answer *is* the decision, and it is also
        // what keeps a waiter from registering a second claim.
        if self.held.insert(key.to_owned()) {
            Claim::Fill
        } else {
            Claim::Wait
        }
    }

    /// A fill finished. `true` when the key was held.
    pub fn release(&mut self, key: &str) -> bool {
        self.held.remove(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_asker_fills_and_the_second_waits() {
        let mut claims = Claims::new();
        assert_eq!(claims.begin("a"), Claim::Fill);
        assert_eq!(claims.begin("a"), Claim::Wait);
    }

    #[test]
    fn a_release_frees_the_key() {
        let mut claims = Claims::new();
        claims.begin("a");
        assert!(claims.release("a"));
        assert_eq!(claims.begin("a"), Claim::Fill);
    }

    #[test]
    fn releasing_a_key_nobody_holds_is_false_and_leaves_it_free() {
        let mut claims = Claims::new();
        assert!(!claims.release("a"));
        assert_eq!(claims.begin("a"), Claim::Fill);
    }

    #[test]
    fn two_keys_are_independent() {
        let mut claims = Claims::new();
        assert_eq!(claims.begin("a"), Claim::Fill);
        assert_eq!(claims.begin("b"), Claim::Fill);
        assert_eq!(claims.len(), 2);
    }

    #[test]
    fn ten_waiters_leave_one_claim_standing() {
        let mut claims = Claims::new();
        claims.begin("a");
        for _ in 0..10 {
            assert_eq!(claims.begin("a"), Claim::Wait);
        }
        assert_eq!(claims.len(), 1, "a waiter was registered as a filler");
    }
}
