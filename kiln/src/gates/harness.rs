//! What the counter harnesses of the slow gates share.
//!
//! Four gates judge a *harness* rather than a file the learner wrote: `bench-budget` compares the
//! counts `tests/budget.rs` prints, `soak` reads the live-resource counters `tests/soak.rs` prints,
//! `fuzz` reads how much of the walk the fuzzing harness did, and `sim` compares the transcripts a
//! simulation prints per seed. They differ in what they do with the numbers; the parts below are the
//! same in all four, and were written out four times before this module existed:
//!
//! * **the prefix** — `<track>-<kind>`, as in `jobq-budget` or `lakelet-soak`. A harness says which
//!   track it speaks for, and a gate cannot read another track's lines (the first version of
//!   `bench-budget` had `jobq-budget` compiled in, which would have made every T4 harness print a T3
//!   track's name);
//! * **the line format** — `<prefix> <name>=<value>`, one metric per line, parsed strictly: a line
//!   that starts with the prefix and is not a metric is a broken harness the learner has to see, not
//!   a line to skip quietly;
//! * **the parameter file** — `course/tracks/<track>/<kind>/<step-id>.toml`, at the trust level of
//!   `spec/`: readable, useless to edit, because the gate reads the course's copy (handoff M2 §1);
//! * **why a re-run failed** — a run of the step's own command can fail four ways and a report has
//!   to name which.
//!
//! Nothing here looks at a clock. Every number a gate compares comes from the code under test, which
//! is why the same verdict arrives on an idle machine and on a loaded one (handoff M2 §1.1).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::de::DeserializeOwned;

use crate::error::{KilnError, Result};
use crate::gates::runner::GateContext;
use crate::process::Ran;

/// The prefix a harness of this track prints: `<track id>-<kind>`.
///
/// The track id is the one `track.toml` declares (`jobq`, `lakelet`), not the directory name: the two
/// agree today, and the metadata is the source of truth if they ever stop agreeing (plan §4).
#[must_use]
pub fn prefix(ctx: &GateContext<'_>, kind: &str) -> String {
    format!("{}-{kind}", ctx.track.id)
}

/// One measurement out of a harness run.
#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    /// Metric name, as the harness spells it.
    pub name: String,
    /// Value the harness reported.
    pub value: f64,
}

/// The measurements in a harness's stdout, or the reason none could be read.
///
/// A malformed line is reported rather than skipped: a harness that prints `lakelet-soak scan=abc` is
/// broken in a way the learner has to see, and pretending it printed nothing would hide it.
pub fn measurements(stdout: &str, prefix: &str) -> std::result::Result<Vec<Measured>, String> {
    let mut found: Vec<Measured> = Vec::new();
    for line in stdout.lines() {
        let Some(rest) = line.trim().strip_prefix(prefix) else {
            continue;
        };
        let Some((name, value)) = rest.trim().split_once('=') else {
            return Err(format!(
                "`{}` is not a `{prefix} <name>=<value>` line — the harness prints one metric per \
                 line, and this gate cannot guess what it meant",
                line.trim()
            ));
        };
        let value = value.trim();
        let Ok(parsed) = value.parse::<f64>() else {
            return Err(format!(
                "the harness reported `{name}=`, and `{value}` is not a number — every `{prefix}` \
                 line carries a count, not prose"
            ));
        };
        // A repeated name is the harness's last word on the metric, which is what a reader of its
        // output would take it to be.
        found.retain(|measurement| measurement.name != name);
        found.push(Measured {
            name: name.trim().to_owned(),
            value: parsed,
        });
    }
    Ok(found)
}

/// One measurement by name, for the gates that judge a single named count.
#[must_use]
pub fn value(measured: &[Measured], name: &str) -> Option<f64> {
    measured
        .iter()
        .find(|measurement| measurement.name == name)
        .map(|measurement| measurement.value)
}

/// A number as a report writes it: `12`, not `12.0`.
#[must_use]
pub fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

/// The course-owned parameter file of one step, read and parsed.
///
/// `Ok(None)` means the course has not written it yet — a hole in the content, not a mistake by the
/// learner — and the caller answers `blocked` naming the path [`parameter_path`] returns, so whoever
/// writes the step next knows exactly which file is missing.
pub fn parameters<T: DeserializeOwned>(
    ctx: &GateContext<'_>,
    kind: &str,
) -> Result<Option<(PathBuf, T)>> {
    let path = parameter_path(ctx, kind);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(KilnError::Read { path, source });
        }
    };
    let parsed = toml::from_str(&text).map_err(|source| KilnError::Metadata {
        path: path.clone(),
        source,
    })?;
    Ok(Some((path, parsed)))
}

/// Where the parameter file of one step lives, whether or not it exists yet.
#[must_use]
pub fn parameter_path(ctx: &GateContext<'_>, kind: &str) -> PathBuf {
    crate::gates::sources::course_file(&ctx.track.dir(ctx.course.root()), ctx.step, kind, "toml")
}

/// A metric table as the parameter files write it: name to bound.
pub type Bounds = BTreeMap<String, f64>;

/// Why a run of the step's own command did not pass, in one line.
///
/// A failing `cargo test` says two things: cargo prints `error: test failed …` on stderr, and the test
/// harness prints the panic that caused it on stdout, a few lines further in, under a `---- <test>
/// stdout ----` heading. The panic is the part worth quoting, so it is looked for first; a run that
/// failed without one — a compile error, a process killed by a signal — falls back to the first line
/// that is not cargo's own narration about its progress (`runner::failure_detail` takes the first line
/// of stderr, which under contention is `Blocking waiting for file lock …`).
#[must_use]
pub fn failure_reason(ran: &Ran) -> String {
    if ran.timed_out {
        return format!("timed out after {:?}", ran.elapsed);
    }
    let output = format!("{}{}", ran.stdout, ran.stderr);
    let lines = || output.lines().map(str::trim);
    let said = lines()
        .find(|line| line.contains(PANIC))
        .or_else(|| lines().find(|line| !line.is_empty() && !crate::process::is_progress(line)));
    match said {
        Some(said) => format!("{}: {said}", crate::process::exit_code_text(ran.code)),
        None => crate::process::exit_code_text(ran.code).to_owned(),
    }
}

/// What a Rust program prints when it panics, which is how a failing spec announces itself.
const PANIC: &str = "panicked at";

/// Seconds a walk took, as a report writes it: `12s`.
///
/// Evidence, never a judgement: every gate that prints this judges counters (handoff M2 §1.1).
#[must_use]
pub fn seconds(elapsed: std::time::Duration) -> String {
    format!("{}s", elapsed.as_secs())
}

/// The text a harness printed for one line prefix, in order.
///
/// `sim` compares these between two runs of the same seed, so the text is kept exactly as it was
/// written — trailing whitespace included, because a run that replays differently in whitespace is a
/// run that replays differently.
#[must_use]
pub fn transcript(stdout: &str, prefix: &str) -> String {
    let mut lines: Vec<&str> = stdout
        .lines()
        .filter(|line| line.trim_start().starts_with(prefix))
        .collect();
    // A trailing newline is the shell's business, not the harness's: two runs that differ only there
    // did the same thing.
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Eight hex characters that stand for a transcript in a report.
#[must_use]
pub fn digest(text: &str) -> String {
    let full = crate::hash::bytes(text.as_bytes());
    let hex = full.strip_prefix(crate::hash::PREFIX).unwrap_or(&full);
    hex.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_metric_line_parses_and_anything_else_is_reported() {
        let measured = measurements(
            "running 1 test\nlakelet-soak cycles=4000\nlakelet-soak open_tasks=12\ntest result: ok\n",
            "lakelet-soak",
        )
        .expect("the fixture harness output parses");
        assert_eq!(measured.len(), 2);
        assert_eq!(value(&measured, "cycles"), Some(4000.0));
        assert_eq!(value(&measured, "open_tasks"), Some(12.0));
        assert_eq!(value(&measured, "absent"), None);

        // Another track's lines are not this track's: the prefix is the whole filter.
        let foreign = measurements("jobq-budget scan_cells=256\n", "lakelet-soak")
            .expect("parsing does not fail on lines it ignores");
        assert!(foreign.is_empty());
    }

    #[test]
    fn a_malformed_metric_line_is_an_error_and_not_a_skip() {
        let broken = measurements("lakelet-soak cycles\n", "lakelet-soak").unwrap_err();
        assert!(broken.contains("<name>=<value>"), "{broken}");
        let prose = measurements("lakelet-soak cycles=soon\n", "lakelet-soak").unwrap_err();
        assert!(prose.contains("not a number"), "{prose}");
    }

    #[test]
    fn a_repeated_name_is_the_harnesss_last_word() {
        let measured = measurements(
            "lakelet-soak open_tasks=20\nlakelet-soak open_tasks=3\n",
            "lakelet-soak",
        )
        .expect("parses");
        assert_eq!(measured.len(), 1);
        assert_eq!(value(&measured, "open_tasks"), Some(3.0));
    }

    #[test]
    fn numbers_are_written_the_way_a_report_reads_them() {
        assert_eq!(number(12.0), "12");
        assert_eq!(number(2.5), "2.5");
    }

    #[test]
    fn a_transcript_keeps_the_lines_the_harness_printed() {
        let stdout = "running 1 test\nlakelet-sim claim series=7 attempt=1\n\n\
                      lakelet-sim complete series=7\ntest result: ok\n\n";
        let text = transcript(stdout, "lakelet-sim");
        assert_eq!(
            text,
            "lakelet-sim claim series=7 attempt=1\nlakelet-sim complete series=7"
        );
        assert_ne!(
            digest(&text),
            digest(""),
            "a transcript has a digest of its own"
        );
        assert_eq!(digest(&text).len(), 8);
        assert_eq!(transcript("nothing here\n", "lakelet-sim"), "");
    }

    #[test]
    fn a_command_that_never_finished_says_so_and_not_its_exit_code() {
        let stuck = Ran {
            code: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
            elapsed: std::time::Duration::from_secs(25),
        };
        assert_eq!(failure_reason(&stuck), "timed out after 25s");
    }
}
