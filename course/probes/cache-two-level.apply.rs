//! probe = "cache/two-level@apply"
//! title = "Who answered: local, shared, or the store"
//! hint = "The answer says which layer served it — that is the evidence the ratio needs; a shared hit is not a miss, and it refills the local layer."
//!
//! Step `cache/6.6-hot-key` puts a short-lived local cache in front of a shared one, and the
//! counters are how the hierarchy is argued about. This probe makes the layer part of the answer
//! instead of a counter: the tests assert *who* answered, which is the same fact from the outside.
//!
//! ## What you write
//!
//! [`TwoLevel::get`] — local first, then shared (refilling local), then the store (filling both).
//!
//! ## What the tests ask
//!
//! The first read is the origin; a read inside the local TTL is local; between the two TTLs it is
//! shared and the local layer is refilled; after both it is the origin again; and the origin is
//! asked exactly once across a local and a shared hit.

use std::collections::BTreeMap;

/// The slow thing behind the cache, counting its reads.
#[derive(Debug, Default)]
pub struct Store {
    data: BTreeMap<String, String>,
    reads: u64,
}

impl Store {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Write a value.
    pub fn put(&mut self, key: &str, value: &str) {
        self.data.insert(key.to_owned(), value.to_owned());
    }

    /// Read a value, counted.
    pub fn get(&mut self, key: &str) -> Option<String> {
        self.reads += 1;
        self.data.get(key).cloned()
    }

    /// How many reads the store answered.
    #[must_use]
    pub fn reads(&self) -> u64 {
        self.reads
    }
}

/// Which layer answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The local layer.
    Local(String),
    /// The shared layer.
    Shared(String),
    /// The store.
    Origin(String),
}

/// A local cache in front of a shared one.
#[derive(Debug)]
pub struct TwoLevel {
    local: BTreeMap<String, (String, u64)>,
    shared: BTreeMap<String, (String, u64)>,
    local_ttl: u64,
    shared_ttl: u64,
}

impl TwoLevel {
    /// Two layers: local entries live `local_ttl` seconds, shared ones `shared_ttl`.
    #[must_use]
    pub fn new(local_ttl: u64, shared_ttl: u64) -> Self {
        Self {
            local: BTreeMap::new(),
            shared: BTreeMap::new(),
            local_ttl,
            shared_ttl,
        }
    }

    /// Read `key` at `now`, saying which layer answered.
    pub fn get(&mut self, store: &mut Store, key: &str, now: u64) -> Option<Answer> {
        todo!("local, then shared (refilling local), then the store")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Store, TwoLevel) {
        let mut store = Store::new();
        store.put("hot", "value");
        (store, TwoLevel::new(3, 30))
    }

    #[test]
    fn the_first_read_is_the_origin() {
        let (mut store, mut cache) = fixture();
        assert_eq!(
            cache.get(&mut store, "hot", 0),
            Some(Answer::Origin("value".to_owned()))
        );
        assert_eq!(store.reads(), 1);
    }

    #[test]
    fn a_read_inside_the_local_ttl_is_local() {
        let (mut store, mut cache) = fixture();
        cache.get(&mut store, "hot", 0);
        assert_eq!(
            cache.get(&mut store, "hot", 2),
            Some(Answer::Local("value".to_owned()))
        );
        assert_eq!(store.reads(), 1);
    }

    #[test]
    fn between_the_ttls_the_shared_layer_answers() {
        let (mut store, mut cache) = fixture();
        cache.get(&mut store, "hot", 0);
        assert_eq!(
            cache.get(&mut store, "hot", 3),
            Some(Answer::Shared("value".to_owned()))
        );
        assert_eq!(store.reads(), 1, "a shared hit went to the store");
    }

    #[test]
    fn a_shared_hit_refills_the_local_layer() {
        let (mut store, mut cache) = fixture();
        cache.get(&mut store, "hot", 0);
        cache.get(&mut store, "hot", 3);
        assert_eq!(
            cache.get(&mut store, "hot", 4),
            Some(Answer::Local("value".to_owned()))
        );
    }

    #[test]
    fn after_both_ttls_the_origin_answers_again() {
        let (mut store, mut cache) = fixture();
        cache.get(&mut store, "hot", 0);
        assert_eq!(
            cache.get(&mut store, "hot", 30),
            Some(Answer::Origin("value".to_owned()))
        );
        assert_eq!(store.reads(), 2);
    }

    #[test]
    fn the_origin_is_asked_once_across_a_local_and_a_shared_hit() {
        let (mut store, mut cache) = fixture();
        cache.get(&mut store, "hot", 0);
        cache.get(&mut store, "hot", 1);
        cache.get(&mut store, "hot", 3);
        assert_eq!(store.reads(), 1);
    }
}
