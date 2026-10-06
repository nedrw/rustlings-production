//! Spec for `lakelet/4.3-write-path-adr`: the fsync decision as a value, the documents that name it,
//! and the write path that obeys it.
//!
//! `kiln run` copies this file into the project as `tests/spec_write_path.rs`; it is the course's
//! file, not the learner's. Fourteen cases, and the shape of them is the point:
//!
//! * **a value and its documents cannot drift apart** — every policy this build can be configured
//!   with answers a unique `name()`, round-trips through `parse(describe())`, and appears in
//!   `docs/crash-points.md`, which is the page an operator reads at 3 a.m. A policy that exists in
//!   the enum and nowhere in the table is a decision nobody outside the code has; a policy in the
//!   table that the enum does not have is worse, because the table is believed;
//! * **the flags and the table are one fact** — `syncs_per_record()` is what a caller branches on
//!   and `losses()` is what the operator plans for, so the two have to say the same thing about
//!   every policy, including interval lengths this build does not ship;
//! * **the record says which value this build is** — `docs/adr/0001-write-path.md` has to name
//!   `Policy::DEFAULT.describe()`, in the spelling `describe()` writes, because that spelling is the
//!   only place the code and the sentence can be compared;
//! * **the spool obeys the policy observably** — `durable_offset() == len()` after an append under
//!   `per-record`, and `durable_offset() < len()` after an append under `never` until `sync()` is
//!   called. That is the claim the step adds over `lakelet/4.1-ingest-protocol`, which wrote bytes
//!   and promised nothing about where they were;
//! * **nothing here waits for time.** No case asserts *when* an interval policy flushes: that answer
//!   depends on the clock, and a case that can pass or fail on a stall is a flake this course does
//!   not ship (`docs/handoff-m2.md` §5). An interval policy is judged by the bound it states and by
//!   the policy it carries, both of which are deterministic.
//!
//! The two documents are read through this file's own helpers, so a missing page is a failure that
//! names the page rather than an assertion that quietly holds.

mod support;

use lakelet::Error;
use lakelet::engine::Spool;
use lakelet::engine::wal::{Losses, Policy};

/// Where the operator's crash-point list lives, below the project root `cargo test` runs in.
const CRASH_POINTS: &str = "docs/crash-points.md";

/// Where the decision record lives.
const ADR: &str = "docs/adr/0001-write-path.md";

/// A payload that is not round-numbered, so a length/payload mix-up cannot pass by accident.
fn payload(marker: u8) -> Vec<u8> {
    (0..37u8).map(|index| marker.wrapping_add(index)).collect()
}

/// Read one of the step's documents, or fail by naming the page.
///
/// A file that is not there is a failure and not a skip: both pages are deliverables of this step,
/// and a spec that treats "not written yet" as "nothing to check" is a spec that passes on the
/// scaffold.
fn document(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!(
            "{path} cannot be read: {error} — this page is part of the step: the policy names in \
             the crash-point list are what keeps the table and the code from drifting apart"
        )
    })
}

/// Every policy the table and the flags are judged on.
///
/// The policies this build lists, plus interval lengths no build ships: a bound that only holds for
/// the number in `ALL` is not a bound, it is a coincidence.
fn policies() -> Vec<Policy> {
    let mut all: Vec<Policy> = Policy::ALL.to_vec();
    all.push(Policy::Interval { millis: 1 });
    all.push(Policy::Interval { millis: 60_000 });
    all
}

/// The crash-point list split into points: each `###` heading and the text below it.
fn points(text: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        match line.strip_prefix("### ") {
            Some(heading) => found.push((heading.trim().to_owned(), String::new())),
            None => {
                if let Some((_, body)) = found.last_mut() {
                    body.push_str(line);
                    body.push('\n');
                }
            }
        }
    }
    found
}

#[test]
fn every_policy_has_a_name_of_its_own() {
    assert!(
        !Policy::ALL.is_empty(),
        "Policy::ALL is empty, so every case in this file that walks it is vacuous: the list is what \
         a document is allowed to name"
    );
    let mut names: Vec<&str> = Policy::ALL.iter().map(Policy::name).collect();
    let listed = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names.len(),
        listed,
        "two policies in Policy::ALL answer to the same name(): {:?} — a document that names a \
         policy has to name exactly one of them",
        Policy::ALL
            .iter()
            .map(Policy::describe)
            .collect::<Vec<String>>()
    );
    for policy in Policy::ALL {
        assert!(
            !policy.name().is_empty(),
            "`{}` has an empty name(), so nothing can point at it",
            policy.describe()
        );
    }
}

#[test]
fn every_policy_round_trips_through_its_describe() {
    for policy in policies() {
        let text = policy.describe();
        assert!(
            text.contains(policy.name()),
            "`{text}` does not carry the name() `{}` of the policy it describes: a document writes \
             the full form and a log line writes the kind, and a reader has to be able to see that \
             they are the same policy",
            policy.name()
        );
        let parsed = Policy::parse(&text).unwrap_or_else(|error| {
            panic!(
                "parse(`{text}`) refused the spelling describe() writes for {policy:?}: {error} — \
                 a policy that cannot be read back is a policy a configuration file cannot set"
            )
        });
        assert_eq!(
            parsed, policy,
            "`{text}` parsed back as {parsed:?}: the round trip through describe() has to land on \
             the policy it started from, number and all"
        );
    }
}

#[test]
fn a_name_that_is_not_a_policy_is_refused_by_listing_the_ones_that_work() {
    let not_policies = [
        "",
        "hourly",
        "interval",
        "interval(ms)",
        "interval(50)",
        "interval(-1ms)",
        "PER-RECORD",
        "per_record",
        "every record",
    ];
    for text in not_policies {
        let error = Policy::parse(text).expect_err(
            "a string that is not a policy this build knows has to be refused, not guessed at: a \
             store that silently falls back to a default on a typo is a store whose durability \
             cannot be read off its configuration",
        );
        assert!(
            matches!(error, Error::Invalid { .. }),
            "`{text}` was refused with {error:?} instead of the configuration error: a caller has to \
             be able to tell a value it got wrong from a store that failed (`err/typed-errors`) \
             using `kind()`, which is `{}` here",
            error.kind()
        );
        let said = error.to_string();
        for policy in Policy::ALL {
            assert!(
                said.contains(policy.name()),
                "the refusal of `{text}` does not name `{}`, which is a policy this build does \
                 know: {said}",
                policy.name()
            );
        }
    }
}

#[test]
fn the_build_ships_one_default_and_it_is_not_an_extreme() {
    assert!(
        Policy::ALL.contains(&Policy::DEFAULT),
        "Policy::DEFAULT is `{}`, which is not in Policy::ALL: the value this build uses has to be \
         one of the values it can be asked for, or the documents have two different ideas of what \
         this build does",
        Policy::DEFAULT.describe()
    );
    let Policy::Interval { millis } = Policy::DEFAULT else {
        panic!(
            "this build's default is `{}`: `per-record` waits for the device inside every append, \
             which is a store that cannot keep up with an ingest, and `never` is a store nobody can \
             defend — the default is the answer that pays a duty cycle and loses a bounded window",
            Policy::DEFAULT.describe()
        );
    };
    assert!(
        millis > 0,
        "this build's default interval is 0 ms, which is a `per-record` policy wearing an \
         interval's name: syncs_per_record() says an append does not wait, and the loss table says \
         a window bounded by nothing"
    );
}

#[test]
fn the_flags_and_the_losses_table_agree() {
    for policy in policies() {
        let syncs = policy.syncs_per_record();
        let loses_nothing = policy.losses() == Losses::Nothing;
        assert_eq!(
            syncs,
            loses_nothing,
            "`{}`: syncs_per_record() is {syncs} and losses() is {:?} — the flag a caller branches on \
             and the table an operator plans for are two views of one fact, so a policy that waits \
             for the device cannot lose records and a policy that loses them cannot be waiting",
            policy.describe(),
            policy.losses()
        );
    }
}

#[test]
fn the_losses_table_says_what_each_policy_gives_up() {
    assert_eq!(
        Policy::PerRecord.losses(),
        Losses::Nothing,
        "a record the spool waited for the device on is on the device: `per-record` loses nothing \
         to a machine that stops"
    );
    assert_eq!(
        Policy::Never.losses(),
        Losses::EverythingSinceTheLastFlush,
        "a store that never waits for the device loses whatever the kernel had not written yet — \
         not a duration, not a count, whatever the operating system decided it had time for"
    );
    for millis in [1u64, 50, 60_000] {
        assert_eq!(
            Policy::Interval { millis }.losses(),
            Losses::BoundedBy { millis },
            "an interval policy's loss is bounded by its own interval: a policy answering a \
             different number than it waits for is a table an operator cannot use"
        );
    }
    assert!(
        Policy::PerRecord.syncs_per_record(),
        "`per-record` is the policy whose append waits for the device"
    );
    assert!(
        !Policy::Never.syncs_per_record(),
        "`never` is the policy that does not wait at all"
    );
    assert!(
        !Policy::Interval { millis: 50 }.syncs_per_record(),
        "an interval policy elides the flush while the interval has not passed, so it is not a \
         per-record policy: if this is true, the interval's bound is a fiction"
    );
}

#[test]
fn the_crash_point_list_names_every_policy_and_still_has_its_points() {
    let text = document(CRASH_POINTS);
    for policy in Policy::ALL {
        assert!(
            text.contains(policy.name()),
            "{CRASH_POINTS} never names `{}`: the moment a policy exists that the table does not \
             mention, the table is a document about a build nobody is running",
            policy.name()
        );
    }
    let found = points(&text);
    assert!(
        found.len() >= 5,
        "{CRASH_POINTS} holds {} point(s) under a `###` heading: the list owes at least five — the \
         process, the machine, the torn write, the directory entry, and the sync that fails",
        found.len()
    );
}

#[test]
fn every_crash_point_says_which_policy_loses_what() {
    for (heading, body) in points(&document(CRASH_POINTS)) {
        let named: Vec<&str> = Policy::ALL
            .iter()
            .map(Policy::name)
            .filter(|name| body.contains(*name))
            .collect();
        assert!(
            !named.is_empty(),
            "the point `{heading}` in {CRASH_POINTS} names no `Policy::name()`: every point owes what \
             is on the device, what recovery will see, and which policy loses what — a point that \
             names none of them is a paragraph about crashes rather than a row of the table"
        );
    }
}

#[test]
fn the_decision_record_names_the_policy_this_build_uses() {
    let text = document(ADR);
    let default = Policy::DEFAULT.describe();
    assert!(
        text.contains(&default),
        "{ADR} does not name `{default}`, which is what `Policy::DEFAULT` is in the spelling \
         `describe()` writes: the record has to say what this build does, and it has to say it in \
         the one spelling the code and the sentence can be compared in"
    );
    for heading in ["## Decision", "## Alternatives", "## Consequences"] {
        assert!(
            text.contains(heading),
            "{ADR} has no `{heading}` section: a decision record that does not say what was \
             decided, what lost, and what it costs is a note about a preference"
        );
    }
}

#[test]
fn both_documents_point_at_the_code_they_are_about() {
    for path in [ADR, CRASH_POINTS] {
        assert!(
            document(path).contains("]("),
            "`{path}` writes no markdown target: the `links` gate can only resolve the targets a \
             page has, and a page about a policy that points at neither the enum nor the write \
             path leaves a reader to grep for it"
        );
    }
}

#[test]
fn a_per_record_spool_is_never_behind_after_an_append() {
    let store = support::StoreDir::new("write-path-per-record");
    let mut spool = Spool::with_policy(store.path(), Policy::PerRecord)
        .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
    assert_eq!(
        spool.policy(),
        Policy::PerRecord,
        "the spool answers the policy it was opened with, or nothing else in this file can be read \
         as a statement about a policy"
    );
    assert_eq!(
        spool.durable_offset(),
        spool.len(),
        "a spool that has written nothing is not behind: {} durable byte(s) and {} written",
        spool.durable_offset(),
        spool.len()
    );

    let payload = payload(0);
    spool
        .append(&payload)
        .expect("an append into a fresh store");
    let written = spool.len();
    assert!(written > 0, "an append writes bytes, and len() is how many");
    assert_eq!(
        spool.durable_offset(),
        written,
        "under `per-record` the append waited for the device before it answered, so every byte it \
         wrote is durable: durable_offset() is {} of len() {written} — a policy that claims to lose \
         nothing and a spool that is behind it are two answers to one question",
        spool.durable_offset()
    );
    assert_eq!(
        spool.records().expect("the record the spool just wrote"),
        vec![payload],
        "waiting for the device does not change what was written"
    );
}

#[test]
fn a_spool_that_does_not_sync_stays_behind_until_it_is_asked() {
    let store = support::StoreDir::new("write-path-never");
    let mut spool = Spool::with_policy(store.path(), Policy::Never)
        .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
    assert_eq!(
        spool.policy(),
        Policy::Never,
        "the spool answers the policy it was opened with"
    );

    let payload = payload(1);
    spool
        .append(&payload)
        .expect("an append into a fresh store");
    let written = spool.len();
    assert!(
        spool.durable_offset() < written,
        "under `never` the append answers before the device has the bytes, so durable_offset() has \
         to be behind len(): {} of {written} — a spool that is already durable here is a policy that \
         syncs per record under another name, and the loss table it publishes is a fiction",
        spool.durable_offset()
    );

    spool
        .sync()
        .expect("a store directory that can be written to can be flushed");
    assert_eq!(
        spool.durable_offset(),
        written,
        "sync() is the call that puts the bytes on the device: durable_offset() is {} and len() is \
         {written}",
        spool.durable_offset()
    );
    assert_eq!(
        spool
            .records()
            .expect("the record the spool just wrote")
            .len(),
        1,
        "the bytes were written before they were durable, and flushing them does not duplicate them"
    );
}

#[test]
fn an_interval_spool_carries_the_policy_it_was_given() {
    let store = support::StoreDir::new("write-path-interval");
    let policy = Policy::Interval { millis: 50 };
    let mut spool = Spool::with_policy(store.path(), policy)
        .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
    assert_eq!(
        spool.policy(),
        policy,
        "the spool answers the interval it was opened with, number and all"
    );
    spool
        .append(&payload(2))
        .expect("an append into a fresh store");
    // Nothing here asserts *when* an interval policy flushes: that answer depends on the clock, and a
    // case that can pass or fail on a stall is a flake. What an interval owes is its bound, and that
    // is `the_losses_table_says_what_each_policy_gives_up` and the crash-point list.
    assert!(
        spool.durable_offset() <= spool.len(),
        "durable bytes are a prefix of written bytes: {} durable of {} written",
        spool.durable_offset(),
        spool.len()
    );
}

#[test]
fn a_spool_opened_without_a_policy_is_what_the_build_ships() {
    let store = support::StoreDir::new("write-path-default");
    let spool = Spool::open(store.path())
        .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
    assert_eq!(
        spool.policy(),
        Policy::DEFAULT,
        "`Spool::open` is the path the daemon takes, so it is the policy a deployment actually \
         gets: it has to be `Policy::DEFAULT`, which is `{}`, the value the decision record names",
        Policy::DEFAULT.describe()
    );
}
