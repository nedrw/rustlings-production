//! Spec for `cache/6.8-cache-failure` — a breaker in front of the cache: open, probe, close.

use cache::breaker::{Breaker, Route, State};

fn breaker() -> Breaker {
    // Two failures open it; it stays open for 5 seconds.
    Breaker::new(2, 5)
}

#[test]
fn a_closed_breaker_uses_the_cache() {
    let mut breaker = breaker();
    assert_eq!(breaker.state(), State::Closed);
    assert_eq!(breaker.route(0), Route::Cache);
}

#[test]
fn failures_open_the_breaker() {
    let mut breaker = breaker();
    breaker.record(false, 1);
    assert_eq!(breaker.route(2), Route::Cache, "one failure is not enough");
    breaker.record(false, 3);
    assert_eq!(breaker.state(), State::Open);
    assert_eq!(breaker.route(4), Route::Direct);
}

#[test]
fn an_open_breaker_stays_direct_until_the_cooldown() {
    let mut breaker = breaker();
    breaker.record(false, 10);
    breaker.record(false, 10);
    assert_eq!(breaker.route(14), Route::Direct);
    assert_eq!(breaker.state(), State::Open);
}

#[test]
fn the_first_call_after_the_cooldown_is_the_probe() {
    let mut breaker = breaker();
    breaker.record(false, 10);
    breaker.record(false, 10);
    assert_eq!(breaker.route(15), Route::Cache);
    assert_eq!(breaker.state(), State::HalfOpen);
}

#[test]
fn only_one_probe_is_handed_out() {
    let mut breaker = breaker();
    breaker.record(false, 10);
    breaker.record(false, 10);
    assert_eq!(breaker.route(15), Route::Cache, "the probe");
    assert_eq!(
        breaker.route(15),
        Route::Direct,
        "a second caller took a probe too"
    );
}

#[test]
fn a_failure_while_open_does_not_extend_the_cooldown() {
    let mut breaker = breaker();
    breaker.record(false, 10);
    breaker.record(false, 10);
    // The cache is not being called while the breaker is open; a failure recorded anyway must not
    // push `opened_at` forward.
    breaker.record(false, 12);
    assert_eq!(breaker.route(15), Route::Cache);
}

#[test]
fn a_success_closes_and_forgets() {
    let mut breaker = breaker();
    breaker.record(false, 1);
    breaker.record(false, 2);
    breaker.record(true, 3);
    assert_eq!(breaker.state(), State::Closed);
    assert_eq!(breaker.failures(), 0);
    assert_eq!(breaker.route(4), Route::Cache);
}

#[test]
fn a_failure_in_half_open_reopens_and_restarts_the_cooldown() {
    let mut breaker = breaker();
    breaker.record(false, 10);
    breaker.record(false, 10);
    assert_eq!(breaker.route(15), Route::Cache);
    breaker.record(false, 15);

    assert_eq!(breaker.state(), State::Open);
    assert_eq!(
        breaker.route(19),
        Route::Direct,
        "the cooldown did not restart"
    );
    assert_eq!(breaker.route(20), Route::Cache);
}
