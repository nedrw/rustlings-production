//! probe = "obs/cardinality-cost@apply"
//! title = "The cap is a counted policy, and a refusal costs nothing"
//! hint = "Count every offer, admit a series the store already tracks, refuse the new name the cap has no room for, and charge the refusal nothing."
//!
//! Step `lakelet/4.18-cardinality-cost` is about the one cost this track cannot measure: every
//! number before it was a *volume* (bytes per payload byte, blocks a plan did not read, work per
//! query phase), and cardinality is a *stock* the application chooses. A label whose values come
//! from a pod name, a user id or a request id turns one metric into a million series while every
//! byte counter stays flat. Everything in this file is a rule somebody paid for once:
//!
//! * **the cap is a policy about names, not about traffic.** A client sending a million samples of
//!   one series is a client this store is delighted to serve; a client sending one sample each of
//!   a million series is the one that costs money, and a cap on requests refuses the wrong one of
//!   the two;
//! * **a series the store already tracks is always admitted**, and the check is on *new* names
//!   only. A deployment that reached its limit would otherwise start losing the data it holds,
//!   one sample at a time, with no error anywhere. A series it has never seen is admitted while
//!   the ledger has room and refused once it does not;
//! * **a refusal charges nothing, moves nothing, and is counted.** The refused name is not in the
//!   ledger, no cost moves, and the only trace of it is a counter — a refused series that still
//!   costs is the bug this step is about, and the bill for it arrives every flush;
//! * **the ledger is by identity, not by sample.** One name is one entry however often it is
//!   offered; the counters count *offers*, and `tracked_count` is the live set the limit is
//!   measured against;
//! * **`offered == admitted + rejected`.** Every offer lands on exactly one side, and this is the
//!   line a leak crosses: a refused name that is inserted anyway leaves the live set above the
//!   limit while the two counters fall short of the offers.
//!
//! ## What you write
//!
//! * [`Ledger::admit`] — one series offered, and what the cap answered about it;
//! * [`Ledger::of_offers`] — the ledger of a run, built from the offers it was handed;
//! * [`Ledger::cost_of`] — what one series has cost, or `None` when it is not tracked;
//! * [`Ledger::report`] — the live set as the lines `lakeletq inspect series` prints.
//!
//! [`Cap`], [`Admission`], [`Cost`], [`Offer`] and the counters are given: the policy as a value,
//! the refusal with the two numbers it was made from, and the arithmetic of the counters are the
//! interface a log line, a client's diagnostics and the page are all written against.
//!
//! ## What the tests ask
//!
//! The series that lands exactly on the cap is admitted and the next one is not, and the refusal
//! names both numbers it was made from. A refused series is never priced: it is not in the ledger,
//! it has no cost, and it is not a line of the report. A series the ledger already holds is
//! admitted at the limit and stays one entry, while the counters count both offers. One series
//! offered twice costs what both offers carried, and its window keeps the extremes in both
//! directions. `offered == admitted + rejected` holds over 1025 offers against the cap this build
//! ships, and again for a ledger of no offers at all and for a cap of nothing. And two runs over
//! the same offers print the same report, in name order, in the line format the tool prints.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no store**. The real ledger is derived state rebuilt at open from
//! the log's records pruned by the sidecar indexes of the segments the store still holds
//! (`Registry::of_store`, `lakelet/4.14-index-pruning`), which is what keeps a cap from resetting
//! on every restart — a process manager restarting a daemon hourly would otherwise let a fresh
//! limit of names in every hour, with no counter anywhere saying so. Here the offers are handed
//! in, because what a single file can honestly re-check is the policy and its arithmetic: which
//! names are admitted, what a refusal leaves behind, and the identity the counters obey.
//!
//! Also missing on purpose: no frames (the write path judges a payload whole,
//! `lakelet/4.5-batch-writes`, and this file judges one series at a time), no clock (`at` is the
//! caller's number, so two runs of one workload report one window), no segments and no bloom
//! (`may_hold` and `unstated` live in `lakelet/4.14-index-pruning`), and no configuration —
//! `series.limit` and its four layers are the daemon's.

use std::collections::BTreeMap;

/// The policy: how many series a store tracks before it refuses a name it has never seen.
///
/// The limit is a deployment's number and not a measurement: `docs/cardinality.md` states the
/// arithmetic that makes [`Cap::DEFAULT`] affordable on the workload it was measured on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cap {
    limit: u64,
}

impl Cap {
    /// The cap this build ships: 1024 series, the number the page states and `series.limit`
    /// defaults to.
    pub const DEFAULT: Self = Self::new(1024);

    /// A cap of `limit` series.
    #[must_use]
    pub const fn new(limit: u64) -> Self {
        Self { limit }
    }

    /// The number the ledger is measured against.
    #[must_use]
    pub const fn limit(self) -> u64 {
        self.limit
    }
}

/// What the cap answered about one series offered to the ledger.
///
/// Both answers are values a caller branches on, and the refusal carries the two numbers the
/// decision was made from: a log line and a client's diagnostics are written against these and not
/// against a sentence somebody has to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// The ledger took the series.
    Admitted,
    /// The ledger has no room for a series it has never seen.
    Rejected {
        /// Series the ledger tracks right now.
        tracked: u64,
        /// What the cap allows.
        limit: u64,
    },
}

impl Admission {
    /// `true` when the ledger took the series.
    #[must_use]
    pub const fn is_admitted(&self) -> bool {
        matches!(self, Self::Admitted)
    }
}

/// What one series has cost this store so far.
///
/// The two ends of the window are the extremes of the instants the series was seen at, not the
/// last one: a series' window is what a query has to prune against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cost {
    /// Records this series was offered.
    pub records: u64,
    /// Bytes the write path was handed for them.
    pub bytes: u64,
    /// The first millisecond it was seen at.
    pub first_millis: u64,
    /// The last one.
    pub last_millis: u64,
}

impl Cost {
    /// What one offer of a series carried: its records, the bytes they arrived in, and the instant
    /// the caller states — both ends of the window are that instant to begin with.
    #[must_use]
    pub const fn offered(records: u64, bytes: u64, at: u64) -> Self {
        Self {
            records,
            bytes,
            first_millis: at,
            last_millis: at,
        }
    }
}

/// One series offered to the ledger, and what it carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// The series name, which is the identity: `cpu,host=h1`.
    pub name: String,
    /// Records this offer carried.
    pub records: u64,
    /// Bytes the write path was handed for them.
    pub bytes: u64,
    /// The millisecond the caller states. The ledger never reads a clock.
    pub at: u64,
}

impl Offer {
    /// One series offered at `at`.
    #[must_use]
    pub fn new(name: &str, records: u64, bytes: u64, at: u64) -> Self {
        Self {
            name: name.to_owned(),
            records,
            bytes,
            at,
        }
    }
}

/// The series a store tracks, what each of them cost, and the counters a run is judged on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ledger {
    cap: Cap,
    tracked: BTreeMap<String, Cost>,
    admitted: u64,
    rejected: u64,
    offered: u64,
}

impl Ledger {
    /// An empty ledger, judged by `cap`.
    #[must_use]
    pub fn new(cap: Cap) -> Self {
        Self {
            cap,
            tracked: BTreeMap::new(),
            admitted: 0,
            rejected: 0,
            offered: 0,
        }
    }

    /// The policy this ledger is judged by.
    #[must_use]
    pub const fn cap(&self) -> Cap {
        self.cap
    }

    /// How many series the ledger holds right now: the number the cap's limit is measured
    /// against.
    #[must_use]
    pub fn tracked_count(&self) -> u64 {
        self.tracked.len() as u64
    }

    /// How many series offers this ledger took.
    #[must_use]
    pub const fn series_admitted(&self) -> u64 {
        self.admitted
    }

    /// How many series offers it refused.
    #[must_use]
    pub const fn series_rejected(&self) -> u64 {
        self.rejected
    }

    /// How many series it was offered: the third side of `offered == admitted + rejected`.
    #[must_use]
    pub const fn offered(&self) -> u64 {
        self.offered
    }

    /// Offer one series, and answer what the cap decided about it.
    ///
    /// A series the ledger already tracks is always admitted, whatever the limit says; a series it
    /// has never seen is admitted while there is room. A refusal counts itself, charges nothing
    /// and moves nothing.
    pub fn admit(&mut self, offer: &Offer) -> Admission {
        let _ = offer;
        todo!("obs/cardinality-cost@apply: one series offered, and what the cap answered about it")
    }

    /// The ledger of a run: every offer of `offers`, in the order the run made them.
    ///
    /// Opening a store is what rebuilds the real ledger, and this is that rebuild with the offers
    /// handed in instead of replayed out of the log.
    #[must_use]
    pub fn of_offers(cap: Cap, offers: &[Offer]) -> Self {
        let _ = (cap, offers);
        todo!("obs/cardinality-cost@apply: the ledger of the offers a run was handed")
    }

    /// What one series has cost, or `None` when the ledger does not track it.
    ///
    /// `None` is a different fact from a series that cost nothing: a refused name is not in the
    /// ledger at all.
    #[must_use]
    pub fn cost_of(&self, name: &str) -> Option<&Cost> {
        let _ = name;
        todo!("obs/cardinality-cost@apply: what one series has cost, or `None`")
    }

    /// The live set, one line per tracked series, in name order.
    ///
    /// The line is what `lakeletq inspect series --store <DIR>` prints — the name, its records,
    /// the bytes it was handed and the two ends of its window — and the order is the `BTreeMap`'s,
    /// so two runs print the same bytes and a page quoting a run is a page somebody can re-run.
    #[must_use]
    pub fn report(&self) -> Vec<String> {
        todo!(
            "obs/cardinality-cost@apply: the live set, one line per tracked series, in name order"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One offer of one record, at the second `at`.
    fn at_second(name: &str, at: u64) -> Offer {
        Offer::new(name, 1, 8, at.saturating_mul(1_000))
    }

    #[test]
    fn the_series_that_lands_exactly_on_the_cap_is_admitted_and_the_next_one_is_not() {
        let mut ledger = Ledger::new(Cap::new(2));
        assert!(ledger.admit(&at_second("cpu,host=h0", 1)).is_admitted());
        assert!(ledger.admit(&at_second("cpu,host=h1", 1)).is_admitted());
        assert_eq!(
            ledger.tracked_count(),
            2,
            "the cap allows two series and the ledger holds exactly two: the series that lands on \
             the limit is inside it"
        );
        assert_eq!(
            ledger.admit(&at_second("cpu,host=h2", 1)),
            Admission::Rejected {
                tracked: 2,
                limit: 2
            },
            "the third name is the refusal, and it carries both numbers the decision was made from"
        );
    }

    #[test]
    fn a_refused_series_is_never_priced() {
        let mut ledger = Ledger::new(Cap::new(1));
        assert!(
            ledger
                .admit(&Offer::new("cpu,host=h0", 5, 100, 7))
                .is_admitted()
        );
        let huge = Offer::new("user=9f2c", 5_000_000, 100_000_000, 9);
        assert!(
            !ledger.admit(&huge).is_admitted(),
            "the cap allows one series and the ledger holds one"
        );
        // A series that still costs after the cap refused it is the bug this step exists to
        // prevent: the refusal is the only trace of it, and it is a counter.
        assert_eq!(
            ledger.cost_of("user=9f2c"),
            None,
            "a refusal charges nothing, so the refused name is not in the ledger at all"
        );
        assert_eq!(ledger.tracked_count(), 1, "and the live set did not move");
        let reported = ledger.report();
        assert!(
            reported.iter().all(|line| !line.starts_with("user=")),
            "a refused series is not a line of the report: {reported:?}"
        );
    }

    #[test]
    fn a_series_the_ledger_already_tracks_is_admitted_at_the_limit() {
        let mut ledger = Ledger::new(Cap::new(1));
        assert!(ledger.admit(&at_second("cpu,host=h0", 1)).is_admitted());
        assert_eq!(
            ledger.admit(&at_second("cpu,host=h0", 2)),
            Admission::Admitted,
            "a cap must not break a series that is being reported: it is already the store's, so \
             the limit has nothing to say about it"
        );
        assert_eq!(
            ledger.tracked_count(),
            1,
            "and it is one entry, not two: the ledger is by identity"
        );
        assert_eq!(
            ledger.series_admitted(),
            2,
            "while the counters count offers, so taking the same series twice is two offers taken"
        );
    }

    #[test]
    fn a_series_seen_twice_is_one_series_that_costs_both_offers() {
        let mut ledger = Ledger::new(Cap::DEFAULT);
        // The second offer is the earlier instant and the first offer is the later one, so
        // neither of them may decide the window on its own.
        let later = Offer::new("cpu,host=h1", 4, 400, 1_700_000_010_000);
        let earlier = Offer::new("cpu,host=h1", 6, 700, 1_700_000_000_000);
        assert!(ledger.admit(&later).is_admitted());
        assert!(ledger.admit(&earlier).is_admitted());
        let cost = *ledger
            .cost_of("cpu,host=h1")
            .expect("the ledger tracks what it admitted");
        assert_eq!(
            cost.records, 10,
            "the live set is by identity, and the price is every sample of that identity"
        );
        assert_eq!(cost.bytes, 1_100, "and the bytes of both offers: {cost:?}");
        assert_eq!(
            cost.first_millis, 1_700_000_000_000,
            "a window keeps the earliest instant in one direction"
        );
        assert_eq!(
            cost.last_millis, 1_700_000_010_000,
            "and the latest in the other: each direction keeps its own extreme"
        );
    }

    #[test]
    fn the_identity_holds_over_a_run_and_the_cap_is_the_number_the_page_states() {
        let offers: Vec<Offer> = (0..1025)
            .map(|index| at_second(&format!("cpu,host=h{index}"), index))
            .collect();
        let ledger = Ledger::of_offers(Cap::DEFAULT, &offers);
        assert_eq!(
            Cap::DEFAULT.limit(),
            1024,
            "the cap this build ships is the number `docs/cardinality.md` states"
        );
        assert_eq!(
            ledger.series_admitted(),
            1024,
            "the cap admits a full cap of names"
        );
        assert_eq!(ledger.series_rejected(), 1, "and refuses the 1025th");
        assert_eq!(
            ledger.tracked_count(),
            1024,
            "the live set is what the limit was measured against, so it never passes it"
        );
        assert_eq!(
            ledger.offered(),
            ledger.series_admitted() + ledger.series_rejected(),
            "offered == admitted + rejected is the line a leak crosses"
        );
        assert_eq!(
            ledger.offered(),
            1025,
            "every offer landed on one side of it"
        );
    }

    #[test]
    fn an_empty_ledger_and_a_cap_of_nothing_are_still_the_same_arithmetic() {
        let empty = Ledger::of_offers(Cap::DEFAULT, &[]);
        assert!(
            empty.report().is_empty(),
            "a store with no series is no lines"
        );
        assert_eq!(
            (
                empty.offered(),
                empty.series_admitted(),
                empty.series_rejected()
            ),
            (0, 0, 0),
            "and no offers is the identity holding in the shallowest case"
        );
        let nothing = Ledger::of_offers(Cap::new(0), &[at_second("cpu,host=h0", 1)]);
        assert_eq!(
            nothing.tracked_count(),
            0,
            "a cap of nothing has no room for a name it has never seen"
        );
        assert_eq!(
            (
                nothing.offered(),
                nothing.series_admitted(),
                nothing.series_rejected()
            ),
            (1, 0, 1),
            "and the refusal is counted rather than lost: {nothing:?}"
        );
    }

    #[test]
    fn the_report_is_sorted_and_is_the_line_the_tool_prints() {
        let offers = vec![
            Offer::new("cpu,host=h2", 3, 300, 1_700_000_000_000),
            Offer::new("cpu,host=h1", 4, 400, 1_700_000_000_000),
            Offer::new("cpu,host=h0", 5, 500, 1_700_000_000_000),
        ];
        let first = Ledger::of_offers(Cap::DEFAULT, &offers);
        let second = Ledger::of_offers(Cap::DEFAULT, &offers);
        assert_eq!(
            first.report(),
            second.report(),
            "two runs over the same offers print the same bytes, or a page quoting a run is a page \
             nobody can re-run"
        );
        let names: Vec<String> = first
            .report()
            .iter()
            .map(|line| line.split(' ').next().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(
            names,
            ["cpu,host=h0", "cpu,host=h1", "cpu,host=h2"],
            "in name order, and the name is the first field of the line"
        );
        assert_eq!(
            first.report()[1],
            "cpu,host=h1 4 400 1700000000000 1700000000000",
            "the line is the name, its records, its bytes and both ends of its window — what \
             `lakeletq inspect series` prints, one line per tracked series"
        );
    }
}
