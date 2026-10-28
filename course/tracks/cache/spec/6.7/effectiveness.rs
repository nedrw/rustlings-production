//! Spec for `cache/6.7-cache-effectiveness` — hit ratio, origin calls, and the SLO boundary.

use cache::effectiveness::{Lookup, Verdict, report, verdict};

#[test]
fn the_ratio_is_hits_over_lookups() {
    let report = report(&[Lookup::Hit, Lookup::Hit, Lookup::Miss, Lookup::Hit]);
    assert_eq!(report.hits, 3);
    assert_eq!(report.misses, 1);
    assert_eq!(report.lookups(), 4);
    assert!(
        (report.ratio() - 0.75).abs() < f64::EPSILON,
        "{}",
        report.ratio()
    );
    assert_eq!(report.origin_calls(), 1);
}

#[test]
fn an_empty_window_is_healthy() {
    let report = report(&[]);
    assert_eq!(report.ratio(), 1.0);
    assert_eq!(verdict(&report, 0.9), Verdict::Healthy);
}

#[test]
fn the_boundary_belongs_to_healthy() {
    let mut lookups = vec![Lookup::Miss];
    lookups.extend(vec![Lookup::Hit; 9]);
    let report = report(&lookups);
    assert!((report.ratio() - 0.9).abs() < f64::EPSILON);
    assert_eq!(verdict(&report, 0.9), Verdict::Healthy);
}

#[test]
fn below_the_slo_is_below() {
    let mut lookups = vec![Lookup::Miss; 2];
    lookups.extend(vec![Lookup::Hit; 8]);
    assert_eq!(verdict(&report(&lookups), 0.9), Verdict::BelowSlo);
}

// Pins given code: `Report` counts what it is told; the step is `ratio`/`verdict`, which the other tests fail on.
#[test]
fn every_miss_is_an_origin_call() {
    let report = report(&[Lookup::Miss, Lookup::Hit, Lookup::Miss]);
    assert_eq!(report.origin_calls(), 2);
    assert_eq!(report.lookups(), 3);
}
