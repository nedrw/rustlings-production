//! Spec for `lakelet/4.16-profiling`: a profile is evidence, and the page that reads it is checkable.
//!
//! `kiln run` copies this file into the project as `tests/spec_profiling.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Thirteen cases, and the shape of them is the point:
//!
//! * **a counted run is reproducible.** No clock, no thread count and no wall time reaches the
//!   counters, so two `--cycles 8` runs print the same bytes — which is what lets a page carry a
//!   table at all (`jobq/3.22-bench-budget`'s rule, applied to the thing a page quotes);
//! * **the counters are a profile, not a print-out.** One line per phase, in the order a query walks
//!   them, every phase in [`Phase::ALL`] exactly once, then the workload's own shape;
//! * **the loop's conservation.** A cycle ingests a batch, seals it and asks about the span it just
//!   wrote, so the records a cycle ingests are the records its query reads and answers: `decode`,
//!   `filter` and `aggregate` are `records` exactly, `blocks` and `decompress` are `cycles` (one
//!   segment read each), and `headers` is one manifest entry per segment per cycle;
//! * **a timed run is not reproducible, and nothing depends on it.** `--seconds 1` runs at least one
//!   cycle and says which bound it was given; how many cycles it got is the machine's business, and
//!   no case here asserts it;
//! * **the refusals are classes**, twice each: a line this mode cannot read is the caller's (`2`), a
//!   path that is not a directory is a deployment that cannot work (`1`), and a store that will not
//!   open is the store's (`4`). Every one of them answers nothing on standard output;
//! * **the page cannot drift from the code.** The block the page carries is a command's output, line
//!   for line: the case finds the block, finds the `--cycles` the page states beside it, runs the
//!   binary with those flags and compares. A count edited by hand stops matching, which is the only
//!   way a page full of numbers stays true;
//! * **the mode is a mode of the loader.** It drives the store in this process — it opens no socket
//!   — and the ingest mode's own grammar, usage and failure class are still what they were.
//!
//! Nothing here sleeps beyond the one bounded `--seconds 1` run, and nothing here reads a clock.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use lakelet::engine::compaction::{self, Segments};
use lakelet::engine::trace::Phase;

/// The prefix every line of a profile carries: the loader's own name.
const PREFIX: &str = "lakelet-load";

/// The page this step delivers, relative to the project.
const PAGE: &str = "docs/profile.md";

/// The module the mode lives in, relative to the project.
const MODULE: &str = "src/bin/lakelet-load/profile.rs";

/// The loader itself, which declares the mode and answers with its codes.
const LOADER: &str = "src/bin/lakelet-load.rs";

/// How many cycles the counted cases run: enough that every phase has a number a defect could move,
/// and small enough that the whole file finishes in seconds.
const CYCLES: u64 = 8;

/// One run of the real binary: what a script would have seen.
#[derive(Debug)]
struct Ran {
    /// The exit code.
    code: i32,
    /// What standard output received.
    stdout: String,
    /// What standard error received.
    stderr: String,
}

/// Run `lakelet-load` with no environment and `/dev/null` on standard input.
fn lakelet_load(args: &[String]) -> Ran {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lakelet-load"));
    command
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("`lakelet-load {args:?}` could not be started: {error}"));
    Ran {
        code: output.status.code().unwrap_or_else(|| {
            panic!("`lakelet-load {args:?}` was killed by a signal: it must exit with a code")
        }),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// The flags a counted run is asked with.
fn counted(cycles: u64) -> Vec<String> {
    vec![
        "--profile".to_owned(),
        "--cycles".to_owned(),
        cycles.to_string(),
    ]
}

/// The same, writing into a store this case names.
fn counted_into(cycles: u64, store: &Dir) -> Vec<String> {
    let mut args = counted(cycles);
    args.push("--store".to_owned());
    args.push(store.argument());
    args
}

/// A store directory of this case's own, removed when it is dropped.
struct Dir {
    /// Where the store lives.
    path: PathBuf,
}

impl Dir {
    /// Claim a directory for `label`.
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lakelet-profiling-spec-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("{} cannot be created: {error}", path.display()));
        Self { path }
    }

    /// Where the store lives.
    fn path(&self) -> &Path {
        &self.path
    }

    /// The path a command line names, as the tool will read it.
    fn argument(&self) -> String {
        self.path
            .to_str()
            .unwrap_or_else(|| panic!("{} is not UTF-8", self.path.display()))
            .to_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// The profile a run printed, as the names and numbers on its lines.
///
/// Every line is `<prefix> <name>=<count>`, which is the shape the page's block is written in: a
/// line that is not that shape is a profile a script cannot read, and the panic says so rather than
/// leaving a case to compare two strings nobody can act on.
fn parsed(stdout: &str) -> Vec<(String, u64)> {
    stdout
        .lines()
        .map(|line| {
            let rest = line.strip_prefix(&format!("{PREFIX} ")).unwrap_or_else(|| {
                panic!(
                    "`{line}` is not a profile line: every line of one starts \
                                           with `{PREFIX} `"
                )
            });
            let (name, value) = rest.split_once('=').unwrap_or_else(|| {
                panic!("`{line}` has no `=`: a profile line is `<name>=<count>`")
            });
            let count = value.parse::<u64>().unwrap_or_else(|_| {
                panic!(
                    "`{line}` does not carry a count: every phase and every shape line is a \
                        whole number"
                )
            });
            (name.to_owned(), count)
        })
        .collect()
}

/// One named number of a profile.
fn count(profile: &[(String, u64)], name: &str) -> u64 {
    profile.iter().find(|(key, _)| key == name).map_or_else(
        || panic!("the profile does not state `{name}`: {profile:?}"),
        |(_, value)| *value,
    )
}

/// One source file of the project, as text.
fn source(relative: &str) -> String {
    std::fs::read_to_string(relative).unwrap_or_else(|error| {
        panic!("{relative} cannot be read: {error} — this case is about it")
    })
}

/// The page this case reads, from the project it is part of.
fn page() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(PAGE);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("{PAGE} is the page this step delivers, and it cannot be read: {error}")
    })
}

/// The block a page quotes and the `--cycles <n>` it says produced it.
///
/// The block is the fenced one whose first line is a profile's own first line — the lines are what
/// identify it, not a heading — and the count is the last `--cycles <n>` stated *before* it, which
/// is the sentence that introduces it.
fn quoted(page: &str) -> (u64, Vec<&str>) {
    let lines: Vec<&str> = page.lines().collect();
    let first = lines
        .iter()
        .position(|line| line.starts_with(&format!("{PREFIX} headers=")))
        .unwrap_or_else(|| {
            panic!(
                "{PAGE} does not quote a profile: it must carry a fenced block whose lines are a \
                 `{PREFIX} --profile` run, and none of its lines starts with `{PREFIX} headers=`"
            )
        });
    let opening = lines[..first]
        .iter()
        .rposition(|line| line.trim_start().starts_with("```"))
        .unwrap_or_else(|| {
            panic!(
                "{PAGE} has a profile's line at {first} outside a fenced block: a reader has to be \
                 able to copy the block out of the page"
            )
        });
    let closing = first
        + lines[first..]
            .iter()
            .position(|line| line.trim_start().starts_with("```"))
            .unwrap_or_else(|| {
                panic!("{PAGE} opens a fenced block it never closes: the block is unabridged")
            });
    let cycles = lines[..opening]
        .iter()
        .rev()
        .find_map(|line| {
            let at = line.find("--profile --cycles ")?;
            let rest = &line[at + "--profile --cycles ".len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u64>().ok()
        })
        .unwrap_or_else(|| {
            panic!(
                "{PAGE} does not say which command produced the block it carries: state \
                 `lakelet-load --profile --cycles <n>` in the sentence above it"
            )
        });
    (cycles, lines[opening + 1..closing].to_vec())
}

#[test]
fn the_same_cycles_print_the_same_bytes() {
    let args = counted(CYCLES);
    let first = lakelet_load(&args);
    let second = lakelet_load(&args);
    assert_eq!(first.code, 0, "a counted run answers: {first:?}");
    assert_eq!(
        first.stderr, "",
        "and it has nothing to diagnose: a profile is the answer, and a warning beside it is a \
         profile a script cannot pipe anywhere"
    );
    assert_eq!(
        first.stdout, second.stdout,
        "two runs of `--cycles {CYCLES}` printed different bytes: no clock, no thread count and no \
         wall time may reach these counters — that is what makes the page's table a measurement \
         rather than a coincidence (`lakelet/4.15-budget-regression`)"
    );
}

#[test]
fn every_phase_is_one_line_in_the_order_a_query_walks_them() {
    let ran = lakelet_load(&counted(CYCLES));
    assert_eq!(ran.code, 0, "a counted run answers: {ran:?}");
    let lines: Vec<&str> = ran.stdout.lines().collect();
    assert_eq!(
        lines.len(),
        Phase::ALL.len() + 3,
        "a profile is one line per phase and then the workload's shape (`cycles`, `records`, \
         `bytes`), and this one has {} line(s): {}",
        lines.len(),
        ran.stdout
    );
    for (index, phase) in Phase::ALL.iter().enumerate() {
        assert!(
            lines[index].starts_with(&format!("{PREFIX} {}=", phase.name())),
            "line {} of a profile is `{}` and this one is `{}`: the phases are printed in the order \
             a query walks them, which is `Phase::ALL`'s order",
            index + 1,
            phase.name(),
            lines[index]
        );
    }
    for (at, name) in ["cycles", "records", "bytes"].iter().enumerate() {
        let line = lines[Phase::ALL.len() + at];
        assert!(
            line.starts_with(&format!("{PREFIX} {name}=")),
            "`{name}` is one of the three lines that say what the workload was, and line {} is \
             `{line}`",
            Phase::ALL.len() + at + 1
        );
    }
    assert!(
        !ran.stdout.contains("seconds="),
        "a counted run was given no clock bound, so its profile has no `seconds=`: {}",
        ran.stdout
    );
    let profile = parsed(&ran.stdout);
    for phase in Phase::ALL {
        let name = phase.name();
        assert_eq!(
            profile.iter().filter(|(key, _)| key == name).count(),
            1,
            "`{name}` is one line and this profile has it {} time(s): a phase that appeared twice \
             would be two counters with one name",
            profile.iter().filter(|(key, _)| key == name).count()
        );
    }
}

#[test]
fn the_phases_add_up_the_way_4_15_says_they_do() {
    let ran = lakelet_load(&counted(CYCLES));
    assert_eq!(ran.code, 0, "a counted run answers: {ran:?}");
    let profile = parsed(&ran.stdout);
    for (bigger, smaller) in [
        (Phase::Headers, Phase::Blocks),
        (Phase::Blocks, Phase::Decompress),
        (Phase::Decode, Phase::Filter),
        (Phase::Filter, Phase::Aggregate),
        (Phase::Aggregate, Phase::Render),
    ] {
        assert!(
            count(&profile, bigger.name()) >= count(&profile, smaller.name()),
            "`{}` is {} and `{}` is {}: every block that was read was decided about, every decoded \
             record is offered to the predicates, and a bucket holds at least one value — one of \
             those identities is broken, so the profile is seven tallies rather than a measurement",
            bigger.name(),
            count(&profile, bigger.name()),
            smaller.name(),
            count(&profile, smaller.name())
        );
    }
    for phase in Phase::ALL {
        assert!(
            count(&profile, phase.name()) > 0,
            "`{}` charged nothing in {CYCLES} cycles: every phase of a query that read a segment \
             and answered it is charged at least once",
            phase.name()
        );
    }
}

#[test]
fn the_records_a_cycle_ingests_are_the_records_a_query_answers() {
    let ran = lakelet_load(&counted(CYCLES));
    assert_eq!(ran.code, 0, "a counted run answers: {ran:?}");
    let profile = parsed(&ran.stdout);
    let cycles = count(&profile, "cycles");
    let records = count(&profile, "records");
    assert_eq!(
        cycles, CYCLES,
        "`--cycles {CYCLES}` is a run of {CYCLES} cycles, and this one got {cycles}"
    );
    assert!(
        records > 0,
        "a run of {CYCLES} cycles ingested no record at all: the workload is what the phases are \
         charged for"
    );
    // One segment per cycle, read by that cycle's own question: the loop is the conservation.
    assert_eq!(
        count(&profile, Phase::Blocks.name()),
        cycles,
        "a cycle seals one segment and asks about it, so a run reads one segment per cycle: \
         `blocks` is {} and `cycles` is {cycles}",
        count(&profile, Phase::Blocks.name())
    );
    assert_eq!(
        count(&profile, Phase::Decompress.name()),
        cycles,
        "every block that was read is inflated: `decompress` is {} and `cycles` is {cycles}",
        count(&profile, Phase::Decompress.name())
    );
    for phase in [Phase::Decode, Phase::Filter, Phase::Aggregate] {
        assert_eq!(
            count(&profile, phase.name()),
            records,
            "`{}` is {} and the cycles ingested {records} record(s): a cycle's question is about \
             the span it just wrote, so the records a cycle ingests are the records its query reads \
             and answers — if this is larger, the question reached a segment another cycle wrote",
            phase.name(),
            count(&profile, phase.name())
        );
    }
    assert_eq!(
        count(&profile, Phase::Headers.name()),
        cycles * (cycles + 1) / 2,
        "every cycle's plan examines every entry of the manifest, and the store grows by one \
         segment per cycle: `headers` is {} and the run's cycles sum to {}",
        count(&profile, Phase::Headers.name()),
        cycles * (cycles + 1) / 2
    );
    assert!(
        count(&profile, "bytes") > records,
        "{} byte(s) of frames carried {records} record(s): a frame carries its bytes and a header, \
         so the workload's own shape is larger than its record count",
        count(&profile, "bytes")
    );
}

#[test]
fn a_seconds_run_is_stopped_by_the_clock_and_says_which_bound() {
    let ran = lakelet_load(&[
        "--profile".to_owned(),
        "--seconds".to_owned(),
        "1".to_owned(),
    ]);
    assert_eq!(
        ran.code, 0,
        "a timed run answers, and it must install no signal handler that makes a sandbox wait: \
         {ran:?}"
    );
    let lines: Vec<&str> = ran.stdout.lines().collect();
    assert_eq!(
        lines.len(),
        Phase::ALL.len() + 4,
        "a timed run prints the same shape as a counted one plus `seconds=`, and this one has {} \
         line(s): {}",
        lines.len(),
        ran.stdout
    );
    let profile = parsed(&ran.stdout);
    assert_eq!(
        count(&profile, "seconds"),
        1,
        "`--seconds 1` says which bound it was given: the line is what tells a reader which run \
         they are looking at, and it is the bound rather than the time it took"
    );
    assert!(
        count(&profile, "cycles") >= 1,
        "a timed run does at least one cycle before it looks at the clock: a profile of nothing is \
         not a measurement"
    );
    assert!(
        count(&profile, "records") > 0,
        "and it ingested what its cycles were made of: {}",
        ran.stdout
    );
    for phase in Phase::ALL {
        assert!(
            count(&profile, phase.name()) > 0,
            "`{}` charged nothing in a run that got {} cycle(s), so the loop stopped before it did \
             any work",
            phase.name(),
            count(&profile, "cycles")
        );
    }
}

#[test]
fn a_line_this_mode_cannot_read_is_the_callers() {
    let cases: [&[&str]; 6] = [
        &["--profile"],
        &["--profile", "--cycles"],
        &["--profile", "--cycles", "many"],
        &["--profile", "--cycles", "0"],
        &["--profile", "--cycles", "2", "--seconds", "2"],
        &["--profile", "--cycles", "2", "--kiln-exit-code-probe"],
    ];
    for case in cases {
        let args: Vec<String> = case.iter().map(|value| (*value).to_owned()).collect();
        // Twice: a class that is decided differently on a second run is not a class.
        for _ in 0..2 {
            let ran = lakelet_load(&args);
            assert_eq!(
                ran.code, 2,
                "`{case:?}` is a line this mode cannot read, and the code for that is `2` — the \
                 caller's: {ran:?}"
            );
            assert_eq!(
                ran.stdout, "",
                "a refused run answers nothing on standard output: a script must never be handed \
                 half a profile beside a complaint — {ran:?}"
            );
            assert!(
                !ran.stderr.is_empty(),
                "and it says which flag it could not read: an exit code on its own is a number a \
                 reader has to go looking for — {ran:?}"
            );
        }
    }
}

#[test]
fn a_path_that_is_not_a_directory_is_a_deployment_that_cannot_work() {
    let dir = Dir::new("not-a-directory");
    let file = dir.path().join("a-file-where-a-store-would-be");
    std::fs::write(&file, b"not a store").unwrap_or_else(|error| {
        panic!("{} cannot be written: {error}", file.display());
    });
    let target = file
        .to_str()
        .unwrap_or_else(|| panic!("{} is not UTF-8", file.display()))
        .to_owned();
    for _ in 0..2 {
        let ran = lakelet_load(&[
            "--profile".to_owned(),
            "--cycles".to_owned(),
            "2".to_owned(),
            "--store".to_owned(),
            target.clone(),
        ]);
        assert_eq!(
            ran.code, 1,
            "a path that exists and is not a directory is nothing about the call and nothing the \
             store can do either, so it is a deployment that cannot work (`1`) — the class \
             `lakelet/4.10-downsampling` gave the read side of this project: {ran:?}"
        );
        assert_eq!(ran.stdout, "", "and it answers nothing: {ran:?}");
        assert!(!ran.stderr.is_empty(), "and it says why: {ran:?}");
    }
}

#[test]
fn a_store_that_cannot_be_opened_is_the_stores() {
    let dir = Dir::new("broken-store");
    let sidecar = dir.path().join(compaction::DIRECTORY);
    std::fs::create_dir_all(&sidecar).unwrap_or_else(|error| {
        panic!("{} cannot be created: {error}", sidecar.display());
    });
    let manifest = dir.path().join(compaction::MANIFEST);
    std::fs::write(&manifest, b"not a manifest this build wrote").unwrap_or_else(|error| {
        panic!("{} cannot be written: {error}", manifest.display());
    });
    for _ in 0..2 {
        let ran = lakelet_load(&counted_into(2, &dir));
        assert_eq!(
            ran.code, 4,
            "a directory whose manifest is not this build's is a store that refused to open, and \
             the store's class is `4` — the run must not answer with a profile of a store it could \
             not read: {ran:?}"
        );
        assert_eq!(ran.stdout, "", "and it answers nothing: {ran:?}");
        assert!(!ran.stderr.is_empty(), "and it says why: {ran:?}");
    }
}

#[test]
fn a_run_appends_to_the_store_it_was_given() {
    let dir = Dir::new("given-store");
    let first = lakelet_load(&counted_into(2, &dir));
    assert_eq!(
        first.code, 0,
        "a run against a store it may write: {first:?}"
    );
    let opened = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("the run's store cannot be opened: {error}"));
    assert_eq!(
        opened.manifest().segments().len(),
        2,
        "two cycles seal two segments, and the store the run was given is where they are: the \
         `--store` flag names a store, and the mode writes to it"
    );

    // A second run against the same store appends to it rather than writing over the spans it
    // states: `decode` is that run's own `records` again, which it could not be if the question it
    // asks met a segment from the first run.
    let second = lakelet_load(&counted_into(2, &dir));
    assert_eq!(
        second.code, 0,
        "a run against a store with segments in it: {second:?}"
    );
    let profile = parsed(&second.stdout);
    assert_eq!(
        count(&profile, Phase::Decode.name()),
        count(&profile, "records"),
        "the second run's question met a segment the first run wrote: a run resumes past the \
         timestamps the store already states, so its own records are the only ones its query reads \
         — {} decoded against {} ingested",
        count(&profile, Phase::Decode.name()),
        count(&profile, "records")
    );
    assert!(
        count(&profile, Phase::Headers.name())
            > count(&parsed(&first.stdout), Phase::Headers.name()),
        "and its plan examined both runs' segments: the manifest is the whole store, so a store \
         that grew is a store whose plans are longer — {} against {}",
        count(&profile, Phase::Headers.name()),
        count(&parsed(&first.stdout), Phase::Headers.name())
    );
    let opened = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("the store cannot be opened: {error}"));
    assert_eq!(
        opened.manifest().segments().len(),
        4,
        "and the second run's segments are in the same store, beside the first run's"
    );
}

#[test]
fn the_page_carries_the_profile_the_command_prints() {
    let page = page();
    let (cycles, block) = quoted(&page);
    let ran = lakelet_load(&counted(cycles));
    assert_eq!(
        ran.code, 0,
        "the command `{PAGE}` states is a run this binary answers: {ran:?}"
    );
    let printed: Vec<&str> = ran.stdout.lines().collect();
    assert_eq!(
        printed, block,
        "`{PAGE}`'s block is not what `{PREFIX} --profile --cycles {cycles}` prints: the page's \
         numbers are a measurement, and the way a page full of measurements stays true is that the \
         command which produced them is re-run and compared. Edit the page from a run, never from \
         an intention.\n  printed: {printed:?}\n  page:    {block:?}"
    );
}

#[test]
fn the_page_states_the_sampler_and_what_it_sampled() {
    let page = page();
    let low = page.to_ascii_lowercase();
    for placeholder in ["todo", "tbd", "fixme"] {
        assert!(
            !low.contains(placeholder),
            "{PAGE} still says `{placeholder}`: a page with a blank in it is a page nobody has \
             written down"
        );
    }
    assert!(
        page.contains("sample <pid>"),
        "{PAGE} does not carry the invocation of the sampler that works here: `sample <pid> \
         <seconds> -file out.txt` is the line `kiln doctor` prints and the line a reader copies"
    );
    assert!(
        page.contains("kiln doctor"),
        "{PAGE} does not say where the answer to \"which sampler does this machine have\" comes \
         from: it is measured, not assumed, and `kiln doctor` is what measures it"
    );
    assert!(
        page.contains("samply"),
        "{PAGE} does not mention `samply`, which is the portable answer and the one this machine \
         does not have: a page about a profile of this project names the samplers it is choosing \
         between"
    );
    assert!(
        page.lines()
            .any(|line| line.contains("flamegraph") && line.contains("dtrace")),
        "{PAGE} does not tie `cargo-flamegraph` to the `dtrace` it drives: on this machine it is \
         installed and cannot run, and a page that promised it would send a reader to a tool that \
         refuses them — the reason has to be on the page"
    );
    for phase in Phase::ALL {
        assert!(
            page.contains(phase.name()),
            "{PAGE} does not state `{}`, which is a phase of the profile it is about",
            phase.name()
        );
    }
    assert!(
        page.contains("--profile") && page.contains("--cycles"),
        "{PAGE} does not state the command that produces the block it carries: `--profile \
         --cycles <n>` is the run a reader reproduces"
    );
}

#[test]
fn the_loader_still_speaks_the_protocol_it_spoke() {
    let help = lakelet_load(&["--help".to_owned()]);
    assert_eq!(help.code, 0, "`--help` answers: {help:?}");
    for flag in ["--target", "--samples", "--batch", "--seed", "--series"] {
        assert!(
            help.stdout.contains(flag),
            "the ingest mode's usage no longer names `{flag}`: this step adds a mode to the loader \
             and takes nothing away from the one that was there (`lakelet/4.1-ingest-protocol`) — {}",
            help.stdout
        );
    }
    assert!(
        !help.stdout.contains("--profile"),
        "a bare `--help` printed the profile mode's usage: each mode answers for itself, and \
         `lakelet-load --profile --help` is the other one — {}",
        help.stdout
    );
    let version = lakelet_load(&["--version".to_owned()]);
    assert_eq!(version.code, 0, "`--version` answers: {version:?}");
    assert!(
        version.stdout.starts_with(PREFIX),
        "and it names this program: {}",
        version.stdout
    );

    // One program answers a failure with one class. A flag the *ingest* mode cannot read is the
    // caller's, exactly as it is in the mode this step added — a deployment that cannot read its
    // own command line would be a lie about what went wrong.
    let bad = lakelet_load(&["--kiln-exit-code-probe".to_owned()]);
    assert_eq!(
        bad.code, 2,
        "a flag the ingest mode cannot read exited {} instead of `2`: both modes of this program \
         leave one class behind, and `exit_code` in the mode below is where that rule lives — \
         {bad:?}",
        bad.code
    );
    assert!(
        bad.stderr.contains("--kiln-exit-code-probe"),
        "and names the argument it refused: {bad:?}"
    );
}

#[test]
fn the_profile_mode_drives_the_store_in_this_process() {
    // The boundary `jobq/3.1-engine-boundary` established and `lakelet/4.9-retention` reads out of
    // the sources: a mode that opens a socket is a mode whose profile is of a network client, and
    // the point of this one is that the store's work happens in the process being sampled.
    let mode = source(MODULE);
    assert!(
        mode.contains("Segments"),
        "{MODULE} does not name the store: the mode drives it in this process rather than over the \
         protocol, and that is what makes a profile of it a profile of this store"
    );
    for forbidden in ["TcpStream", "tokio::net"] {
        assert!(
            !mode.contains(forbidden),
            "{MODULE} names `{forbidden}`, so the mode has become a second client: the ingest mode \
             already owns the socket, and a profile of the loader has to be a profile of the work \
             rather than of a connection"
        );
    }
    let loader = source(LOADER);
    assert!(
        loader.contains("mod profile"),
        "{LOADER} does not declare the mode: the entry point is a mode of `lakelet-load`, and a \
         second binary would be a second program with its own contract"
    );
    assert!(
        loader.contains("--target"),
        "{LOADER} no longer knows the daemon it writes to: this step adds a mode and changes no \
         word of the one that was there"
    );
}
