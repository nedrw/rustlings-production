//! `container-repro` — the image this project describes runs the binary this project builds.
//!
//! A spec can prove that an image answers the right things; only a rebuild can prove that the answer
//! comes from *this* tree. So this gate does the one thing a test inside the project cannot: it
//! builds the image the `Dockerfile` describes, holds it to a size budget, and asks the binary inside
//! it the same question the binary on this host is asked — `--check-config` (step
//! `mailer/1.4-config`), whose lines are the configuration a deployment would get, sorted so that two
//! answers can be compared line by line.
//!
//! Three outcomes, and the difference matters to the person reading them (the same split `images`
//! makes):
//!
//! * *passed* — the image built, fits the budget, and answered what this host's binary answered;
//! * *failed* — it built and does not: over budget, or a different answer, which is the sentence
//!   saying the shipped binary is not the one this tree builds;
//! * *blocked* — there is no Dockerfile, no engine, or the build did not finish inside the deadline:
//!   a fact about this machine rather than about the project.
//!
//! The base images come from `kiln images pull`, resolved the way a step's `images = [...]` metadata
//! is (see [`build_arguments`]), and the build never reaches a registry for them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{KilnError, Result};
use crate::gates::GateOutcome;
use crate::gates::GateSpec;
use crate::gates::runner::{GateContext, clip};
use crate::process;
use crate::runtime::{self, Engine};
use crate::scratch;

/// What the runtime stage of a service image may weigh, in bytes.
///
/// The step's own spec holds the image to the same ceiling: a runtime stage that is really the
/// builder is hundreds of megabytes, and this is the number that says so once the step is done.
const SIZE_CEILING: u64 = 25 * 1024 * 1024;

/// How many lines of a failing build or a differing report a gate report keeps.
const DETAIL_LINES: usize = 12;

/// Judge the image a step's project describes.
pub fn run(ctx: &GateContext<'_>, _spec: &GateSpec) -> Result<GateOutcome> {
    let Some(project) = ctx.existing_project() else {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    };
    let dockerfile = project.join("Dockerfile");
    let Ok(source) = std::fs::read_to_string(&dockerfile) else {
        return Ok(GateOutcome::blocked(format!(
            "{} has no Dockerfile, and this step's deliverable is the image: there is nothing to \
             build yet",
            ctx.project_dir.display()
        )));
    };
    let Some(engine) = runtime::first_engine(ctx.probe_timeout()) else {
        return Ok(GateOutcome::blocked(
            "no container engine answered, and this gate builds an image — install podman (or \
             docker) and run `kiln doctor`",
        ));
    };

    let tag = format!("kiln-{}-repro", ctx.step.track);
    judge(engine, project, &source, &tag, ctx.timeout)
}

/// The decision, kept apart from the gate context so it can be exercised with a real engine.
///
/// # Errors
///
/// Reports what [`process::run`] reports when a command cannot be started at all, and what a manifest
/// that cannot be parsed reports.
pub fn judge(
    engine: Engine,
    project: &Path,
    dockerfile: &str,
    tag: &str,
    timeout: Duration,
) -> Result<GateOutcome> {
    let argv = build_argv(engine, project, dockerfile, tag, timeout)?;
    let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
    let built = process::run(engine.binary(), &borrowed, project, timeout)?;
    if !built.success() {
        return Ok(GateOutcome::failed(build_failure(&built, timeout)));
    }

    let size = size_of(engine, tag, project, timeout)?;
    if size > SIZE_CEILING {
        return Ok(GateOutcome::failed(format!(
            "the image is {:.1} MB, over the {:.0} MB this step allows: a runtime stage that is \
             really the builder ships the toolchain and the sources",
            megabytes(size),
            megabytes(SIZE_CEILING)
        )));
    }

    let environment = image_environment(engine, tag, project, timeout)?;
    let inside = process::run(
        engine.binary(),
        &["run", "--rm", tag, "--check-config"],
        project,
        timeout,
    )?;
    if !inside.success() {
        return Ok(GateOutcome::failed(format!(
            "the image does not run this tree's binary: `--check-config` inside it said\n    {}",
            clip(&said(&inside), DETAIL_LINES).replace('\n', "\n    ")
        )));
    }

    let outside = match check_config_outside(project, &environment, timeout)? {
        HostReport::Answered(report) => report,
        HostReport::Failed(detail) => {
            return Ok(GateOutcome::blocked(format!(
                "the image answered, but the binary on this host could not: {detail}"
            )));
        }
    };

    let differing = differences(&inside.stdout, &outside);
    if !differing.is_empty() {
        return Ok(GateOutcome::failed(format!(
            "the image does not run the binary this tree builds: {} of the configuration report's \
             line(s) differ\n    {}",
            differing.len(),
            clip(&differing.join("\n"), DETAIL_LINES).replace('\n', "\n    ")
        )));
    }

    Ok(GateOutcome::passed(format!(
        "`{tag}` builds from this tree, weighs {:.1} MB of the {:.0} MB budget, and its \
         `--check-config` answers the same {} line(s) the binary on this host answers",
        megabytes(size),
        megabytes(SIZE_CEILING),
        inside
            .stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count()
    )))
}

/// What the build said, when it did not produce an image.
fn build_failure(built: &process::Ran, timeout: Duration) -> String {
    if built.timed_out {
        return format!(
            "the image build did not finish within {timeout:?}: the first build on a machine \
             compiles the whole dependency tree. Give it more time — `kiln --timeout 1800 check` — \
             and every build after the first will be seconds"
        );
    }
    format!(
        "the image did not build, so there is nothing to judge:\n    {}",
        clip(&said(built), DETAIL_LINES).replace('\n', "\n    ")
    )
}

/// The arguments of `podman build`: the base images this machine has, a shared cache, and no pulls.
fn build_argv(
    engine: Engine,
    project: &Path,
    dockerfile: &str,
    tag: &str,
    timeout: Duration,
) -> Result<Vec<String>> {
    let context = project.to_str().ok_or_else(|| {
        KilnError::Invalid(format!(
            "the build context has to be a path that is valid UTF-8: {}",
            project.display()
        ))
    })?;
    let mut argv: Vec<String> = vec![
        "build".to_owned(),
        // `--layers` keeps a layer per step, so the second build of the same image is a cache hit;
        // `--pull=never` keeps a missing base image from becoming a build that hangs on a registry —
        // images come from `kiln images pull`, and the `images` gate names that command for the ones
        // that are not there.
        "--layers".to_owned(),
        "--pull=never".to_owned(),
        "-t".to_owned(),
        tag.to_owned(),
    ];
    for (name, value) in build_arguments(engine, dockerfile, timeout)? {
        argv.push("--build-arg".to_owned());
        argv.push(format!("{name}={value}"));
    }
    for (directory, mount) in caches() {
        argv.push("-v".to_owned());
        argv.push(format!("{}:{mount}", directory.display()));
    }
    argv.push(context.to_owned());
    Ok(argv)
}
/// The `ARG name=value` defaults of a Dockerfile, resolved to the names this engine has.
///
/// An image reference is the one part of a `Dockerfile` that cannot be the same on two machines:
/// behind a registry mirror `rust:1.98-alpine` is stored as
/// `docker.1ms.run/library/rust:1.98-alpine`, and a literal `FROM` would not find it. The gate
/// resolves each default the way `kiln images pull` does and passes the result back as a build
/// argument, which is what keeps the file portable and the build offline. A default the engine does
/// not have yet is passed through as written, and the engine's own message is what the learner reads.
fn build_arguments(
    engine: Engine,
    dockerfile: &str,
    timeout: Duration,
) -> Result<Vec<(String, String)>> {
    let mut arguments = Vec::new();
    for (name, value) in arg_defaults(dockerfile) {
        // `runtime::images` already picks the local copy a *build* can use (the one for this
        // machine's architecture); a default the engine does not have is passed through as written,
        // and the engine's own message is what the learner reads.
        let resolved = runtime::images(engine, std::slice::from_ref(&value), timeout)?
            .into_iter()
            .find_map(|report| report.local)
            .unwrap_or(value);
        arguments.push((name, resolved));
    }
    Ok(arguments)
}

/// The `ARG name=value` lines of a Dockerfile, in the order they are written.
///
/// Only the ones with a default: `ARG NAME` takes its value from the command line, and there is
/// nothing for the gate to resolve.
fn arg_defaults(dockerfile: &str) -> Vec<(String, String)> {
    dockerfile
        .lines()
        .filter_map(|line| line.trim().strip_prefix("ARG "))
        .filter_map(|rest| rest.split_once('='))
        .map(|(name, value)| (name.trim(), value.trim()))
        .filter(|(name, value)| !name.is_empty() && !value.is_empty())
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

/// Where the build cache is mounted, and what the build is told to use it for.
///
/// The same path a step's fixture mounts, so the two builds share one cache instead of paying for
/// each other's work. Deliberately *not* the directory a Dockerfile compiles into by itself: a mount
/// is not a layer, so a cache over the default target path would leave an image empty of the binary
/// its entry point names — punishing the naive Dockerfile instead of speeding up the careful one.
const TARGET_MOUNT: &str = "/opt/kiln/target";

/// Where a container build keeps its work, when this machine has a cache directory at all.
///
/// The registry a container build reads crates from is the **host's own cargo registry** — the
/// versions this machine has actually resolved, which is the one set of crate files a build inside
/// the image can be sure to need: the tree being built was resolved by this machine's cargo, so
/// its lockfile names versions this registry already holds. A curated copy, however fresh when it
/// was made, silently loses that property the day the registry moves and a fresh resolve names a
/// version nobody cached — found by `lakelet/4.22`, whose freshly resolved lockfile named a newly
/// released tokio the curated cache had never seen. `$CARGO_HOME` wins when it has a registry; a
/// machine without one falls back to the curated copy under the cache directory, and a machine
/// with neither still builds — it just pays the network.
fn caches() -> Vec<(PathBuf, &'static str)> {
    let mut caches = Vec::new();
    if let Some(target) = scratch::container_cache("container-target") {
        caches.push((target, TARGET_MOUNT));
    }
    let registry = cargo_home_registry().or_else(|| scratch::container_cache("container-registry"));
    if let Some(registry) = registry {
        caches.push((registry, "/usr/local/cargo/registry"));
    }
    caches
}

/// The host's own cargo registry directory, when `cargo` on this machine has one.
fn cargo_home_registry() -> Option<PathBuf> {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))?;
    let registry = home.join("registry");
    registry.is_dir().then_some(registry)
}

/// How many bytes the image weighs, as the engine reports it.
fn size_of(engine: Engine, tag: &str, dir: &Path, timeout: Duration) -> Result<u64> {
    let inspected = process::run(
        engine.binary(),
        &["image", "inspect", tag, "--format", "{{.Size}}"],
        dir,
        timeout,
    )?;
    inspected.stdout.trim().parse().map_err(|_| {
        KilnError::Invalid(format!(
            "`{} image inspect {tag}` did not report a size: {}",
            engine.binary(),
            inspected.summary()
        ))
    })
}

/// The settings the image declares for itself, and only those.
///
/// `MAILER__…` because that is the service's own namespace (step `mailer/1.4-config`): a `PATH` or a
/// `HOME` from the image says nothing about the binary, and copying one into the host's process would
/// break the very command this comparison runs.
fn image_environment(
    engine: Engine,
    tag: &str,
    dir: &Path,
    timeout: Duration,
) -> Result<Vec<(String, String)>> {
    let inspected = process::run(
        engine.binary(),
        &["image", "inspect", tag, "--format", "{{json .Config.Env}}"],
        dir,
        timeout,
    )?;
    if !inspected.success() {
        return Err(KilnError::Invalid(format!(
            "`{} image inspect {tag}` failed: {}",
            engine.binary(),
            inspected.summary()
        )));
    }
    let declared: Vec<String> = serde_json::from_str(inspected.stdout.trim()).unwrap_or_default();
    Ok(declared
        .iter()
        .filter_map(|entry| entry.split_once('='))
        .filter(|(name, _)| name.starts_with("MAILER__"))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect())
}

/// What the binary this tree builds answered.
enum HostReport {
    /// The configuration report, line by line.
    Answered(String),
    /// The host binary was not built, or refused the request: what it said.
    Failed(String),
}

/// `--check-config`, answered by the binary this tree builds.
///
/// Run from a directory of its own, because the configuration report is about the *binary*: a
/// `mailer.toml` next to the project would be read on this side and not inside the image, and the
/// difference would look like a difference between two binaries.
fn check_config_outside(
    project: &Path,
    environment: &[(String, String)],
    timeout: Duration,
) -> Result<HostReport> {
    let manifest = project.join("Cargo.toml");
    let Some(package) = package_name(&manifest)? else {
        return Ok(HostReport::Failed(format!(
            "{} declares no `[package] name`",
            manifest.display()
        )));
    };
    let manifest = manifest.to_str().ok_or_else(|| {
        KilnError::Invalid(format!(
            "the manifest path has to be valid UTF-8: {}",
            manifest.display()
        ))
    })?;
    let scratch = scratch::Scratch::new("repro").map_err(|source| KilnError::Write {
        path: std::env::temp_dir(),
        source,
    })?;
    let ran = process::run_with_env(
        "cargo",
        &[
            "run",
            "--quiet",
            "--manifest-path",
            manifest,
            "--package",
            package.as_str(),
            "--",
            "--check-config",
        ],
        scratch.path(),
        timeout,
        environment,
    )?;
    if !ran.success() {
        return Ok(HostReport::Failed(clip(&said(&ran), DETAIL_LINES)));
    }
    Ok(HostReport::Answered(ran.stdout))
}

/// The name of the package a project's manifest declares.
///
/// Read rather than assumed: the gate judges a project, and what the project calls itself is the
/// project's own business.
fn package_name(manifest: &Path) -> Result<Option<String>> {
    #[derive(serde::Deserialize)]
    struct Manifest {
        package: Option<Package>,
    }
    #[derive(serde::Deserialize)]
    struct Package {
        name: String,
    }
    let Ok(source) = std::fs::read_to_string(manifest) else {
        return Ok(None);
    };
    let parsed: Manifest = toml::from_str(&source).map_err(|source| KilnError::Metadata {
        path: manifest.to_path_buf(),
        source,
    })?;
    Ok(parsed.package.map(|package| package.name))
}

/// The lines the two reports do not agree on, with both sides shown.
fn differences(inside: &str, outside: &str) -> Vec<String> {
    let lines = |text: &str| -> Vec<String> {
        text.lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let inside = lines(inside);
    let outside = lines(outside);
    let mut differing = Vec::new();
    for (index, line) in inside.iter().enumerate() {
        match outside.get(index) {
            Some(other) if other == line => {}
            Some(other) => {
                differing.push(format!("inside the image: {line}\non this host: {other}"))
            }
            None => differing.push(format!("inside the image: {line}\non this host: (nothing)")),
        }
    }
    for line in outside.iter().skip(inside.len()) {
        differing.push(format!("inside the image: (nothing)\non this host: {line}"));
    }
    differing
}

/// Both streams as one block, the way a report quotes a command.
fn said(ran: &process::Ran) -> String {
    format!("{}{}", ran.stdout, ran.stderr)
}

/// Bytes as megabytes, for a sentence a person reads.
fn megabytes(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_arguments_of_a_dockerfile_are_its_arg_defaults() {
        let dockerfile = "# The base images, named by the deployment.\n\
                          ARG BUILDER=rust:1.98-alpine\n\
                          FROM ${BUILDER} AS builder\n\
                          ARG RUNTIME=alpine:3.20\n\
                          ARG EMPTY=\n\
                          ARG FROM_THE_COMMAND_LINE\n";
        assert_eq!(
            arg_defaults(dockerfile),
            [
                ("BUILDER".to_owned(), "rust:1.98-alpine".to_owned()),
                ("RUNTIME".to_owned(), "alpine:3.20".to_owned())
            ]
        );
    }

    #[test]
    fn a_dockerfile_without_build_arguments_asks_for_none() {
        assert!(arg_defaults("FROM rust:1.98-alpine\nRUN cargo build\n").is_empty());
    }

    #[test]
    fn two_configuration_reports_are_compared_line_by_line() {
        let same = "database.url = unset\nlisten.host = 0.0.0.0\n";
        assert!(differences(same, same).is_empty());

        let differing = differences(
            "database.url = unset\nlisten.host = 0.0.0.0\n",
            "database.url = unset\nlisten.host = 127.0.0.1\n",
        );
        assert_eq!(differing.len(), 1, "{differing:?}");
        assert!(differing[0].contains("0.0.0.0"), "{}", differing[0]);
        assert!(differing[0].contains("127.0.0.1"), "{}", differing[0]);

        // A report that lost a line is a difference too: one side has nothing to show.
        let missing = differences("a = 1\nb = 2\n", "a = 1\n");
        assert_eq!(missing.len(), 1, "{missing:?}");
        assert!(missing[0].contains("(nothing)"), "{}", missing[0]);
    }

    #[test]
    fn a_size_budget_is_reported_in_megabytes() {
        assert!((megabytes(16 * 1024 * 1024) - 16.0).abs() < f64::EPSILON);
        assert!(megabytes(SIZE_CEILING) > 20.0);
    }
}
