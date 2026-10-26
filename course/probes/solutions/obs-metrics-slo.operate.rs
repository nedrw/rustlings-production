//! probe = "obs/metrics-slo@operate"
//! title = "Two windows, one ratio of sums, and a metric that cannot support either"
//! hint = "Sum the attempts before dividing, refuse a selection with no attempts, count a window whole only when its end is inside the span, and let both windows cross the threshold before it pages."
//!
//! Step `meshkv/5.18-cluster-observability` is the `operate` rung of this capability: a service is
//! judged by its latency and its errors, a cluster by whether its nodes agree. The shallower probes
//! already own the scrape (`obs/metrics-slo@apply`, `relay/2.18`) and the burn arithmetic and its
//! hysteresis (`obs/metrics-slo@design`, `jobq/3.17`); this file is the operator's question over a
//! *stream* of observations, and it adds the parts neither of those could:
//!
//! 1. **an SLI is a ratio of sums, not a mean of ratios.** A quiet peer with one request and one
//!    failure is 0% good; a busy one with 999 and 999 is 100%. Pooled they are 999 of 1 000, or
//!    99.9%; averaged per series they are 50%, which describes the peers and not the cluster. The
//!    denominator is what makes a ratio, so it is summed first and divided once.
//! 2. **a window with no attempts has no SLI, not a perfect one.** An idle minute is a real
//!    observation and must be accepted, but a selection whose attempts sum to zero cannot support a
//!    ratio: reporting 100% for it is how an idle cluster looks healthy through an outage. The
//!    refusal names the selection rather than inventing a denominator. The remaining budget has
//!    the same edge in its other direction: a thousand attempts with two failures at 99.9% is
//!    minus one, not zero, because overspending by one is a different release decision from
//!    breaking even.
//! 3. **a window is counted whole, and only when its end is inside the span.** An interval cannot be
//!    split without inventing counts for the part outside the span, so the selection takes whole
//!    observations whose end is strictly after the span's start. A window ending exactly at that
//!    edge is outside: the boundary is the one an alert's window arithmetic lives on.
//! 4. **two windows, and both must agree.** The fast window notices a burn; the slow one refuses to
//!    page on a minute of noise. A short window over the threshold *and* a long one over it is the
//!    rule; either alone is a blip, which is the conjunction `obs/metrics-slo@design` deliberately
//!    left out of its file.
//! 5. **a label set that grows with traffic is not a metric.** Every observation carries a series
//!    name; a new series past the cap is refused, because an SLO over a million series is a query
//!    that dies before it pages anyone, and a label taken from a request is how that happens.
//!
//! ## What you write
//!
//! * [`Ledger::push`] — one observation, admitted or refused with the reason;
//! * [`Ledger::sli_ppm`] — the whole ledger's ratio, commits over attempts, in millionths;
//! * [`Ledger::budget_left`] — the failures the objective still allows over the whole ledger,
//!   negative when it is overspent;
//! * [`Ledger::burn_ppm`] — the burn over the last `span_s` seconds, as a multiple of the budget,
//!   in millionths;
//! * [`Ledger::verdict`] — the two-window conjunction, in the order the operator reads it.
//!
//! [`Observation`], [`Totals`], [`Slo`], [`Verdict`], [`Flaw`], [`MAX_SERIES`] and [`PPM`] are
//! given. [`Slo::burn_ppm`] is `jobq/3.17`'s arithmetic, given because it is not this file's
//! subject: what is asked here is *which* observations feed it and what a selection without a
//! denominator does. The series cap and the observation's interval are the vocabulary the page and
//! the collector both use; rebuilding them would be checking a different collector.
//!
//! ## What the tests ask
//!
//! Two series, one quiet and one busy, pool to 999 000 parts per million — the mean of their two
//! ratios would be 500 000 and is the wrong answer. An idle ledger is accepted observation by
//! observation and refused as an SLI, while an idle window beside a busy one adds nothing to the
//! ratio. The budget is exact and signed: one failure in a thousand at 99.9% leaves nothing, a
//! second is minus one, a perfect thousand has one to spend, and no attempts has no budget. A run
//! that reports more commits than attempts is refused at the push, and a perfect window is not. The
//! 1025th distinct series is refused at the cap, while the same series pushed 1 034 times stays
//! one series and is still admitted. A series may not overlap itself; a window starting exactly
//! where the last ended is the normal case, and two series keep their own clocks. The span is
//! half-open: an old bad window enters the burn only when the span reaches its end, and a window
//! ending exactly at the span's start is left out. Two hot minutes in an hour are quiet; twelve of
//! them in the short and long windows page. The threshold is exact: 14 400 bad of a million at
//! 99.9% is exactly 14.4 burn, one attempt either side flips it, and the verdict requires both.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no collector, no query engine and no time-series store**. An
//! observation is an interval the query has already summed, not a cumulative counter, so the
//! counter-reset arithmetic a real exporter needs is not here; nothing scrapes, stores, or evaluates
//! a query. The real step's `metrics.rs` keeps the per-node counters this file consumes — peer
//! indexes, elections, refusals, commits — and `meshkv/5.18`'s page is about the two shapes a
//! failing node has. What is re-checked here is the SLO layer *over* those counters: pooling,
//! selection, the two-window conjunction and the refusals that stop a metric pretending it supports
//! an SLO.
//!
//! Missing on purpose: there is no per-tenant or per-node SLI (one number for the cluster is the
//! operator's first question, and slicing it is `obs/metrics-slo@apply`'s label discipline), no
//! histogram or quantile (latency is a different metric from availability, and the scrape is
//! `relay/2.18`'s), no alert routing (`Verdict` is the decision, not the page), and no retention:
//! the ledger holds what it was given, so cardinality is capped by [`MAX_SERIES`] rather than aged
//! out, and the budget is the whole stream's rather than a rolling period's — burn is the rate
//! that answers the period question. The cap's cost model is `obs/cardinality-cost@apply`, and the
//! hysteresis that keeps one incident to one page is `obs/metrics-slo@design`'s.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// How a fraction is spelled here: a millionth, so every boundary is an integer.
pub const PPM: u64 = 1_000_000;

/// The most distinct series one ledger tracks.
///
/// A cap on *names*, not on observations: one series may be observed a million times; a million
/// series observed once each is the query that dies.
pub const MAX_SERIES: usize = 1_024;

/// One interval of counts, as a scrape summed it for one series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// The series the counts belong to — the node, or the tenant, one label and no more.
    pub series: String,
    /// When the interval opened, in seconds.
    pub start_s: u64,
    /// When it closed, in seconds. Strictly after [`Observation::start_s`].
    pub end_s: u64,
    /// Writes attempted inside the interval.
    pub attempts: u64,
    /// Of those, the ones that committed.
    pub commits: u64,
}

/// The totals of a selection: what a ratio and a burn are computed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Totals {
    /// Attempts in the selection.
    pub attempts: u64,
    /// Commits in the selection.
    pub commits: u64,
}

impl Totals {
    /// The attempts that did not commit.
    #[must_use]
    pub fn bad(&self) -> u64 {
        self.attempts.saturating_sub(self.commits)
    }
}

/// The objective, and the burn that is worth a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slo {
    /// The objective in parts per million: 999 000 is "99.9% of attempts commit".
    pub objective_ppm: u64,
    /// The burn, in parts per million, at or above which a window wants to page.
    pub page_ppm: u64,
}

impl Slo {
    /// The bad fraction the objective allows, per million.
    #[must_use]
    pub fn allowed_bad_ppm(&self) -> u64 {
        PPM - self.objective_ppm
    }

    /// The burn of one selection, in millionths — [`Slo::allowed_bad_ppm`] against the observed bad
    /// fraction. `jobq/3.17`'s arithmetic, given because this file is about which windows feed it.
    ///
    /// `None` when the selection has no attempts: a ratio without a denominator is not a ratio.
    /// `Some(u64::MAX)` when the objective allows nothing and something failed.
    #[must_use]
    pub fn burn_ppm(&self, totals: &Totals) -> Option<u64> {
        if totals.attempts == 0 {
            return None;
        }
        let bad = totals.bad();
        if bad == 0 {
            return Some(0);
        }
        let allowed = self.allowed_bad_ppm();
        if allowed == 0 {
            return Some(u64::MAX);
        }
        let bad_ppm = bad.saturating_mul(PPM) / totals.attempts;
        Some(bad_ppm.saturating_mul(PPM) / allowed)
    }
}

/// What the two-window rule is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Quiet: no page is owed.
    Quiet,
    /// Firing: both windows are over the threshold.
    Firing,
}

/// Why an observation or a selection cannot support an SLO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flaw {
    /// The interval is empty or backwards.
    NotAWindow {
        /// The interval's start.
        start_s: u64,
        /// The interval's end.
        end_s: u64,
    },
    /// More commits than attempts is not a ratio.
    ImpossibleRatio {
        /// What the observation claims.
        commits: u64,
        /// Against what.
        attempts: u64,
    },
    /// A new series with no room under the cap.
    TooManySeries {
        /// How many series the ledger already holds.
        series: usize,
        /// The cap.
        cap: usize,
    },
    /// The series went backwards or overlapped itself.
    OutOfOrder {
        /// The series.
        series: String,
        /// Where the last accepted window ended.
        last_end_s: u64,
        /// Where the refused one starts.
        start_s: u64,
    },
    /// The selection has no attempts, so it has no ratio. A span of zero means the whole ledger.
    NoDenominator {
        /// The span that was asked for.
        span_s: u64,
    },
    /// The fast window is not inside the slow one.
    WindowsNotNested {
        /// The short window.
        short_s: u64,
        /// The long window.
        long_s: u64,
    },
}

impl fmt::Display for Flaw {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAWindow { start_s, end_s } => write!(
                formatter,
                "an interval from {start_s} to {end_s} covers no time, so it cannot hold traffic"
            ),
            Self::ImpossibleRatio { commits, attempts } => write!(
                formatter,
                "{commits} commits out of {attempts} attempts is not a ratio: a success is a subset of \
                 the attempts"
            ),
            Self::TooManySeries { series, cap } => write!(
                formatter,
                "the ledger holds {series} series and its cap is {cap}: a label set that grows with \
                 traffic cannot support an SLO"
            ),
            Self::OutOfOrder {
                series,
                last_end_s,
                start_s,
            } => write!(
                formatter,
                "`{series}` has a window ending at {last_end_s} and this one starts at {start_s}: \
                 overlapping windows count the same attempts twice"
            ),
            Self::NoDenominator { span_s } => write!(
                formatter,
                "the selection over {span_s} second(s) has no attempts, so a ratio would be a rumour"
            ),
            Self::WindowsNotNested { short_s, long_s } => write!(
                formatter,
                "the short window of {short_s} second(s) is not inside the long one of {long_s}: two \
                 windows are the rule, one window is not"
            ),
        }
    }
}

/// The observations a cluster's SLO is computed from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ledger {
    observations: Vec<Observation>,
    series: BTreeSet<String>,
    last_end: BTreeMap<String, u64>,
}

impl Ledger {
    /// A ledger that has seen nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many observations it holds.
    #[must_use]
    pub fn observations(&self) -> usize {
        self.observations.len()
    }

    /// Admit one observation, or refuse it with the reason.
    ///
    /// The interval must cover time, the commit count must be a subset of the attempts, a new series
    /// must fit under [`MAX_SERIES`], and a series must move forward: its next window may start
    /// exactly where the last ended and not before.
    ///
    /// # Errors
    ///
    /// One [`Flaw`] for each rule above, naming the observation's own numbers.
    pub fn push(&mut self, observation: Observation) -> Result<(), Flaw> {
        if observation.end_s <= observation.start_s {
            return Err(Flaw::NotAWindow {
                start_s: observation.start_s,
                end_s: observation.end_s,
            });
        }
        if observation.commits > observation.attempts {
            return Err(Flaw::ImpossibleRatio {
                commits: observation.commits,
                attempts: observation.attempts,
            });
        }
        let is_new = !self.series.contains(&observation.series);
        if is_new && self.series.len() == MAX_SERIES {
            return Err(Flaw::TooManySeries {
                series: self.series.len(),
                cap: MAX_SERIES,
            });
        }
        if let Some(&last_end_s) = self.last_end.get(&observation.series) {
            if observation.start_s < last_end_s {
                return Err(Flaw::OutOfOrder {
                    series: observation.series,
                    last_end_s,
                    start_s: observation.start_s,
                });
            }
        }
        self.last_end
            .insert(observation.series.clone(), observation.end_s);
        self.series.insert(observation.series.clone());
        self.observations.push(observation);
        Ok(())
    }

    /// The ratio of commits to attempts over the whole ledger, in millionths.
    ///
    /// The totals are summed across series before the division: a peer with one request does not get
    /// the same vote as a peer with a thousand.
    ///
    /// # Errors
    ///
    /// [`Flaw::NoDenominator`] with a span of `0` when the ledger's attempts sum to zero — an idle
    /// ledger is not a perfect one.
    pub fn sli_ppm(&self) -> Result<u64, Flaw> {
        let mut totals = Totals {
            attempts: 0,
            commits: 0,
        };
        for observation in &self.observations {
            totals.attempts += observation.attempts;
            totals.commits += observation.commits;
        }
        if totals.attempts == 0 {
            return Err(Flaw::NoDenominator { span_s: 0 });
        }
        Ok(totals.commits.saturating_mul(PPM) / totals.attempts)
    }

    /// The failures the whole ledger's objective still allows, negative when it is overspent.
    ///
    /// This is the step's `budget_left` put on the stream: `attempts × allowed_bad_ppm /
    /// 1_000_000 − bad`, in integers, because a release decision hangs on the exact number. An
    /// objective of 99.9% over a thousand attempts and one failure leaves nothing; a second failure
    /// is minus one, which is different from breaking even.
    ///
    /// # Errors
    ///
    /// [`Flaw::NoDenominator`] with a span of `0` when the ledger has no attempts.
    pub fn budget_left(&self, slo: &Slo) -> Result<i64, Flaw> {
        let mut totals = Totals {
            attempts: 0,
            commits: 0,
        };
        for observation in &self.observations {
            totals.attempts += observation.attempts;
            totals.commits += observation.commits;
        }
        if totals.attempts == 0 {
            return Err(Flaw::NoDenominator { span_s: 0 });
        }
        let attempts = i64::try_from(totals.attempts).expect("a count fits in i64");
        let allowed_ppm = i64::try_from(slo.allowed_bad_ppm()).expect("a fraction fits in i64");
        let million = i64::try_from(PPM).expect("a million fits in i64");
        let allowed = attempts.saturating_mul(allowed_ppm) / million;
        let spent = i64::try_from(totals.bad()).expect("a count fits in i64");
        Ok(allowed.saturating_sub(spent))
    }

    /// The burn over the last `span_s` seconds, ending at the newest end the ledger has seen.
    ///
    /// An observation is counted whole when its `end_s` is strictly after `latest − span_s`; the
    /// window that ends exactly on that edge belongs to the older span. Windows are not split:
    /// splitting one would invent counts for the part outside the span.
    ///
    /// # Errors
    ///
    /// [`Flaw::NoDenominator`] when the selection has no attempts or reaches no observation at all.
    pub fn burn_ppm(&self, slo: &Slo, span_s: u64) -> Result<u64, Flaw> {
        let Some(latest) = self
            .observations
            .iter()
            .map(|observation| observation.end_s)
            .max()
        else {
            return Err(Flaw::NoDenominator { span_s });
        };
        let floor = latest.saturating_sub(span_s);
        let mut totals = Totals {
            attempts: 0,
            commits: 0,
        };
        for observation in &self.observations {
            if observation.end_s > floor {
                totals.attempts += observation.attempts;
                totals.commits += observation.commits;
            }
        }
        slo.burn_ppm(&totals).ok_or(Flaw::NoDenominator { span_s })
    }

    /// The multi-window rule: page only when the short window and the long window are both at or
    /// over the same burn threshold.
    ///
    /// The short window is the one that notices; the long one is the one that refuses to page on a
    /// minute of noise. A short window that is not strictly inside the long one is refused, because
    /// that is not this rule — it is one window wearing two names.
    ///
    /// # Errors
    ///
    /// [`Flaw::WindowsNotNested`] when `short_s` is zero or not shorter than `long_s`, and whatever
    /// [`Ledger::burn_ppm`] refuses for either window.
    pub fn verdict(&self, slo: &Slo, short_s: u64, long_s: u64) -> Result<Verdict, Flaw> {
        if short_s == 0 || short_s >= long_s {
            return Err(Flaw::WindowsNotNested { short_s, long_s });
        }
        let short = self.burn_ppm(slo, short_s)?;
        let long = self.burn_ppm(slo, long_s)?;
        if short >= slo.page_ppm && long >= slo.page_ppm {
            Ok(Verdict::Firing)
        } else {
            Ok(Verdict::Quiet)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 99.9% good, and 14.4× the budget is worth waking somebody.
    fn slo() -> Slo {
        Slo {
            objective_ppm: 999_000,
            page_ppm: 14_400_000,
        }
    }

    /// One sixty-second interval.
    fn window(series: &str, start_s: u64, attempts: u64, commits: u64) -> Observation {
        Observation {
            series: series.to_owned(),
            start_s,
            end_s: start_s + 60,
            attempts,
            commits,
        }
    }

    /// Feed `count` one-minute windows, all good or all at `commits` of 1 000 attempts.
    fn feed(ledger: &mut Ledger, count: usize, commits: u64) {
        let first = ledger.observations() as u64;
        for index in first..first + count as u64 {
            ledger
                .push(window("n1", index * 60, 1_000, commits))
                .expect("a forward window inside the cap");
        }
    }

    #[test]
    fn the_sli_is_a_ratio_of_sums_and_a_quiet_peer_does_not_get_a_vote() {
        let mut ledger = Ledger::new();
        ledger
            .push(window("n1", 0, 1, 0))
            .expect("one attempt, one failure");
        ledger
            .push(window("n2", 0, 999, 999))
            .expect("one busy, perfect peer");

        assert_eq!(
            ledger.sli_ppm(),
            Ok(999_000),
            "999 commits of 1 000 attempts is 99.9%: the mean of the two series' ratios would say \
             50% and would describe the peers instead of the cluster"
        );

        let mut perfect = Ledger::new();
        perfect
            .push(window("n1", 0, 10, 10))
            .expect("a perfect window");
        assert_eq!(
            perfect.sli_ppm(),
            Ok(PPM),
            "every attempt committed is the whole million"
        );

        let mut failed = Ledger::new();
        failed
            .push(window("n1", 0, 10, 0))
            .expect("a failed window");
        assert_eq!(failed.sli_ppm(), Ok(0), "no commit is no good traffic");
    }

    #[test]
    fn a_selection_with_no_denominator_is_refused_not_called_perfect() {
        let mut idle = Ledger::new();
        idle.push(window("n1", 0, 0, 0))
            .expect("an idle minute is a real observation");
        assert_eq!(
            idle.sli_ppm(),
            Err(Flaw::NoDenominator { span_s: 0 }),
            "an idle cluster has no SLI; 100% would be a promise nobody tested"
        );
        assert!(
            matches!(
                idle.burn_ppm(&slo(), 60),
                Err(Flaw::NoDenominator { span_s: 60 })
            ),
            "and no attempt is no burn to measure"
        );

        let mut mixed = Ledger::new();
        mixed.push(window("n1", 0, 0, 0)).expect("an idle minute");
        mixed
            .push(window("n1", 60, 1_000, 1_000))
            .expect("a busy minute beside it");
        assert_eq!(
            mixed.sli_ppm(),
            Ok(1_000_000),
            "the idle window adds nothing to a selection that has traffic"
        );
    }

    #[test]
    fn a_run_that_reports_more_commits_than_attempts_is_not_a_ratio() {
        let mut ledger = Ledger::new();
        assert_eq!(
            ledger.push(window("n1", 0, 10, 11)),
            Err(Flaw::ImpossibleRatio {
                commits: 11,
                attempts: 10,
            }),
            "a success is a subset of the attempts, and eleven is not inside ten"
        );
        assert_eq!(
            ledger.push(window("n1", 0, 10, 10)),
            Ok(()),
            "a window where every attempt committed is still a ratio"
        );
        assert_eq!(ledger.observations(), 1, "the refused run was not kept");
    }

    #[test]
    fn a_series_that_grows_with_traffic_is_cut_off_at_the_cap() {
        let mut ledger = Ledger::new();
        for index in 0..MAX_SERIES {
            ledger
                .push(window(&format!("peer-{index}"), 0, 1, 1))
                .expect("inside the cap");
        }
        assert_eq!(
            ledger.push(window("one-too-many", 0, 1, 1)),
            Err(Flaw::TooManySeries {
                series: MAX_SERIES,
                cap: MAX_SERIES,
            }),
            "a new name past the cap is the cardinality the metric cannot afford"
        );

        let mut repeated = Ledger::new();
        for index in 0..(MAX_SERIES + 10) {
            repeated
                .push(window("acme", index as u64 * 60, 1, 1))
                .expect("the same series is not a new series");
        }
        assert_eq!(
            repeated.sli_ppm(),
            Ok(PPM),
            "a million observations of one series are one series, and the cap is about names"
        );
    }

    #[test]
    fn a_series_moves_forward_and_each_series_keeps_its_own_clock() {
        let mut ledger = Ledger::new();
        ledger
            .push(window("n1", 0, 10, 10))
            .expect("the first window");

        assert_eq!(
            ledger.push(window("n1", 59, 10, 10)),
            Err(Flaw::OutOfOrder {
                series: "n1".to_owned(),
                last_end_s: 60,
                start_s: 59,
            }),
            "an overlapping window would count the same attempts twice"
        );
        assert_eq!(
            ledger.push(window("n1", 60, 10, 10)),
            Ok(()),
            "the next window starts exactly where the last ended"
        );
        ledger
            .push(window("n2", 0, 5, 5))
            .expect("n2 has not reported yet and may start earlier");
        assert_eq!(
            ledger.push(Observation {
                series: "n1".to_owned(),
                start_s: 120,
                end_s: 120,
                attempts: 1,
                commits: 1,
            }),
            Err(Flaw::NotAWindow {
                start_s: 120,
                end_s: 120,
            }),
            "a window of no time cannot hold traffic"
        );
    }

    #[test]
    fn a_window_is_counted_whole_and_only_when_its_end_is_inside_the_span() {
        let mut ledger = Ledger::new();
        ledger
            .push(window("n1", 0, 1_000, 0))
            .expect("a bad minute");
        ledger
            .push(window("n1", 60, 1_000, 1_000))
            .expect("a good minute");
        ledger
            .push(window("n1", 120, 1_000, 1_000))
            .expect("another good one");

        assert_eq!(
            ledger.burn_ppm(&slo(), 60),
            Ok(0),
            "the last minute is good and the bad one is outside the span"
        );
        assert_eq!(
            ledger.burn_ppm(&slo(), 120),
            Ok(0),
            "the window that ends exactly at the span's start is outside it: half-open, like every \
             window in this course"
        );
        assert_eq!(
            ledger.burn_ppm(&slo(), 180),
            Ok(333_333_000),
            "once the span reaches the bad window it is counted whole: a third of the attempts are \
             bad, which is 333 times the budget"
        );
        assert!(
            matches!(
                ledger.burn_ppm(&slo(), 0),
                Err(Flaw::NoDenominator { span_s: 0 })
            ),
            "a span of no time reaches no window and has no burn"
        );
    }

    #[test]
    fn a_couple_of_hot_minutes_does_not_page_but_both_windows_over_do() {
        let mut ledger = Ledger::new();
        feed(&mut ledger, 100, 1_000);
        feed(&mut ledger, 2, 800);

        let short = ledger.burn_ppm(&slo(), 60).expect("the last minute");
        let long = ledger.burn_ppm(&slo(), 3_600).expect("the last hour");
        assert!(
            short >= slo().page_ppm,
            "the last minute is twenty percent bad, which is two hundred times the budget: {short}"
        );
        assert!(
            long < slo().page_ppm,
            "but the hour is a blip: two bad minutes among sixty are six times the budget: {long}"
        );
        assert_eq!(
            ledger.verdict(&slo(), 60, 3_600),
            Ok(Verdict::Quiet),
            "a page needs both windows, and the slow one says this is noise"
        );

        feed(&mut ledger, 10, 800);
        assert_eq!(
            ledger.verdict(&slo(), 60, 3_600),
            Ok(Verdict::Firing),
            "twelve bad minutes of the last sixty are forty times the budget in both windows"
        );
    }

    #[test]
    fn the_threshold_is_exact_and_one_attempt_either_side_moves_it() {
        let at_threshold = |commits: u64| {
            let mut ledger = Ledger::new();
            ledger
                .push(Observation {
                    series: "n1".to_owned(),
                    start_s: 0,
                    end_s: 60,
                    attempts: 1_000_000,
                    commits,
                })
                .expect("one long window");
            ledger.verdict(&slo(), 60, 120)
        };

        assert_eq!(
            at_threshold(985_600),
            Ok(Verdict::Firing),
            "14 400 bad of a million is 14.4 burn at 99.9%, and at exactly the threshold it fires"
        );
        assert_eq!(
            at_threshold(985_601),
            Ok(Verdict::Quiet),
            "one attempt less bad is 14.399 and below the threshold"
        );
        assert_eq!(
            at_threshold(985_599),
            Ok(Verdict::Firing),
            "one attempt more bad is 14.401 and above it"
        );
    }

    #[test]
    fn the_remaining_budget_is_exact_signed_and_needs_a_denominator() {
        let mut at_the_edge = Ledger::new();
        at_the_edge
            .push(window("n1", 0, 1_000, 999))
            .expect("one failure");
        assert_eq!(
            at_the_edge.budget_left(&slo()),
            Ok(0),
            "99.9% of a thousand attempts allows exactly one failure, and one was spent"
        );

        let mut overspent = Ledger::new();
        overspent
            .push(window("n1", 0, 1_000, 998))
            .expect("two failures");
        assert_eq!(
            overspent.budget_left(&slo()),
            Ok(-1),
            "a second failure overspends by one, which is not the same as breaking even"
        );

        let mut perfect = Ledger::new();
        perfect
            .push(window("n1", 0, 1_000, 1_000))
            .expect("no failures");
        assert_eq!(
            perfect.budget_left(&slo()),
            Ok(1),
            "a perfect window has the whole budget to spend"
        );

        assert_eq!(
            Ledger::new().budget_left(&slo()),
            Err(Flaw::NoDenominator { span_s: 0 }),
            "a ledger with no attempts has no budget to be left of"
        );
    }
    #[test]
    fn two_windows_have_to_be_nested() {
        let mut ledger = Ledger::new();
        ledger
            .push(window("n1", 0, 1_000, 1_000))
            .expect("a good minute");

        assert_eq!(
            ledger.verdict(&slo(), 120, 60),
            Err(Flaw::WindowsNotNested {
                short_s: 120,
                long_s: 60,
            }),
            "a fast window longer than the slow one is not the multi-window rule"
        );
        assert_eq!(
            ledger.verdict(&slo(), 60, 60),
            Err(Flaw::WindowsNotNested {
                short_s: 60,
                long_s: 60,
            }),
            "one window wearing two names is still one window"
        );
        assert_eq!(
            ledger.verdict(&slo(), 0, 60),
            Err(Flaw::WindowsNotNested {
                short_s: 0,
                long_s: 60,
            }),
            "a window of no time cannot be the fast one"
        );
        assert_eq!(
            ledger.verdict(&slo(), 59, 60),
            Ok(Verdict::Quiet),
            "a short window strictly inside the long one is the rule"
        );
    }
}
