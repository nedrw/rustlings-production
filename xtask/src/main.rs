//! Repository tasks.
//!
//! `cargo xtask ci` is the only thing the CI workflow calls, so local and CI checks cannot drift
//! (plan §10). Adding a check means adding it here, not in YAML.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

fn main() -> ExitCode {
    let task = match std::env::args().nth(1) {
        Some(task) => task,
        None => {
            eprintln!("usage: cargo xtask <task>\n\n  ci    run every check CI runs");
            return ExitCode::from(2);
        }
    };

    let result = match task.as_str() {
        "ci" => ci(),
        other => Err(format!("unknown task `{other}`: try `cargo xtask ci`")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("\nxtask: {message}");
            ExitCode::FAILURE
        }
    }
}

/// The course's own anti-rot check, once per authored track (plan §10).
///
/// `kiln selftest` walks **one** track in order — that is deliberate, because the steps before the
/// ones being asserted are the state the step starts from. CI has to cover the rest of the path, so
/// the loop lives here rather than in the runner: a learner typing `kiln selftest` gets their own
/// track, and the repository's CI gets all of them, in path order.
///
/// A track is *authored* when at least one of its steps has a body, a spec and a reference solution;
/// a track whose content is still being written is named and skipped instead of failing the build.
fn selftest(root: &Path) -> Result<(), String> {
    let course = kiln::info::Course::load(root).map_err(|error| error.to_string())?;
    let mut tracks: Vec<&kiln::info::Track> = course.tracks().iter().collect();
    tracks.sort_by_key(|track| track.index);

    let mut walked = 0usize;
    for track in tracks {
        let authored = track
            .step
            .iter()
            .any(|step| kiln::project::missing_content(&course, track, step).is_empty());
        if !authored {
            println!(
                "xtask ci: kiln selftest — {} has no authored step yet, skipped",
                track.id
            );
            continue;
        }
        println!("xtask ci: kiln selftest --track {}", track.id);
        run(
            root,
            "cargo",
            &[
                "run",
                "--quiet",
                "--package",
                "kiln",
                "--",
                "selftest",
                "--track",
                &track.id,
            ],
        )?;
        walked += 1;
    }
    if walked == 0 {
        return Err("no track of this course has authored content yet".to_owned());
    }
    Ok(())
}

/// Every check CI runs, in the order that fails fastest.
fn ci() -> Result<(), String> {
    let root = workspace_root()?;
    run(&root, "cargo", &["fmt", "--all", "--check"])?;
    // The files cargo cannot see: the specs and the probe exercises live under `course/` and only
    // ever reach a compiler as a copy, but the course asks its learners for formatted code
    // (authoring.md §6.7), so its own material is held to the same rule here.
    format_course_rust(&root)?;
    run(
        &root,
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run(&root, "cargo", &["test", "--workspace"])?;
    spec_integrity(&root)?;
    patches_check(&root)?;
    lint_caps(&root)?;
    // `links` reads the tree and answers in under a second; `probe_check` starts a scratch crate.
    links(&root)?;
    probe_check(&root)?;
    // The course's own anti-rot check (plan §10). It is the command a learner can run too, so CI
    // and a human have one implementation between them, not two. Every authored track is walked:
    // `kiln selftest` defaults to the course's default track (the one a learner is on), and CI has
    // to cover the others or T2+ would only be checked when somebody remembers to.
    selftest(&root)?;
    println!("\nxtask ci: all checks passed");
    Ok(())
}

/// Both directions of the spec hash check, plus the metadata that names the spec files.
///
/// Delegates to the library for the same reason `lint_caps` does: `kiln` and `xtask` must agree on
/// what "the specs are unchanged" means, and the only way to guarantee that is one implementation.
fn spec_integrity(root: &Path) -> Result<(), String> {
    println!("xtask ci: spec integrity (spec/** against spec/manifest.toml)");
    let audit = kiln::spec::audit(root).map_err(|error| error.to_string())?;
    println!(
        "  {} spec file(s) pinned, {} step(s) declare their own",
        audit.files, audit.steps
    );
    if audit.is_clean() {
        return Ok(());
    }
    for problem in &audit.problems {
        println!("  {problem}");
    }
    Err(format!(
        "{} spec integrity problem(s)",
        audit.problems.len()
    ))
}

/// `tools/rebuild-patches.py --check` over every authored track: every committed scaffold and
/// solution patch equals what the trees it defines produce.
///
/// The patches are derived artifacts — step N's scaffold is the diff between step N−1's full tree
/// and its stub tree — and a hand-edited patch corrupts its hunks long before anything complains
/// (three measured incidents, handoff-m4-log 段 12). This check makes the derivation a property CI
/// holds, in both directions: a patch edited without its trees fails, and a fix applied to the
/// trees without regenerating the patches fails too. The tool lays the chains with `kiln` itself
/// and never compiles, so the whole pass costs seconds.
fn patches_check(root: &Path) -> Result<(), String> {
    println!("xtask ci: rebuild-patches --self-test (the tool's own checks)");
    let self_test = Command::new("python3")
        .arg(root.join("tools/rebuild-patches.py"))
        .arg("--self-test")
        .current_dir(root)
        .stdin(Stdio::null())
        .status()
        .map_err(|err| format!("cannot run rebuild-patches.py --self-test: {err}"))?;
    if !self_test.success() {
        return Err(format!("rebuild-patches --self-test failed ({self_test})"));
    }

    println!("xtask ci: rebuild-patches --check (every patch is what its trees produce)");
    let course = kiln::info::Course::load(root).map_err(|error| error.to_string())?;
    let mut tracks: Vec<&kiln::info::Track> = course.tracks().iter().collect();
    tracks.sort_by_key(|track| track.index);

    // The tool lays the chains with kiln; point it at the binary `cargo test --workspace` built a
    // step ago rather than paying a `cargo run` per chain command.
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let kiln = target.join("debug").join("kiln");
    if !kiln.is_file() {
        return Err(format!(
            "no kiln binary at {} — build it first (`cargo build -p kiln`)",
            kiln.display()
        ));
    }

    for track in tracks {
        let authored = track
            .step
            .iter()
            .any(|step| kiln::project::missing_content(&course, track, step).is_empty());
        if !authored {
            continue;
        }
        let status = Command::new("python3")
            .arg(root.join("tools/rebuild-patches.py"))
            .arg("--track")
            .arg(&track.id)
            .arg("--check")
            .env("KILN_BIN", &kiln)
            .current_dir(root)
            .stdin(Stdio::null())
            .status()
            .map_err(|err| format!("cannot run rebuild-patches.py: {err}"))?;
        if !status.success() {
            return Err(format!(
                "rebuild-patches --check failed for {} ({status}) — the committed patches no \
                 longer match the trees they define; the tool's own output above says which, and \
                 regenerating them with the same tool is the fix",
                track.id
            ));
        }
    }
    Ok(())
}

/// The course's own linter, called through the library the runner uses.
///
/// Deliberately not a nested `cargo run`: `xtask` and `kiln` must agree on what the metadata
/// rules are, and the only way to guarantee that is to call the same code (plan §10).
fn lint_caps(root: &Path) -> Result<(), String> {
    println!("xtask ci: kiln lint-caps (rules R0-R6 over course/)");
    let report = kiln::caps::lint::lint_root(root).map_err(|error| error.to_string())?;
    println!(
        "  {} capabilities · {} slots · {} ledger positions · {} authored steps",
        report.capabilities, report.slots, report.ledger_positions, report.authored_steps
    );
    for notice in &report.skipped {
        println!("  skipped: {notice}");
    }
    for (step, reason) in &report.reteaches {
        println!("  reteach {step}: {reason}");
    }
    if report.is_clean() {
        return Ok(());
    }
    for issue in &report.issues {
        println!("  {issue}");
    }
    Err(format!("{} lint-caps issue(s)", report.issues.len()))
}

/// `kiln probe --check`: every probe reference solution compiles and passes.
///
/// Called through the binary rather than the library because it starts `cargo` in a scratch crate:
/// one implementation, the one a learner can run (`kiln probe --check`), and the reason this check
/// is worth its seconds is that a probe which cannot pass sends people to a broken exercise.
fn probe_check(root: &Path) -> Result<(), String> {
    println!("xtask ci: kiln probe --check (every reference solution passes)");
    run_allow_not_authored(
        root,
        "cargo",
        &[
            "run",
            "--quiet",
            "--package",
            "kiln",
            "--",
            "probe",
            "--check",
        ],
    )
}

/// Format the Rust the course ships that cargo cannot reach: specs and probe exercises.
///
/// They live under `course/` and are copied into a project (or a scratch crate) before anything
/// compiles them, so `cargo fmt --all` never sees them. One `rustfmt` invocation over every file:
/// the command a maintainer runs by hand (authoring.md §6.7) is the command CI runs.
fn format_course_rust(root: &Path) -> Result<(), String> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_rust(&root.join("course/tracks"), &mut files)?;
    collect_rust(&root.join("course/probes"), &mut files)?;
    files.sort();
    if files.is_empty() {
        return Ok(());
    }
    println!(
        "xtask ci: rustfmt --check on {} course file(s) under course/",
        files.len()
    );
    let mut command = Command::new("rustfmt");
    command
        .args([
            "--edition",
            "2024",
            "--config-path",
            "rustfmt.toml",
            // Every file is named on the command line, so rustfmt must not follow `mod`
            // declarations: a spec's `mod support;` only resolves after a scaffold has copied the
            // fixtures into a project (which is where the `fmt` gate sees them again).
            "--config",
            "skip_children=true",
            "--check",
        ])
        .args(&files)
        .current_dir(root)
        .stdin(Stdio::null());
    let status = command
        .status()
        .map_err(|err| format!("cannot run rustfmt: {err} — `rustup component add rustfmt`"))?;
    if status.success() {
        return Ok(());
    }
    Err(format!(
        "rustfmt reports files under course/ that are not formatted ({status})"
    ))
}

/// Every `.rs` file below `dir`, recursively. A missing directory is not an error.
fn collect_rust(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

/// The repository's own markdown, held to the rule a learner's pages are held to.
///
/// `links` is offline by decision (plan §6), which is what makes it CI material: every thing it
/// judges — does a relative target resolve, does an anchor name a heading that exists, is a target
/// a placeholder — is a fact about the tree, and the answer is the same on two machines. Until this
/// step existed, link rot in the repository's own pages (README, docs/) was only caught by a
/// maintainer remembering to run `kiln links` by hand.
///
/// The learner projects under `app/` are skipped, exactly as the command's default report skips
/// them: they are working copies, mid-step placeholders are the point of them, and the step's own
/// `links` gate is what judges them.
fn links(root: &Path) -> Result<(), String> {
    println!("xtask ci: kiln links (offline; the repository's own markdown)");
    let course = kiln::info::Course::load(root).map_err(|error| error.to_string())?;
    let skipped = course.project_roots();
    let names: Vec<&str> = skipped.iter().map(String::as_str).collect();
    let scan =
        kiln::gates::links::scan_skipping(root, &names).map_err(|error| error.to_string())?;
    println!(
        "  {} page(s) · {} link(s) · {} external target(s) (not fetched)",
        scan.pages(),
        scan.links(),
        scan.external().len()
    );
    for name in &skipped {
        println!("  skipped {name}/ — a learner's project, judged by its step's `links` gate");
    }
    if scan.is_clean() {
        return Ok(());
    }
    for finding in scan.findings() {
        println!("  {}", finding.line());
    }
    Err(format!(
        "{} link(s) in the repository's markdown do not resolve",
        scan.findings().len()
    ))
}

/// Repository root, derived from this crate's location so the task works from anywhere.
fn workspace_root() -> Result<PathBuf, String> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{} has no parent directory", manifest.display()))
}

/// Run one check, streaming its output.
fn run(root: &Path, program: &str, args: &[&str]) -> Result<(), String> {
    println!("xtask ci: {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .status()
        .map_err(|err| format!("cannot run {program}: {err}"))?;
    if status.success() {
        return Ok(());
    }
    Err(format!("{program} {} failed ({status})", args.join(" ")))
}

/// Run one check that is allowed to answer "not authored yet" (exit code 3).
///
/// The runner's exit codes are part of its contract (`kiln/src/cmd/mod.rs`): 3 means nothing was
/// judged because the content does not exist. For a check over content that is still being written
/// (probes arrive with M1), that is not a CI failure — but it must be *said*, not silently allowed.
fn run_allow_not_authored(root: &Path, program: &str, args: &[&str]) -> Result<(), String> {
    println!("xtask ci: {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .status()
        .map_err(|err| format!("cannot run {program}: {err}"))?;
    if status.success() {
        return Ok(());
    }
    if status.code() == Some(3) {
        println!("  (exit 3: nothing authored yet — not a failure)");
        return Ok(());
    }
    Err(format!("{program} {} failed ({status})", args.join(" ")))
}
