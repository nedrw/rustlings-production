//! probe = "http/rate-limit@apply"
//! title = "A bucket per key, and a clock you are handed"
//! hint = "One bucket per key, refill from the clock you were given, and `Retry-After` is whole seconds — never zero."
//!
//! Step `relay/2.13-rate-limit` is per-tenant rate limits that cannot starve each other: a shared
//! bucket is a shared fate, because one noisy tenant empties it for everyone. So the limiter keeps
//! **one bucket per key** — one per tenant, plus one for the callers nobody can name — and a
//! request that is refused is told when to come back.
//!
//! The clock is injected. A limiter reads `Instant::now()` in the service; a probe that waits
//! seconds to watch a bucket refill is a probe nobody runs, so every rule here is in terms of the
//! `now_ms` it is handed. Milliseconds, because a rate below one request per second is a real
//! configuration and a token bucket that counts whole seconds cannot express it.
//!
//! ## The rules
//!
//! * a key nobody has asked for yet gets a **full** bucket;
//! * a request takes one token. [`Verdict::Allow`] means it came out of the bucket;
//! * a bucket refills at `per_second` tokens per second, *fractionally*: half a second at 1/s is
//!   half a token, which is not a token yet;
//! * a bucket never holds more than `capacity`: an idle tenant is not owed an unbounded burst for
//!   having been quiet;
//! * a refused request **takes nothing** — the tenant is not charged for asking again, and the
//!   bucket does not go into debt;
//! * [`Verdict::Refuse`] carries `retry_after_secs`, the whole seconds until the next token is
//!   available, **rounded up and never zero**: `Retry-After: 0` invites the client straight back
//!   into another refusal;
//! * keys are independent. Exhausting `ada` must not cost `grace` a single request — that is the
//!   starvation the step exists to prevent.
//!
//! ## What you write
//!
//! [`Limiter::new`] and [`Limiter::check`]. [`Bucket`] is given as a token count and the clock
//! reading it belongs to, which is one way to write the arithmetic and not the only one: if you
//! would rather keep a level and a rate, change the field and keep the tests.

use std::collections::BTreeMap;

/// What every key gets: a burst, and a rate to refill it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    /// How many requests a key may make at once, starting from an idle bucket.
    pub capacity: u32,
    /// How many tokens a second the bucket refills at.
    pub per_second: u32,
}

/// What one request is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// A token came out of the bucket: the request goes on.
    Allow,
    /// The bucket is empty. This is the `Retry-After` value, and it is at least 1.
    Refuse {
        /// Whole seconds until one token is available.
        retry_after_secs: u64,
    },
}

/// One key's bucket.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    /// Tokens available right now, whole and fractional.
    pub tokens: f64,
    /// The clock reading, in milliseconds, `tokens` belongs to.
    pub updated_ms: u64,
}

/// The limiter: one bucket per key, and the clock it is handed.
#[derive(Debug)]
pub struct Limiter {
    quota: Quota,
    buckets: BTreeMap<String, Bucket>,
}

impl Limiter {
    /// A limiter whose keys each get `quota`.
    #[must_use]
    pub fn new(quota: Quota) -> Self {
        Self {
            quota,
            buckets: BTreeMap::new(),
        }
    }

    /// One request from `key` at `now_ms`.
    ///
    /// The key is whatever the route decided identifies the caller — a tenant, a token, or
    /// `anonymous` for a request with no identity at all. One bucket per key is the whole point:
    /// the limiter must not know or care what the key means.
    pub fn check(&mut self, key: &str, now_ms: u64) -> Verdict {
        let quota = self.quota;
        let bucket = self.buckets.entry(key.to_owned()).or_insert(Bucket {
            // A key nobody has asked for yet starts full: the burst is part of the contract.
            tokens: f64::from(quota.capacity),
            updated_ms: now_ms,
        });

        // Refill from the clock it was handed, capped at the capacity: a tenant that was quiet for
        // a minute is owed its burst, not a minute's worth of rate.
        let elapsed_ms = now_ms.saturating_sub(bucket.updated_ms);
        bucket.tokens = (bucket.tokens + elapsed_ms as f64 / 1_000.0 * f64::from(quota.per_second))
            .min(f64::from(quota.capacity));
        bucket.updated_ms = now_ms;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Verdict::Allow;
        }

        // Whole seconds until one token exists, rounded up and never zero: `Retry-After: 0` tells
        // the client to come straight back into another refusal.
        let missing = 1.0 - bucket.tokens;
        let rate = f64::from(quota.per_second);
        let seconds = if rate > 0.0 {
            (missing / rate).ceil()
        } else {
            f64::from(u32::MAX)
        };
        Verdict::Refuse {
            retry_after_secs: seconds.max(1.0) as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Five requests at once, one more per second: small enough to count in a test by hand.
    const PER_KEY: Quota = Quota {
        capacity: 5,
        per_second: 1,
    };

    fn per_key() -> Limiter {
        Limiter::new(PER_KEY)
    }

    /// Ask `count` times at one instant and answer the verdicts, so a test says "the sixth request
    /// is refused" instead of a wall of asserts.
    fn burst(limiter: &mut Limiter, key: &str, now_ms: u64, count: usize) -> Vec<Verdict> {
        (0..count).map(|_| limiter.check(key, now_ms)).collect()
    }

    #[test]
    fn a_burst_up_to_the_bucket_is_allowed_and_the_next_request_is_told_when_to_come_back() {
        let mut limiter = per_key();
        let verdicts = burst(&mut limiter, "ada", 0, PER_KEY.capacity as usize);
        assert_eq!(
            verdicts,
            vec![Verdict::Allow; PER_KEY.capacity as usize],
            "a key with a full bucket may spend it: {verdicts:?}"
        );
        assert_eq!(
            limiter.check("ada", 0),
            Verdict::Refuse {
                retry_after_secs: 1
            },
            "the request after the burst is refused, and it is told when to come back"
        );
        assert_eq!(
            burst(&mut limiter, "ada", 0, 3),
            vec![
                Verdict::Refuse {
                    retry_after_secs: 1
                };
                3
            ],
            "a refused request is not charged: asking again at once gets the same answer, not a \
             larger one"
        );
    }

    #[test]
    fn another_key_spends_a_bucket_of_its_own() {
        let mut limiter = per_key();
        burst(&mut limiter, "ada", 0, PER_KEY.capacity as usize);
        assert!(matches!(limiter.check("ada", 0), Verdict::Refuse { .. }));

        // A second tenant, and the bucket for the callers nobody can name: both full.
        assert_eq!(
            burst(&mut limiter, "grace", 0, PER_KEY.capacity as usize),
            vec![Verdict::Allow; PER_KEY.capacity as usize],
            "a noisy tenant must not starve a quiet one: one bucket per key"
        );
        assert_eq!(
            burst(&mut limiter, "anonymous", 0, PER_KEY.capacity as usize),
            vec![Verdict::Allow; PER_KEY.capacity as usize],
            "unauthenticated callers get a bucket instead of sharing somebody else's"
        );

        // Spending those did not refill `ada` either: the buckets are not one counter.
        assert!(matches!(limiter.check("ada", 0), Verdict::Refuse { .. }));
    }

    #[test]
    fn the_clock_is_the_one_it_was_given() {
        let mut limiter = per_key();
        burst(&mut limiter, "ada", 0, PER_KEY.capacity as usize);

        // No time passed, so no token came back — however many times the limiter is asked.
        assert!(matches!(limiter.check("ada", 0), Verdict::Refuse { .. }));

        // Half a second at one token per second is half a token, which is not a token yet.
        assert_eq!(
            limiter.check("ada", 500),
            Verdict::Refuse {
                retry_after_secs: 1
            },
            "a partial refill is not a request"
        );
        assert_eq!(
            limiter.check("ada", 1_000),
            Verdict::Allow,
            "a second later there is exactly one token, and it is spent"
        );
        assert!(matches!(
            limiter.check("ada", 1_000),
            Verdict::Refuse { .. }
        ));
        assert_eq!(
            limiter.check("ada", 2_000),
            Verdict::Allow,
            "a second after the token was spent there is one more"
        );

        // And an idle bucket stops at its capacity: a tenant that was quiet for a minute is owed
        // its burst, not a minute's worth of rate.
        let mut idle = per_key();
        assert_eq!(
            burst(&mut idle, "ada", 60_000, PER_KEY.capacity as usize),
            vec![Verdict::Allow; PER_KEY.capacity as usize]
        );
        assert!(matches!(idle.check("ada", 60_000), Verdict::Refuse { .. }));
    }

    #[test]
    fn every_refusal_says_when_to_come_back_never_zero() {
        let mut limiter = Limiter::new(Quota {
            capacity: 2,
            per_second: 2,
        });
        assert_eq!(limiter.check("ada", 0), Verdict::Allow);
        assert_eq!(limiter.check("ada", 0), Verdict::Allow);

        // At two per second the next token is half a second away — and `Retry-After` is whole
        // seconds, so it is 1, not 0: a client told to come back now comes back into a refusal.
        let refused = limiter.check("ada", 0);
        assert_eq!(
            refused,
            Verdict::Refuse {
                retry_after_secs: 1
            },
            "a rate below one per second still has to answer in whole seconds"
        );

        // Half a second later the token exists, and the retry count came back down to where it
        // started: refusals did not push the bucket into debt.
        assert_eq!(limiter.check("ada", 500), Verdict::Allow);
        assert!(matches!(limiter.check("ada", 500), Verdict::Refuse { .. }));
        assert_eq!(
            limiter.check("ada", 1_000),
            Verdict::Allow,
            "one token per half second at two per second"
        );

        // Whatever the rate, a refusal never tells the caller to wait no time at all.
        let mut slow = Limiter::new(Quota {
            capacity: 1,
            per_second: 1,
        });
        assert_eq!(slow.check("ada", 0), Verdict::Allow);
        for now_ms in [0, 1, 999] {
            match slow.check("ada", now_ms) {
                Verdict::Allow => panic!("nothing was refilled at {now_ms}ms"),
                Verdict::Refuse { retry_after_secs } => assert!(
                    retry_after_secs >= 1,
                    "`Retry-After: 0` at {now_ms}ms invites the client straight back into another \
                     refusal"
                ),
            }
        }
    }
}
