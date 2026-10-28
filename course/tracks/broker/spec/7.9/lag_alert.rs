//! Spec for `broker/7.9-lag-alert` — alert on the run that says "behind", not on one sample.

use broker::alert::{Alert, evaluate};

#[test]
fn a_history_shorter_than_the_run_alerts_nothing() {
    assert_eq!(evaluate(&[10], 5, 2), Alert::None);
    assert_eq!(evaluate(&[], 5, 1), Alert::None);
}

#[test]
fn a_lag_at_the_threshold_is_not_past_it() {
    assert_eq!(evaluate(&[6, 5], 5, 2), Alert::None);
}

#[test]
fn a_run_above_the_threshold_alerts() {
    assert_eq!(
        evaluate(&[3, 6, 7], 5, 2),
        Alert::Lagging { samples: 2, lag: 7 }
    );
}

#[test]
fn a_dip_resets_the_run() {
    assert_eq!(evaluate(&[9, 9, 1, 9], 5, 3), Alert::None);
    assert_eq!(
        evaluate(&[9, 1, 9, 9], 5, 2),
        Alert::Lagging { samples: 2, lag: 9 }
    );
}

#[test]
fn the_run_reported_is_the_trailing_one() {
    assert_eq!(
        evaluate(&[9, 9, 9, 9], 5, 2),
        Alert::Lagging { samples: 4, lag: 9 }
    );
}

#[test]
fn a_run_of_zero_is_no_alert() {
    assert_eq!(evaluate(&[9, 9], 5, 0), Alert::None);
}
