//! probe = "cache/read-through@apply"
//! title = "A hit does not fetch"
//! hint = "Check the entry's deadline before the fetch: a hit that still calls the slow path is a cache that only adds latency."
//!
//! Step `cache/6.1-read-through` answers reads from a cache and asks the store only when it has to.
//! This probe is the same judgement with the slow side passed in as a closure: the cache cannot know
//! what the fetch costs, so the test counts the calls instead — and a hit that still calls it is the
//! failure the count makes visible.
//!
//! ## What you write
//!
//! [`Cache::get`] — answer from the cache while `now < expires_at`; otherwise call `fetch`, and keep
//! the answer with `expires_at = now + ttl` when there is one. `None` from `fetch` is not cached.
//!
//! ## What the tests ask
//!
//! A miss fetches once; a hit fetches nothing; the deadline second is expired; a missing key stays
//! missing (fetched again); and an expired entry is replaced by the new answer.

use std::collections::BTreeMap;

/// A cache with one time-to-live for everything it holds.
#[derive(Debug)]
pub struct Cache {
    entries: BTreeMap<String, (String, u64)>,
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

    /// Read `key` at `now`, calling `fetch` only when the cache cannot answer.
    pub fn get<F>(&mut self, key: &str, now: u64, mut fetch: F) -> Option<String>
    where
        F: FnMut(&str) -> Option<String>,
    {
        if let Some((value, expires_at)) = self.entries.get(key)
            && now < *expires_at
        {
            return Some(value.clone());
        }
        // The fetch is the slow path, and the count in the tests is what proves it was not taken.
        // A key the fetch does not have is not cached: "not there" is the answer that changes fastest.
        let value = fetch(key)?;
        self.entries
            .insert(key.to_owned(), (value.clone(), now + self.ttl));
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_miss_fetches_once_and_a_hit_fetches_nothing() {
        let mut cache = Cache::new(10);
        let calls = std::cell::Cell::new(0_u32);
        let mut fetch = |key: &str| {
            calls.set(calls.get() + 1);
            (key == "a").then(|| format!("value-{key}"))
        };

        assert_eq!(cache.get("a", 0, &mut fetch), Some("value-a".to_owned()));
        assert_eq!(calls.get(), 1);
        assert_eq!(cache.get("a", 5, &mut fetch), Some("value-a".to_owned()));
        assert_eq!(calls.get(), 1, "a hit called the slow path");
    }

    #[test]
    fn the_deadline_second_is_expired() {
        let mut cache = Cache::new(10);
        let calls = std::cell::Cell::new(0_u32);
        let mut fetch = |key: &str| {
            calls.set(calls.get() + 1);
            (key == "a").then(|| format!("value-{key}"))
        };
        cache.get("a", 0, &mut fetch);
        cache.get("a", 9, &mut fetch);
        assert_eq!(calls.get(), 1);
        cache.get("a", 10, &mut fetch);
        assert_eq!(
            calls.get(),
            2,
            "the deadline second was served from the cache"
        );
    }

    #[test]
    fn a_missing_key_is_not_cached() {
        let mut cache = Cache::new(10);
        let calls = std::cell::Cell::new(0_u32);
        let mut fetch = |key: &str| {
            calls.set(calls.get() + 1);
            (key == "a").then(|| format!("value-{key}"))
        };
        assert_eq!(cache.get("z", 0, &mut fetch), None);
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.get("z", 1, &mut fetch), None);
        assert_eq!(calls.get(), 2, "a missing key was cached");
    }

    #[test]
    fn an_expired_entry_is_replaced_by_the_new_answer() {
        let mut cache = Cache::new(10);
        let mut answer = "first".to_owned();
        cache.get("a", 0, |_| Some(answer.clone()));
        answer = "second".to_owned();
        assert_eq!(
            cache.get("a", 10, |_| Some(answer.clone())),
            Some("second".to_owned())
        );
        assert_eq!(
            cache.get("a", 11, |_| Some(answer.clone())),
            Some("second".to_owned())
        );
    }

    #[test]
    fn keys_are_independent_and_the_count_stays_honest() {
        let mut cache = Cache::new(10);
        let calls = std::cell::Cell::new(0_u32);
        let mut fetch = |key: &str| {
            calls.set(calls.get() + 1);
            Some(key.to_owned())
        };
        cache.get("a", 0, &mut fetch);
        cache.get("b", 0, &mut fetch);
        assert_eq!(calls.get(), 2);
        assert_eq!(cache.len(), 2);
    }
}
