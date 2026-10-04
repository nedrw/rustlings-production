//! probe = "perf/load-testing@apply"
//! title = "The shape of the curve, not the throughput number"
//! hint = "Same seed, same sample; a percentile whose rule you can state; and a curve whose p99 and backlog never fall as the producers rise."
//!
//! Step `relay/2.20-load-testing` writes down a capacity, and the number that survives a review is
//! not requests per second. It is three shapes:
//!
//! 1. **a run is reproducible.** A load test whose numbers cannot be re-drawn is a measurement of
//!    the afternoon, not of the service: the same seed has to produce the same sample, and every
//!    request of a run has to have a key of its own so that "1 000 requests" is a fact rather than
//!    a hope;
//! 2. **a percentile is a rule, not a vibe.** `p99` is not "the slowest request" and not "the
//!    99th of ten buckets": it is one stated interpolation between two ranks, and the reason to
//!    state it is that a capacity ADR is a document two people have to be able to check by hand;
//! 3. **the curve has a shape, and the shape is the finding.** p99 and backlog may stay flat, and
//!    they may rise; they may not fall as concurrency rises. A run that reports a better p99 at
//!    64 producers than at 32 measured something other than the service — a warm cache, an
//!    under-sampled window, a rigged generator — and the check that catches it belongs in the test
//!    suite rather than in the ADR's prose.
//!
//! ## What you write
//!
//! * [`Load::keys`] — every request one run sends, producer-major, none missing and none twice;
//! * [`Load::latency_ms`] and [`Load::samples`] — the sample itself, drawn from the seed: a body
//!   around [`BODY_MS`] and a tail from [`TAIL_MIN_MS`], with no sample past [`MAX_MS`];
//! * [`percentile`] — the rule, on a sorted sample;
//! * [`summarise`] — one run's p50/p95/p99, count and worst case;
//! * [`check_curve`] — the shape check, naming the two rungs a regression happened between.
//!
//! [`mix`] (the mixer the sample is drawn through) and [`Load::key`] are given: the mixer is a hash
//! rather than a random number on purpose, and it is not the lesson.
//!
//! ## What the tests ask
//!
//! One sample of 100 hand-made latencies pins the interpolation rule at p0, p50, p95, p99 and p100
//! — 50.5, 95.05 and 99.01 are numbers a reader can arrive at with a calculator, which is the point.
//! A run's keys are `producers × per_producer` distinct strings. The same seed is the same sample,
//! a different seed is a different one, and no sample leaves `BODY_MS…MAX_MS`. The generator's tail
//! is what makes a p99 mean anything, so a long run's p99 is checked to be up among the tail rather
//! than among the body. And a curve whose p99, backlog or producer count goes backwards is reported
//! with the rung it fell at.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies and no sockets: this is the arithmetic and the shape of a load test's
//! report, not a load test of this service. **The sample is drawn from `(seed, producer, sequence)`
//! as a pure function**, where the real generator seeds one RNG and shares it across producers —
//! which is a *stronger* property than the service needs (order-independent, re-drawable per
//! request) and the one that makes a sample reproducible in a test. There is no clock, no worker
//! pool and no queue: a [`Rung`] is a measured point handed in, and what the exercise is about is
//! what a report may claim about a series of them.

/// The body of the distribution, in milliseconds: what a healthy delivery costs.
pub const BODY_MS: u64 = 20;

/// How far above the body the common case sits: `BODY_MS .. BODY_MS + BODY_SPREAD_MS`.
pub const BODY_SPREAD_MS: u64 = 10;

/// One request in this many is a slow one. A generator with no tail produces a load test that
/// cannot see the number a capacity review is about.
pub const TAIL_ONE_IN: u64 = 20;

/// Where the tail starts, in milliseconds.
pub const TAIL_MIN_MS: u64 = 200;

/// Nothing this service does takes longer than this. A subscriber that holds a connection past it
/// is a deadline that fired, and the deadline is not what this table measures.
pub const MAX_MS: u64 = 2_000;

/// The mixer the sample is drawn through: a SplitMix64-style finaliser, so that two neighbouring
/// inputs are not two neighbouring outputs.
///
/// A hash rather than a random number on purpose: it makes a sample a pure function of the seed.
/// Given, not written here — the distribution is the lesson, not the mixing.
#[must_use]
pub fn mix(a: u64, b: u64) -> u64 {
    let mut z = a
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(b.wrapping_mul(0xbf58_476d_1ce4_e5b9));
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// One run of a load test: the seed it is drawn from, and how much traffic it sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Load {
    /// What the run is drawn from. The same seed is the same run, every time.
    pub seed: u64,
    /// How many producers run at once.
    pub producers: u32,
    /// How many requests each of them sends.
    pub per_producer: u32,
}

impl Load {
    /// The id of one request: unique inside a run, and the same string for the same triple.
    #[must_use]
    pub fn key(&self, producer: u32, sequence: u32) -> String {
        format!("{}-{producer}-{sequence}", self.seed)
    }

    /// How many requests this run sends: `producers × per_producer`.
    #[must_use]
    pub fn total(&self) -> u64 {
        u64::from(self.producers) * u64::from(self.per_producer)
    }

    /// Every request one run sends: all of producer `0`, then producer `1`, and so on.
    ///
    /// Producer-major rather than shuffled because the sample has to line up with the keys: a run
    /// that sent a request nobody counted is a throughput number with a hole in it.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        todo!()
    }

    /// How long the request at `(producer, sequence)` takes, in milliseconds.
    ///
    /// A pure function of the triple, so a sample is the same sample in any order and on any
    /// machine: the common case is inside the body, one request in [`TAIL_ONE_IN`] is inside the
    /// tail, and nothing reaches [`MAX_MS`].
    #[must_use]
    pub fn latency_ms(&self, producer: u32, sequence: u32) -> u64 {
        todo!()
    }

    /// The run's latencies, in [`Load::keys`] order.
    #[must_use]
    pub fn samples(&self) -> Vec<u64> {
        todo!()
    }
}

/// The p-th percentile of a sorted sample, or `None` for an empty one.
///
/// `p` is a fraction (`0.99`), and the rule is linear interpolation between the two nearest ranks —
/// rank `r = p × (n - 1)`, then `s[⌊r⌋] + (r - ⌊r⌋) × (s[⌊r⌋ + 1] - s[⌊r⌋])`. Written down because a
/// percentile is only comparable to another one computed the same way: "the 99th of 100" is 99.01
/// under this rule and 100 under the nearest-rank one, and a capacity ADR has to say which. A `p`
/// outside `0.0..=1.0` is clamped rather than refused: somebody asking for the 110th percentile
/// wants the worst case, and a panic is not a number.
#[must_use]
pub fn percentile(sorted: &[u64], p: f64) -> Option<f64> {
    todo!()
}

/// One run, summarised: the three percentiles a capacity table quotes, and the two facts that say
/// how much of a sample they came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    /// How many latencies were in the sample.
    pub count: u64,
    /// The worst one — which is not the p99, and is the number that answers "how bad can it get".
    pub max: u64,
    /// The median.
    pub p50: f64,
    /// The one the SLO is written against.
    pub p95: f64,
    /// The one that grows first when a target goes slow.
    pub p99: f64,
}

/// Summarise one run's sample, in whatever order it arrived in.
#[must_use]
pub fn summarise(samples: &[u64]) -> Option<Summary> {
    todo!()
}

/// One rung of a load test: how many producers were running, and what the run measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rung {
    /// The concurrency this point was measured at.
    pub producers: u32,
    /// The 99th percentile of that run, in milliseconds.
    pub p99_ms: f64,
    /// What was queued when the run ended.
    pub backlog: u64,
}

/// A point of the curve that went the wrong way, and the point before it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Regression {
    /// The rung the falling value was measured at.
    pub previous: Rung,
    /// The rung it fell at.
    pub rung: Rung,
    /// Which of the three fields fell: `"producers"`, `"p99_ms"` or `"backlog"`.
    pub field: &'static str,
}

/// The shape a capacity curve has to have before its numbers mean anything.
///
/// The rungs have to rise by producer count, and `p99_ms` and `backlog` may never fall: the first
/// rung that does is the answer. Flat is fine — a service that saturates and holds is the result
/// somebody wants — and a fall is not a lucky afternoon, it is a measurement that missed something.
///
/// # Errors
///
/// The first [`Regression`], naming both rungs and the field.
pub fn check_curve(rungs: &[Rung]) -> Result<(), Regression> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run big enough that its tail is a population rather than a coincidence.
    fn a_long_run(seed: u64) -> Load {
        Load {
            seed,
            producers: 8,
            per_producer: 1_250,
        }
    }

    /// The hand-computed percentiles are compared as hand-computed numbers: `0.95 × 99` is not
    /// exactly representable, so the answer is within a millionth of a millisecond rather than
    /// equal to it.
    fn near(got: Option<f64>, want: f64) -> bool {
        got.is_some_and(|value| (value - want).abs() < 1e-6)
    }

    #[test]
    fn the_percentiles_are_the_rule_the_capacity_note_states() {
        let sample: Vec<u64> = (1..=100).collect();

        assert_eq!(
            percentile(&sample, 0.0),
            Some(1.0),
            "the p0 is the fastest request"
        );
        assert_eq!(
            percentile(&sample, 1.0),
            Some(100.0),
            "the p100 is the worst one"
        );
        assert!(
            near(percentile(&sample, 0.5), 50.5),
            "rank 0.5 × 99 = 49.5, halfway between the 50th and the 51st value"
        );
        assert!(
            near(percentile(&sample, 0.95), 95.05),
            "rank 94.05: the 95th value and a twentieth of the way to the 96th"
        );
        assert!(
            near(percentile(&sample, 0.99), 99.01),
            "rank 98.01 — the nearest-rank rule would answer 100 here, and that difference is \
             why the rule is written down"
        );
        assert!(
            near(percentile(&sample, 2.0), 100.0),
            "asking for the 200th percentile is a request for the worst case"
        );

        assert_eq!(
            percentile(&[], 0.99),
            None,
            "a run with no requests has no p99 to quote"
        );
        assert_eq!(summarise(&[]), None);
    }

    #[test]
    fn one_run_sends_every_request_once_and_each_has_a_key_of_its_own() {
        let load = Load {
            seed: 7,
            producers: 4,
            per_producer: 250,
        };
        let keys = load.keys();

        assert_eq!(
            keys.len(),
            1_000,
            "four producers sending 250 each is 1 000 requests, not \"about\" 1 000"
        );
        let unique: std::collections::BTreeSet<&String> = keys.iter().collect();
        assert_eq!(
            unique.len(),
            1_000,
            "two requests sharing a key are one request"
        );
        for producer in 0..load.producers {
            for sequence in 0..load.per_producer {
                assert!(
                    keys.contains(&load.key(producer, sequence)),
                    "the request at ({producer}, {sequence}) is missing from the run"
                );
            }
        }
        assert_eq!(load.samples().len(), keys.len());
    }

    #[test]
    fn the_same_seed_is_the_same_sample_and_a_different_seed_is_a_different_one() {
        let load = a_long_run(42);
        assert_eq!(
            load.samples(),
            a_long_run(42).samples(),
            "a load test nobody can re-draw measured the afternoon"
        );
        assert_ne!(load.samples(), a_long_run(43).samples());

        let sample = load.samples();
        let (mut low, mut high) = (sample[0], sample[0]);
        for millis in &sample {
            low = low.min(*millis);
            high = high.max(*millis);
        }
        assert!(low >= BODY_MS, "nothing is delivered faster than the body");
        assert!(
            high < MAX_MS,
            "the deadline is not a latency this table reports"
        );
    }

    #[test]
    fn the_tail_is_what_makes_a_p99_a_number_worth_quoting() {
        let sample = a_long_run(42).samples();
        let summary = summarise(&sample).expect("a run has a summary");

        assert_eq!(summary.count, 10_000);
        assert!(
            summary.p50 <= (BODY_MS + BODY_SPREAD_MS) as f64,
            "most deliveries are the body, or the curve is measuring something else"
        );
        assert!(
            summary.p99 >= TAIL_MIN_MS as f64,
            "one request in {TAIL_ONE_IN} is slow, so the p99 of 10 000 requests is a slow one: \
             a generator with no tail reports a p50 in a p99's clothing"
        );
        assert!(
            summary.max >= TAIL_MIN_MS,
            "the tail is in the sample at all"
        );
        assert!(
            summary.p99 <= summary.max as f64,
            "the worst case is at or past the p99, never below it"
        );
    }

    #[test]
    fn a_curve_whose_p99_falls_is_reported_with_the_two_rungs_it_fell_between() {
        let healthy = [
            Rung {
                producers: 8,
                p99_ms: 90.0,
                backlog: 0,
            },
            Rung {
                producers: 16,
                p99_ms: 120.5,
                backlog: 3,
            },
            Rung {
                producers: 32,
                p99_ms: 120.5,
                backlog: 40,
            },
            Rung {
                producers: 64,
                p99_ms: 900.0,
                backlog: 900,
            },
        ];
        assert_eq!(
            check_curve(&healthy),
            Ok(()),
            "flat stretches and a saturated rung are a finding, not a regression"
        );
        assert_eq!(check_curve(&[]), Ok(()));

        let mut dipped = healthy;
        dipped[2].p99_ms = 61.0;
        assert_eq!(
            check_curve(&dipped),
            Err(Regression {
                previous: healthy[1],
                rung: dipped[2],
                field: "p99_ms",
            }),
            "a better p99 at more concurrency is a measurement to re-run, not a result"
        );
    }

    #[test]
    fn a_backlog_that_falls_is_reported_and_a_backlog_that_holds_is_not() {
        let mut held = [
            Rung {
                producers: 8,
                p99_ms: 90.0,
                backlog: 0,
            },
            Rung {
                producers: 16,
                p99_ms: 95.0,
                backlog: 12,
            },
            Rung {
                producers: 32,
                p99_ms: 95.0,
                backlog: 12,
            },
        ];
        assert_eq!(check_curve(&held), Ok(()));

        held[2].backlog = 11;
        assert_eq!(
            check_curve(&held),
            Err(Regression {
                previous: held[1],
                rung: held[2],
                field: "backlog",
            }),
            "a queue that got shorter while twice as much traffic arrived is a queue that was \
             drained by something other than the workers"
        );
    }

    #[test]
    fn rungs_that_do_not_rise_by_producers_are_a_report_about_the_wrong_points() {
        let rungs = [
            Rung {
                producers: 64,
                p99_ms: 900.0,
                backlog: 900,
            },
            Rung {
                producers: 32,
                p99_ms: 120.5,
                backlog: 40,
            },
        ];
        assert_eq!(
            check_curve(&rungs).map_err(|regression| regression.field),
            Err("producers"),
            "the curve is read left to right, so its points have to be in that order"
        );
    }

    #[test]
    fn a_sample_that_arrives_out_of_order_is_summarised_into_the_same_numbers() {
        let mut shuffled = a_long_run(9).samples();
        shuffled.reverse();
        assert_eq!(
            summarise(&shuffled),
            summarise(&a_long_run(9).samples()),
            "percentiles are a function of the sample, not of the order the producers finished in"
        );
    }
}
