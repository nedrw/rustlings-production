//! The platform layer a service ships on: probes, rollouts, budgets, secrets and identity.
//!
//! Every step of this track adds to the library; the binary stays a shell. There is no cluster
//! here: a manifest is data, a rollout is arithmetic, and a token is a struct.

// The region later scaffolds are allowed to write into (authoring.md §5). A scaffold may only
// *insert* lines inside it, and only *remove* nothing.
// kiln:scaffold:begin
// kiln:scaffold:end

/// What a manifest's probes are wired to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probes {
    /// Whether the liveness probe checks a dependency (a database, a broker).
    pub liveness_checks_dependencies: bool,
    /// Whether a readiness probe exists at all.
    pub has_readiness: bool,
    /// Whether the readiness probe actually checks something, or always passes.
    pub readiness_checks_something: bool,
}

/// Something the probe wiring gets wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Finding {
    /// A liveness probe on a dependency restarts a healthy process.
    LivenessChecksDependency,
    /// No readiness probe: traffic reaches a pod that cannot serve.
    NoReadinessProbe,
    /// A readiness probe that always passes is a probe that says nothing.
    ReadinessAlwaysPasses,
}

/// Every problem with `probes`, sorted.
#[must_use]
pub fn audit(probes: Probes) -> Vec<Finding> {
    let _ = probes;
    todo!("liveness must be about the process; readiness must be about serving")
}

/// The version of the service.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
