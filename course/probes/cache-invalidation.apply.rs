//! probe = "cache/invalidation@apply"
//! title = "Write the store, then take the knowledge away"
//! hint = "Store first, cache second: an entry dropped before the write is refilled from the old row, and an entry updated instead of dropped can keep a value the store never had."
//!
//! Step `cache/6.2-invalidation` is the write half of cache-aside. This probe puts the two copies in
//! front of you — a backend that counts its reads and a cache that only holds strings — and asks for
//! the one order that survives: put the value where the authority lives, then drop what the cache
//! thinks it knows.
//!
//! ## What you write
//!
//! [`write`] — `backend.put(key, value)`, then drop the cached entry for that key. Neither a cache
//! update nor a backend read belongs in the write path.
//!
//! ## What the tests ask
//!
//! The store has the new value and the cache no longer has the key; the write did not read the
//! backend; another key stays cached; a write to an uncached key is harmless; and after the write
//! the store answers the new value to the next reader.

use std::collections::BTreeMap;

/// The authority: a table of values, counting how often it was read.
#[derive(Debug, Default)]
pub struct Backend {
    data: BTreeMap<String, String>,
    reads: u64,
}

impl Backend {
    /// An empty backend.
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

    /// How many reads the backend answered.
    #[must_use]
    pub fn reads(&self) -> u64 {
        self.reads
    }
}

/// A cache that only knows how to hold and drop strings.
#[derive(Debug, Default)]
pub struct Cache {
    entries: BTreeMap<String, String>,
}

impl Cache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep a value.
    pub fn insert(&mut self, key: &str, value: &str) {
        self.entries.insert(key.to_owned(), value.to_owned());
    }

    /// Is this key cached?
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// How many keys are cached.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when nothing is cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drop a key, answering whether it was there.
    pub fn remove(&mut self, key: &str) -> bool {
        self.entries.remove(key).is_some()
    }

    /// What the cache holds for `key`, if anything.
    #[must_use]
    pub fn peek(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }
}

/// Write `value` for `key`: the store first, then the cache.
pub fn write(backend: &mut Backend, cache: &mut Cache, key: &str, value: &str) {
    todo!("store first; then drop the cached entry")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Backend, Cache) {
        let mut backend = Backend::new();
        backend.put("a", "old");
        backend.put("b", "other");
        let mut cache = Cache::new();
        cache.insert("a", "old");
        cache.insert("b", "other");
        (backend, cache)
    }

    #[test]
    fn the_store_has_the_new_value_and_the_cache_forgot_the_key() {
        let (mut backend, mut cache) = fixture();
        write(&mut backend, &mut cache, "a", "new");
        assert_eq!(backend.get("a"), Some("new".to_owned()));
        assert!(
            !cache.contains("a"),
            "the cache still holds the written key"
        );
    }

    #[test]
    fn the_write_does_not_read_the_backend() {
        let (mut backend, mut cache) = fixture();
        let reads_before = backend.reads();
        write(&mut backend, &mut cache, "a", "new");
        assert_eq!(
            backend.reads(),
            reads_before,
            "the write path read the store"
        );
    }

    #[test]
    fn another_key_stays_cached() {
        let (mut backend, mut cache) = fixture();
        write(&mut backend, &mut cache, "a", "new");
        assert_eq!(cache.len(), 1);
        assert!(cache.contains("b"));
    }

    #[test]
    fn writing_a_key_that_is_not_cached_is_harmless() {
        let (mut backend, mut cache) = fixture();
        write(&mut backend, &mut cache, "c", "fresh");
        assert_eq!(cache.len(), 2);
        assert_eq!(backend.get("c"), Some("fresh".to_owned()));
    }

    #[test]
    fn the_next_reader_sees_the_new_value() {
        let (mut backend, mut cache) = fixture();
        write(&mut backend, &mut cache, "a", "new");
        // The reader asks the cache first; the entry is gone, so the answer comes from the store.
        assert_eq!(cache.peek("a"), None);
        assert_eq!(backend.get("a"), Some("new".to_owned()));
    }
}
