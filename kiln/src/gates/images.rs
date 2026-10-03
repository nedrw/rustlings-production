//! `images` — the images a step declares are in the local image store.
//!
//! A step that needs a container says so in its metadata (`images = ["postgres:17-alpine"]`,
//! plan §9.3). This gate is the half that keeps the fast path fast: before `kiln run` starts a
//! spec that pulls a container, the image is already there, or the gate says which command fetches
//! it. Nothing here reaches the network — the answer comes from the engine's own list.
//!
//! Three outcomes, and the difference matters to the person reading them:
//!
//! * *passed* — the engine has every declared image (possibly under a mirror's name);
//! * *failed* — an image is missing, and the detail is the command that fixes it;
//! * *blocked* — no engine answered, which is not the learner's code being wrong.

use crate::gates::GateSpec;
use crate::gates::registry::GateOutcome;
use crate::gates::runner::GateContext;
use crate::runtime::{self, Engine, ImageReport};

/// Decide whether the engine can satisfy a step's images.
pub fn run(ctx: &GateContext<'_>, _spec: &GateSpec) -> crate::error::Result<GateOutcome> {
    if ctx.step.images.is_empty() {
        return Ok(GateOutcome::passed("this step needs no images"));
    }

    let Some(engine) = runtime::first_engine(ctx.probe_timeout()) else {
        return Ok(GateOutcome::blocked(format!(
            "no container engine answered, and this step needs {} — install podman (or docker) \
             and run `kiln doctor`",
            plural(ctx.step.images.len())
        )));
    };

    let reports = runtime::images(engine, &ctx.step.images, ctx.probe_timeout())?;
    Ok(judge(engine, &reports))
}

/// Turn what the engine has into an outcome.
///
/// Kept apart from the probing so the decision — and the sentence it produces — can be checked
/// without a container runtime in the test environment.
#[must_use]
pub fn judge(engine: Engine, reports: &[ImageReport]) -> GateOutcome {
    let missing: Vec<&ImageReport> = reports.iter().filter(|it| !it.is_present()).collect();
    if missing.is_empty() {
        let present = reports
            .iter()
            .map(|image| image.line(engine))
            .collect::<Vec<String>>()
            .join("; ");
        return GateOutcome::passed(present);
    }

    let detail = missing
        .iter()
        .map(|image| {
            format!(
                "{} is not in the local image store — pull it with `{}`",
                image.declared,
                image.pull_command(engine)
            )
        })
        .collect::<Vec<String>>()
        .join("; ");
    GateOutcome::failed(detail)
}

/// `<n> image`, `<n> images`.
fn plural(count: usize) -> String {
    if count == 1 {
        "1 image".to_owned()
    } else {
        format!("{count} images")
    }
}

#[cfg(test)]
mod tests {
    use super::{ImageReport, judge, plural};
    use crate::gates::registry::Outcome;
    use crate::runtime::Engine;

    fn report(declared: &str, local: Option<&str>) -> ImageReport {
        ImageReport {
            declared: declared.to_owned(),
            resolved: declared.to_owned(),
            local: local.map(str::to_owned),
        }
    }

    #[test]
    fn a_present_image_passes_and_names_the_name_it_has() {
        let outcome = judge(
            Engine::Podman,
            &[report(
                "postgres:17-alpine",
                Some("docker.1ms.run/library/postgres:17-alpine"),
            )],
        );
        assert_eq!(outcome.outcome, Outcome::Passed);
        assert!(
            outcome.detail.contains("docker.1ms.run"),
            "{}",
            outcome.detail
        );
    }

    #[test]
    fn a_missing_image_fails_with_the_command_that_fetches_it() {
        let outcome = judge(Engine::Podman, &[report("postgres:17-alpine", None)]);
        assert_eq!(outcome.outcome, Outcome::Failed);
        assert!(
            outcome.detail.contains("podman pull postgres:17-alpine"),
            "{}",
            outcome.detail
        );
    }

    #[test]
    fn every_missing_image_is_reported_not_just_the_first() {
        let outcome = judge(
            Engine::Docker,
            &[
                report("postgres:17-alpine", None),
                report("redis:7-alpine", None),
            ],
        );
        assert!(
            outcome.detail.contains("postgres:17-alpine"),
            "{}",
            outcome.detail
        );
        assert!(
            outcome.detail.contains("redis:7-alpine"),
            "{}",
            outcome.detail
        );
        assert!(outcome.detail.contains("docker pull"), "{}", outcome.detail);
    }

    #[test]
    fn a_step_without_images_is_vacuously_green() {
        let outcome = judge(Engine::Podman, &[]);
        assert_eq!(outcome.outcome, Outcome::Passed);
        assert_eq!(plural(1), "1 image");
        assert_eq!(plural(2), "2 images");
    }
}
