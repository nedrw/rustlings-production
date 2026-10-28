//! Spec for `platform/9.1-probes` — liveness is about the process, readiness is about serving.

use platform::{Finding, Probes, audit};

fn probes(
    liveness_checks_dependencies: bool,
    has_readiness: bool,
    readiness_checks_something: bool,
) -> Probes {
    Probes {
        liveness_checks_dependencies,
        has_readiness,
        readiness_checks_something,
    }
}

#[test]
fn a_clean_wiring_has_nothing_to_report() {
    assert_eq!(audit(probes(false, true, true)), []);
}

#[test]
fn a_liveness_probe_on_a_dependency_restarts_a_healthy_process() {
    assert_eq!(
        audit(probes(true, true, true)),
        [Finding::LivenessChecksDependency]
    );
}

#[test]
fn a_service_without_a_readiness_probe_gets_traffic_it_cannot_serve() {
    assert_eq!(
        audit(probes(false, false, false)),
        [Finding::NoReadinessProbe]
    );
}

#[test]
fn a_readiness_probe_that_always_passes_says_nothing() {
    assert_eq!(
        audit(probes(false, true, false)),
        [Finding::ReadinessAlwaysPasses]
    );
}

#[test]
fn every_problem_is_reported_and_sorted() {
    let findings = audit(probes(true, false, false));
    assert_eq!(
        findings,
        [Finding::LivenessChecksDependency, Finding::NoReadinessProbe]
    );
}

#[test]
fn both_readiness_problems_are_reported_together() {
    let findings = audit(probes(true, true, false));
    assert_eq!(
        findings,
        [
            Finding::LivenessChecksDependency,
            Finding::ReadinessAlwaysPasses
        ]
    );
}
