//! Spec for `lakelet/4.20-capacity-model`: a model of what the store costs, and the comparison that
//! says whether it still matches the store.
//!
//! `kiln run` copies this file into the project as `tests/spec_capacity.rs`; it is the course's file,
//! not the learner's (plan §3 D2). Fourteen cases, and the shape of them is the point:
//!
//! * **the model is a value with units.** [`Model`] is named integers — bytes per sample, bytes per
//!   series per segment, bytes per record in the log, bytes per query — and [`Model::DEFAULT`] is the
//!   project's answer. The first two cases measure the store the way the constants' own documentation
//!   says they were measured (a sealed segment's bytes over its records, an index file's bytes over
//!   the series sharing the segment) and check the model against that, at the tolerance the model
//!   itself states;
//! * **the arithmetic is arithmetic.** Bytes per day is linear in the event rate and in the sample
//!   cost, retention is the multiplier, a retention of zero means what one day holds rather than
//!   nothing, and `total == ingest + storage + query` is checked as an identity over a matrix of
//!   usages rather than asserted in prose;
//! * **the money has no floating point in it.** The month is a named constant, a year is twelve of
//!   those months exactly, and the shares are per-mille integers that add up to a thousand;
//! * **a comparison, in code.** [`Measured`] is what a store wrote and [`Model::agrees`] says whether
//!   the model is close enough — so "the model drifted" is a failing case with a direction and a
//!   constant's name in it, not a stale sentence on a page;
//! * **the page is the model's own output.** `docs/capacity.md` carries a fenced block that is one
//!   call to the model, line for line, and the case re-runs that call and compares. Its three-row
//!   retention table is three more calls, checked the same way: a number edited by hand stops
//!   matching, which is the only way a page full of numbers stays true (`lakelet/4.16-profiling`'s
//!   rule, and `lakelet/4.19-backup-restore` applies it to a command);
//! * **the module reads nothing.** `src/engine/cost.rs` names no service-layer module and opens no
//!   file, no directory and no clock: a model is not a measurement, and this is where that stops
//!   being a slogan. Measuring the store is `tests/budget_cost.rs`'s job.
//!
//! The store every byte case works on is the write path's own — `Spool::append` for the log,
//! `Segments::seal` for the segment — so the model is compared with a store a deployment would have,
//! and the queries go through the step's planner and executor so that "bytes a query reads" is a
//! plan's decision rather than this file's arithmetic.

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lakelet::engine::compaction::{Segments, file_of};
use lakelet::engine::cost::{
    Agreement, DAYS_PER_MONTH, Drift, Estimate, MICROS, MONTHS_PER_YEAR, Measured, Model, Reading,
    Usage,
};
use lakelet::engine::index;
use lakelet::engine::query::exec;
use lakelet::engine::query::parse::{Statement, parse};
use lakelet::engine::query::plan::Plan;
use lakelet::engine::spool::Spool;
use lakelet::protocol::batch::{self, Record};

use support::StoreDir;

/// The page this step delivers, relative to the project.
const PAGE: &str = "docs/capacity.md";

/// Segments the fixture's store is built from.
const SEGMENTS: u64 = 24;

/// Records one sealed segment holds: thirty of its own host and thirty of the fleet's.
const PER_SERIES: usize = 30;

/// Milliseconds between two samples of one series: two minutes, so one hour is thirty samples.
const STEP: u64 = 120_000;

/// One hour — the width of a segment, of a window, and of the arithmetic below.
const HOUR: u64 = 3_600_000;

/// The first timestamp of the fixture's store, on an hour boundary.
const EPOCH: u64 = 472_222 * HOUR;

/// The first hour a fixture's queries ask about: half the store.
const FROM_HOUR: u64 = SEGMENTS / 2;

/// The series every segment carries, so that a segment's vocabulary is never one term.
const FLEET: &str = "fleet";

/// The host the selective query asks for: a series that appears in exactly one segment.
const SELECTIVE: u64 = 20;

/// Days the page's retention table reports, smallest first.
const RETENTIONS: [u64; 3] = [7, 30, 365];

/// The day the page's list prices were read, as `docs/capacity.md` states it.
const PRICED: &str = "2026-10-08";

/// What the page has to say to be the page this step promised, one claim per entry.
///
/// The list is deliberately about *claims* rather than wording: the price list and the date it was
/// read, the multiplier a person chooses, the cardinality term, the pruning that keeps a query cheap,
/// and the three things the model does not model at all. A page missing one of them has answered a
/// narrower question than the title asks.
const CLAIMS: [&str; 8] = [
    "retention",
    "list price",
    "cardinality",
    "pruning",
    "latency",
    "compaction",
    "manifest",
    "does not",
];

/// The records one sealed segment holds: its own host, then the fleet's.
fn segment_records(segment: u64) -> Vec<Record> {
    let start = EPOCH + segment * HOUR;
    let mut records: Vec<Record> = (0..PER_SERIES)
        .map(|offset| Record {
            series: format!("cpu,host=h{segment}"),
            ts_millis: (start + offset as u64 * STEP) as i64,
            value: value_of(segment * PER_SERIES as u64 + offset as u64),
        })
        .collect();
    records.extend((0..PER_SERIES).map(|offset| Record {
        series: format!("cpu,host={FLEET}"),
        ts_millis: (start + offset as u64 * STEP) as i64,
        value: value_of(segment * PER_SERIES as u64 + offset as u64),
    }));
    records
}

/// The value of sample `position`: distinct bit patterns, and no structure a column can exploit.
///
/// The bytes a segment costs are half of what the model's constants are, and a store of values a
/// compressor likes would make this fixture an argument about `zstd` rather than about the model.
fn value_of(position: u64) -> f64 {
    let mixed = position.wrapping_mul(2_654_435_761) % 100_003;
    mixed as f64 / 8.0
}

/// One query of this fixture, spelled over the last half of the store.
fn text(series: &str) -> String {
    format!(
        "SELECT count(value) FROM cpu WHERE host = '{series}' AND ts BETWEEN {} AND {} \
         GROUP BY time(1h), series",
        EPOCH + FROM_HOUR * HOUR,
        EPOCH + SEGMENTS * HOUR
    )
}

/// The bytes a file costs on the disk, or zero when it is not there.
fn bytes_of(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

/// Write the fixture's store through the real write path and count what it weighs.
///
/// Returns the directory with the measurement, because the directory removes itself when it is
/// dropped and a measurement of a store that no longer exists is a number nobody can check.
fn built(label: &str) -> (StoreDir, Measured) {
    let directory = StoreDir::new(label);
    let mut store = Segments::open(directory.path())
        .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
    let mut spool = Spool::open(directory.path())
        .unwrap_or_else(|error| panic!("the same directory holds the log: {error}"));
    let mut vocabulary: BTreeSet<String> = BTreeSet::new();
    let mut measured = Measured {
        segments: 0,
        records: 0,
        series: 0,
        series_slots: 0,
        segment_bytes: 0,
        index_bytes: 0,
        log_bytes: 0,
        queries: Vec::new(),
    };
    for segment in 0..SEGMENTS {
        let records = segment_records(segment);
        spool
            .append(&batch::encode(&records))
            .unwrap_or_else(|error| {
                panic!("segment {segment} is a payload the log takes: {error}")
            });
        store
            .seal(&records)
            .unwrap_or_else(|error| panic!("segment {segment} is what a flush closes: {error}"));
        measured.segments += 1;
        measured.records += records.len() as u64;
        let local: BTreeSet<String> = records.iter().map(|record| record.series.clone()).collect();
        measured.series_slots += local.len() as u64;
        vocabulary.extend(local);
        measured.segment_bytes += bytes_of(&file_of(directory.path(), segment));
        measured.index_bytes += bytes_of(&index::file_of(directory.path(), segment));
    }
    measured.series = vocabulary.len() as u64;
    measured.log_bytes = bytes_of(&spool.path());

    let snapshot = store.snapshot();
    for (quantity, series) in [
        ("selective_query_bytes", format!("h{SELECTIVE}")),
        ("wide_query_bytes", FLEET.to_owned()),
    ] {
        let query = parse(&text(&series))
            .unwrap_or_else(|refusal| panic!("the {quantity} query is this grammar's: {refusal}"));
        let Statement::Query(query) = query else {
            panic!("the fixture asks a query, not a plan of one")
        };
        let plan = Plan::of(&snapshot, &query)
            .unwrap_or_else(|refusal| panic!("the {quantity} query is a plan: {refusal}"));
        let outcome = exec::execute(&snapshot, &query)
            .unwrap_or_else(|refusal| panic!("the {quantity} query runs: {refusal}"));
        measured.queries.push(Reading {
            quantity,
            records: outcome.rows_scanned(),
            bytes: plan
                .opened()
                .iter()
                .fold(0u64, |total, info| total.saturating_add(info.bytes)),
        });
    }
    (directory, measured)
}

/// The page this step delivers, from the project it is part of.
fn page() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(PAGE);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("{PAGE} is the page this step delivers, and it cannot be read: {error}")
    })
}

/// The fenced block a page quotes: the one carrying the model's own output.
///
/// The block is identified by its first line rather than by a heading, because what makes it the
/// model's output is that it *is* the model's output — `Estimate::lines` starts at `events_per_day`
/// and ends at the cost per million events, and a page may quote a fragment of the same answer
/// elsewhere without that fragment being the block this case compares.
fn quoted(page: &str) -> Vec<String> {
    let lines: Vec<&str> = page.lines().collect();
    let anchor = lines
        .iter()
        .position(|line| line.starts_with("events_per_day="))
        .unwrap_or_else(|| {
            panic!(
                "{PAGE} does not quote the model: a reader has to be able to see the answer the \
                 model gives for the usage the page is about, and no line of it starts at \
                 `events_per_day=`"
            )
        });
    let opening = lines[..anchor]
        .iter()
        .rposition(|line| line.trim_start().starts_with("```"))
        .unwrap_or_else(|| {
            panic!(
                "{PAGE} has the model's first line at {anchor} outside a fenced block: a reader \
                 has to be able to copy the block out of the page"
            )
        });
    let closing = anchor
        + lines[anchor..]
            .iter()
            .position(|line| line.trim_start().starts_with("```"))
            .unwrap_or_else(|| panic!("{PAGE} opens a fenced block it never closes"));
    let body: Vec<String> = lines[opening + 1..closing]
        .iter()
        .map(|line| (*line).to_owned())
        .collect();
    assert!(
        body.last()
            .is_some_and(|line| line.starts_with("cost_per_million_events_micros=")),
        "{PAGE}'s block does not end where the model's answer ends: quote the whole thing — the \
         question's four numbers to the cost per million events — so that a reader sees what the \
         spec compares"
    );
    body
}

/// The retention table a page carries, in the order the page lists it: `(days, stored_bytes,
/// storage_micros, monthly_micros, per_million)`.
///
/// A row is a markdown table line whose first five cells are all numbers. Anything else in a table is
/// a heading or a separator and is not a row.
fn table(page: &str) -> Vec<(u64, u64, u64, u64, u64)> {
    let mut rows: Vec<(u64, u64, u64, u64, u64)> = Vec::new();
    for line in page.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = trimmed
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        let Some(cells) = cells.get(..5) else {
            continue;
        };
        let numbers: Option<Vec<u64>> = cells.iter().map(|cell| cell.parse::<u64>().ok()).collect();
        let Some(cells) = numbers else {
            continue;
        };
        let [days, stored, storage, monthly, per_million] = cells[..] else {
            continue;
        };
        rows.push((days, stored, storage, monthly, per_million));
    }
    rows
}

/// The drift an agreement reported for one quantity.
fn drift_of(agreement: &Agreement, quantity: &str) -> Drift {
    agreement
        .drifts
        .iter()
        .find(|drift| drift.quantity == quantity)
        .copied()
        .unwrap_or_else(|| {
            panic!(
                "the model reported no drift for `{quantity}`, so nothing compared it with the \
                 store: every quantity the harness prints has to be one the comparison answers for"
            )
        })
}

/// One source file of the project, as text.
fn source(relative: &str) -> String {
    std::fs::read_to_string(relative).unwrap_or_else(|error| {
        panic!("{relative} cannot be read: {error} — this case is about it")
    })
}

#[test]
fn the_models_sample_cost_is_what_the_stores_encoding_wrote() {
    let (_directory, measured) = built("spec-cost-sample");
    let model = Model::DEFAULT;
    let predicted = model.predicts(&measured);
    let rate = measured.segment_bytes.saturating_mul(1000) / measured.records.max(1);
    assert!(
        rate.abs_diff(model.bytes_per_sample.saturating_mul(1000)) <= model.agreement_permille,
        "the store's own encoding produced {rate} bytes per thousand records and `bytes_per_sample` \
         says {} — {:?} bytes over {:?} records. The constant was measured on this workload \
         (`tests/budget_cost.rs`), so a disagreement means one of the two changed: re-measure the \
         constant from the harness's output rather than widening the tolerance",
        model.bytes_per_sample.saturating_mul(1000),
        measured.segment_bytes,
        measured.records
    );
    let agreement = model.agrees(&measured);
    assert!(
        agreement.within,
        "the shipped model does not agree with the store it was measured on: {agreement}"
    );
    assert_eq!(
        drift_of(&agreement, "segment_bytes").permille,
        23,
        "the model's segment bytes against the store's: {agreement}"
    );
    assert_eq!(
        predicted.on_disk_bytes,
        predicted
            .segment_bytes
            .saturating_add(predicted.index_bytes)
            .saturating_add(predicted.log_bytes),
        "the model's own total is not the three file sets added"
    );
}

#[test]
fn the_series_overhead_is_one_index_files_share_of_its_series() {
    let (directory, measured) = built("spec-cost-series");
    let model = Model::DEFAULT;
    // One sidecar file, the way `lakelet/4.14-index-pruning` writes it: a file per closed segment,
    // whose size is the same whatever the segment holds. What a *series* costs is that file divided
    // by the series sharing the segment, which is why the workload matters to the constant.
    let file = index::file_of(directory.path(), 0);
    let series_in_segment = segment_records(0)
        .iter()
        .map(|record| record.series.clone())
        .collect::<BTreeSet<String>>()
        .len() as u64;
    let per_series = bytes_of(&file) / series_in_segment.max(1);
    assert_eq!(
        per_series,
        model.series_overhead_bytes,
        "{} is {} bytes for {series_in_segment} series, so a series costs {per_series} — and \
         `series_overhead_bytes` says {}. The constant is the sidecar's share and the file's size is \
         the layout's (`lakelet/4.14-index-pruning`); if the layout changed, the constant has to \
         change with it",
        file.display(),
        bytes_of(&file),
        model.series_overhead_bytes
    );
    let agreement = model.agrees(&measured);
    assert_eq!(
        model.predicts(&measured).index_bytes,
        measured.index_bytes,
        "the model's sidecar bytes are not the store's: {} predicted over {} (segment, series) pairs \
         against {} written. The term is exact — a fixed file per segment and the constants are both \
         integers — so a difference is a different number of series or segments, not rounding",
        model.predicts(&measured).index_bytes,
        measured.series_slots,
        measured.index_bytes
    );
    assert_eq!(
        drift_of(&agreement, "index_bytes").permille,
        0,
        "the sidecar comparison is exact: {agreement}"
    );
}

#[test]
fn the_storage_term_is_linear_in_the_event_rate_and_in_the_sample_cost() {
    let model = Model::DEFAULT;
    let usage = Usage::DEFAULT;
    // Doubling the load doubles the samples' bytes, exactly, in integers.
    let doubled = Usage {
        events_per_day: usage.events_per_day.saturating_mul(2),
        ..usage
    };
    assert_eq!(
        model.sample_bytes_per_day(&doubled),
        model.sample_bytes_per_day(&usage).saturating_mul(2),
        "the samples' bytes are `events x bytes_per_sample`: twice the events is twice the bytes, \
         with no rounding anywhere — that is the whole reason a capacity model is worth writing"
    );
    // And doubling the constant doubles the *samples*, not the total: the series and the log do not
    // care how big a record is.
    let fatter = Model {
        bytes_per_sample: model.bytes_per_sample.saturating_mul(2),
        ..model
    };
    assert_eq!(
        fatter.sample_bytes_per_day(&usage),
        model.sample_bytes_per_day(&usage).saturating_mul(2),
        "twice `bytes_per_sample` is twice the sample term"
    );
    assert_eq!(
        fatter.series_bytes_per_day(&usage),
        model.series_bytes_per_day(&usage),
        "and it leaves the series term alone: the sidecar does not care how big a record is"
    );
    // A day is the sum of its parts, whatever the load is: two halves add up. The series count is zero
    // here because it is the one term that does not follow the event rate, and this claim is about the
    // load.
    let load = |events: u64| Usage {
        events_per_day: events,
        series: 0,
        queries_per_day: 0,
        retention_days: 1,
    };
    assert_eq!(
        model.bytes_per_day(&load(1000)),
        model
            .bytes_per_day(&load(400))
            .saturating_add(model.bytes_per_day(&load(600))),
        "a day's bytes are additive in the load: the first four hundred events plus the next six \
         hundred are the thousand, to the byte"
    );
}

#[test]
fn retention_is_the_multiplier_and_zero_days_is_one_days_worth() {
    let model = Model::DEFAULT;
    let usage = Usage::DEFAULT;
    let per_day = model.bytes_per_day(&usage);
    assert_eq!(
        model.stored_bytes(&usage),
        per_day.saturating_mul(usage.retention_days),
        "the stored bytes are one day times the retention, which is the only multiplier in this \
         model that a person chooses"
    );
    let halved = Usage {
        retention_days: usage.retention_days / 2,
        ..usage
    };
    assert_eq!(
        model.stored_bytes(&halved),
        model.stored_bytes(&usage) / 2,
        "halving the retention halves what the disk holds"
    );
    let one = Usage {
        retention_days: 1,
        ..usage
    };
    let none = Usage {
        retention_days: 0,
        ..usage
    };
    assert_eq!(
        model.stored_bytes(&none),
        model.stored_bytes(&one),
        "a retention of zero days is what one day holds, not an empty store: a store that is \
         ingesting has a day's bytes on the disk before anything expires, and a model that answered \
         nothing would price every deployment that had not set the policy at zero"
    );
    assert_ne!(
        model.stored_bytes(&none),
        0,
        "and it is not nothing, which is the failure this decision is stated to avoid"
    );
    assert_eq!(
        usage.retention_days, model.retention_days,
        "the shipped usage runs at the retention the model's policy ships — the page's headline \
         question is about a store somebody is running"
    );
}

#[test]
fn the_total_is_the_three_shares_for_a_matrix_of_usages() {
    let model = Model::DEFAULT;
    let usages = [
        Usage::DEFAULT,
        Usage {
            events_per_day: 1,
            series: 1,
            queries_per_day: 1,
            retention_days: 1,
        },
        Usage {
            events_per_day: 10_000_000,
            series: 250_000,
            queries_per_day: 1_000_000,
            retention_days: 365,
        },
        Usage {
            events_per_day: 0,
            series: 0,
            queries_per_day: 0,
            retention_days: 0,
        },
    ];
    for usage in usages {
        let total = model.monthly_micros(&usage);
        assert_eq!(
            total,
            model
                .ingest_micros(&usage)
                .saturating_add(model.storage_micros(&usage))
                .saturating_add(model.query_micros(&usage)),
            "`total == ingest + storage + query` is the identity the page's block is built on, and \
             it has to hold for {usage:?}"
        );
        let shares = [
            model.ingest_share_permille(&usage),
            model.storage_share_permille(&usage),
            model.query_share_permille(&usage),
        ];
        if total == 0 {
            assert_eq!(
                shares,
                [0, 0, 0],
                "a usage with no load has no shares: {usage:?}"
            );
            continue;
        }
        assert_eq!(
            shares.iter().sum::<u64>(),
            1000,
            "the three shares of the bill are per-mille and add to a thousand (the largest carries \
             the rounding): {shares:?} for {usage:?}"
        );
        for (share, term) in [
            (shares[0], model.ingest_micros(&usage)),
            (shares[1], model.storage_micros(&usage)),
            (shares[2], model.query_micros(&usage)),
        ] {
            let wanted = term.saturating_mul(1000) / total;
            assert!(
                share.abs_diff(wanted) <= 1,
                "a share is its own term's share of the total: {share} against {wanted} for \
                 {usage:?} — the per-mille figure of {term} out of {total}"
            );
        }
        assert_eq!(
            model.yearly_micros(&usage),
            total.saturating_mul(MONTHS_PER_YEAR),
            "a year is twelve of the months the bill is computed for, not a second computation"
        );
    }
}

#[test]
fn the_page_block_is_what_the_model_prints() {
    let text = page();
    let block = quoted(&text);
    let printed: Vec<String> = Estimate::of(&Model::DEFAULT).lines();
    assert_eq!(
        printed, block,
        "{PAGE}'s block is not what the model prints for the shipped usage: the page's numbers are \
         the model's own output, and the way that stays true is that the call which produced them is \
         re-run and compared. Edit the page from the model, never from an intention — and never by \
         hand"
    );
}

#[test]
fn the_month_is_named_and_a_year_is_twelve_of_them() {
    let model = Model::DEFAULT;
    let usage = Usage::DEFAULT;
    assert_eq!(
        DAYS_PER_MONTH, 30,
        "the month every price is quoted per is a named constant, so that a reader of the page can \
         see which month the bill is for"
    );
    assert_eq!(
        model.ingest_micros(&usage),
        usage
            .events_per_day
            .saturating_mul(DAYS_PER_MONTH)
            .saturating_mul(model.price_per_million_events_micros)
            / MICROS,
        "the ingest term is charged per month, which is the period the price list states"
    );
    assert_eq!(
        model.yearly_micros(&usage),
        model.monthly_micros(&usage).saturating_mul(MONTHS_PER_YEAR),
        "a cost reported per month and per year agrees exactly in micro-units: the year is twelve \
         of those months, each rounded once"
    );
    assert_eq!(
        MONTHS_PER_YEAR, 12,
        "and there are twelve of them in a year"
    );
    // The arithmetic that had to be chosen between: a year of three hundred and sixty-five days,
    // computed once, is *not* twelve of the monthly figures. The model reports twelve months because
    // two answers for one year is a bill nobody can reconcile.
    let by_the_day = usage
        .events_per_day
        .saturating_mul(365)
        .saturating_mul(model.price_per_million_events_micros)
        / MICROS;
    assert_ne!(
        by_the_day,
        model.ingest_micros(&usage).saturating_mul(MONTHS_PER_YEAR),
        "a year computed from 365 days and a year computed as twelve months are different numbers, \
         which is why the model states which one it reports"
    );
}

#[test]
fn agreement_accepts_a_model_five_percent_off_and_refuses_one_twenty_off() {
    let model = Model::DEFAULT;
    // A store whose segments are what the model predicted, and one whose segments are not: the
    // measurement is stated by this case so that "five percent off" is arithmetic rather than a
    // percentage of an arbitrary number.
    let store = |records: u64, segment_bytes: u64| Measured {
        segments: records / (PER_SERIES as u64 * 2),
        records,
        series: 1,
        series_slots: 0,
        segment_bytes,
        index_bytes: 0,
        log_bytes: records.saturating_mul(model.log_bytes_per_sample),
        queries: Vec::new(),
    };
    let predicted = model.bytes_per_sample.saturating_mul(1000);
    let exact = store(1000, predicted);
    assert!(
        model.agrees(&exact).within,
        "a store that wrote exactly what the model predicts agrees with it: {}",
        model.agrees(&exact)
    );
    // Five percent over: the store wrote five percent fewer bytes than the model predicted, which is
    // inside the tolerance the model states.
    let five = store(1000, predicted.saturating_mul(95) / 100);
    let agreement = model.agrees(&five);
    assert_eq!(
        drift_of(&agreement, "segment_bytes").permille,
        50,
        "a model five percent over the store is fifty per-mille: {agreement}"
    );
    assert!(
        agreement.within,
        "and it is accepted, because the model states its own tolerance: {agreement}"
    );
    // Twenty percent over: outside it, and the refusal says which constant and which way.
    let twenty = store(1000, predicted.saturating_mul(80) / 100);
    let agreement = model.agrees(&twenty);
    let drift = drift_of(&agreement, "segment_bytes");
    assert_eq!(
        drift.permille, 200,
        "a model twenty percent over the store is two hundred per-mille: {agreement}"
    );
    assert!(
        !agreement.within,
        "and that is a model that has drifted from the store: {agreement}"
    );
    let said = agreement.to_string();
    assert!(
        said.contains("bytes_per_sample") && said.contains("over"),
        "the refusal names the constant that drifted and the direction it went, or an operator \
         cannot act on it: {said}"
    );
}

#[test]
fn the_drift_arithmetic_counts_per_mille_and_keeps_the_direction() {
    let model = Model::DEFAULT;
    let records = 1000u64;
    let predicted = model.bytes_per_sample.saturating_mul(records);
    let store = |segment_bytes: u64| Measured {
        segments: records / (PER_SERIES as u64 * 2),
        records,
        series: 1,
        series_slots: 0,
        segment_bytes,
        index_bytes: 0,
        log_bytes: records.saturating_mul(model.log_bytes_per_sample),
        queries: Vec::new(),
    };
    let exact = drift_of(&model.agrees(&store(predicted)), "segment_bytes");
    assert_eq!(
        (exact.permille, exact.direction),
        (0, lakelet::engine::cost::Direction::Exact),
        "the model and the store saying the same number is a drift of zero and no direction: {exact}"
    );
    // Ten percent more bytes than the model expected: one hundred per-mille, and the model is *under*
    // what the store wrote.
    let under = drift_of(
        &model.agrees(&store(predicted.saturating_mul(110) / 100)),
        "segment_bytes",
    );
    assert_eq!(
        (under.permille, under.direction),
        (100, lakelet::engine::cost::Direction::Under),
        "a measurement ten percent larger than the prediction is one hundred per-mille under: \
         {under}"
    );
    // Ten percent fewer: the same distance, the other way — an operator who is told "ten percent
    // off" and not which way has been told nothing they can act on.
    let over = drift_of(
        &model.agrees(&store(predicted.saturating_mul(90) / 100)),
        "segment_bytes",
    );
    assert_eq!(
        (over.permille, over.direction),
        (100, lakelet::engine::cost::Direction::Over),
        "a measurement ten percent smaller is one hundred per-mille over: {over}"
    );
    assert_ne!(
        under.direction, over.direction,
        "the two directions are different answers, not one distance"
    );
}

#[test]
fn the_query_term_is_what_the_plan_opened() {
    let (_directory, measured) = built("spec-cost-query");
    let model = Model::DEFAULT;
    let selective = measured
        .queries
        .iter()
        .find(|reading| reading.quantity == "selective_query_bytes")
        .copied()
        .expect("the fixture asks a selective query");
    let wide = measured
        .queries
        .iter()
        .find(|reading| reading.quantity == "wide_query_bytes")
        .copied()
        .expect("the fixture asks a wide query");
    // Pruning is by segment and not by series: the one segment the selective query opens holds the
    // host it asked for *and* the fleet, so sixty records is what it read and thirty of them are
    // answers.
    assert_eq!(
        selective.records,
        PER_SERIES as u64 * 2,
        "the selective query is pruned to one segment, and a segment is read whole"
    );
    assert_eq!(
        wide.records,
        PER_SERIES as u64 * 2 * (SEGMENTS - FROM_HOUR),
        "and the wide one asks for the series every segment carries, so the index rules nothing out \
         and the plan opens every segment the range meets — `lakelet/4.14-index-pruning` as the read \
         it costs rather than as a rate"
    );
    assert!(
        wide.bytes > selective.bytes,
        "a query the index cannot help reads more than one it can: {} against {}",
        wide.bytes,
        selective.bytes
    );
    let agreement = model.agrees(&measured);
    for reading in [selective, wide] {
        let predicted = model.query_bytes(&reading);
        let drift = drift_of(&agreement, reading.quantity);
        assert_eq!(
            drift.model, predicted,
            "the drift of `{}` is the model's own query arithmetic",
            reading.quantity
        );
        assert!(
            drift.permille <= model.agreement_permille,
            "the model prices `{}` at {predicted} against the store's {}: {drift}",
            reading.quantity,
            reading.bytes
        );
    }
    assert!(
        agreement.within,
        "the model agrees with the store about both readings and about the bytes: {agreement}"
    );
}

#[test]
fn the_series_term_is_priced_and_four_times_the_series_pay_four_times() {
    let model = Model::DEFAULT;
    let usage = Usage::DEFAULT;
    let term = model.series_bytes_per_day(&usage);
    assert!(
        term > 0,
        "cardinality is a term of the model and not a sentence about it: `series x \
         series_overhead_bytes` is {term} bytes a day at the shipped usage"
    );
    let four = Usage {
        series: usage.series.saturating_mul(4),
        ..usage
    };
    assert_eq!(
        model.series_bytes_per_day(&four),
        term.saturating_mul(4),
        "a store with four times the series pays four times this term, exactly"
    );
    let text = page();
    assert!(
        quoted(&text)
            .iter()
            .any(|line| line.starts_with("series_share_permille=")),
        "{PAGE}'s block does not carry the series term's share: the page has to say what cardinality \
         costs *on this store*, and the share is the model's own answer to that"
    );
    for claim in ["cardinality", "series_overhead_bytes"] {
        assert!(
            text.contains(claim),
            "{PAGE} does not argue the cardinality term (`{claim}` is nowhere on it): a number with \
             no argument behind it is the stale sentence this step exists to replace"
        );
    }
}

#[test]
fn the_step_stays_inside_the_engine_and_reads_nothing() {
    let module = source("src/engine/cost.rs");
    for forbidden in ["crate::service", "crate::cli"] {
        assert!(
            !module.contains(forbidden),
            "src/engine/cost.rs names `{forbidden}` — the command line uses the engine, never the \
             other way (`lakelet/4.1-ingest-protocol`'s boundary, and every step since keeps it)"
        );
    }
    // A model is not a measurement: it is arithmetic over values a caller hands it. Every one of
    // these would be the module measuring something, and every one of them is a way for the answer to
    // depend on the machine it ran on.
    for forbidden in [
        "std::fs",
        "std::path",
        "std::env",
        "std::time",
        "std::thread",
        "std::process",
        "File::",
        "OpenOptions",
        "read_to_string",
        "read_dir",
        "metadata",
        "SystemTime",
        "Instant",
        "rand",
    ] {
        assert!(
            !module.contains(forbidden),
            "src/engine/cost.rs contains `{forbidden}`: the model reads nothing — no file, no \
             directory, no clock, no environment, no seeded generator. Measuring the store is \
             `tests/budget_cost.rs`'s job, and the harness hands the numbers in"
        );
    }
    let harness = source("tests/budget_cost.rs");
    for needed in ["Spool", "Segments", "Model::DEFAULT", "agrees"] {
        assert!(
            harness.contains(needed),
            "tests/budget_cost.rs does not use `{needed}`: the course ships that harness complete, and \
             it is the half of this step that is allowed to touch a store"
        );
    }
}

#[test]
fn the_page_exists_says_what_the_step_is_about_and_has_no_blanks() {
    let text = page();
    let lowered = text.to_ascii_lowercase();
    for placeholder in ["todo", "tbd", "fixme"] {
        assert!(
            !lowered.contains(placeholder),
            "{PAGE} still says `{placeholder}`: a page with a blank in it is a page nobody has \
             written down"
        );
    }
    assert!(
        text.contains(PRICED),
        "{PAGE} does not state the date its list prices were read ({PRICED}): the one input this \
         model does not measure is the one that needs a source and a date beside it"
    );
    for claim in CLAIMS {
        assert!(
            lowered.contains(claim),
            "{PAGE} does not say `{claim}`, which is one of the claims this page promised: the list \
             price and when it was read, the retention tradeoff, the cardinality term, the pruning \
             that keeps a query cheap, and the three things the model does not model"
        );
    }
    assert!(
        quoted(&text).len() == Estimate::of(&Model::DEFAULT).lines().len(),
        "{PAGE}'s block is not the model's whole answer: a page that quotes half of it is a page \
         that cannot be regenerated"
    );
}

#[test]
fn the_retention_table_is_three_calls_to_the_model() {
    let text = page();
    let rows = table(&text);
    assert_eq!(
        rows.len(),
        RETENTIONS.len(),
        "{PAGE}'s retention table has {} row(s) and this step promised one per retention \
         ({RETENTIONS:?}): the tradeoff is the thing a person actually decides, so it is a table and \
         not a sentence",
        rows.len()
    );
    let model = Model::DEFAULT;
    for (row, days) in rows.iter().zip(RETENTIONS) {
        let usage = Usage {
            retention_days: days,
            ..Usage::DEFAULT
        };
        assert_eq!(
            (row.0, row.1, row.2, row.3, row.4),
            (
                days,
                model.stored_bytes(&usage),
                model.storage_micros(&usage),
                model.monthly_micros(&usage),
                model.cost_per_million_events_micros(&usage),
            ),
            "the row for {days} days is not what the model answers: every cell of this table is a \
             call to the same model the block quotes, and a hand-edited cell is a number nobody can \
             reproduce"
        );
    }
    let first = rows.first().expect("a first row");
    let last = rows.last().expect("a last row");
    assert!(
        last.1 > first.1,
        "keeping data longer holds more bytes: {} against {}",
        last.1,
        first.1
    );
    let difference = last.3.saturating_sub(first.3);
    assert!(
        text.contains(&difference.to_string()),
        "{PAGE} does not state what the difference between {} and {} days costs ({difference} \
         micro-units a month): the middle row is what a policy change costs, and the ends are what \
         \"keep it for ever\" costs on this store",
        first.0,
        last.0
    );
}
