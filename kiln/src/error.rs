//! Typed failures for the runner.
//!
//! Course steps are judged by a process exit code, so the runner needs a small, stable set of
//! codes — but a code alone is not a diagnosis. Every failure carries the file, command or
//! tool it came from, and a `source` chain where one exists.

use std::path::PathBuf;

/// The step (or the whole run) passed.
pub const EXIT_OK: u8 = 0;
/// The runner judged the step and it did not pass (spec tests or a must gate).
pub const EXIT_FAILED: u8 = 1;
/// The runner could not judge: bad metadata, unknown step, missing tool, timeout.
pub const EXIT_ERROR: u8 = 2;
/// The requested content is not authored yet — nothing was judged.
pub const EXIT_NOT_AUTHORED: u8 = 3;

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, KilnError>;

/// Everything that can stop the runner from answering "does this step pass?".
#[derive(Debug, thiserror::Error)]
pub enum KilnError {
    /// A course file could not be read.
    #[error("cannot read {path}")]
    Read {
        /// File that could not be read.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// A file could not be written (progress state, restored spec).
    #[error("cannot write {path}")]
    Write {
        /// File that could not be written.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// A course file is not valid TOML for its schema.
    #[error("invalid metadata in {path}")]
    Metadata {
        /// Metadata file that failed to parse.
        path: PathBuf,
        /// Parser error, including line and column.
        #[source]
        source: toml::de::Error,
    },

    /// Metadata the runner owns could not be encoded.
    #[error("cannot encode {path}")]
    Encode {
        /// File that could not be encoded.
        path: PathBuf,
        /// Encoder error.
        #[source]
        source: toml::ser::Error,
    },

    /// The metadata parsed but contradicts the plan or itself.
    #[error("{0}")]
    Invalid(String),

    /// No repository root (a directory containing `course/course.toml`) was found.
    #[error("no course manifest found in {start} or any parent directory")]
    NoCourse {
        /// Directory the search started in.
        start: PathBuf,
    },

    /// A step reference matched nothing.
    #[error("unknown step `{requested}`{suggestion}")]
    UnknownStep {
        /// What the user typed.
        requested: String,
        /// Closest known step, or an empty string.
        suggestion: String,
    },

    /// A track was requested that does not exist.
    #[error("unknown track `{0}`")]
    UnknownTrack(String),

    /// A step id was given to a command that only takes a track.
    #[error(
        "`{given}` is a step, not a track — `kiln list {track}` lists its track, `kiln hint {given}` \
         shows the step"
    )]
    StepWhereTrackIsExpected {
        /// What the user typed.
        given: String,
        /// Track the step belongs to, as the user would type it.
        track: String,
    },

    /// A command that needs a step reference was given nothing.
    #[error("no step id given — `kiln run` on its own judges the first unfinished step")]
    NoStepGiven,

    /// The learner project is not there yet, so there is nothing to prepare or judge.
    #[error(
        "{project} does not exist yet — `kiln run {first}` creates it from the track skeleton; the \
         steps build on each other, so they are done in order"
    )]
    MissingProject {
        /// Project directory, relative to the repository root (`app/mailer`).
        project: String,
        /// Step that creates it.
        first: String,
    },

    /// A step reference matched more than one step.
    #[error("ambiguous step `{requested}`: it matches {matches:?}")]
    AmbiguousStep {
        /// What the user typed.
        requested: String,
        /// Candidate step ids, in track order.
        matches: Vec<String>,
    },

    /// A step in `track.toml` does not follow the `<phase>.<index>-<slug>` id convention.
    #[error("step `{id}` in {track}.toml must look like `1.14-worker`: {reason}")]
    StepId {
        /// Offending local step id.
        id: String,
        /// Track that declares it.
        track: String,
        /// What is wrong with it.
        reason: String,
    },

    /// A scaffold or reference-solution patch does not fit the tree it was applied to.
    #[error("patch does not apply to {file}: {reason}")]
    Patch {
        /// File (relative to the learner project) the patch could not write.
        file: String,
        /// What went wrong, phrased so it can be acted on.
        reason: String,
    },

    /// The spec files in the learner project are not the ones the course shipped.
    #[error(
        "{file} is not the shipped spec, so any result would be meaningless — run \
         `kiln spec restore {step}`"
    )]
    SpecModified {
        /// Spec file that drifted (relative to the learner project).
        file: String,
        /// Step whose spec drifted, as the learner types it.
        step: String,
    },

    /// An external tool the course depends on is not installed.
    #[error("`{tool}` is not available: {hint}")]
    MissingTool {
        /// Tool name as the user would type it.
        tool: String,
        /// Copy-pasteable install command.
        hint: String,
    },

    /// The requested content has not been authored yet.
    #[error("{what} is not authored yet")]
    NotAuthored {
        /// What is missing, phrased for the learner.
        what: String,
    },

    /// A command exceeded the runner's timeout (implemented in Rust: there is no `timeout(1)`).
    #[error("`{command}` timed out after {seconds}s")]
    Timeout {
        /// Command line that was killed.
        command: String,
        /// Timeout that was applied.
        seconds: u64,
    },

    /// A command could not be started at all.
    #[error("cannot run `{command}` in {dir}")]
    Spawn {
        /// Command line that failed to start.
        command: String,
        /// Working directory it was started in.
        dir: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// Another kiln run held a file this command needed, and the wait ran out.
    ///
    /// The message names the file, the pid of the run that holds it and how long it has held it,
    /// because those are what a person needs to decide between waiting and deleting a file whose
    /// run is gone.
    #[error("{message}")]
    Busy {
        /// The line naming the holder and the file it holds.
        message: String,
    },
}

impl KilnError {
    /// Exit code a shell sees for this failure.
    ///
    /// `EXIT_NOT_AUTHORED` exists so scripts can tell "you still have work to do" apart from
    /// "M0 has not written this step yet".
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::NotAuthored { .. } | Self::MissingProject { .. } => EXIT_NOT_AUTHORED,
            _ => EXIT_ERROR,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_authored_is_distinguishable_from_a_real_failure() {
        let missing = KilnError::NotAuthored {
            what: "step mailer/1.1".into(),
        };
        assert_eq!(missing.exit_code(), EXIT_NOT_AUTHORED);
        assert_ne!(missing.exit_code(), EXIT_FAILED);
    }

    #[test]
    fn errors_quote_the_path_that_failed() {
        let err = KilnError::Read {
            path: PathBuf::from("course/course.toml"),
            source: std::io::Error::other("boom"),
        };
        assert_eq!(err.to_string(), "cannot read course/course.toml");
        // The source chain is what makes the message actionable.
        assert!(std::error::Error::source(&err).is_some());
    }
}
