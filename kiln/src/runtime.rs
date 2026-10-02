//! Container runtime detection (plan §9).
//!
//! Two measured facts shape this module: on this machine `docker` is a **Podman shim**, and
//! `docker compose` forwards to a provider that cannot start a project (`docker compose version`
//! succeeds while `docker compose config` fails). Version strings are therefore not evidence —
//! every candidate is probed by *running* something, which is what a step needs to do too.
//!
//! No course step may write a literal `docker compose`: steps ask this module which command
//! actually works, and fall back to running the container directly.

use std::path::PathBuf;
use std::time::Duration;

use crate::process;
use crate::scratch::Scratch;

/// A container engine the course can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// Preferred: one binary, no plugin, and what this machine actually has.
    Podman,
    /// Docker Engine, or a shim over something else.
    Docker,
}

impl Engine {
    /// Binary name to invoke.
    #[must_use]
    pub fn binary(self) -> &'static str {
        match self {
            Engine::Podman => "podman",
            Engine::Docker => "docker",
        }
    }

    /// Human name for reports.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Engine::Podman => "podman",
            Engine::Docker => "docker",
        }
    }

    /// Engines in preference order.
    #[must_use]
    pub fn all() -> &'static [Engine] {
        &[Engine::Podman, Engine::Docker]
    }
}

/// What probing one engine found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineReport {
    /// Engine that was probed.
    pub engine: Engine,
    /// Where the binary lives, when it is on `PATH`.
    pub path: Option<PathBuf>,
    /// Version line the engine printed.
    pub version: Option<String>,
    /// `true` when the engine answered `<binary> info`.
    pub available: bool,
    /// Why it is not usable, when it is not.
    pub detail: Option<String>,
    /// `true` when `docker` turned out to be a Podman shim (measured, not guessed).
    pub is_podman_shim: bool,
}

/// A compose-style command, and whether it can really do anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeReport {
    /// Program to invoke (`podman`, `docker-compose`, …).
    pub program: &'static str,
    /// Subcommand that would configure a project (`compose config`, `config`).
    pub args: &'static [&'static str],
    /// `true` when a trivial project could be configured with it.
    pub available: bool,
    /// Why it failed, when it did.
    pub detail: Option<String>,
}

impl ComposeReport {
    /// Command line as a learner would type it.
    #[must_use]
    pub fn command(&self) -> String {
        process::command_line(self.program, self.args)
    }
}

/// Everything `kiln doctor` needs to say about containers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeReport {
    /// One entry per engine, in preference order.
    pub engines: Vec<EngineReport>,
    /// One entry per compose candidate, in the order the plan lists them.
    pub compose: Vec<ComposeReport>,
    /// The engine to use, if any works.
    pub preferred: Option<Engine>,
}

impl RuntimeReport {
    /// Report of one engine.
    #[must_use]
    pub fn engine(&self, engine: Engine) -> Option<&EngineReport> {
        self.engines.iter().find(|report| report.engine == engine)
    }

    /// The command a step should use to run a container, e.g. `podman run …`.
    #[must_use]
    pub fn run_prefix(&self) -> Option<String> {
        self.preferred.map(|engine| engine.binary().to_owned())
    }

    /// Whether at least one compose candidate works here.
    #[must_use]
    pub fn compose_works(&self) -> bool {
        self.compose.iter().any(|report| report.available)
    }
}

/// Probe the engines and the compose candidates on this machine.
#[must_use]
pub fn detect(timeout: Duration) -> RuntimeReport {
    let mut engines = Vec::new();
    for engine in Engine::all() {
        engines.push(probe_engine(*engine, timeout));
    }
    let preferred = engines
        .iter()
        .find(|report| report.available)
        .map(|report| report.engine);

    let compose = compose_candidates()
        .iter()
        .map(|(program, args)| probe_compose(program, args, timeout))
        .collect();

    RuntimeReport {
        engines,
        compose,
        preferred,
    }
}

/// Candidate compose commands, in the order the plan lists them (plan §9.2).
static COMPOSE_CANDIDATES: &[(&str, &[&str])] = &[
    ("podman", &["compose", "config"]),
    ("podman-compose", &["config"]),
    ("docker", &["compose", "config"]),
    ("docker-compose", &["config"]),
];

fn compose_candidates() -> &'static [(&'static str, &'static [&'static str])] {
    COMPOSE_CANDIDATES
}

fn probe_engine(engine: Engine, timeout: Duration) -> EngineReport {
    let binary = engine.binary();
    let path = process::locate(binary);
    if path.is_none() {
        return EngineReport {
            engine,
            path: None,
            version: None,
            available: false,
            detail: Some(format!("`{binary}` is not on PATH")),
            is_podman_shim: false,
        };
    }

    let info = process::run(binary, &["info"], &std::env::temp_dir(), timeout);
    let (available, detail) = match &info {
        Ok(ran) if ran.success() => (true, None),
        Ok(ran) => (false, Some(ran.summary())),
        Err(error) => (false, Some(error.to_string())),
    };

    let version = process::run(binary, &["--version"], &std::env::temp_dir(), timeout)
        .ok()
        .filter(process::Ran::success)
        .and_then(|ran| {
            ran.stdout
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_owned)
        });

    // A `docker` that reports Podman internals is a shim: the version string alone would hide it.
    let is_podman_shim = engine == Engine::Docker
        && info
            .as_ref()
            .is_ok_and(|ran| ran.stdout.to_ascii_lowercase().contains("podman"));

    EngineReport {
        engine,
        path,
        version,
        available,
        detail,
        is_podman_shim,
    }
}

fn probe_compose(
    program: &'static str,
    args: &'static [&'static str],
    timeout: Duration,
) -> ComposeReport {
    let unavailable = |detail: String| ComposeReport {
        program,
        args,
        available: false,
        detail: Some(detail),
    };

    if process::locate(program).is_none() {
        return unavailable(format!("`{program}` is not on PATH"));
    }

    let Some(scratch) = Scratch::new("compose-probe").ok() else {
        return unavailable("no temporary directory is writable".to_owned());
    };
    if let Err(error) = std::fs::write(
        scratch.path().join("compose.yaml"),
        "services:\n  probe:\n    image: alpine:3.20\n",
    ) {
        return unavailable(format!("cannot write a probe project: {error}"));
    }

    match process::run(program, args, scratch.path(), timeout) {
        Ok(ran) if ran.success() => ComposeReport {
            program,
            args,
            available: true,
            detail: None,
        },
        Ok(ran) => unavailable(ran.summary()),
        Err(error) => unavailable(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_reports_every_candidate_it_knows() {
        let report = detect(Duration::from_secs(60));
        assert_eq!(report.engines.len(), Engine::all().len());
        assert_eq!(report.compose.len(), COMPOSE_CANDIDATES.len());
        for engine in Engine::all() {
            assert!(report.engine(*engine).is_some());
        }
        // A report never claims an engine works without a path to it.
        for engine in &report.engines {
            if engine.available {
                assert!(engine.path.is_some(), "{engine:?}");
            }
        }
        // The compose candidates are probed functionally, so their answers can differ.
        for candidate in &report.compose {
            assert!(!candidate.command().is_empty());
        }
        assert!(report.run_prefix().is_none() || report.preferred.is_some());
    }

    #[test]
    fn a_missing_candidate_says_so_instead_of_claiming_success() {
        let report = probe_compose("kiln-no-compose", &["config"], Duration::from_secs(5));
        assert!(!report.available);
        assert!(
            report
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("not on PATH"),
            "{report:?}"
        );
    }
}
