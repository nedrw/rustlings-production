//! A read cache in front of a slow store.
//!
//! Every step of this track adds to the library; the binary stays a shell.
//!
//! The clock is a parameter, never a call to the system one: an expiry rule that reads the wall
//! clock is a rule no test can pin down.

// The region later scaffolds are allowed to write into (authoring.md §5). A scaffold may only
// *insert* lines inside it, and only *remove* nothing.
// kiln:scaffold:begin
// kiln:scaffold:end

use std::collections::BTreeMap;

/// The slow thing the cache protects: a table of values, and how often it was read.
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

    /// Read a value. Counted, because the whole point of the cache is that this did not happen.
    pub fn get(&mut self, key: &str) -> Option<String> {
        self.reads += 1;
        self.data.get(key).cloned()
    }

    /// How many reads the store has answered.
    #[must_use]
    pub fn reads(&self) -> u64 {
        self.reads
    }

    /// How many values the store holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// `true` when the store holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// One cached value and when it stops being valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The value that was read when the entry was filled.
    pub value: String,
    /// The first second at which the entry is no longer valid.
    pub expires_at: u64,
}

/// A read-through cache with a time-to-live per entry.
#[derive(Debug)]
pub struct Cache {
    entries: BTreeMap<String, Entry>,
    ttl: u64,
}

impl Cache {
    /// A cache whose entries live for `ttl` seconds.
    #[must_use]
    pub fn new(ttl: u64) -> Self {
        Self {
            entries: BTreeMap::new(),
            ttl,
        }
    }

    /// How many entries the cache holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when the cache holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drop an entry, if it is there. Returns whether anything was dropped.
    pub fn remove(&mut self, key: &str) -> bool {
        self.entries.remove(key).is_some()
    }

    /// Read `key`: from the cache while its entry is valid, from the store otherwise.
    ///
    /// A key the store does not have is not cached.
    pub fn get(&mut self, store: &mut Store, key: &str, now: u64) -> Option<String> {
        let _ = (&self.ttl, store, key, now);
        todo!("a valid entry is a hit; otherwise fill from the store")
    }
}

/// The version of the service.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
