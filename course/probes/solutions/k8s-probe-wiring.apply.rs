//! probe = "k8s/probe-wiring@apply"
//! title = "Two questions, two consequences"
//! hint = "Liveness asks whether the process is alive (its failure restarts it); readiness asks whether it can serve (its failure only removes it from the endpoints)."
//!
//! Step `platform/9.1-probes` is the manifest side of the `/healthz` vs `/readyz` split. This probe
//! names what each probe is wired to, so the audit is about the wiring rather than the endpoint.
//!
//! ## What you write
//!
//! [`audit`] — the wiring problems, sorted:
//!
//! * [`Issue::LivenessOnDependencies`] — a liveness probe on a dependency restarts a healthy process;
//! * [`Issue::NoReadiness`] — no readiness probe means traffic reaches a pod that cannot serve;
//! * [`Issue::ReadinessChecksNothing`] — a readiness probe that always passes says nothing.
//!
//! ## What the tests ask
//!
//! A clean wiring; a liveness probe on the dependencies; no readiness probe; a readiness probe that
//! checks nothing; both readiness problems; and a readiness probe on the process, which is allowed.

/// What a probe is wired to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// The process itself.
    Process,
    /// Its dependencies.
    Dependencies,
    /// Nothing: it always passes.
    Nothing,
}

/// The probes of one workload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probes {
    /// What the liveness probe checks.
    pub liveness: Check,
    /// What the readiness probe checks, if there is one.
    pub readiness: Option<Check>,
}

/// Something the wiring gets wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Issue {
    /// A liveness probe on a dependency.
    LivenessOnDependencies,
    /// No readiness probe at all.
    NoReadiness,
    /// A readiness probe that checks nothing.
    ReadinessChecksNothing,
}

/// Every wiring problem, sorted.
#[must_use]
pub fn audit(probes: Probes) -> Vec<Issue> {
    let mut issues = Vec::new();
    // A liveness probe's failure restarts the process: on a dependency, the restart is the outage.
    if probes.liveness == Check::Dependencies {
        issues.push(Issue::LivenessOnDependencies);
    }
    match probes.readiness {
        None => issues.push(Issue::NoReadiness),
        // A readiness probe that checks nothing looks like a check and lets every pod through.
        Some(Check::Nothing) => issues.push(Issue::ReadinessChecksNothing),
        Some(_) => {}
    }
    issues.sort();
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probes(liveness: Check, readiness: Option<Check>) -> Probes {
        Probes {
            liveness,
            readiness,
        }
    }

    #[test]
    fn a_clean_wiring_has_nothing_to_report() {
        assert_eq!(audit(probes(Check::Process, Some(Check::Dependencies))), []);
    }

    #[test]
    fn a_readiness_probe_on_the_process_is_allowed() {
        assert_eq!(audit(probes(Check::Process, Some(Check::Process))), []);
    }

    #[test]
    fn a_liveness_probe_on_dependencies_restarts_a_healthy_process() {
        assert_eq!(
            audit(probes(Check::Dependencies, Some(Check::Dependencies))),
            [Issue::LivenessOnDependencies]
        );
    }

    #[test]
    fn no_readiness_probe_gets_traffic_it_cannot_serve() {
        assert_eq!(audit(probes(Check::Process, None)), [Issue::NoReadiness]);
    }

    #[test]
    fn a_readiness_probe_that_checks_nothing_says_nothing() {
        assert_eq!(
            audit(probes(Check::Process, Some(Check::Nothing))),
            [Issue::ReadinessChecksNothing]
        );
    }

    #[test]
    fn every_issue_is_reported_and_sorted() {
        assert_eq!(
            audit(probes(Check::Dependencies, Some(Check::Nothing))),
            [Issue::LivenessOnDependencies, Issue::ReadinessChecksNothing]
        );
    }
}
