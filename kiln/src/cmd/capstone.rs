//! `kiln capstone [PATH]` — judge a free project with the course's own gates.
//!
//! The capstone (docs/capstone.md, plan §11 M10) has no scaffold and no reference solution: the
//! learner builds a service of their choosing and the course cannot know what it looks like. What it
//! *can* do is hold the result to the same production ladders every step was held to. This command
//! runs those gates against a directory, so "the capstone is done" means the same kind of thing as
//! "the step is done" — a scorecard, not a feeling.
//!
//! It is not a step and not a CI gate: there is no capstone project in this repository (the course
//! ships none), so the command is learner-side, and the review checklist in `docs/capstone.md` is
//! the human half.

use std::path::{Path, PathBuf};

use crate::caps::CapRequirement;
use crate::cmd::Session;
use crate::error::{EXIT_ERROR, EXIT_FAILED, EXIT_OK, KilnError, Result};
use crate::gates::runner::{self, GateContext};
use crate::gates::{GateSpec, Tier};
use crate::info::{Origin, Step, StepKind, Track};

/// The ladders a capstone is judged by: the generic ones, the same every step carries.
///
/// Deliberately without the step-specific gates (`spec-integrity`, `caps`, `i18n`, `images`,
/// `conformance`, `mutants`, …): those judge the course's own material, and the capstone has none.
const LADDER: &[&str] = &["fmt", "clippy@1", "no-todo", "docs:public", "exit-code"];

/// The capstone's own shape check, next to the gates: the project denies `unsafe`.
///
/// `unsafe-budget@0` is *step-scoped* — it counts the files a step's patches write, and a capstone
/// has no patches, so the gate can only answer "not judgeable" here. The lint is the project-wide
/// version of the same rule, and it is the one the compiler enforces for free.
const DENY_UNSAFE: &str = "unsafe-deny";

/// Judge the project at `path` (default `app/capstone`).
pub fn run(session: &Session, path: Option<&str>) -> Result<u8> {
    let project = match path {
        Some(path) => PathBuf::from(path),
        None => session.course().root().join("app/capstone"),
    };
    if !project.is_dir() {
        return Err(KilnError::Invalid(format!(
            "{} is not a directory — a capstone is a project you build (docs/capstone.md)",
            project.display()
        )));
    }

    let specs = ladder()?;
    let (track, step) = metadata(&project, &specs);
    let context = GateContext {
        course: session.course(),
        track: &track,
        step: &step,
        project_dir: project.clone(),
        timeout: session.timeout(),
        locale: session.locale(),
    };

    println!("kiln capstone · {}", project.display());
    let mut failed = 0;
    match denies_unsafe(&project) {
        Ok(()) => {
            println!("  ✓ {DENY_UNSAFE:<14} `[lints.rust] unsafe_code = \"deny\"` is in Cargo.toml")
        }
        Err(detail) => {
            println!("  ✗ {DENY_UNSAFE:<14} {detail}");
            failed += 1;
        }
    }
    let results = runner::run_all(&context, &specs, false);
    for result in &results {
        println!("  {}", result.line());
        if result.breaks_the_step() {
            failed += 1;
        }
    }
    if failed == 0 {
        println!("✓ every generic gate is green — the rest is docs/capstone.md's review checklist");
        return Ok(EXIT_OK);
    }
    println!(
        "✗ {failed} check(s) stop the capstone — a blocked gate is not a pass: it means the project \
         cannot be judged yet"
    );
    Ok(if failed == LADDER.len() + 1 {
        EXIT_ERROR
    } else {
        EXIT_FAILED
    })
}

/// Whether the project's manifest denies `unsafe` at the crate level.
fn denies_unsafe(project: &Path) -> std::result::Result<(), String> {
    let manifest = project.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|error| format!("cannot read {}: {error}", manifest.display()))?;
    let parsed: toml::Value = toml::from_str(&text)
        .map_err(|error| format!("{} is not valid TOML: {error}", manifest.display()))?;
    let denied = parsed
        .get("lints")
        .and_then(|lints| lints.get("rust"))
        .and_then(|rust| rust.get("unsafe_code"))
        .and_then(toml::Value::as_str)
        == Some("deny");
    if denied {
        Ok(())
    } else {
        Err("add `[lints.rust] unsafe_code = \"deny\"` to Cargo.toml — a capstone is safe code              unless it argues otherwise"
            .to_owned())
    }
}

/// The gate specs, must-tier, in ladder order.
fn ladder() -> Result<Vec<GateSpec>> {
    // Parsed, not built with `GateSpec::must`: `clippy@1` carries a parameter, and `must` keeps the
    // whole string as the name — the registry then says "unknown gate", which is `Pending`, which
    // the scorecard prints as a `·` and counts as a pass.
    let mut specs = Vec::with_capacity(LADDER.len());
    for name in LADDER {
        let mut spec: GateSpec = name.parse()?;
        spec.tier = Tier::Must;
        specs.push(spec);
    }
    Ok(specs)
}

/// The metadata the gates read: a synthetic track and step, because a capstone has no `track.toml`.
///
/// Only the fields the gates touch are meaningful; the ids exist so a report line can name what it
/// judged (`capstone/1.1-capstone`).
fn metadata(project: &Path, gates: &[GateSpec]) -> (Track, Step) {
    let track = Track {
        format_version: 1,
        id: "capstone".to_owned(),
        index: 10,
        title: "Capstone — a service of your own".to_owned(),
        localized_titles: std::collections::BTreeMap::new(),
        project: project.to_string_lossy().into_owned(),
        requires_track: None,
        exit: Vec::<CapRequirement>::new(),
        step: Vec::new(),
    };
    let step = Step {
        id: "1.1-capstone".to_owned(),
        title: "The capstone".to_owned(),
        kind: StepKind::Milestone,
        origin: Origin::Teach,
        entry: String::new(),
        time_hint: "8h".to_owned(),
        concepts: Vec::new(),
        teaches: Vec::<CapRequirement>::new(),
        requires: Vec::new(),
        run: None,
        spec: Vec::new(),
        scaffold: None,
        solution: None,
        gates: gates.to_vec(),
        hint: None,
        images: Vec::new(),
        reteach: None,
        track: "capstone".to_owned(),
        number: "1.1".to_owned(),
    };
    (track, step)
}

/// What the command judges, for the books.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_is_the_generic_one() {
        let specs = ladder().expect("the ladder parses");
        let names: Vec<String> = specs.iter().map(ToString::to_string).collect();
        assert_eq!(
            names,
            ["fmt", "clippy@1", "no-todo", "docs:public", "exit-code",]
        );
    }

    #[test]
    fn every_ladder_gate_is_a_must() {
        let specs = ladder().expect("the ladder parses");
        assert!(specs.iter().all(|spec| spec.tier == Tier::Must));
    }

    #[test]
    fn the_parameterised_gate_parses() {
        let specs = ladder().expect("the ladder parses");
        let clippy = specs
            .iter()
            .find(|spec| spec.name == "clippy")
            .expect("clippy is on the ladder");
        assert_eq!(clippy.param(), Some("1"));
    }

    #[test]
    fn a_manifest_without_the_deny_lint_is_refused() {
        let temp = crate::test_support::TempDir::new("capstone-deny");
        temp.write("Cargo.toml", "[package]\nname = \"x\"\n");
        assert!(denies_unsafe(temp.path()).is_err());
        temp.write(
            "Cargo.toml",
            "[package]\nname = \"x\"\n\n[lints.rust]\nunsafe_code = \"deny\"\n",
        );
        assert!(denies_unsafe(temp.path()).is_ok());
    }
}
