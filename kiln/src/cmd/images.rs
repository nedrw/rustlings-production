//! `kiln images [pull]` — the images the authored steps declare (plan §9.3).
//!
//! Container steps are the one part of the course that cannot start from nothing: an image has to
//! exist before a spec can run it. This is the command that answers "what does this course need, and
//! what is already here", and `pull` is the one that fetches what is missing — one command for all
//! of it, so a learner does not collect `podman pull` lines from four different steps.
//!
//! The report is grouped by **image**, not by step: `postgres:17-alpine` is declared by thirty steps
//! and printing it thirty times buries the answer to the only question here. It ends with the two
//! lines that make the offline case decidable: how many declared images are present, and whether the
//! engine can actually *start* one (probed by running, per this module's doctrine — an engine that
//! answers `info` is not necessarily an engine that starts containers).
//!
//! Exit codes follow the rest of the runner: `0` everything is here, `1` something is missing (the
//! detail says which command fetches it), `2` nothing could be judged because no engine answered.

use crate::cmd::Session;
use crate::error::{EXIT_ERROR, EXIT_FAILED, EXIT_OK, Result};
use crate::process;
use crate::runtime::{self, DeclaredImage, Engine, ImageReport};

/// What `kiln images` was asked to do.
pub fn run(session: &Session, pull: bool) -> Result<u8> {
    let declared = runtime::declared_images(session.course());
    if declared.is_empty() {
        println!("no step of the authored tracks declares an image yet");
        return Ok(EXIT_OK);
    }

    let Some(engine) = runtime::first_engine(session.timeout()) else {
        println!(
            "no container engine answered: install podman (or docker) and run `kiln doctor`.\n\n\
             The images these steps declare:"
        );
        for image in &declared {
            println!("  {}", image.reference);
        }
        return Ok(EXIT_ERROR);
    };

    println!(
        "images declared by the authored steps (`{}`), grouped by image:",
        engine.name()
    );
    report_mirror();

    // One listing answers for every image: the engine is asked once, not once per step.
    let wanted: Vec<String> = declared
        .iter()
        .map(|image| image.reference.clone())
        .collect();
    let reports = runtime::images(engine, &wanted, session.timeout())?;
    let mut missing = Vec::new();
    for (image, report) in declared.iter().zip(&reports) {
        println!(
            "  {} {}",
            if report.is_present() { "✓" } else { "✗" },
            report.line(engine)
        );
        println!("      needed by {}", needed(image));
        if !report.is_present() {
            missing.push(report.clone());
        }
    }

    let present = declared.len() - missing.len();
    println!(
        "\n{} declared image(s): {present} present{} — a step can start its container without the \
         network once its image is here",
        declared.len(),
        if missing.is_empty() {
            String::new()
        } else {
            format!(", {} missing", missing.len())
        }
    );
    report_smoke(engine, session);

    if missing.is_empty() {
        return Ok(EXIT_OK);
    }
    if !pull {
        println!(
            "\n{missing_count} — `kiln images pull` fetches them in one go",
            missing_count = plural(missing.len())
        );
        return Ok(EXIT_FAILED);
    }
    let failed = fetch(engine, &missing, session)?;
    if failed > 0 {
        println!("\n{failed} pull(s) failed — the detail above is the engine's own message");
        return Ok(EXIT_FAILED);
    }
    Ok(EXIT_OK)
}

/// Say whether the engine can start a container, once, after the listing.
fn report_smoke(engine: Engine, session: &Session) {
    let smoke = runtime::smoke(engine, session.timeout());
    let marker = if smoke.started { "✓" } else { "!" };
    println!("  {marker} {}", smoke.detail);
}

/// The steps that need one image, first three by name and a count for the rest.
fn needed(image: &DeclaredImage) -> String {
    const SHOWN: usize = 3;
    let shown = image
        .steps
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<String>>()
        .join(", ");
    let rest = image.steps.len().saturating_sub(SHOWN);
    if rest == 0 {
        format!("{shown} ({} step(s))", image.steps.len())
    } else {
        format!("{shown} and {rest} more ({} step(s))", image.steps.len())
    }
}

/// Pull every image that is not here yet, in the order the steps declare them.
///
/// Answers how many pulls failed: one registry being unreachable should not stop the others from
/// being fetched, and it should not look like success either.
fn fetch(engine: Engine, missing: &[ImageReport], session: &Session) -> Result<usize> {
    let mut failed = 0;
    for report in missing {
        let args = ["pull", report.resolved.as_str()];
        println!(
            "\n> {} (deadline {:?})",
            process::command_line(engine.binary(), &args),
            session.timeout()
        );
        // A pull streams progress; the deadline is what keeps a registry that never answers from
        // holding the command forever.
        let ran = process::run(
            engine.binary(),
            &args,
            &std::env::temp_dir(),
            session.timeout(),
        )?;
        if ran.success() {
            println!("✓ {}", report.declared);
        } else {
            failed += 1;
            println!("✗ {} — {}", report.declared, ran.summary());
        }
    }
    Ok(failed)
}

/// `<n> image is` / `<n> images are`.
fn plural(count: usize) -> String {
    if count == 1 {
        "1 image is".to_owned()
    } else {
        format!("{count} images are")
    }
}

/// Say which mirror pulls go through, because it decides the name a pull uses.
fn report_mirror() {
    match runtime::mirror() {
        Some(mirror) => println!("KILN_REGISTRY_MIRROR={mirror}: pulls are prefixed with it"),
        None => println!(
            "KILN_REGISTRY_MIRROR is not set: pulls go to the registry a step names, which is slow \
             or unreachable on some networks"
        ),
    }
}
