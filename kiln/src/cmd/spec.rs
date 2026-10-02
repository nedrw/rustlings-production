//! `kiln spec` — the read-only spec files.
//!
//! Two commands, and they answer two different problems:
//!
//! * `kiln spec restore [step]` puts the course copy back into the learner project. It is the command
//!   the `spec-integrity` gate names in its failure, so the gate's message never ends in a dead end.
//!   It refuses to restore from a course copy that no longer matches `spec/manifest.toml` — copying
//!   a tampered source into the project would spread the tampering instead of repairing it.
//! * `kiln spec manifest [--track <track>]` re-pins the hashes. This is a course-maintenance
//!   command: run it after adding or deliberately editing a spec, and commit the result.

use crate::cmd::{Session, SpecCommand};
use crate::error::{EXIT_ERROR, EXIT_OK, Result};
use crate::info::Track;
use crate::spec;

/// Dispatch `kiln spec`.
pub fn run(session: &Session, command: &SpecCommand) -> Result<u8> {
    match command {
        SpecCommand::Restore { step } => restore(session, step.as_deref()),
        SpecCommand::Manifest { track } => manifest(session, track.as_deref()),
    }
}

/// Re-sync spec files from the course copy into the learner project.
fn restore(session: &Session, reference: Option<&str>) -> Result<u8> {
    let (steps, without_content) = selected(session, reference)?;
    if steps.is_empty() {
        println!("no step in this course ships spec files yet — there is nothing to restore");
        return Ok(EXIT_OK);
    }
    let mut restored = 0usize;
    let mut blocked = 0usize;

    for (track, step) in &steps {
        let report = spec::check_course(session.course().root(), track)?;
        if !report.is_clean() {
            blocked += 1;
            println!(
                "✗ {}: the course copy of the spec is not the pinned one, so restoring from it \
                 would spread the change:",
                step.qualified_id()
            );
            for problem in &report.problems {
                println!("    {problem}");
            }
            continue;
        }
        let project_dir = track.project_dir(session.course().root());
        let synced = spec::sync(session.course().root(), track, step, &project_dir, true)?;
        println!("{} {}", step.qualified_id(), step.title);
        for entry in &synced {
            println!("  {:<10} {}", entry.action.as_str(), entry.file);
            if entry.action != spec::SyncAction::Unchanged {
                restored += 1;
            }
        }
    }

    println!();
    if blocked > 0 {
        println!(
            "{restored} file(s) restored, {blocked} step(s) refused: run `kiln spec manifest` if \
             the course copies are right, or restore them from the course repository"
        );
        return Ok(EXIT_ERROR);
    }
    println!("the project now holds the course's spec files ({restored} written).");
    if without_content > 0 {
        println!(
            "({without_content} step(s) of these tracks have no spec files yet: their content is \
             not authored)"
        );
    }
    Ok(EXIT_OK)
}

/// Regenerate the generated hash manifests.
fn manifest(session: &Session, track: Option<&str>) -> Result<u8> {
    let tracks: Vec<&Track> = match track {
        Some(id) => vec![session.course().track(id)?],
        None => session.course().tracks().iter().collect(),
    };
    let mut total = 0usize;
    for track in tracks {
        let track_dir = track.dir(session.course().root());
        let manifest = spec::Manifest::generate(&track_dir)?;
        if manifest.is_empty() {
            println!(
                "{}: no spec files under {} — nothing pinned",
                track.id,
                track_dir.join(spec::SPEC_DIR).display()
            );
            continue;
        }
        let path = manifest.save(&track_dir)?;
        total += manifest.len();
        println!(
            "{}: pinned {} file(s) in {}",
            track.id,
            manifest.len(),
            relative(session, &path)
        );
        for (file, hash) in &manifest.spec {
            println!("  {hash}  {file}");
        }
    }
    println!();
    println!("{total} spec file(s) pinned — commit the manifest with the specs it pins.");
    Ok(EXIT_OK)
}

/// The steps a `spec restore` should look at: one step, or every step that ships spec files.
///
/// Steps whose spec files are not written yet are skipped rather than reported as broken: a track
/// is authored one step at a time, and `spec restore` is a command a learner runs to get back to
/// work, not a course linter.
fn selected<'a>(
    session: &'a Session,
    reference: Option<&str>,
) -> Result<(Vec<(&'a Track, crate::info::Step)>, usize)> {
    let ships_specs = |track: &Track, step: &crate::info::Step| {
        !step.spec.is_empty()
            && step
                .spec
                .iter()
                .all(|relative| track.dir(session.course().root()).join(relative).is_file())
    };
    match reference {
        Some(reference) => {
            let step = session.course().step(reference)?.clone();
            let track = session.course().track(&step.track)?;
            Ok((vec![(track, step)], 0))
        }
        None => {
            let mut selected = Vec::new();
            let mut skipped = 0usize;
            for track in session.course().tracks() {
                for step in &track.step {
                    if ships_specs(track, step) {
                        selected.push((track, step.clone()));
                    } else {
                        skipped += 1;
                    }
                }
            }
            Ok((selected, skipped))
        }
    }
}

fn relative(session: &Session, path: &std::path::Path) -> String {
    path.strip_prefix(session.course().root())
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
