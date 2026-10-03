//! Container runtime detection (plan §9).
//!
//! Two measured facts shape this module: on this machine `docker` is a **Podman shim**, and
//! `docker compose` forwards to a provider that cannot start a project (`docker compose version`
//! succeeds while `docker compose config` fails). Version strings are therefore not evidence —
//! every candidate is probed by *running* something, which is what a step needs to do too.
//!
//! No course step may write a literal `docker compose`: steps ask this module which command
//! actually works, and fall back to running the container directly.
//!
//! The same module owns images (plan §9.3): a step declares the reference it wants
//! (`postgres:17-alpine`), and this decides whether the engine already has it — under that name or
//! under the name a registry mirror gave it.

use std::path::PathBuf;
use std::time::Duration;

use crate::error::KilnError;
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

/// The engine to use, probing in preference order.
///
/// Engines only: a caller that needs an image does not care whether a compose provider works, and
/// the compose probes write a project to disk.
#[must_use]
pub fn first_engine(timeout: Duration) -> Option<Engine> {
    Engine::all()
        .iter()
        .copied()
        .find(|engine| probe_engine(*engine, timeout).available)
}

/// Registry a machine pulls through, when it is configured (`KILN_REGISTRY_MIRROR`).
///
/// This is the local workaround the plan measured (`docker.1ms.run`), not a course-wide default:
/// with nothing set, references are used exactly as the steps declare them.
#[must_use]
pub fn mirror() -> Option<String> {
    let value = std::env::var("KILN_REGISTRY_MIRROR").ok()?;
    let value = value.trim().trim_end_matches('/');
    (!value.is_empty()).then(|| value.to_owned())
}

/// The reference a pull would actually use.
///
/// A declared reference is a Docker Hub name (`postgres:17-alpine`, `axllent/mailpit:v1.20`) unless
/// it names its own registry (`ghcr.io/acme/app:1`, `localhost:5000/app:1`) — the same heuristic the
/// engine uses. Official images live under `library/` in a registry, so a mirror gets that added:
/// `docker.1ms.run/library/postgres:17-alpine`, the command this machine was measured with.
#[must_use]
pub fn resolve(declared: &str) -> String {
    resolve_with(declared, mirror().as_deref())
}

/// [`resolve`], with the mirror passed in: the rule is data, the environment is not.
#[must_use]
fn resolve_with(declared: &str, mirror: Option<&str>) -> String {
    let Some(mirror) = mirror else {
        return declared.to_owned();
    };
    if names_its_registry(declared) {
        return declared.to_owned();
    }
    if declared.contains('/') {
        format!("{mirror}/{declared}")
    } else {
        format!("{mirror}/library/{declared}")
    }
}

/// `true` when the first path component is a registry host.
fn names_its_registry(declared: &str) -> bool {
    let Some((first, _)) = declared.split_once('/') else {
        return false;
    };
    first == "localhost" || first.contains('.') || first.contains(':')
}

/// One reference without the registry that hosts it, when it names one.
fn without_registry(reference: &str) -> &str {
    match reference.split_once('/') {
        Some((first, rest))
            if first == "localhost" || first.contains('.') || first.contains(':') =>
        {
            rest
        }
        _ => reference,
    }
}

/// Whether a name the engine reports is the image a step declared.
///
/// The engine answers with the name it was pulled under, so `postgres:17-alpine` can be local as
/// `docker.1ms.run/library/postgres:17-alpine`. What has to match is the *repository path*: a
/// registry in front of it is the machine's business, a namespace in the middle is not —
/// `myorg/postgres:17-alpine` is a different image from `library/postgres:17-alpine`.
#[must_use]
pub fn is_declared_image(local: &str, declared: &str) -> bool {
    if local == declared {
        return true;
    }
    let repository = without_registry(local);
    repository == declared
        // A registry stores an official image under `library/`, which the declared name omits.
        || (!declared.contains('/') && repository == format!("library/{declared}"))
}

/// The image names the engine has locally.
///
/// Asked of the engine, never read out of a cache file: this is the answer the engine would give a
/// `run`, and it costs one process.
///
/// # Errors
///
/// Reports the same failures [`process::run`] does — the engine could not be started.
pub fn local_images(engine: Engine, timeout: Duration) -> Result<Vec<String>, KilnError> {
    let ran = process::run(
        engine.binary(),
        &["images", "--format", "{{.Repository}}:{{.Tag}}"],
        &std::env::temp_dir(),
        timeout,
    )?;
    if !ran.success() {
        return Err(KilnError::Invalid(format!(
            "`{} images` failed: {}",
            engine.binary(),
            ran.summary()
        )));
    }
    Ok(ran
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "<none>:<none>")
        .map(str::to_owned)
        .collect())
}

/// Where one declared image is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageReport {
    /// Reference as the step declared it.
    pub declared: String,
    /// Reference a pull would fetch.
    pub resolved: String,
    /// Name the engine has it under, when it has it.
    pub local: Option<String>,
}

impl ImageReport {
    /// Whether the engine already has this image.
    #[must_use]
    pub fn is_present(&self) -> bool {
        self.local.is_some()
    }

    /// The command that would fetch it.
    #[must_use]
    pub fn pull_command(&self, engine: Engine) -> String {
        process::command_line(engine.binary(), &["pull", &self.resolved])
    }

    /// One line, phrased for the person who has to fetch it.
    #[must_use]
    pub fn line(&self, engine: Engine) -> String {
        match &self.local {
            Some(local) if *local == self.declared => {
                format!("{} — in the local image store", self.declared)
            }
            Some(local) => format!("{} — in the local image store as {local}", self.declared),
            None => format!(
                "{} — not in the local image store; pull it with `{}`",
                self.declared,
                self.pull_command(engine)
            ),
        }
    }
}

/// Where every declared image is, locally.
///
/// # Errors
///
/// Reports what [`local_images`] reports when the engine cannot be asked.
pub fn images(
    engine: Engine,
    declared: &[String],
    timeout: Duration,
) -> Result<Vec<ImageReport>, KilnError> {
    let local = local_images(engine, timeout)?;
    Ok(declared
        .iter()
        .map(|declared| ImageReport {
            declared: declared.clone(),
            resolved: resolve(declared),
            local: local
                .iter()
                .find(|name| is_declared_image(name, declared))
                .cloned(),
        })
        .collect())
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
    }

    #[test]
    fn an_official_image_goes_through_the_mirror_under_library() {
        // The rule this machine's mirror was measured with, and the one a step's body documents.
        assert!(is_declared_image(
            "docker.1ms.run/library/postgres:17-alpine",
            "postgres:17-alpine"
        ));
        assert!(is_declared_image(
            "postgres:17-alpine",
            "postgres:17-alpine"
        ));
        assert!(is_declared_image(
            "docker.1ms.run/axllent/mailpit:v1.20",
            "axllent/mailpit:v1.20"
        ));
        // Same repository path under another registry, or a different path: not the same image.
        assert!(!is_declared_image(
            "docker.1ms.run/library/redis:7-alpine",
            "postgres:17-alpine"
        ));
        assert!(!is_declared_image(
            "docker.1ms.run/myorg/postgres:17-alpine",
            "postgres:17-alpine"
        ));
        assert!(!is_declared_image(
            "docker.1ms.run/library/postgres:17-alpine-extra",
            "postgres:17-alpine"
        ));
        assert!(is_declared_image(
            "localhost:5000/library/postgres:17-alpine",
            "postgres:17-alpine"
        ));
    }

    #[test]
    fn the_resolution_rule_leaves_the_environment_in_charge() {
        // The rule, with the mirror passed in: the environment is the caller's business.
        assert_eq!(
            resolve_with("postgres:17-alpine", Some("docker.1ms.run")),
            "docker.1ms.run/library/postgres:17-alpine"
        );
        assert_eq!(
            resolve_with("axllent/mailpit:v1.20", Some("docker.1ms.run")),
            "docker.1ms.run/axllent/mailpit:v1.20"
        );
        assert_eq!(
            resolve_with("ghcr.io/acme/app:1", Some("docker.1ms.run")),
            "ghcr.io/acme/app:1"
        );
        assert_eq!(
            resolve_with("postgres:17-alpine", None),
            "postgres:17-alpine"
        );
        // Without a mirror configured, what a step declares is what a pull would use.
        assert_eq!(resolve("ghcr.io/acme/app:1"), "ghcr.io/acme/app:1");
        assert_eq!(resolve("localhost:5000/app:1"), "localhost:5000/app:1");
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
