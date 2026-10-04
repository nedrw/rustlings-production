//! probe = "obs/metrics-slo@apply"
//! title = "One sample per line, and a label set that stays small"
//! hint = "Render `name{labels} value` with sorted labels, make the buckets cumulative, and fold a tenant nobody knows into one label value."
//!
//! Step `relay/2.18-delivery-metrics` asks two questions and a probe can answer both in one file:
//!
//! 1. **is a tenant slow, and how deep is its queue?** A p99 without a backlog depth says
//!    something is slow and not whether the subscriber is unlucky or the worker is drowning, so the
//!    scrape carries both: a latency histogram per tenant and a backlog gauge per tenant;
//! 2. **is the label set bounded?** A label built from a request is how a metrics backend dies:
//!    every subscriber domain becomes a series, and a busy week becomes millions of them.
//!    [`tenant_label`] folds the request's host into the tenants this service knows about and one
//!    [`OTHER`] — a fixed number of series, whatever the traffic.
//!
//! ## What you write
//!
//! * [`tenant_label`] — the label a request's host becomes;
//! * [`Histogram::observe`] — one latency, into cumulative buckets plus a count and a sum;
//! * [`Metrics::delivered`] and [`Metrics::backlog`] — the counter, the histogram and the gauge;
//! * [`Metrics::render`] — the text: `name{labels} value`, one sample per line, sorted, and one
//!   histogram is `_bucket` lines in bound order followed by `_count` and `_sum`.
//!
//! [`Key`] (the identity of a series, with its labels sorted so the same series is always the same
//! text) and the label list are given. The rendering rules are the discipline; the string handling
//! is not.
//!
//! ## What the tests ask
//!
//! A scrape is compared to the exact text it has to be — labels in name order, buckets cumulative,
//! `le="+Inf"` equal to `_count`, the backlog a gauge rather than a sum. A tenant nobody has heard
//! of is one label value rather than 300 series. And two registries fed the same observations in
//! different orders render the same bytes, because a dashboard that reads one text and a test that
//! reads the other have to be looking at the same scrape.
//!
//! ## Where this is honest, and where it is not
//!
//! No client library, no exporter process and no registry: this is the part a worker owns — the
//! numbers and their text. There is no aggregation over time and no quantile: p99 is computed by the
//! query side from exactly these buckets, and computing one here would be a second implementation of
//! a decision that is not the service's. Label values are the fixed set this service produces
//! (a tenant, an outcome, a bucket bound), so the projection does not escape `"` or `\` — a real
//! exporter does, and a label value that is not from this set is a bug to fix rather than a string
//! to quote.

use std::collections::BTreeMap;
use std::fmt;

/// The tenants this service knows about. Everything else is [`OTHER`].
///
/// A list rather than a rule on purpose: which subscribers this service has is a fact it holds, and
/// a label derived from a request is the label explosion the bounded set exists to prevent.
pub const TENANTS: [&str; 3] = ["acme", "globex", "initech"];

/// The label value for a tenant nobody has heard of. One series for all of them, which is the point.
pub const OTHER: &str = "other";

/// The latency histogram's bounds, in milliseconds. `+Inf` is implicit and is not in this list.
pub const LATENCY_BUCKETS_MS: [u64; 5] = [5, 20, 100, 500, 2_000];

/// The label a request's host becomes: the tenant, or [`OTHER`].
///
/// The host is what the subscriber sent (`acme.relay.test`), and the first label is the tenant. The
/// answer is always one of four fixed strings, so a week of traffic cannot grow the label set.
#[must_use]
pub fn tenant_label(host: &str) -> &'static str {
    // The first label of the host is the tenant, and the comparison is case-insensitive because a
    // hostname is: `ACME.relay.test` is the same subscriber as `acme.relay.test`.
    let first = host.split('.').next().unwrap_or_default();
    TENANTS
        .iter()
        .copied()
        .find(|tenant| tenant.eq_ignore_ascii_case(first))
        .unwrap_or(OTHER)
}

/// How a delivery went, as a label value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It went out.
    Delivered,
    /// It did not, for now.
    Failed,
}

impl Outcome {
    /// The label value.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Failed => "failed",
        }
    }
}

/// The identity of a series: a metric name and its labels, sorted by label name so that the same
/// series is always the same `Key` and always the same text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    name: String,
    labels: Vec<(String, String)>,
}

impl Key {
    fn new(name: &str, labels: &[(&str, &str)]) -> Self {
        let mut labels: Vec<(String, String)> = labels
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        labels.sort();
        Self {
            name: name.to_owned(),
            labels,
        }
    }

    /// The same labels under another name (`…_count`, `…_sum`).
    fn named(&self, name: &str) -> Self {
        Self {
            name: name.to_owned(),
            labels: self.labels.clone(),
        }
    }

    /// The same series with one label added (the histogram's `le`).
    fn with_label(&self, name: &str, value: &str) -> Self {
        let mut labels = self
            .labels
            .iter()
            .filter(|(label, _)| label != name)
            .cloned()
            .collect::<Vec<(String, String)>>();
        labels.push((name.to_owned(), value.to_owned()));
        labels.sort();
        Self {
            name: self.name.clone(),
            labels,
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        if self.labels.is_empty() {
            return Ok(());
        }
        let labels = self
            .labels
            .iter()
            .map(|(name, value)| format!("{name}=\"{value}\""))
            .collect::<Vec<String>>()
            .join(",");
        write!(f, "{{{labels}}}")
    }
}

/// The latencies of one tenant, counted into cumulative buckets.
#[derive(Debug)]
pub struct Histogram {
    /// `buckets[i]` counts the observations `<= LATENCY_BUCKETS_MS[i]`, cumulatively.
    buckets: Vec<u64>,
    /// Every observation, including the ones past the last bound.
    count: u64,
    /// Every observation, added up: the mean is `sum / count` and nothing here divides.
    sum: u64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

impl Histogram {
    /// An empty histogram with one counter per bound.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buckets: vec![0; LATENCY_BUCKETS_MS.len()],
            count: 0,
            sum: 0,
        }
    }

    /// One delivery's latency, in milliseconds.
    ///
    /// The buckets are cumulative — a sample inside the 20 ms bucket is inside the 100 ms one too —
    /// because that is what makes a quantile computable from them afterwards.
    pub fn observe(&mut self, millis: u64) {
        self.count += 1;
        self.sum += millis;

        // Cumulative, so the sample lands in its own bound *and* in every larger one: that is what
        // makes `le` a bound rather than a range, and what makes a quantile computable later.
        for (index, bound) in LATENCY_BUCKETS_MS.iter().enumerate() {
            if millis <= *bound {
                self.buckets[index] += 1;
            }
        }
    }

    /// How many latencies were observed.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }
}

/// What the delivery worker exposes: one counter, one histogram and one gauge, all labelled by
/// tenant.
#[derive(Debug, Default)]
pub struct Metrics {
    counters: BTreeMap<Key, u64>,
    gauges: BTreeMap<Key, u64>,
    histograms: BTreeMap<Key, Histogram>,
}

impl Metrics {
    /// Nothing observed yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// One delivery finished: the counter goes up, and its latency goes into the tenant's histogram.
    ///
    /// The tenant is [`tenant_label`]'s, not the host: that is the whole defence of the label set.
    pub fn delivered(&mut self, tenant: &str, millis: u64, outcome: Outcome) {
        // The label is the tenant, never the host: a label built from a request is a series per
        // subscriber, which is how a metrics backend dies of success.
        let tenant = tenant_label(tenant);

        let counter = Key::new(
            "relay_deliveries_total",
            &[("tenant", tenant), ("outcome", outcome.label())],
        );
        *self.counters.entry(counter).or_insert(0) += 1;

        self.histograms
            .entry(Key::new("relay_delivery_millis", &[("tenant", tenant)]))
            .or_default()
            .observe(millis);
    }

    /// The backlog depth of one tenant, as the collector last saw it.
    ///
    /// A gauge is a level, not a total: it is the number now, and a scrape that missed a rise and a
    /// fall is supposed to say the level it found rather than the sum of everything it ever saw.
    pub fn backlog(&mut self, tenant: &str, depth: u64) {
        // A gauge is a level: the reading replaces the last one instead of adding to it, so the
        // scrape says how deep the queue is now rather than how much has ever been in it.
        self.gauges.insert(
            Key::new(
                "relay_backlog_deliveries",
                &[("tenant", tenant_label(tenant))],
            ),
            depth,
        );
    }

    /// The text to scrape: one sample per line, `name{labels} value`.
    ///
    /// Series are in name order so that two scrapes of the same numbers are the same bytes, and one
    /// histogram is its `_bucket` lines in bound order (so a reader can walk them and find them
    /// non-decreasing), then `le="+Inf"` — which equals `_count` — then `_count` and `_sum`.
    #[must_use]
    pub fn render(&self) -> String {
        // One line per series, grouped by the series' identity and then written in name order: a
        // `BTreeMap` keyed by `Key` is that order, and the lines inside a group are the one order
        // that is not lexicographic — the bounds of a histogram, smallest first.
        let mut groups: BTreeMap<Key, Vec<String>> = BTreeMap::new();

        for (key, value) in &self.counters {
            groups
                .entry(key.clone())
                .or_default()
                .push(format!("{key} {value}"));
        }
        for (key, value) in &self.gauges {
            groups
                .entry(key.clone())
                .or_default()
                .push(format!("{key} {value}"));
        }
        for (key, histogram) in &self.histograms {
            let lines = groups.entry(key.clone()).or_default();
            let bucket_name = format!("{}_bucket", key.name);
            for (index, bound) in LATENCY_BUCKETS_MS.iter().enumerate() {
                let bucket = key.named(&bucket_name).with_label("le", &bound.to_string());
                lines.push(format!("{bucket} {}", histogram.buckets[index]));
            }
            // `+Inf` is the count, and it comes last: a reader walks the buckets downwards and
            // can check they are non-decreasing on the way.
            let infinite = key.named(&bucket_name).with_label("le", "+Inf");
            lines.push(format!("{infinite} {}", histogram.count));
            lines.push(format!(
                "{} {}",
                key.named(&format!("{}_count", key.name)),
                histogram.count
            ));
            lines.push(format!(
                "{} {}",
                key.named(&format!("{}_sum", key.name)),
                histogram.sum
            ));
        }

        groups
            .into_values()
            .flatten()
            .collect::<Vec<String>>()
            .join("\n")
    }

    /// How many series this registry holds: the number that has to stay bounded.
    #[must_use]
    pub fn series(&self) -> usize {
        self.counters.len() + self.gauges.len() + self.histograms.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scrape_is_one_sample_per_line_in_name_order() {
        let mut metrics = Metrics::new();
        metrics.delivered("acme.relay.test", 12, Outcome::Delivered);
        metrics.backlog("acme.relay.test", 3);

        // The whole text, because the format is the deliverable: a scraper reads lines, not a
        // structure, and a test that merely `contains` the parts does not pin the sample above.
        let expected = "\
            relay_backlog_deliveries{tenant=\"acme\"} 3\n\
            relay_deliveries_total{outcome=\"delivered\",tenant=\"acme\"} 1\n\
            relay_delivery_millis_bucket{le=\"5\",tenant=\"acme\"} 0\n\
            relay_delivery_millis_bucket{le=\"20\",tenant=\"acme\"} 1\n\
            relay_delivery_millis_bucket{le=\"100\",tenant=\"acme\"} 1\n\
            relay_delivery_millis_bucket{le=\"500\",tenant=\"acme\"} 1\n\
            relay_delivery_millis_bucket{le=\"2000\",tenant=\"acme\"} 1\n\
            relay_delivery_millis_bucket{le=\"+Inf\",tenant=\"acme\"} 1\n\
            relay_delivery_millis_count{tenant=\"acme\"} 1\n\
            relay_delivery_millis_sum{tenant=\"acme\"} 12";
        assert_eq!(
            metrics.render(),
            expected,
            "labels are in name order, so the same series is always the same text"
        );
    }

    #[test]
    fn the_buckets_are_cumulative_and_the_infinite_one_is_the_count() {
        let mut metrics = Metrics::new();
        for millis in [3, 40, 700, 5_000] {
            metrics.delivered("acme", millis, Outcome::Delivered);
        }

        let rendered = metrics.render();
        let buckets: Vec<(String, u64)> = rendered
            .lines()
            .filter_map(|line| {
                let (series, value) = line.rsplit_once(' ')?;
                let bound = series
                    .split(['{', ','])
                    .find_map(|label| label.strip_prefix("le=\""))?
                    .trim_end_matches('"')
                    .to_owned();
                Some((bound, value.parse().ok()?))
            })
            .collect();
        assert_eq!(
            buckets,
            vec![
                ("5".to_owned(), 1),
                ("20".to_owned(), 1),
                ("100".to_owned(), 2),
                ("500".to_owned(), 2),
                ("2000".to_owned(), 3),
                ("+Inf".to_owned(), 4),
            ],
            "every sample inside a bound is inside every larger one, and `+Inf` is all of them: \
             {rendered}"
        );
        assert!(
            buckets.windows(2).all(|pair| pair[0].1 <= pair[1].1),
            "a reader walks the buckets and expects them non-decreasing: {buckets:?}"
        );
        assert_eq!(
            buckets.last().map(|(_, count)| *count),
            Some(4),
            "`le=\"+Inf\"` is the count, so the sum of the histogram is checkable"
        );
        assert!(
            rendered.contains("relay_delivery_millis_sum{tenant=\"acme\"} 5743"),
            "the sum is every observation, including the one past the last bucket: {rendered}"
        );
        assert!(
            rendered.contains("relay_delivery_millis_count{tenant=\"acme\"} 4"),
            "the count is every observation too: {rendered}"
        );
    }

    #[test]
    fn a_tenant_nobody_knows_becomes_one_label_value() {
        assert_eq!(tenant_label("acme.relay.test"), "acme");
        assert_eq!(
            tenant_label("ACME.relay.test"),
            "acme",
            "a host is not a case-sensitive thing"
        );
        assert_eq!(tenant_label("acme"), "acme", "and it is not always dotted");
        assert_eq!(tenant_label("nobody.example.com"), OTHER);
        assert_eq!(tenant_label("mail.globex.example"), OTHER);

        // 300 subscribers nobody has heard of, and the label set does not move.
        let mut metrics = Metrics::new();
        for index in 0..300 {
            metrics.delivered(
                &format!("subscriber-{index}.example.com"),
                10,
                Outcome::Delivered,
            );
        }
        assert_eq!(
            metrics.series(),
            2,
            "one counter and one histogram for every unknown tenant in the world"
        );
        let rendered = metrics.render();
        assert!(
            rendered.contains("relay_deliveries_total{outcome=\"delivered\",tenant=\"other\"} 300"),
            "they are one series, not 300: {rendered}"
        );
        assert!(
            !rendered.contains("subscriber-0.example.com"),
            "and the host never reaches a label"
        );

        // The known tenants are what the list is for, and the two outcomes are two series.
        let mut metrics = Metrics::new();
        metrics.delivered("acme.relay.test", 10, Outcome::Delivered);
        metrics.delivered("acme.relay.test", 10, Outcome::Failed);
        metrics.delivered("globex.relay.test", 10, Outcome::Delivered);
        assert_eq!(
            metrics.series(),
            5,
            "two tenants, two outcomes, two histograms, one summed"
        );
    }

    #[test]
    fn a_gauge_is_the_level_now_and_not_a_total() {
        let mut metrics = Metrics::new();
        metrics.backlog("acme", 3);
        metrics.backlog("acme", 7);
        let rendered = metrics.render();

        assert_eq!(
            rendered, "relay_backlog_deliveries{tenant=\"acme\"} 7",
            "a gauge is a level: two readings are one series holding the last one, not ten"
        );

        // A depth of zero is a fact and has a series: a missing line and an empty queue look the
        // same to a dashboard that only renders what it is sent.
        let mut metrics = Metrics::new();
        metrics.backlog("acme", 3);
        metrics.backlog("acme", 0);
        assert_eq!(
            metrics.render(),
            "relay_backlog_deliveries{tenant=\"acme\"} 0"
        );
    }

    #[test]
    fn the_same_numbers_render_the_same_bytes_whatever_order_they_arrived_in() {
        let mut first = Metrics::new();
        first.backlog("acme", 4);
        first.delivered("acme", 12, Outcome::Delivered);
        first.delivered("acme", 800, Outcome::Failed);
        first.backlog("globex", 0);

        let mut second = Metrics::new();
        second.delivered("acme", 800, Outcome::Failed);
        second.backlog("globex", 0);
        second.delivered("acme", 12, Outcome::Delivered);
        second.backlog("acme", 4);

        assert_eq!(
            first.render(),
            second.render(),
            "a scrape is a function of the numbers, not of the order the worker happened to \
             observe them in"
        );
        assert!(
            first
                .render()
                .contains("relay_backlog_deliveries{tenant=\"globex\"} 0")
        );
    }

    #[test]
    fn a_slow_tenant_and_a_deep_one_are_both_in_the_scrape() {
        let mut metrics = Metrics::new();

        // acme is slow: nothing lands in the fast buckets.
        for millis in [150, 220, 190, 310] {
            metrics.delivered("acme.relay.test", millis, Outcome::Delivered);
            metrics.backlog("acme.relay.test", 2);
        }
        // globex is fast, and drowning.
        metrics.delivered("globex.relay.test", 4, Outcome::Delivered);
        metrics.backlog("globex.relay.test", 900);

        let rendered = metrics.render();
        assert!(
            rendered.contains("relay_delivery_millis_bucket{le=\"100\",tenant=\"acme\"} 0"),
            "the latency says acme is slow: {rendered}"
        );
        assert!(
            rendered.contains("relay_backlog_deliveries{tenant=\"globex\"} 900"),
            "and the backlog says globex is behind: a latency without a depth says something is \
             slow and not what to do about it: {rendered}"
        );
    }
}
