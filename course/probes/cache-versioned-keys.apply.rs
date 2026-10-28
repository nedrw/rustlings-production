//! probe = "cache/versioned-keys@apply"
//! title = "Move the prefix, not the entries"
//! hint = "Derive the entry key from namespace, version and key — and ask the store with the caller's key, because the store knows nothing about versions."
//!
//! Step `cache/6.5-versioned-keys` invalidates a whole namespace with one write: the version is part
//! of the cache key, so bumping it makes every entry of the namespace unreachable without a scan.
//! The subtlety the tests pin is whose key is versioned: the cache's, not the store's.
//!
//! ## What you write
//!
//! [`Versioned::get`] — a valid entry under the derived key is a hit; otherwise fetch with the
//! caller's key, keep the answer under the derived key, and answer it.
//!
//! ## What the tests ask
//!
//! A second read in the same version is a hit; a bump fetches again and sees the new value; a bump
//! does not shrink the entry map; another namespace is unaffected; and the same key in two
//! namespaces is two entries.

use std::collections::BTreeMap;

/// A cache whose entries carry a namespace version.
#[derive(Debug)]
pub struct Versioned {
    versions: BTreeMap<String, u64>,
    entries: BTreeMap<String, (String, u64)>,
    ttl: u64,
}

impl Versioned {
    /// A versioned cache whose entries live for `ttl` seconds.
    #[must_use]
    pub fn new(ttl: u64) -> Self {
        Self {
            versions: BTreeMap::new(),
            entries: BTreeMap::new(),
            ttl,
        }
    }

    /// The namespace's current version.
    #[must_use]
    pub fn version(&self, namespace: &str) -> u64 {
        self.versions.get(namespace).copied().unwrap_or(0)
    }

    /// Make every entry of `namespace` unreachable.
    pub fn bump(&mut self, namespace: &str) {
        *self.versions.entry(namespace.to_owned()).or_insert(0) += 1;
    }

    /// How many entries are held, reachable or not.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when no entry is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Read `key` in `namespace`, fetching through `fetch` on a miss.
    pub fn get<F>(&mut self, namespace: &str, key: &str, now: u64, mut fetch: F) -> Option<String>
    where
        F: FnMut(&str) -> Option<String>,
    {
        todo!("derive the cache key from the namespace's version; fetch with the caller's key")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fetch_calls() -> std::cell::Cell<u32> {
        std::cell::Cell::new(0)
    }

    fn fetch(
        calls: &std::cell::Cell<u32>,
        value: String,
    ) -> impl FnMut(&str) -> Option<String> + '_ {
        move |_key: &str| {
            calls.set(calls.get() + 1);
            Some(value.clone())
        }
    }

    #[test]
    fn a_second_read_in_the_same_version_is_a_hit() {
        let mut cache = Versioned::new(60);
        let calls = fetch_calls();
        assert_eq!(
            cache.get("subs", "a", 0, fetch(&calls, "v1".to_owned())),
            Some("v1".to_owned())
        );
        assert_eq!(
            cache.get("subs", "a", 1, fetch(&calls, "v1".to_owned())),
            Some("v1".to_owned())
        );
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn a_bump_fetches_again_and_sees_the_new_value() {
        let mut cache = Versioned::new(60);
        let calls = fetch_calls();
        cache.get("subs", "a", 0, fetch(&calls, "old".to_owned()));
        cache.bump("subs");
        assert_eq!(
            cache.get("subs", "a", 1, fetch(&calls, "new".to_owned())),
            Some("new".to_owned())
        );
        assert_eq!(calls.get(), 2, "the old entry was still reachable");
    }

    #[test]
    fn a_bump_does_not_shrink_the_map() {
        let mut cache = Versioned::new(60);
        let calls = fetch_calls();
        cache.get("subs", "a", 0, fetch(&calls, "v1".to_owned()));
        let before = cache.len();
        cache.bump("subs");
        assert_eq!(cache.len(), before, "the bump swept the cache");
    }

    #[test]
    fn another_namespace_is_unaffected() {
        let mut cache = Versioned::new(60);
        let calls = fetch_calls();
        cache.get("subs", "a", 0, fetch(&calls, "v1".to_owned()));
        cache.get("news", "a", 0, fetch(&calls, "v1".to_owned()));
        cache.bump("subs");
        assert_eq!(
            cache.get("news", "a", 1, fetch(&calls, "v1".to_owned())),
            Some("v1".to_owned())
        );
        assert_eq!(calls.get(), 2, "bumping one namespace dropped another");
    }

    #[test]
    fn the_same_key_in_two_namespaces_is_two_entries() {
        let mut cache = Versioned::new(60);
        let calls = fetch_calls();
        cache.get("subs", "a", 0, fetch(&calls, "subs-value".to_owned()));
        cache.get("news", "a", 0, fetch(&calls, "news-value".to_owned()));
        assert_eq!(cache.len(), 2);
        assert_eq!(
            cache.get("subs", "a", 1, fetch(&calls, "x".to_owned())),
            Some("subs-value".to_owned())
        );
        assert_eq!(
            cache.get("news", "a", 1, fetch(&calls, "x".to_owned())),
            Some("news-value".to_owned())
        );
        assert_eq!(calls.get(), 2);
    }
}
