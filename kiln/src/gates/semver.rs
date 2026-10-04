//! `semver@baseline` — the public API the course pinned still compiles.
//!
//! A library whose users are not in this repository cannot be checked by the repository's own specs:
//! deleting a public item, changing a signature or adding a variant to a public enum breaks somebody
//! else's build, and every one of those compiles perfectly here. So the course keeps a *witness* —
//! `course/tracks/<track>/api/<step-id>.rs`, written in a form the compiler can judge: a function
//! pointer coercion per signature, an exhaustive `match` over each public enum, and `..` on the
//! `#[non_exhaustive]` structs so field count is not pinned by accident (handoff M2 §1).
//!
//! A witness is written by the step that **establishes** the baseline and then holds for every later
//! step of the track, which is why the gate does not look for one file per step: it takes the newest
//! witness at or before the step it is judging ([`sources::newest_witness`]). A step that means to
//! break the API ships a new witness under its own number, and that one is picked up with no change
//! here — which is the documented breaking-release path.
//!
//! The gate copies that file into the project as `tests/semver_baseline.rs` and runs
//! `cargo test -p <package> --test semver_baseline`, over the package the step's own `run.cmd` names.
//! The verdict is the compiler's: a build failure is a breaking change, and the report quotes the
//! first error lines rather than describing them.
//!
//! **The gate writes inside the learner's tree and puts it back when it is done.**
//! `tests/semver_baseline.rs` is course-owned, so whatever was there is restored — and a file that was
//! not there is removed again — including when the gate fails or the runner errors out ([`Witness`]).
//! Leaving it behind would change what the learner's next `cargo test` compiles; leaving a *stale* one
//! behind would let the next run of this gate judge the wrong API.

use std::path::{Path, PathBuf};

use crate::error::{KilnError, Result};
use crate::gates::runner::{GateContext, clip};
use crate::gates::sources;
use crate::gates::{GateOutcome, GateSpec};
use crate::process;

/// Where the witness is copied to, relative to the project.
pub const WITNESS: &str = "tests/semver_baseline.rs";

/// The cargo test target the witness becomes, which is fixed: `--test` names a file stem.
const TARGET: &str = "semver_baseline";

/// Lines of a failing build the report keeps.
const DETAIL_LINES: usize = 12;

/// The parameter this gate takes: it names the kind of evidence, not a threshold.
const BASELINE: &str = "baseline";

/// Refuse a parameter this gate cannot mean, before anything is read or written.
fn check_param(spec: &GateSpec) -> Result<()> {
    if spec.param() == Some(BASELINE) {
        return Ok(());
    }
    Err(KilnError::Invalid(format!(
        "`{spec}`: this gate takes no threshold — the parameter is `{BASELINE}` (`semver@{BASELINE}`), \
         because the course's own witness file decides what `compatible` means"
    )))
}

/// Build the step's public-API witness against the learner's tree.
pub fn run(ctx: &GateContext<'_>, spec: &GateSpec) -> Result<GateOutcome> {
    check_param(spec)?;
    let Some(project) = ctx.existing_project() else {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    };

    // The witness this step is judged against: the newest one at or before it, because establishing
    // the baseline is one step's job and holding it is every later step's (see the module docs).
    let track_dir = ctx.track.dir(ctx.course.root());
    let Some(witness) = sources::newest_witness(&track_dir, ctx.step.number())? else {
        return Ok(GateOutcome::blocked(format!(
            "{} declares `semver@{BASELINE}` but the track ships no witness at or before {} — the \
             course has to write one ({}), so there is nothing to build against yet",
            ctx.step.qualified_id(),
            ctx.step.number(),
            track_dir.join("api").join("<step-id>.rs").display()
        )));
    };
    let text = std::fs::read_to_string(&witness).map_err(|source| KilnError::Read {
        path: witness.clone(),
        source,
    })?;
    // How a report names it: `api/3.2-public-api-baseline.rs`, which is where a reader looks.
    let named = witness
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .map_or_else(
            || "api/<step-id>.rs".to_owned(),
            |name| format!("api/{name}"),
        );

    let Some(run) = ctx.step.run.as_ref() else {
        return Ok(GateOutcome::blocked(format!(
            "{} declares no `run.cmd`, so the package the witness has to build against is unknown",
            ctx.step.qualified_id()
        )));
    };
    let Some(package) = package(&run.cmd)? else {
        return Ok(GateOutcome::blocked(format!(
            "`{}` names no package (`-p <name>`), so `{WITNESS}` cannot be built as a test of the \
             learner's crate",
            run.cmd
        )));
    };

    let _witness = Witness::install(&project.join(WITNESS), &text)?;
    let ran = process::run(
        "cargo",
        &["test", "-p", &package, "--test", TARGET],
        project,
        ctx.timeout,
    )?;
    if ran.timed_out {
        return Ok(GateOutcome::blocked(format!(
            "`cargo test -p {package} --test {TARGET}` was still building after {:?}: the witness \
             cannot be judged on a machine this busy — re-run the step",
            ctx.timeout
        )));
    }
    if !ran.success() {
        return Ok(GateOutcome::failed(format!(
            "the course's public-API witness ({named}) no longer builds against this tree, which is \
             a breaking change to `{package}`'s public API:\n    {}",
            clip(&ran.stderr, DETAIL_LINES)
        )));
    }

    Ok(GateOutcome::passed(format!(
        "the course's public-API witness ({named}) still compiles against `{package}`'s public API"
    )))
}

/// The package a `run.cmd` judges, out of its `-p <name>` / `--package <name>`.
fn package(command: &str) -> Result<Option<String>> {
    let (_, args) = process::split_command(command)?;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        if argument == "-p" || argument == "--package" {
            return Ok(args.next().map(str::to_owned));
        }
        if let Some(name) = argument.strip_prefix("--package=") {
            return Ok(Some(name.to_owned()));
        }
    }
    Ok(None)
}

/// `tests/semver_baseline.rs` for as long as the gate runs.
///
/// The file is course-owned for the duration and the learner's tree has to come back exactly as it
/// was: this guard restores what it found, and removes the file when there was none.
#[derive(Debug)]
struct Witness {
    /// Where the witness was written.
    path: PathBuf,
    /// What was there before, if anything.
    previous: Option<String>,
}

impl Witness {
    /// Write the course's witness and remember what it replaced.
    fn install(path: &Path, text: &str) -> Result<Self> {
        let witness = Self {
            path: path.to_path_buf(),
            previous: std::fs::read_to_string(path).ok(),
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| KilnError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(path, text).map_err(|source| KilnError::Write {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(witness)
    }
}

impl Drop for Witness {
    fn drop(&mut self) {
        match &self.previous {
            Some(previous) => {
                let _ = std::fs::write(&self.path, previous);
            }
            None => {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gates::GateSpec;
    use crate::test_support::TempDir;

    #[test]
    fn the_package_comes_from_the_command_the_step_runs() {
        assert_eq!(
            package("cargo test -p jobq --test spec_api").unwrap(),
            Some("jobq".to_owned())
        );
        assert_eq!(
            package("cargo test --package jobq").unwrap(),
            Some("jobq".to_owned())
        );
        assert_eq!(
            package("cargo test --package=jobq --test spec_api").unwrap(),
            Some("jobq".to_owned())
        );
        assert_eq!(package("cargo test").unwrap(), None);
        assert_eq!(package("cargo test -p").unwrap(), None);
    }

    #[test]
    fn the_witness_is_put_back_the_way_it_was_found() {
        let scratch = TempDir::new("semver-witness");
        let path = scratch.path().join(WITNESS);

        {
            let _guard = Witness::install(&path, "course copy").expect("the file is writable");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "course copy");
        }
        // The learner did not have one, so the gate takes it away again.
        assert!(!path.exists(), "the witness may not outlive the gate");

        // And an edited copy of the learner's own is restored, not kept.
        scratch.write(WITNESS, "learner copy");
        {
            let _guard = Witness::install(&path, "course copy").expect("the file is writable");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "course copy");
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "learner copy");
    }

    #[test]
    fn only_the_baseline_word_is_a_usable_parameter() {
        let mut spec = "semver@baseline".parse::<GateSpec>().unwrap();
        assert!(check_param(&spec).is_ok());

        spec = "semver@80".parse::<GateSpec>().unwrap();
        let error = check_param(&spec).unwrap_err();
        assert!(error.to_string().contains("takes no threshold"), "{error}");
        assert!(error.to_string().contains("semver@baseline"), "{error}");

        let bare = "semver".parse::<GateSpec>().unwrap();
        assert!(check_param(&bare).is_err());
    }
}
