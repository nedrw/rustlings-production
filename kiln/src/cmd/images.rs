//! `kiln images [pull]` — the images the authored steps declare (plan §9.3).
//!
//! Container steps are the one part of the course that cannot start from nothing: an image has to
//! exist before a spec can run it. This is the command that answers "what does this course need, and
//! what is already here", and `pull` is the one that fetches what is missing — one command for all
//! of it, so a learner does not collect `podman pull` lines from four different steps.
//!
//! Exit codes follow the rest of the runner: `0` everything is here, `1` something is missing (the
//! detail says which command fetches it), `2` nothing could be judged because no engine answered.

use crate::cmd::Session;
use crate::error::{EXIT_ERROR, EXIT_FAILED, EXIT_OK, Result};
use crate::info::Step;
use crate::process;
use crate::runtime::{self, Engine, ImageReport};

/// What `kiln images` was asked to do.
pub fn run(session: &Session, pull: bool) -> Result<u8> {
    let declared = declared_images(session);
    if declared.is_empty() {
        println!("no step of the authored tracks declares an image yet");
        return Ok(EXIT_OK);
    }

    let Some(engine) = runtime::first_engine(session.timeout()) else {
        println!(
            "no container engine answered: install podman (or docker) and run `kiln doctor`.\n\n\
             The images these steps declare:"
        );
        for (_, image) in &declared {
            println!("  {image}");
        }
        return Ok(EXIT_ERROR);
    };

    println!(
        "images declared by the authored steps (`{}`):",
        engine.name()
    );
    report_mirror();

    // One listing answers for every image: the engine is asked once, not once per step.
    let wanted: Vec<String> = declared.iter().map(|(_, image)| image.clone()).collect();
    let reports = runtime::images(engine, &wanted, session.timeout())?;
    let mut missing = Vec::new();
    for ((step, _), report) in declared.iter().zip(reports) {
        println!(
            "  {} {}",
            if report.is_present() { "✓" } else { "✗" },
            report.line(engine)
        );
        println!("      needed by {}", step.qualified_id());
        if !report.is_present() {
            missing.push(report);
        }
    }

    if missing.is_empty() {
        println!("\nevery declared image is in the local image store");
        return Ok(EXIT_OK);
    }
    if !pull {
        println!(
            "\n{} missing — `kiln images pull` fetches {} in one go",
            plural(missing.len()),
            if missing.len() == 1 { "it" } else { "them" }
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

/// Every image the authored steps declare, with the step that declares it.
fn declared_images(session: &Session) -> Vec<(&Step, String)> {
    let mut images = Vec::new();
    for track in session.course().tracks() {
        for step in &track.step {
            for image in &step.images {
                images.push((step, image.clone()));
            }
        }
    }
    images
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

/// `<n> image`, `<n> images`.
fn plural(count: usize) -> String {
    if count == 1 {
        "1 image is".to_owned()
    } else {
        format!("{count} images are")
    }
}
