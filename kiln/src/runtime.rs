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

/// One image the authored metadata declares, and the steps that declare it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredImage {
    /// Reference as the steps wrote it (`postgres:17-alpine`).
    pub reference: String,
    /// Qualified ids of the steps that need it, in track order.
    pub steps: Vec<String>,
}

/// Every image the authored metadata declares, grouped by reference, in first-seen order.
///
/// Grouped because one image is normally needed by many steps (`postgres:17-alpine` is declared by
/// thirty of them), and a report that repeats the same answer thirty times buries the one line that
/// matters: whether anything is missing. The steps are kept, because "which step needs this" is the
/// question a learner asks when a container step will not start.
#[must_use]
pub fn declared_images(course: &crate::info::Course) -> Vec<DeclaredImage> {
    let mut declarations: Vec<(String, String)> = Vec::new();
    for track in course.tracks() {
        for step in &track.step {
            for image in &step.images {
                declarations.push((image.trim().to_owned(), step.qualified_id()));
            }
        }
    }
    group(declarations)
}

/// Group `(reference, step)` pairs the way the reports want them.
///
/// Kept apart from the walk over the metadata so the rule can be checked without a course on disk:
/// first-seen order, one entry per reference, every step that names it in track order.
fn group(declarations: Vec<(String, String)>) -> Vec<DeclaredImage> {
    let mut found: Vec<DeclaredImage> = Vec::new();
    for (reference, step) in declarations {
        match found.iter_mut().find(|seen| seen.reference == reference) {
            Some(seen) => seen.steps.push(step),
            None => found.push(DeclaredImage {
                reference,
                steps: vec![step],
            }),
        }
    }
    found
}

/// What a run-probe of the container engine found.
///
/// Detection asks the engine what it is (`info`), and a reply is not the same as a container that
/// starts: on this machine's history, `docker compose version` answered while `docker compose config`
/// could not configure a project, and `podman info` answers while the machine is booting. The probe
/// below is the doctrine of this module applied to the engine itself — **probe by running**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmokeReport {
    /// Image the probe ran, when one cheap enough to try was here.
    pub image: Option<String>,
    /// `true` when a container started and its command exited 0.
    pub started: bool,
    /// One line for a report.
    pub detail: String,
}

/// Images small and quiet enough to prove the engine works by running one command in one of them.
///
/// Only images whose entrypoint is a shell that can run `true` are listed: an image whose entrypoint
/// is its own service (`postgres:17-alpine`) would fail the probe for a reason that has nothing to do
/// with the engine, and "the engine is broken" is exactly the wrong lesson to take from that.
const SMOKE_CANDIDATES: &[&str] = &[
    "alpine:3.20",
    "alpine",
    "debian:bookworm-slim",
    "rust:1.98-alpine",
    "rust:1.98-slim",
];

/// Start one container to find out whether the engine can start one at all.
///
/// The probe is bounded by `timeout` and by [`SMOKE_CANDIDATES`]: when none of those images is in the
/// local store, the report says so instead of pulling anything (nothing in this course reaches the
/// network to answer a question about the machine).
#[must_use]
pub fn smoke(engine: Engine, timeout: Duration) -> SmokeReport {
    let candidates: Vec<String> = SMOKE_CANDIDATES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    let Ok(reports) = images(engine, &candidates, timeout) else {
        return SmokeReport {
            image: None,
            started: false,
            detail: format!("`{} images` could not be asked", engine.binary()),
        };
    };
    let Some(present) = reports.iter().find(|report| report.is_present()) else {
        return SmokeReport {
            image: None,
            started: false,
            detail: format!(
                "no small image to try (looked for {}): `kiln images pull` fetches the ones the \
                 steps declare",
                SMOKE_CANDIDATES.join(", ")
            ),
        };
    };
    // The name the store actually has it under: a mirrored image is `docker.1ms.run/library/alpine:3.20`
    // here, and naming the declared one would make the engine try to resolve a name it does not know.
    let local = present
        .local
        .clone()
        .unwrap_or_else(|| present.declared.clone());
    let args = ["run", "--rm", local.as_str(), "true"];
    let ran = match process::run(engine.binary(), &args, &std::env::temp_dir(), timeout) {
        Ok(ran) => ran,
        Err(error) => {
            return SmokeReport {
                image: Some(present.declared.clone()),
                started: false,
                detail: format!("`{}` did not run: {error}", engine.binary()),
            };
        }
    };
    SmokeReport {
        image: Some(present.declared.clone()),
        started: ran.success(),
        detail: if ran.success() {
            format!(
                "containers can start ({} `{}`, {:.1}s)",
                engine.binary(),
                present.declared,
                ran.elapsed.as_secs_f64()
            )
        } else {
            format!(
                "a container did not start: `{}` exited {} — {}",
                process::command_line(engine.binary(), &args),
                process::exit_code_text(ran.code),
                ran.summary()
            )
        },
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
    // Which local copy is the one a *build* would use: a store filled through more than one mirror
    // holds the same repository path more than once — once per architecture — and the engine
    // refuses the copy that is not this machine's. Naming the wrong one here would send a learner
    // to a pull that changes nothing.
    let host = host_architecture(engine, timeout);
    Ok(declared
        .iter()
        .map(|declared| {
            let matching: Vec<&String> = local
                .iter()
                .filter(|name| is_declared_image(name, declared))
                .collect();
            let chosen = matching
                .iter()
                .find(|name| {
                    image_architecture(engine, name, timeout).as_deref() == Some(host.as_str())
                })
                .or_else(|| matching.first());
            ImageReport {
                declared: declared.clone(),
                resolved: resolve(declared),
                local: chosen.map(|name| (*name).clone()),
            }
        })
        .collect())
}

/// The architecture this engine runs images of, as it reports it.
///
/// Falls back to the architecture this binary was compiled for, which is the same machine in every
/// case that matters here.
fn host_architecture(engine: Engine, timeout: Duration) -> String {
    process::run(
        engine.binary(),
        &["info", "--format", "{{.Host.Arch}}"],
        &std::env::temp_dir(),
        timeout,
    )
    .ok()
    .filter(|ran| ran.success())
    .map(|ran| ran.stdout.trim().to_owned())
    .filter(|arch| !arch.is_empty())
    .unwrap_or_else(|| std::env::consts::ARCH.to_owned())
}

/// The architecture an image was built for, when the engine will say.
fn image_architecture(engine: Engine, name: &str, timeout: Duration) -> Option<String> {
    let ran = process::run(
        engine.binary(),
        &["image", "inspect", name, "--format", "{{.Architecture}}"],
        &std::env::temp_dir(),
        timeout,
    )
    .ok()?;
    ran.success()
        .then(|| ran.stdout.trim().to_owned())
        .filter(|arch| !arch.is_empty())
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

    #[test]
    fn declared_images_are_grouped_by_reference_and_keep_their_steps() {
        // The shape the authored metadata actually has: one image named by many steps, in track
        // order. Thirty rows of `postgres:17-alpine` is what this replaced.
        let grouped = group(vec![
            (
                "postgres:17-alpine".to_owned(),
                "mailer/1.9-postgres".to_owned(),
            ),
            ("alpine:3.20".to_owned(), "mailer/1.21-container".to_owned()),
            (
                "postgres:17-alpine".to_owned(),
                "relay/2.1-project-skeleton".to_owned(),
            ),
            (
                "alpine:3.20".to_owned(),
                "lakelet/4.22-reproducible-build".to_owned(),
            ),
        ]);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].reference, "postgres:17-alpine");
        assert_eq!(
            grouped[0].steps,
            ["mailer/1.9-postgres", "relay/2.1-project-skeleton"]
        );
        assert_eq!(grouped[1].reference, "alpine:3.20");
        assert_eq!(grouped[1].steps.len(), 2);

        // A declaration with surrounding whitespace is the same reference, not a second one.
        let trimmed = group(vec![
            ("alpine:3.20".to_owned(), "a/1.1-step".to_owned()),
            ("alpine:3.20".to_owned(), "b/2.1-step".to_owned()),
        ]);
        assert_eq!(trimmed.len(), 1);
        assert_eq!(trimmed[0].steps.len(), 2);
        assert!(group(Vec::new()).is_empty());
    }
}
