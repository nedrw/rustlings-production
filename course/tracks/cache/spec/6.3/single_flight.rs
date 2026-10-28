//! Spec for `cache/6.3-single-flight` — one filler per key, and everybody else waits for it.

use cache::single_flight::{Decision, InFlight};

#[test]
fn the_first_asker_fills_and_the_second_waits() {
    let mut flight = InFlight::new();
    assert_eq!(flight.begin("a"), Decision::Fill);
    assert_eq!(flight.begin("a"), Decision::Wait);
}

#[test]
fn completing_a_fill_lets_the_next_asker_fill() {
    let mut flight = InFlight::new();
    flight.begin("a");
    assert!(flight.complete("a"));
    assert_eq!(flight.begin("a"), Decision::Fill);
}

#[test]
fn completing_a_key_that_is_not_in_flight_is_false() {
    let mut flight = InFlight::new();
    assert!(!flight.complete("a"));
}

#[test]
fn two_keys_fill_independently() {
    let mut flight = InFlight::new();
    assert_eq!(flight.begin("a"), Decision::Fill);
    assert_eq!(flight.begin("b"), Decision::Fill);
    assert_eq!(flight.len(), 2);
}

#[test]
fn waiting_does_not_register_a_second_filler() {
    let mut flight = InFlight::new();
    flight.begin("a");
    for _ in 0..10 {
        assert_eq!(flight.begin("a"), Decision::Wait);
    }
    assert_eq!(flight.len(), 1, "a waiter was registered as a filler");
}

#[test]
fn after_completion_every_waiter_is_a_filler_again() {
    let mut flight = InFlight::new();
    flight.begin("a");
    flight.begin("a");
    flight.complete("a");
    assert_eq!(flight.begin("a"), Decision::Fill);
}
