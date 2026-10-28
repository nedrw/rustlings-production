//! `kiln new-track <id>` — lay out a new track's mechanical half (authoring.md §12).
//!
//! Adding a track touches two kinds of thing, and only one of them is mechanical:
//!
//! * **Mechanical**: the directory layout, the `track.toml` header and one `[[step]]` per step,
//!   the learner's project skeleton, the spec file's name, the entry's path, the gate ladder's
//!   starting rung. Every one of those has exactly one right answer, and getting it wrong produces
//!   a wall of red that says nothing about what to fix.
//! * **Judgment**: which capability a step teaches and at what depth (the ledger is the source of
//!   truth and it is a curriculum decision), what the track's exit capabilities are, and the probe
//!   that re-checks each `apply` slot. Those are *not* generated here: a generator that invented
//!   them would produce a course that passes its own checks and teaches nothing.
//!
//! So this command writes the first kind and prints the second as a checklist, and it refuses to
//! write over anything that already exists.

use std::path::{Path, PathBuf};

use crate::cmd::Session;
use crate::error::{EXIT_OK, KilnError, Result};

/// The gates a track's steps start at, and what each later step adds.
///
/// The rule is plan §6's: **a gate is introduced by the step that teaches it**, and stays (the ladder
/// only tightens). The three that come with every step are the ones that judge the course's own
/// material — the spec's hash, the capability ledger, and the i18n structure. The rest arrive with
/// the step that teaches the habit they police, which is why the first step of a track does *not*
/// carry them: a gate that appears before the capability it polices fails a learner for something
/// the track has not taught yet.
///
/// `clippy@1` is the exception every track starts with: `mailer/1.1` and `jobq/3.1` both carry the
/// lint ladder from their first step, because the project skeleton is where the policy is set.
const FIRST_STEP_GATES: &[&str] = &["spec-integrity", "caps", "i18n", "fmt", "clippy@1"];

/// What the second step adds.
const SECOND_STEP_GATES: &[&str] = &["no-todo", "docs:public", "exit-code"];

/// What the third step adds, and every step after it keeps.
const LATER_STEP_GATES: &[&str] = &["unsafe-budget@0"];

/// One step as the author asked for it.
struct Wanted {
    /// Local id, `4.3-downsampling`.
    id: String,
    /// The capability and depth it teaches, `query/downsampling@apply`.
    teaches: String,
    /// Why this step re-establishes a depth the course already has, when it does.
    reteach: Option<String>,
}

/// Create a track's layout.
pub fn run(
    session: &Session,
    id: &str,
    title: Option<&str>,
    project: Option<&str>,
    steps: &[String],
) -> Result<u8> {
    let course = session.course();
    let root = course.root();
    let track_dir = root.join("course/tracks").join(id);
    if track_dir.exists() {
        return Err(KilnError::Invalid(format!(
            "{} already exists — `kiln new-track` never writes over a track",
            track_dir.display()
        )));
    }

    let wanted = parse_steps(steps)?;
    if wanted.is_empty() {
        return Err(KilnError::Invalid(
            "a track needs at least one step: pass `--step <phase>.<index>-<slug>:<capability>@<depth>` \
             (for example `--step 1.1-first-step:repo/workspace-layout@apply`)"
                .to_owned(),
        ));
    }

    let index = next_track_index(course);
    let project = project.map_or_else(|| format!("app/{id}"), str::to_owned);
    let crate_name = project.rsplit('/').next().unwrap_or(id).to_owned();

    write_track_toml(
        &track_dir,
        id,
        title.unwrap_or(id),
        &project,
        index,
        &wanted,
    )?;
    write_skeleton(&track_dir, &crate_name)?;
    write_steps(root, id, &wanted)?;
    write_specs(&track_dir, id, &wanted)?;
    // The two patch directories exist even when they are empty: a track whose `scaffold/` and
    // `solutions/` are missing is a track the author has to guess the shape of, and the shape is
    // one of the things this command is here to lay down.
    for directory in ["scaffold", "solutions"] {
        let path = track_dir.join(directory);
        std::fs::create_dir_all(&path).map_err(|source| KilnError::Write {
            path: path.clone(),
            source,
        })?;
    }
    // The spec manifest pins the hash of every spec file, and the `spec-integrity` gate reads it —
    // so a track without a manifest fails its first step on a technicality the author never had a
    // chance to know about. Generating it here is the difference between "the gate is red because
    // your spec is wrong" and "the gate is red because a file nobody mentioned is missing".
    let manifest = crate::spec::Manifest::generate(&track_dir)?;
    let manifest_path = manifest.save(&track_dir)?;

    // The ledger knows where every capability is established, so the generator can say *now* that
    // the step the author asked for conflicts with what the course already teaches — instead of
    // letting `kiln lint-caps` say it later in a wall of red.
    let conflicts = ledger_conflicts(course, &wanted);
    if !conflicts.is_empty() {
        println!("the ledger already establishes what these steps ask for:");
        for line in conflicts {
            println!("  {line}");
        }
        println!();
    }

    println!("laid the track `{id}`:");
    println!("  {}", track_dir.join("track.toml").display());
    println!("  {}", track_dir.join("skeleton").display());
    println!("  {} spec file(s)", wanted.len());
    println!("  {}", manifest_path.display());
    println!(
        "  {} step bod{}",
        wanted.len(),
        if wanted.len() == 1 { "y" } else { "ies" }
    );
    println!();
    println!("what a generator must not decide for you — the checklist:");
    println!("  1. the ledger: every `teaches` above needs a slot in course/capabilities.toml");
    println!("     (add it at the depth you wrote, with this step's id); `kiln lint-caps` says so");
    println!(
        "  2. the exit capabilities: `exit = [...]` in the track header, each one established"
    );
    println!("     by a step of this track (rule R6)");
    println!("  3. the probes: every `apply` slot needs a probe in course/probes/ — `kiln probe");
    println!("     --check` and authoring.md §11 say how");
    println!(
        "  4. the spec and the solution: the generated spec is a placeholder that fails; write"
    );
    println!("     the real one, and the solution patch that answers it");
    println!("  5. the step bodies: the generated ones are templates — write the prose");
    println!();
    println!("then:");
    println!("  cargo run -p kiln -- lint-caps");
    println!("  cargo run -p kiln -- selftest --track {id}");
    Ok(EXIT_OK)
}

/// Parse `--step <id>:<capability>@<depth>` arguments.
fn parse_steps(steps: &[String]) -> Result<Vec<Wanted>> {
    let mut wanted = Vec::with_capacity(steps.len());
    for step in steps {
        let mut parts = step.split(':');
        let id = parts.next().unwrap_or_default();
        let teaches = parts.next().ok_or_else(|| {
            KilnError::Invalid(format!(
                "`{step}` is not `<phase>.<index>-<slug>:<capability>@<depth>[:<why>]`"
            ))
        })?;
        let reteach = parts.next().map(str::to_owned);
        if parts.next().is_some() {
            return Err(KilnError::Invalid(format!(
                "`{step}` has more than three parts"
            )));
        }
        if !teaches.contains('@') {
            return Err(KilnError::Invalid(format!(
                "`{teaches}` has no depth: a capability is taught *at* a depth (`...@apply`)"
            )));
        }
        wanted.push(Wanted {
            id: id.to_owned(),
            teaches: teaches.to_owned(),
            reteach,
        });
    }
    Ok(wanted)
}

/// The next free track index.
fn next_track_index(course: &crate::info::Course) -> u32 {
    course
        .tracks()
        .iter()
        .map(|track| track.index)
        .max()
        .map_or(1, |highest| highest + 1)
}

/// The track's metadata file, with one `[[step]]` per wanted step.
fn write_track_toml(
    track_dir: &Path,
    id: &str,
    title: &str,
    project: &str,
    index: u32,
    steps: &[Wanted],
) -> Result<()> {
    let crate_name = project.rsplit('/').next().unwrap_or(id);
    let mut lines: Vec<String> = vec![
        format!("# The `{id}` track — {title}."),
        "#".to_owned(),
        "# Written by `kiln new-track`. What a generator can decide is here; what needs a"
            .to_owned(),
        "# curriculum decision (the ledger slots, the exit capabilities, the probes) is listed"
            .to_owned(),
        "# in the command's output and in authoring.md §12.".to_owned(),
        String::new(),
        "format_version = 1".to_owned(),
        format!("id = \"{id}\""),
        format!("index = {index}"),
        format!("title = \"{title}\""),
        format!("project = \"{project}\""),
        "exit = []".to_owned(),
    ];
    for (number, step) in steps.iter().enumerate() {
        let stem = step
            .id
            .split_once('-')
            .map_or(step.id.as_str(), |(_, slug)| slug);
        let spec_stem = stem.replace('-', "_");
        let scaffold = if number == 0 {
            "skeleton".to_owned()
        } else {
            format!("scaffold/{}.patch", step.id)
        };
        lines.push(String::new());
        lines.push("[[step]]".to_owned());
        lines.push(format!("id = \"{}\"", step.id));
        lines.push("title = \"TODO: the title of this step\"".to_owned());
        lines.push("kind = \"milestone\"".to_owned());
        lines.push("origin = \"teach\"".to_owned());
        lines.push(format!("entry = \"docs/steps/en/{id}/{}.md\"", step.id));
        lines.push("time_hint = \"60m\"".to_owned());
        lines.push("concepts = []".to_owned());
        lines.push(format!("teaches = [\"{}\"]", step.teaches));
        if let Some(reason) = &step.reteach {
            lines.push(format!("reteach = \"{reason}\""));
        }
        lines.push("requires = []".to_owned());
        lines.push(format!(
            "run = {{ cmd = \"cargo test -p {crate_name} --test spec_{spec_stem}\" }}"
        ));
        lines.push(format!(
            "spec = [\"spec/{}/{}.rs\"]",
            number_of(&step.id),
            spec_stem
        ));
        lines.push(format!("scaffold = \"{scaffold}\""));
        lines.push(format!("solution = \"solutions/{}.patch\"", step.id));
        lines.push("gates = [".to_owned());
        let mut gates: Vec<&str> = FIRST_STEP_GATES.to_vec();
        if number >= 1 {
            gates.extend(SECOND_STEP_GATES.iter().copied());
        }
        if number >= 2 {
            gates.extend(LATER_STEP_GATES.iter().copied());
        }
        for gate in gates {
            lines.push(format!("    \"{gate}\","));
        }
        lines.push("]".to_owned());
    }
    write_file(&track_dir.join("track.toml"), &join(lines))
}

/// The conflicts between the wanted steps and the ledger, in the author's words.
fn ledger_conflicts(course: &crate::info::Course, steps: &[Wanted]) -> Vec<String> {
    let ledger = course.ledger();
    let mut conflicts = Vec::new();
    for step in steps {
        let Some((capability, depth)) = step.teaches.split_once('@') else {
            continue;
        };
        let Ok(capability) = crate::caps::CapabilityId::parse(capability) else {
            conflicts.push(format!(
                "{}: `{capability}` is not a capability id (`<domain>/<name>`)",
                step.id
            ));
            continue;
        };
        let Some(level) = crate::caps::Level::ALL
            .into_iter()
            .find(|level| level.as_str() == depth)
        else {
            conflicts.push(format!(
                "{}: `{depth}` is not a depth (aware, apply, design, operate)",
                step.id
            ));
            continue;
        };
        if let Some(position) = ledger.slot(&capability, level) {
            if step.reteach.is_some() {
                // Declared on purpose: `lint-caps` reports every re-teach, which is the point.
                conflicts.push(format!(
                    "{}: `{capability}` at {level} was already established by {}/{} — this step \
                     declares the re-teach, so the ledger stays as it is and `lint-caps` will \
                     report it",
                    step.id, position.track, position.number
                ));
            } else {
                conflicts.push(format!(
                    "{}: `{capability}` is already established at {level} by {}/{} — a new track \
                     either teaches a *deeper* depth, or says `reteach = \"why\"` on the step (and \
                     the ledger needs a slot for it)",
                    step.id, position.track, position.number
                ));
            }
        }
    }
    conflicts
}

/// The phase index of a step id: `4.3-downsampling` → `4.3`.
fn number_of(id: &str) -> &str {
    id.split_once('-').map_or(id, |(number, _)| number)
}

/// The skeleton's `Cargo.toml`.
///
/// `[lints.rust] unsafe_code = "deny"` is not decoration: `unsafe-budget@0` lands on the third
/// step (see `LATER_STEP_GATES`), and the gate asks the project to *deny* `unsafe`, not merely to
/// contain none — a gate that only counts what is there today would go green on a tree that grows
/// an `unsafe` block tomorrow. A track that later teaches `unsafe` removes this in the scaffold
/// that does.
fn manifest(crate_name: &str) -> String {
    join(vec![
        format!("# The `{crate_name}` service, as it comes out of the box."),
        "#".to_owned(),
        "# The `kiln:scaffold:begin`/`end` blocks are the regions later scaffolds are allowed to"
            .to_owned(),
        "# write into (authoring.md §5).".to_owned(),
        String::new(),
        "[package]".to_owned(),
        format!("name = \"{crate_name}\""),
        "version = \"0.1.0\"".to_owned(),
        "edition = \"2024\"".to_owned(),
        String::new(),
        "[dependencies]".to_owned(),
        "# kiln:scaffold:begin".to_owned(),
        "# kiln:scaffold:end".to_owned(),
        String::new(),
        "[lints.rust]".to_owned(),
        "unsafe_code = \"deny\"".to_owned(),
        String::new(),
        "# This project is its own workspace, so it never picks up the course's settings"
            .to_owned(),
        "# (docs/plan.md §3 D6).".to_owned(),
        "[workspace]".to_owned(),
        "members = [".to_owned(),
        "    # kiln:scaffold:begin".to_owned(),
        "    # kiln:scaffold:end".to_owned(),
        "]".to_owned(),
    ])
}

/// The learner's project skeleton: a crate that builds, with the regions later scaffolds write into.
fn write_skeleton(track_dir: &Path, crate_name: &str) -> Result<()> {
    let manifest = manifest(crate_name);
    // The stub *fails*: a skeleton whose stub already satisfies a spec would make the first step
    // "BROKEN STEP — the spec passes on the scaffold" the moment the author wrote the spec. The
    // learner is meant to replace the `todo!()`; the reference solution replaces it too.
    let lib = join(vec![
        "//! The service, as it comes out of the box.".to_owned(),
        "//!".to_owned(),
        "//! Every step of this track adds to the library; the binary stays a shell.".to_owned(),
        "//!".to_owned(),
        "//! # TODO: the first step of this track replaces this stub.".to_owned(),
        String::new(),
        "/// The version of the service.".to_owned(),
        "#[must_use]".to_owned(),
        "pub fn version() -> &'static str {".to_owned(),
        "    todo!(\"the first step of this track implements this\")".to_owned(),
        "}".to_owned(),
        String::new(),
        "// The region later scaffolds are allowed to write into (authoring.md §5). A scaffold may"
            .to_owned(),
        "// only *insert* lines inside it, and only *remove* nothing.".to_owned(),
        "// kiln:scaffold:begin".to_owned(),
        "// kiln:scaffold:end".to_owned(),
    ]);
    // The binary refuses an argument it does not understand: the `exit-code` gate (from the second
    // step on) asks the program to exit non-zero and *name* the argument it refused, and a binary
    // that ignores unknown arguments would fail that gate on its first run.
    let main = join(vec![
        "//! The binary: it asks the library what to do and exits with the code it decides."
            .to_owned(),
        "//!".to_owned(),
        "//! # TODO: the first step of this track replaces this stub.".to_owned(),
        String::new(),
        "use std::process::ExitCode;".to_owned(),
        String::new(),
        "const USAGE: &str = \"\\".to_owned(),
        format!("usage: {crate_name} [OPTIONS]"),
        String::new(),
        "OPTIONS:".to_owned(),
        "    --help       print this help and exit".to_owned(),
        "    --version    print the version and exit".to_owned(),
        "\";".to_owned(),
        String::new(),
        "fn main() -> ExitCode {".to_owned(),
        "    let args: Vec<String> = std::env::args().skip(1).collect();".to_owned(),
        "    let Some(flag) = args.first() else {".to_owned(),
        format!("        eprintln!(\"{crate_name}: nothing to do yet (try --help)\");"),
        "        return ExitCode::SUCCESS;".to_owned(),
        "    };".to_owned(),
        "    match flag.as_str() {".to_owned(),
        "        \"--help\" => {".to_owned(),
        "            println!(\"{USAGE}\");".to_owned(),
        "            ExitCode::SUCCESS".to_owned(),
        "        }".to_owned(),
        "        \"--version\" => {".to_owned(),
        format!("            println!(\"{crate_name} {{}}\", {crate_name}::version());"),
        "            ExitCode::SUCCESS".to_owned(),
        "        }".to_owned(),
        "        other => {".to_owned(),
        format!(
            "            eprintln!(\"{crate_name}: unknown argument `{{other}}`\\n{{USAGE}}\");"
        ),
        "            ExitCode::from(2)".to_owned(),
        "        }".to_owned(),
        "    }".to_owned(),
        "}".to_owned(),
    ]);
    write_file(&track_dir.join("skeleton/Cargo.toml"), &manifest)?;
    write_file(&track_dir.join("skeleton/src/lib.rs"), &lib)?;
    write_file(&track_dir.join("skeleton/src/main.rs"), &main)?;
    Ok(())
}

/// One step body per step: the section skeleton the origin asks for.
fn write_steps(root: &Path, id: &str, steps: &[Wanted]) -> Result<()> {
    for step in steps {
        let body = join(vec![
            "+++".to_owned(),
            format!("id = \"{id}/{}\"", step.id),
            "title = \"TODO: the title of this step\"".to_owned(),
            "locale = \"en\"".to_owned(),
            "+++".to_owned(),
            String::new(),
            "# TODO: the title of this step".to_owned(),
            String::new(),
            "## WHY".to_owned(),
            String::new(),
            "TODO: what this step is for, and the decision it hides.".to_owned(),
            String::new(),
            "## TASK".to_owned(),
            String::new(),
            "TODO: what the learner has to build.".to_owned(),
            String::new(),
            "## DONE WHEN".to_owned(),
            String::new(),
            format!("- `kiln run {id}/{}` — the spec passes", step.id),
            format!("- `kiln check {id}/{}` — every must gate is green", step.id),
            String::new(),
            "## TRAP".to_owned(),
            String::new(),
            "1. TODO: the mistake this step is about, and how the spec catches it.".to_owned(),
            String::new(),
            "## FURTHER".to_owned(),
            String::new(),
            "- TODO: one reading, and why it is worth the reader's time".to_owned(),
        ]);
        write_file(
            &root
                .join("course/docs/steps/en")
                .join(id)
                .join(format!("{}.md", step.id)),
            &body,
        )?;
    }
    Ok(())
}

/// One spec file per step: a placeholder that *fails*, so the step cannot pass until it is written.
fn write_specs(track_dir: &Path, id: &str, steps: &[Wanted]) -> Result<()> {
    for step in steps {
        let stem = step
            .id
            .split_once('-')
            .map_or(step.id.as_str(), |(_, slug)| slug)
            .replace('-', "_");
        let spec = join(vec![
            format!("//! Spec for `{id}/{}` — TODO: what this step must be true about.", step.id),
            "//!".to_owned(),
            "//! This is the placeholder `kiln new-track` wrote: it fails, on purpose. A spec that".to_owned(),
            "//! passes on the scaffold cannot tell the two states apart (authoring.md §6.2), so the".to_owned(),
            "//! placeholder must fail until the real assertion replaces it.".to_owned(),
            String::new(),
            "#[test]".to_owned(),
            "fn todo_this_spec_must_be_written() {".to_owned(),
            format!("    panic!(\"the spec for `{id}/{}` is still the placeholder\")", step.id),
            "}".to_owned(),
        ]);
        write_file(
            &track_dir
                .join("spec")
                .join(number_of(&step.id))
                .join(format!("{stem}.rs")),
            &spec,
        )?;
    }
    Ok(())
}

/// Join lines with newlines and end with one.
fn join(lines: Vec<String>) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// Write a file, refusing to overwrite one that is there.
fn write_file(path: &Path, content: &str) -> Result<()> {
    if path.exists() {
        return Err(KilnError::Invalid(format!(
            "{} already exists — `kiln new-track` never writes over a file",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| KilnError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, content).map_err(|source| KilnError::Write {
        path: PathBuf::from(path),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The acceptance run's wall (handoff-m5 §16): without the lint, the third step's
    /// `unsafe-budget@0` gate was red on a project that had never written `unsafe`.
    #[test]
    fn the_skeleton_denies_unsafe_because_the_third_step_asks_it_to() {
        let manifest = manifest("toy");
        assert!(
            manifest.contains("[lints.rust]\nunsafe_code = \"deny\""),
            "the generated manifest must deny unsafe:\n{manifest}"
        );
    }

    /// Step ids carry the phase: `4.3-downsampling` → `4.3`, which is the spec directory name.
    #[test]
    fn a_step_id_knows_its_number() {
        assert_eq!(number_of("4.3-downsampling"), "4.3");
        assert_eq!(number_of("1.1"), "1.1");
    }
}
