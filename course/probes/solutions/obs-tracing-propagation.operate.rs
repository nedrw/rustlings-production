//! probe = "obs/tracing-propagation@operate"
//! title = "Sample from the trace id, and keep the failures the head dropped"
//! hint = "Make the head decision a threshold on a value derived from the trace id, so the same id always answers the same, and let the tail keep a failing trace whatever the head said."
//!
//! Step `jobq/3.18-trace-sampling` is the operational half of propagation: a fleet that traces every
//! request drowns the collector, so most traces are dropped at the root — and the traces a policy
//! drops are exactly the ones worth having when something is wrong. Two decisions, and the first one
//! is what makes the second one testable:
//!
//! 1. **the head decision is reproducible from the trace id.** A sampler that draws a random number
//!    has two of them the moment there are two processes: the collector that decides with one and the
//!    worker that decides with the other disagree, and the trace ends up half-kept. Deriving the
//!    decision from the trace id means every process computes the same answer for the same trace, and
//!    re-running the same traffic samples the same traces — which is what makes an incident
//!    reproducible rather than a one-off;
//! 2. **the tail keeps what the head dropped when the trace failed.** The head cannot know the
//!    request was going to fail; the tail can, and it keeps a failing trace whatever the head said,
//!    so the one trace worth having is not the one that is thrown away. The tail can only *add*:
//!    nothing it does un-keeps a trace the head already kept.
//!
//! ## What you write
//!
//! * [`Sampler::head`] — the head decision, from the trace id alone;
//! * [`Sampler::keep`] — the decision at the end of the trace: the head's, or the tail's override
//!   for a failure.
//!
//! [`TraceId`], [`Sampler::new`], [`Outcome`] and [`mix`] are given. The hash is not the lesson — a
//! sampler that hashed differently would make the same point — and what is re-checked is that the
//! decision is *derived from the id* and what that buys.
//!
//! ## What the tests ask
//!
//! The same id answers the same, over a thousand ids and twice, and two fresh samplers at the same
//! rate agree on all of them — a random sampler cannot. A rate of zero keeps nothing, a rate of a
//! million keeps everything, and a rate of a tenth keeps about a tenth of a hundred thousand traces.
//! Raising the rate never drops a trace the lower rate was keeping: the decision is a threshold on
//! the id, so a rate change only ever adds. Then the tail: a failing trace is kept even when its head
//! said no, a successful one the head dropped stays dropped, and a kept trace is kept whatever
//! happens.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no collector, no subscriber and no span storage**: what is asserted
//! is the decision, on the values a real sampler would have — the trace id at the head and the
//! outcome at the tail. [`mix`] is a deterministic hash rather than the real sampler's, and the rate
//! is a knob the test sets, because the point is not which hash but that the answer is a function of
//! the id at all.
//!
//! Missing on purpose: the propagation of the decision in the `traceparent` flags is
//! `obs/tracing-propagation@apply`'s file, so a hop here inherits nothing and re-decides nothing;
//! nothing samples on a parent's decision or shares a rate across services; and slow traces — the
//! other thing a tail keeps — are this same override with a different predicate, deliberately left
//! to the reader.

/// Traces kept per million, at the head.
pub const PPM: u32 = 1_000_000;

/// A 128-bit trace id, as the propagation header carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TraceId([u8; 16]);

impl TraceId {
    /// The id these bytes spell.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The sixteen bytes, for a hash that does not care how they are spelled.
    #[must_use]
    pub fn bytes(self) -> [u8; 16] {
        self.0
    }
}

/// A deterministic value for a trace id: the head decision's only input.
///
/// Given, and not the lesson. It is an FNV-style finalizer over the id's bytes, spread well enough
/// that a rate is a rate and stable enough that two processes agree; a sampler that hashed
/// differently would make the same point. What is being checked is that the *decision* comes from
/// the id at all, and what that buys.
#[must_use]
pub fn mix(trace: TraceId) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in trace.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^= hash >> 27;
    hash = hash.wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^= hash >> 31;
    hash
}

/// How a trace ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The request or job finished cleanly.
    Ok,
    /// It failed: an error status, a panic, a dead letter.
    Failed,
}

/// The sampling policy: a head rate, and the tail override that goes with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sampler {
    /// Traces kept per million, at the head.
    pub rate_ppm: u32,
}

impl Sampler {
    /// A sampler that keeps `rate_ppm` traces per million at the head.
    #[must_use]
    pub fn new(rate_ppm: u32) -> Self {
        Self { rate_ppm }
    }

    /// The head decision for one trace, from its id alone.
    ///
    /// The same id answers the same at every process and in every run, which is the property that
    /// makes a sampled trace reproducible: a random number per process would let the collector and
    /// the worker disagree about the same trace.
    #[must_use]
    pub fn head(&self, trace: TraceId) -> bool {
        // A threshold on a value derived from the id: the same id sits on the same side of the line
        // every time, and raising the rate moves the line rather than the id.
        (mix(trace) % u64::from(PPM)) < u64::from(self.rate_ppm)
    }

    /// The decision at the end of the trace.
    ///
    /// The head's answer, or the tail's override: a trace that failed is kept whatever the head
    /// said, because the one trace worth having is the one where something went wrong.
    #[must_use]
    pub fn keep(&self, trace: TraceId, outcome: Outcome) -> bool {
        self.head(trace) || outcome == Outcome::Failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A distinct trace id per number, spread across the whole id so the hash has something to chew.
    fn id(number: u64) -> TraceId {
        let spread = number.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&spread.to_le_bytes());
        bytes[8..].copy_from_slice(&number.to_le_bytes());
        TraceId::from_bytes(bytes)
    }

    #[test]
    fn the_head_decision_comes_from_the_trace_id_alone() {
        let sampler = Sampler::new(100_000);
        for number in 0..1_000 {
            let trace = id(number);
            assert_eq!(
                sampler.head(trace),
                sampler.head(trace),
                "the same id answers the same, every time"
            );
        }

        // A second process with the same rate — a fresh `Sampler` — agrees with the first on every
        // id. This is what lets the collector and the worker make the same decision without talking
        // to each other.
        let other = Sampler::new(100_000);
        for number in 0..1_000 {
            assert_eq!(
                sampler.head(id(number)),
                other.head(id(number)),
                "two processes at the same rate decide the same trace the same way"
            );
        }
    }

    #[test]
    fn the_head_respects_the_extremes_of_the_rate() {
        let none = Sampler::new(0);
        let all = Sampler::new(PPM);
        for number in 0..1_000 {
            assert!(!none.head(id(number)), "a rate of zero keeps nothing");
            assert!(all.head(id(number)), "a rate of a million keeps everything");
        }
    }

    #[test]
    fn the_head_is_a_rate_not_a_lottery() {
        let sampler = Sampler::new(100_000);
        let kept = (0..100_000u64)
            .filter(|number| sampler.head(id(*number)))
            .count();
        assert!(
            (9_000..=11_000).contains(&kept),
            "a 10% rate keeps about a tenth of a hundred thousand traces, not {kept}"
        );
    }

    #[test]
    fn raising_the_rate_never_drops_a_trace_it_was_keeping() {
        let low = Sampler::new(100_000);
        let high = Sampler::new(300_000);
        for number in 0..10_000 {
            let trace = id(number);
            assert!(
                !low.head(trace) || high.head(trace),
                "a trace the low rate kept is one the high rate keeps: the decision is a threshold \
                 on the id, so a rate change only ever adds"
            );
        }
    }

    #[test]
    fn the_tail_keeps_a_failure_whatever_the_head_said() {
        let sampler = Sampler::new(100_000);
        let mut dropped = 0;
        for number in 0..1_000 {
            let trace = id(number);
            if sampler.head(trace) {
                assert!(sampler.keep(trace, Outcome::Ok), "a kept trace stays kept");
                assert!(
                    sampler.keep(trace, Outcome::Failed),
                    "and it is still kept when it fails: the tail never drops"
                );
            } else {
                dropped += 1;
                assert!(
                    sampler.keep(trace, Outcome::Failed),
                    "a failing trace is kept even though its head said no: this is the whole reason \
                     the tail exists"
                );
                assert!(
                    !sampler.keep(trace, Outcome::Ok),
                    "and a successful one the head dropped stays dropped"
                );
            }
        }
        assert!(
            dropped > 500,
            "the rate really did drop most of them: {dropped} of 1 000"
        );
    }

    #[test]
    fn a_failure_that_was_dropped_at_the_head_survives_the_tail() {
        // One in a million: almost everything is dropped at the head.
        let sampler = Sampler::new(1);
        let trace = (0..10_000u64)
            .map(id)
            .find(|trace| !sampler.head(*trace))
            .expect("a dropped trace exists");
        assert!(
            sampler.keep(trace, Outcome::Failed),
            "the tail is the second chance a failure gets, and it takes it"
        );
    }
}
