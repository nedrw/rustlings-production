//! probe = "obs/metrics-slo@design"
//! title = "A budget, a burn rate, and the band that keeps the pager quiet"
//! hint = "Turn the objective into a budget and the budget into a burn rate, then let the alert fire on the high threshold and clear only below the low one."
//!
//! Step `jobq/3.17-error-budget-alerts` turns the scrape of `relay/2.18` into a policy: an objective,
//! and the rule that may wake somebody when the objective is failing. A probe cannot run a query
//! engine, so this one takes the thing a query would have produced — a series of windowed counts — as
//! data, and asks what the policy decides over it. Two decisions, and the second one is where pager
//! noise comes from:
//!
//! 1. **a budget is a fraction of the traffic, not a number of errors.** 99.9% over a million requests
//!    allows a thousand bad ones; the same objective over ten requests allows a hundredth of an
//!    error, which is none. [`Slo::budget`] is that arithmetic and [`Slo::burn_ppm`] is the other
//!    half of it: a burn rate is the observed bad fraction divided by the allowed one, so a burn of
//!    1.0 spends the whole budget over the period and a burn of 14.4 spends it in a fourteenth of it;
//! 2. **an alert is a rule with hysteresis, not a threshold.** A rule that fires the instant one
//!    window crosses the line and clears the instant it crosses back pages on noise: a series that
//!    hovers around the threshold is a page, a resolve and a page again. [`Policy`] has a high
//!    threshold to fire and a lower one to clear, and the band between them is what makes one
//!    incident one page instead of forty.
//!
//! ## What you write
//!
//! * [`Slo::budget`] — the bad requests a window of `total` is allowed to spend;
//! * [`Slo::burn_ppm`] — how fast the window spends it, in millionths: `1_000_000` is exactly on pace
//!   for the period, `14_400_000` is fourteen times too fast;
//! * [`Alert::observe`] — one window into the rule, and the decision it leaves behind.
//!
//! [`Window`], [`Policy`], [`Decision`], [`Alert::new`], [`Alert::firing`] and [`Alert::observe_all`]
//! (the loop that feeds a whole series) are given. The series is the data and the rule is the lesson;
//! walking a `&[Window]` is not.
//!
//! ## What the tests ask
//!
//! The budget arithmetic at the edges: a hundredth of a request is zero, a window with no traffic is
//! not a division by zero, and the burn is a rate — twice the bad requests is twice the burn. Then
//! the policy over a series written as data. One hot window does not page and a sustained run does; a
//! series that alternates across the fire threshold but stays inside the band pages exactly once,
//! while the same series under a rule with no band pages on every window. At exactly the fire
//! threshold it fires, and at exactly the clear threshold it does not clear — one thousandth of a
//! percent either side of both.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no collector and no query engine**: the series a real query would
//! have returned is passed in as [`Window`]s, so what is re-checked is the *policy* over the numbers
//! and not the numbers themselves. The arithmetic is integer parts per million on purpose — a
//! threshold shipped as floating point is a threshold whose boundary is a rumour, and "did it fire
//! at 14.4?" has to have one answer.
//!
//! Missing on purpose: nothing scrapes or renders (`relay/2.18`'s file owns the scrape), the
//! multi-window rule — a fast window *and* a slow one above the threshold — is this same hysteresis
//! with more counters and is deliberately not modelled, and nothing sends a page: the answer here is
//! a [`Decision`], and a notification channel is `ops/runbook-drill`'s business.

/// How a fraction is spelled everywhere in this file.
///
/// Burn rates and thresholds are all parts per million, so `1_000_000` is 1.0 and every boundary is
/// an integer a test can name.
pub const PPM: u64 = 1_000_000;

/// The objective: how much bad traffic is allowed, per million requests.
///
/// Stored as the complement of the objective — an SLO of 99.9% is `1_000` — because the budget is
/// what a bad request spends and the objective is what it was measured against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slo {
    /// Bad requests allowed per million.
    pub allowed_bad_ppm: u64,
}

impl Slo {
    /// The bad requests a window of `total` requests is allowed to spend.
    ///
    /// A fraction of a request rounds down to none: ten requests at 99.9% allow a hundredth of an
    /// error, and a budget of one would be a budget ten times too large. An objective measured
    /// against ten requests is not a budget, it is a coin toss, and saying so is the honest answer.
    #[must_use]
    pub fn budget(&self, total: u64) -> u64 {
        // Integer throughout, and wide enough that a busy window cannot overflow the round trip:
        // the budget of a year's traffic is a number a `u64` holds, but the intermediate product is
        // not something to guess about.
        let allowed = u128::from(total) * u128::from(self.allowed_bad_ppm) / u128::from(PPM);
        u64::try_from(allowed).unwrap_or(u64::MAX)
    }

    /// The burn rate over one window, in millionths: `1_000_000` is spending the budget exactly on
    /// pace for the period, `14_400_000` is spending it fourteen times too fast.
    ///
    /// A window with no traffic has no burn. An objective that allows nothing has an unbounded one:
    /// any bad request at all is spending a budget that is not there.
    #[must_use]
    pub fn burn_ppm(&self, window: &Window) -> u64 {
        if window.total == 0 {
            return 0;
        }
        if self.allowed_bad_ppm == 0 {
            return if window.bad == 0 { 0 } else { u64::MAX };
        }
        // burn = (bad / total) / (allowed / PPM), and the answer is burn * PPM. One expression
        // rather than a float: a boundary that a test cannot name is a boundary nobody can defend.
        let burn = u128::from(window.bad) * u128::from(PPM) * u128::from(PPM)
            / (u128::from(window.total) * u128::from(self.allowed_bad_ppm));
        u64::try_from(burn).unwrap_or(u64::MAX)
    }
}

/// One window of traffic: what a query summed over a period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// Requests seen.
    pub total: u64,
    /// Of those, the ones that missed the objective.
    pub bad: u64,
}

/// The alert's thresholds and durations.
///
/// `clear_ppm` is strictly below `page_ppm` on purpose: the gap between the two is the hysteresis,
/// and a rule whose two thresholds are equal is a rule that flaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Burn at or above this wants to fire.
    pub page_ppm: u64,
    /// Burn below this, while firing, wants to clear. A firing rule whose burn sits between the two
    /// thresholds neither pages again nor resolves.
    pub clear_ppm: u64,
    /// Consecutive windows at or above `page_ppm` before the rule fires.
    pub for_windows: u32,
    /// Consecutive windows below `clear_ppm` before a firing rule clears.
    pub clear_windows: u32,
}

/// What the rule is doing after a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Quiet: no page is owed.
    Quiet,
    /// Firing: somebody is being paged now, or already was.
    Firing,
}

/// The rule's state: whether it is firing, and the runs of windows it has counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alert {
    slo: Slo,
    policy: Policy,
    firing: bool,
    above: u32,
    below: u32,
}

impl Alert {
    /// A quiet rule.
    #[must_use]
    pub fn new(slo: Slo, policy: Policy) -> Self {
        Self {
            slo,
            policy,
            firing: false,
            above: 0,
            below: 0,
        }
    }

    /// Whether the rule is firing right now.
    #[must_use]
    pub fn firing(&self) -> bool {
        self.firing
    }

    /// Feed one window to the rule and answer what it is doing afterwards.
    ///
    /// While quiet the rule counts consecutive windows at or above the fire threshold; the window
    /// that completes the run is the first [`Decision::Firing`]. While firing it counts consecutive
    /// windows *below* the clear threshold; the window that completes that run is the first
    /// [`Decision::Quiet`]. A window in between leaves a firing rule firing and a quiet rule quiet,
    /// which is the whole point of two thresholds.
    #[must_use]
    pub fn observe(&mut self, window: &Window) -> Decision {
        let burn = self.slo.burn_ppm(window);
        if self.firing {
            if burn < self.policy.clear_ppm {
                self.below += 1;
                if self.below >= self.policy.clear_windows {
                    // The run below the clear threshold is long enough: one incident is over. Reset
                    // the other run too, so the next page needs its own confirmation.
                    self.firing = false;
                    self.above = 0;
                    self.below = 0;
                    return Decision::Quiet;
                }
            } else {
                // In the band or above it: whatever run below the threshold there was is broken.
                self.below = 0;
            }
            Decision::Firing
        } else {
            if burn >= self.policy.page_ppm {
                self.above += 1;
                if self.above >= self.policy.for_windows {
                    self.firing = true;
                    self.above = 0;
                    return Decision::Firing;
                }
            } else {
                // Below the fire threshold, the run of hot windows is broken.
                self.above = 0;
            }
            Decision::Quiet
        }
    }

    /// Feed a whole series, one window at a time, and answer what the rule did on each.
    #[must_use]
    pub fn observe_all(&mut self, windows: &[Window]) -> Vec<Decision> {
        windows.iter().map(|window| self.observe(window)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 99.9%: a thousand bad requests out of a million is the whole budget.
    const SLO: Slo = Slo {
        allowed_bad_ppm: 1_000,
    };

    /// A window of a million requests, so `bad` is the burn in thousandths of a percent: 1 000 bad
    /// is burn 1.0 and 14 400 bad is burn 14.4.
    fn window(bad: u64) -> Window {
        Window {
            total: 1_000_000,
            bad,
        }
    }

    /// A rule a person would defend: page at 14.4 once it holds for two windows, clear below 7.2
    /// once *that* holds for two.
    fn policy() -> Policy {
        Policy {
            page_ppm: 14_400_000,
            clear_ppm: 7_200_000,
            for_windows: 2,
            clear_windows: 2,
        }
    }

    /// How many separate incidents the decisions describe: the first page, plus every quiet-to-
    /// firing edge after it.
    fn incidents(decisions: &[Decision]) -> usize {
        let mut count = usize::from(decisions.first() == Some(&Decision::Firing));
        for pair in decisions.windows(2) {
            if pair[0] == Decision::Quiet && pair[1] == Decision::Firing {
                count += 1;
            }
        }
        count
    }

    #[test]
    fn a_budget_is_a_fraction_of_the_traffic_not_a_number_of_errors() {
        assert_eq!(
            SLO.budget(1_000_000),
            1_000,
            "99.9% of a million leaves a thousand bad requests"
        );
        assert_eq!(
            SLO.budget(10),
            0,
            "a hundredth of a request is no budget at all: rounding a budget up is how a window of \
             ten requests gets a free error"
        );
        assert_eq!(SLO.budget(0), 0, "and no traffic is no budget");
        assert_eq!(
            SLO.budget(2_000_000),
            2_000,
            "the budget scales with the traffic, which is why it is a fraction"
        );
    }

    #[test]
    fn the_burn_is_a_rate() {
        assert_eq!(SLO.burn_ppm(&window(0)), 0, "no bad requests is no burn");
        assert_eq!(
            SLO.burn_ppm(&window(1_000)),
            PPM,
            "exactly the budget over the window is burn 1.0"
        );
        assert_eq!(
            SLO.burn_ppm(&window(2_000)),
            2 * PPM,
            "twice the bad requests is twice the burn"
        );
        assert_eq!(SLO.burn_ppm(&window(14_400)), 14_400_000);
        assert_eq!(
            SLO.burn_ppm(&Window { total: 0, bad: 0 }),
            0,
            "a window no request arrived in has no burn rather than a division by zero"
        );

        // An objective that allows nothing is an objective any bad request blows: the burn is
        // unbounded rather than a made-up number.
        let strict = Slo { allowed_bad_ppm: 0 };
        assert_eq!(strict.burn_ppm(&window(0)), 0);
        assert_eq!(strict.burn_ppm(&window(1)), u64::MAX);
    }

    #[test]
    fn a_single_hot_window_does_not_page_but_a_sustained_one_does() {
        let mut alert = Alert::new(SLO, policy());
        assert_eq!(
            alert.observe_all(&[window(14_400)]),
            vec![Decision::Quiet],
            "one window over the threshold is a blip: the rule waits for the run the profile names"
        );
        assert!(!alert.firing(), "and nothing is paged");

        let mut alert = Alert::new(SLO, policy());
        assert_eq!(
            alert.observe_all(&[window(14_400), window(14_500), window(14_400)]),
            vec![Decision::Quiet, Decision::Firing, Decision::Firing],
            "the window that completes the run is the one that fires"
        );
        assert!(alert.firing());

        // A clean window between two hot ones resets the run: two failures with a clean window
        // between them are two runs of one, not a run of two.
        let mut alert = Alert::new(SLO, policy());
        assert_eq!(
            alert.observe_all(&[window(14_400), window(0), window(14_400), window(14_400)]),
            vec![
                Decision::Quiet,
                Decision::Quiet,
                Decision::Quiet,
                Decision::Firing
            ],
            "the count is of consecutive windows"
        );
        assert!(
            alert.firing(),
            "the clean window reset the run, so a fresh run of two was needed to fire again"
        );
    }

    #[test]
    fn the_band_between_the_thresholds_is_what_stops_the_pager_flapping() {
        // A series that alternates across the fire threshold but never falls below the clear one:
        // 14 400 fires, 10 000 sits in the band. This is a service that is bad without being
        // catastrophically bad, and it is exactly what a rule with no band pages on forty times.
        let flapping: Vec<Window> = (0..40)
            .map(|index| {
                if index % 2 == 0 {
                    window(14_400)
                } else {
                    window(10_000)
                }
            })
            .collect();

        // The most eager rule there is: one window to fire, one to clear. The band does all the
        // work.
        let eager = Policy {
            page_ppm: 14_400_000,
            clear_ppm: 7_200_000,
            for_windows: 1,
            clear_windows: 1,
        };
        let mut alert = Alert::new(SLO, eager);
        let decisions = alert.observe_all(&flapping);
        assert!(
            decisions
                .iter()
                .all(|decision| *decision == Decision::Firing),
            "once it fires it stays firing while the burn is above the clear threshold: {decisions:?}"
        );
        assert_eq!(
            incidents(&decisions),
            1,
            "one incident is one page, however long it alternates: {decisions:?}"
        );

        // The same series under a rule with no band — clear at the fire threshold — flaps on every
        // window, which is the pager noise the band exists to remove.
        let naive = Policy {
            page_ppm: 14_400_000,
            clear_ppm: 14_400_000,
            for_windows: 1,
            clear_windows: 1,
        };
        let mut alert = Alert::new(SLO, naive);
        let decisions = alert.observe_all(&flapping);
        assert!(
            incidents(&decisions) > 10,
            "a rule with no band pages and resolves on every window: {} incidents",
            incidents(&decisions)
        );
    }

    #[test]
    fn the_fire_and_clear_thresholds_are_exact() {
        let eager = Policy {
            page_ppm: 14_400_000,
            clear_ppm: 7_200_000,
            for_windows: 1,
            clear_windows: 1,
        };
        let mut alert = Alert::new(SLO, eager);
        assert_eq!(
            alert.observe(&window(14_399)),
            Decision::Quiet,
            "one thousandth of a percent under the threshold is not the threshold"
        );
        assert_eq!(
            alert.observe(&window(14_400)),
            Decision::Firing,
            "at exactly the threshold it fires"
        );
        assert_eq!(
            alert.observe(&window(7_200)),
            Decision::Firing,
            "at exactly the clear threshold it keeps firing: clearing needs strictly below"
        );
        assert_eq!(
            alert.observe(&window(7_199)),
            Decision::Quiet,
            "one thousandth of a percent below it clears"
        );
    }

    #[test]
    fn a_quiet_series_is_never_a_page_and_the_band_is_a_band() {
        let policy = policy();
        assert!(
            policy.clear_ppm < policy.page_ppm,
            "the clear threshold is strictly below the fire threshold: the gap between them is the \
             hysteresis"
        );

        let mut alert = Alert::new(SLO, policy);
        assert_eq!(
            alert.observe_all(&[window(0); 8]),
            vec![Decision::Quiet; 8],
            "a series with no bad requests is never a page"
        );
        assert!(!alert.firing());
    }
}
