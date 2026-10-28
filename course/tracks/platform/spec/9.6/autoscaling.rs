//! Spec for `platform/9.6-autoscaling` — the formula, the band, and the asymmetry.

use platform::autoscaling::{Stabilizer, desired};

#[test]
fn the_formula_scales_by_the_ratio() {
    // Three pods at double the target ask for six.
    assert_eq!(desired(3, 200, 100, 1, 10), 6);
    // Three pods at half the target ask for two (ceil of 1.5).
    assert_eq!(desired(3, 50, 100, 1, 10), 2);
}

#[test]
fn a_metric_inside_the_band_leaves_the_count_alone() {
    assert_eq!(desired(3, 105, 100, 1, 10), 3);
    assert_eq!(desired(3, 95, 100, 1, 10), 3);
}

#[test]
fn the_band_boundary_is_outside() {
    // 111% is past the band: the formula runs.
    assert_eq!(desired(3, 111, 100, 1, 10), 4);
}

#[test]
fn the_answer_is_clamped_to_the_bounds() {
    assert_eq!(desired(3, 1_000, 100, 1, 5), 5);
    assert_eq!(desired(3, 1, 100, 2, 10), 2);
}

#[test]
fn a_target_of_zero_keeps_the_current_count() {
    assert_eq!(desired(3, 200, 0, 1, 10), 3);
}

#[test]
fn scaling_up_is_immediate() {
    let mut stabilizer = Stabilizer::new(3, 5);
    assert_eq!(stabilizer.observe(6), 6);
}

#[test]
fn scaling_down_waits_for_the_run() {
    let mut stabilizer = Stabilizer::new(6, 3);
    assert_eq!(stabilizer.observe(3), 6);
    assert_eq!(stabilizer.observe(3), 6);
    assert_eq!(stabilizer.observe(3), 3, "three low windows scale down");
}

#[test]
fn a_spike_resets_the_downward_run() {
    let mut stabilizer = Stabilizer::new(6, 3);
    stabilizer.observe(3);
    stabilizer.observe(3);
    assert_eq!(stabilizer.observe(9), 9);
    assert_eq!(stabilizer.observe(3), 9, "the run started over");
    assert_eq!(stabilizer.observe(3), 9);
    assert_eq!(stabilizer.observe(3), 3);
}
