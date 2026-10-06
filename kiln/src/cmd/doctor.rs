//! `kiln doctor` — one report that answers "why can I not run this step yet?".
//!
//! It exists because the failure modes of this course are environmental and specific: a `docker`
//! that is really Podman, a `docker compose` that cannot configure anything, missing cargo tools,
//! a toolchain that does not match the pinned one, and image pulls that need a mirror (plan §9).
//! Everything it prints was measured just now; nothing is inferred from a version string.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::cmd::Session;
use crate::error::{EXIT_ERROR, EXIT_OK, Result};
use crate::gates::tool::{self, Tool};
use crate::process;
use crate::runtime::{self, Engine, RuntimeReport};

/// How long each probe may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// The pinned toolchain, read from `rust-toolchain.toml`.
#[derive(Debug, Deserialize)]
struct ToolchainFile {
    toolchain: PinnedToolchain,
}

/// The `[toolchain]` table: only `channel` matters here, but the file may carry components,
/// targets and a profile, so unknown keys are expected rather than mistakes.
#[derive(Debug, Deserialize)]
struct PinnedToolchain {
    channel: String,
}

/// Print the environment report.
pub fn run(session: &Session) -> Result<u8> {
    let mut blockers = 0usize;
    let mut warnings = 0usize;

    println!("kiln doctor — environment report\n");

    println!("toolchain");
    let (blocked, warned) = toolchain(session.course().root());
    blockers += blocked;
    warnings += warned;

    println!("\ncontainer runtime");
    let report = runtime::detect(PROBE_TIMEOUT);
    let (blocked, warned) = containers(&report);
    blockers += blocked;
    warnings += warned;

    println!("\ncargo tools");
    let (blocked, warned) = cargo_tools();
    blockers += blocked;
    warnings += warned;

    // `lakelet/4.16-profiling` asks for a profile and a conclusion drawn from it, and the answer to
    // "with what" is different on every platform. The course's own must-half does not need any of
    // these tools (it judges attribution counters, plan §6), so this section is a map rather than a
    // requirement: it says which route works *here*, measured by looking, and which one is a trap.
    println!("\nprofilers");
    let (blocked, warned) = profilers();
    blockers += blocked;
    warnings += warned;

    println!("\nregistry mirror and images");
    let (blocked, warned) = registry(session);
    blockers += blocked;
    warnings += warned;

    println!();
    if blockers == 0 {
        println!("no blockers · {warnings} warning(s)");
        Ok(EXIT_OK)
    } else {
        println!("{blockers} blocker(s) · {warnings} warning(s)");
        Ok(EXIT_ERROR)
    }
}

/// Returns `(blockers, warnings)` for the section just printed.
type Tally = (usize, usize);

fn toolchain(root: &Path) -> Tally {
    let mut blockers = 0;
    let pinned = std::fs::read_to_string(root.join("rust-toolchain.toml"))
        .ok()
        .and_then(|text| toml::from_str::<ToolchainFile>(&text).ok())
        .map(|file| file.toolchain.channel);

    for (program, required) in [("rustc", true), ("cargo", true)] {
        let ran = crate::process::run(program, &["--version"], root, PROBE_TIMEOUT);
        match ran {
            Ok(ran) if ran.success() => {
                let version = ran.stdout.trim().to_owned();
                match &pinned {
                    Some(channel) if version.contains(channel.as_str()) => {
                        println!("  ✓ {version} — matches rust-toolchain.toml ({channel})");
                    }
                    Some(channel) => {
                        println!(
                            "  ! {version} — rust-toolchain.toml pins {channel}; run `rustup show` to \
                             install it"
                        );
                    }
                    None => println!("  ✓ {version}"),
                }
            }
            Ok(ran) => {
                println!("  ✗ {program} — {}", ran.summary());
                if required {
                    blockers += 1;
                }
            }
            Err(error) => {
                println!("  ✗ {program} — {error}");
                if required {
                    blockers += 1;
                }
            }
        }
    }
    if pinned.is_none() {
        println!("  ! rust-toolchain.toml is missing or has no `channel`");
    }
    (blockers, usize::from(pinned.is_none()))
}

fn containers(report: &RuntimeReport) -> Tally {
    for engine in &report.engines {
        let location = engine
            .path
            .as_deref()
            .map_or_else(String::new, |path| format!(" ({})", path.display()));
        let version = engine.version.as_deref().map_or(String::new(), |version| {
            version
                .trim_start_matches(engine.engine.binary())
                .trim()
                .to_owned()
        });
        let shim = if engine.is_podman_shim {
            " — a Podman shim, not Docker Engine"
        } else {
            ""
        };
        if engine.available {
            println!("  ✓ {} {version}{location}{shim}", engine.engine.name());
        } else {
            let detail = engine.detail.as_deref().unwrap_or("not usable");
            println!("  ✗ {} — {detail}{shim}", engine.engine.name());
        }
    }

    println!("  compose");
    for candidate in &report.compose {
        let marker = if candidate.available { "✓" } else { "✗" };
        let detail = candidate
            .detail
            .as_deref()
            .map_or_else(String::new, |detail| format!(" — {detail}"));
        println!("    {marker} {}{detail}", candidate.command());
    }
    if report.compose_works() {
        println!(
            "  ✓ a compose command configured a minimal project — probing the engine above is \
             what says whether it can start one"
        );
    } else {
        println!(
            "  ! no compose command works here: the course starts containers through the runtime \
             abstraction (direct `run`) instead of writing `docker compose` into a step"
        );
    }

    let mut blockers = 0;
    let mut warnings = 0;
    match report.preferred {
        Some(engine) => {
            println!("  → using `{}`", engine.binary());
            // Asking the engine what it is (`info`) is not the same as one container that starts:
            // `docker compose version` answered on this machine while `docker compose config` could
            // not configure a project, and `podman info` answers while its machine is still booting.
            let smoke = runtime::smoke(engine, PROBE_TIMEOUT);
            if smoke.started {
                println!("  ✓ {}", smoke.detail);
            } else {
                println!("  ! {}", smoke.detail);
                warnings += 1;
            }
        }
        None => {
            println!("  ✗ no container engine works: container steps cannot run");
            blockers += 1;
        }
    }
    if report
        .engine(Engine::Docker)
        .is_some_and(|report| report.is_podman_shim)
    {
        warnings += 1;
    }
    if !report.compose_works() {
        warnings += 1;
    }
    (blockers, warnings)
}

fn cargo_tools() -> Tally {
    let mut warnings = 0;
    for probe in tool::probe_all(Tool::all(), PROBE_TIMEOUT) {
        if !probe.available {
            warnings += 1;
        }
        println!("  {}", probe.line());
        if !probe.available {
            // What the course wants it for, so "install this" has a reason attached.
            println!("      needed for {}", probe.tool.purpose());
        }
    }
    (0, warnings)
}

/// Which sampler works on this machine, and which one only looks like it does.
///
/// Nothing here is a blocker: `perf/profiling@operate` is judged by the counters a harness prints
/// (`bench-budget`) and by the page a learner draws from them, so a machine without a sampling
/// profiler can still finish the step. What a learner cannot afford is spending an hour on a tool
/// that was never going to run: `cargo-flamegraph` is installed here and drives `dtrace`, which is
/// not, and `perf` is a Linux program. Saying so is the whole point of the section.
fn profilers() -> Tally {
    let mut warnings = 0;

    // The platform sampler: part of macOS, needs no root for a process you own.
    match process::locate("sample") {
        Some(path) => {
            println!(
                "  ✓ sample ({}) — `sample <pid> <seconds> -file out.txt` works on your own \
                 processes without root",
                path.display()
            );
        }
        None => println!("  · sample — not on PATH (this is not macOS)"),
    }

    // `samply` is the portable answer; the course names it, and this machine does not have it.
    match process::locate("samply") {
        Some(path) => println!("  ✓ samply ({})", path.display()),
        None => {
            println!("  ✗ samply — no such command (cargo install samply --locked)");
            println!(
                "      needed for a flame graph you can open and share; the platform sampler \
                 above is what works here"
            );
        }
    }

    // Installed and unusable is the interesting case, and the reason this section exists: on this
    // machine `cargo-flamegraph` is there and `dtrace` is there, and a non-root `dtrace` answers
    // "DTrace requires additional privileges". Probing by running is the only way to know.
    if let Some(path) = process::locate("cargo-flamegraph") {
        match dtrace_runs() {
            Some(true) => println!("  ✓ cargo flamegraph ({})", path.display()),
            Some(false) => {
                println!(
                    "  ! cargo flamegraph ({}) is installed and cannot run here: it drives \
                     `dtrace`, and a `dtrace` this user can start refuses with \"DTrace requires \
                     additional privileges\"",
                    path.display()
                );
                warnings += 1;
            }
            None => println!(
                "  ✓ cargo flamegraph ({}) — `dtrace` is not on this platform, which is what \
                 cargo-flamegraph needs on Linux too",
                path.display()
            ),
        }
    } else {
        println!("  ✗ cargo flamegraph — no such command (cargo install flamegraph --locked)");
    }

    // `perf` is Linux's sampler. Reporting it as missing on macOS would be noise, not a finding.
    if cfg!(target_os = "linux") {
        match process::locate("perf") {
            Some(path) => println!("  ✓ perf ({})", path.display()),
            None => {
                println!("  ✗ perf — no such command (install linux-tools for your kernel)");
                warnings += 1;
            }
        }
    }

    (0, warnings)
}

/// Whether `dtrace` can start here, as far as a probe can tell.
///
/// `Some(true)` when a trivial `dtrace` script ran, `Some(false)` when it is present and refused
/// (macOS refuses without root: "DTrace requires additional privileges"), and `None` when there is no
/// `dtrace` on this platform at all — which is not a defect, since `cargo-flamegraph` only needs it on
/// macOS.
fn dtrace_runs() -> Option<bool> {
    process::locate("dtrace")?;
    let ran = process::run(
        "dtrace",
        &["-n", "BEGIN { exit(0) }"],
        &std::env::temp_dir(),
        PROBE_TIMEOUT,
    )
    .ok()?;
    Some(ran.success() && !ran.stderr.contains("privileges"))
}

/// The registry mirror, the cargo source replacement, and every image the authored steps declare.
fn registry(session: &Session) -> Tally {
    let mut warnings = 0;
    let mirror = std::env::var("KILN_REGISTRY_MIRROR").ok();
    match &mirror {
        Some(mirror) => {
            println!("  ✓ KILN_REGISTRY_MIRROR={mirror} — image references are prefixed with it")
        }
        None => {
            println!(
                "  ! KILN_REGISTRY_MIRROR is not set: images come from their default registry, \
                 which can be slow or unreachable here"
            );
            warnings += 1;
        }
    }

    match cargo_source_mirror() {
        Some(path) => println!(
            "  ✓ cargo source replacement configured in {}",
            path.display()
        ),
        None => {
            println!(
                "  ! no cargo source replacement configured (~/.cargo/config.toml): crates.io is \
                 used directly"
            );
            warnings += 1;
        }
    }

    println!("  images declared by the authored tracks:");
    let declared = runtime::declared_images(session.course());
    for image in &declared {
        let resolved = mirror.as_deref().map_or_else(
            || image.reference.clone(),
            |mirror| format!("{mirror}/{}", image.reference),
        );
        println!("    {resolved}  ({} step(s))", image.steps.len());
    }
    if declared.is_empty() {
        println!("    none declared in the authored metadata yet");
    }
    (0, warnings)
}

/// `true` when `~/.cargo/config.toml` (or `$CARGO_HOME/config.toml`) replaces crates.io.
fn cargo_source_mirror() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("CARGO_HOME").map_or_else(
        || std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".cargo")),
        |cargo_home| Some(std::path::PathBuf::from(cargo_home)),
    )?;
    let path = home.join("config.toml");
    let text = std::fs::read_to_string(&path).ok()?;
    text.contains("replace-with").then_some(path)
}
