//! Spec for `jobq/3.20-mutation-threshold`: what a mutation run proves, and what it does not.
//!
//! `kiln run` copies this file into the project as `tests/spec_mutation.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Nine cases, and they are about three different things:
//!
//! * **the arithmetic** — a score is a share of the mutants that were *judged*, and an empty run has
//!   no share at all rather than a share of zero. A `0%` is a number a threshold compares with, and
//!   a build that looked at nothing would be reported as a build that failed;
//! * **the rule** — a threshold and a floor, three answers rather than two, and a met threshold is
//!   *met*: `>=`, not `>`. A policy nobody can satisfy (a threshold above 100, a floor of zero) is a
//!   decision no review passed;
//! * **the run** — `caught` walks the invariants of `jobq/3.19-property-invariants` over a table of
//!   planted defects, and the case that matters is the last one: a suite whose invariants always say
//!   yes kills nothing, which is the whole argument for mutation testing in one line.
//!
//! What this file deliberately does not do is mutate this course's own source: that is the `mutants`
//! gate's job (`kiln check jobq/3.20-mutation-threshold --slow`), it mutates the module this step
//! adds, and the score it reports is the score of *these* cases. A spec that mutated its own code
//! would be a compiler inside a test, and the bookkeeping is why the tool exists.

use jobq::engine::{
    Event, Illegal, Invariant, JobState, Model, MutationPolicy, Report, Score, Step, Survivor,
    Verdict, caught, transition, walk,
};

/// The property this step scores a suite with: a job that finished never moves.
///
/// The same property `jobq/3.19-property-invariants` walks with, and the reason the two steps are
/// neighbours: a mutation run is that walk pointed at rules somebody planted.
struct TerminalIsFinal;

impl Invariant for TerminalIsFinal {
    fn holds(&self, _history: &[Event], step: &Step) -> bool {
        !(step.before().is_terminal() && step.after() != step.before())
    }
}

/// A suite that cannot fail: every step is allowed.
///
/// The one invariant a project is guaranteed to have, and the reason a mutation score is worth more
/// than a green suite: `true` is the cheapest property to write and it catches nothing.
struct Holds;

impl Invariant for Holds {
    fn holds(&self, _history: &[Event], _step: &Step) -> bool {
        true
    }
}

/// A model with one rule changed: a job that ran out of attempts may be claimed again.
struct ReopensFailed;

impl Model for ReopensFailed {
    fn step(&self, state: JobState, event: Event) -> Result<JobState, Illegal> {
        if state == JobState::Failed && event == Event::Claim {
            Ok(JobState::Running)
        } else {
            transition(state, event)
        }
    }
}

/// A model with one rule changed: a finished job may be claimed again.
struct ReopensSucceeded;

impl Model for ReopensSucceeded {
    fn step(&self, state: JobState, event: Event) -> Result<JobState, Illegal> {
        if state == JobState::Succeeded && event == Event::Claim {
            Ok(JobState::Running)
        } else {
            transition(state, event)
        }
    }
}

/// A model with one rule changed: a completed job is recorded as failed.
///
/// A defect `TerminalIsFinal` cannot see, and that is the point of the case below: the property is
/// about *leaving* the end of a job's life, not about which end. A suite with one property has one
/// blind spot, and a mutation run is how a project finds out where it is.
struct CallsItFailed;

impl Model for CallsItFailed {
    fn step(&self, state: JobState, event: Event) -> Result<JobState, Illegal> {
        if state == JobState::Running && event == Event::Complete {
            Ok(JobState::Failed)
        } else {
            transition(state, event)
        }
    }
}

/// A model with one rule changed: the attempt budget is ignored and every failure is retried.
struct RetriesForever;

impl Model for RetriesForever {
    fn step(&self, state: JobState, event: Event) -> Result<JobState, Illegal> {
        match (state, event) {
            (JobState::Running, Event::Fail { .. }) => Ok(JobState::Queued),
            _ => transition(state, event),
        }
    }
}

/// The table of planted defects the two running cases use.
fn planted() -> [&'static dyn Model; 4] {
    [
        &ReopensFailed,
        &CallsItFailed,
        &ReopensSucceeded,
        &RetriesForever,
    ]
}

/// The same count, taken by walking each mutant by hand.
///
/// Written out rather than taken on faith, because a score is only worth what the walks behind it are
/// worth: this is the loop `caught` is supposed to be.
fn killed_by_hand(seed: u64, length: usize, mutants: &[&dyn Model]) -> usize {
    mutants
        .iter()
        .filter(|model| walk(**model, seed, length, &TerminalIsFinal).is_some())
        .count()
}

#[test]
fn the_score_counts_what_the_suite_noticed() {
    assert_eq!(Score::new(8, 2).killed(), 8);
    assert_eq!(Score::new(8, 2).survived(), 2);
    assert_eq!(
        Score::new(8, 2).judged(),
        10,
        "the denominator is what was judged, and a mutant that was not judged is in neither count"
    );

    // The share, case by case. The empty run is the one that is not a number, and the truncation is
    // deliberate: 5 of 11 is 45%, because a threshold of 80 is a decision about whole percent and
    // rounding up would let a run one mutant short of it read as a pass.
    for (killed, survived, percent) in [
        (0, 0, None),
        (1, 0, Some(100)),
        (5, 0, Some(100)),
        (8, 2, Some(80)),
        (4, 1, Some(80)),
        (7, 3, Some(70)),
        (1, 1, Some(50)),
        (5, 6, Some(45)),
        (0, 5, Some(0)),
    ] {
        assert_eq!(
            Score::new(killed, survived).percent(),
            percent,
            "{killed} killed of {}",
            killed + survived
        );
    }
}

#[test]
fn an_empty_run_has_no_percentage_and_is_not_a_pass() {
    let empty = Score::new(0, 0);
    assert_eq!(
        empty.percent(),
        None,
        "a share of nothing is not `0%`: a zero here would compare with a threshold like any other
         number, and a build that looked at nothing would read as the worst possible score rather
         than as the absence of one"
    );

    // And the arithmetic keeps a floor of one judged mutant whatever the policy was configured
    // with: a run that looked at nothing has proven nothing, and that is not a setting.
    assert_eq!(
        MutationPolicy::new(80, 6).judge(empty),
        Verdict::Unproven,
        "not enough evidence"
    );
    assert_eq!(
        MutationPolicy::new(80, 0).judge(empty),
        Verdict::Unproven,
        "a policy with a floor of zero still does not pass an empty run: the floor is raised to one
         mutant by `judge`, and `is_satisfiable` is where the review finds out that the floor was a
         mistake"
    );
}

#[test]
fn a_policy_nobody_can_meet_is_not_a_policy() {
    // Two shapes of a rule nobody can satisfy: a threshold above everything, and a floor of nothing.
    // Both are decisions somebody has to have read, which is why the answer is a method and not a
    // paragraph in a style guide.
    for (threshold, min_judged, satisfiable) in [
        (80, 6, true),
        (100, 6, true),
        (80, 1, true),
        (101, 6, false),
        (80, 0, false),
    ] {
        assert_eq!(
            MutationPolicy::new(threshold, min_judged).is_satisfiable(),
            satisfiable,
            "a policy of {threshold}% over at least {min_judged} judged mutant(s)"
        );
    }
    assert_eq!(MutationPolicy::new(80, 6).threshold(), 80);
    assert_eq!(MutationPolicy::new(80, 6).min_judged(), 6);
}

#[test]
fn the_threshold_is_met_at_the_boundary_and_not_below_it() {
    let policy = MutationPolicy::new(80, 6);
    assert_eq!(
        policy.judge(Score::new(6, 0)),
        Verdict::Passed,
        "exactly the floor is enough evidence"
    );
    assert_eq!(
        policy.judge(Score::new(5, 0)),
        Verdict::Unproven,
        "100% of five mutants is not the evidence of 100% of six: below the floor there is no answer
         to give, and a two-valued gate would have to call this a failure — which sends the
         maintainer looking for a test to write instead of for the reason the run stopped looking"
    );
    assert_eq!(
        policy.judge(Score::new(8, 2)),
        Verdict::Passed,
        "80% meets 80%"
    );
    assert_eq!(
        policy.judge(Score::new(7, 3)),
        Verdict::Failed,
        "70% does not"
    );
    assert_eq!(
        policy.judge(Score::new(0, 6)),
        Verdict::Failed,
        "nothing killed"
    );
    assert_eq!(
        MutationPolicy::new(100, 2).judge(Score::new(2, 0)),
        Verdict::Passed,
        "100% is reachable, and that is the one threshold where it has to be"
    );
    assert_eq!(
        MutationPolicy::new(100, 2).judge(Score::new(49, 1)),
        Verdict::Failed,
        "98% fails a threshold of 100"
    );
}

#[test]
fn the_verdict_is_the_word_a_report_prints() {
    assert_eq!(Verdict::Passed.as_str(), "passed");
    assert_eq!(Verdict::Failed.as_str(), "failed");
    assert_eq!(Verdict::Unproven.as_str(), "unproven");
    assert_eq!(Verdict::Passed.to_string(), "passed");
    assert_eq!(Verdict::Unproven.to_string(), "unproven");
}

#[test]
fn a_survivor_says_where_to_look() {
    let survivor = Survivor::new("src/engine/mutation.rs", 42, 17, "`>=` → `>`");
    assert_eq!(
        survivor.place(),
        "src/engine/mutation.rs:42:17",
        "a survivor is something to write a case for, and a case needs the place: one line can hold
         two of the same operator, so the column is part of the answer"
    );
    assert_eq!(survivor.edit(), "`>=` → `>`");
}

#[test]
fn a_report_names_the_survivors_and_prints_the_run() {
    let policy = MutationPolicy::new(80, 6);
    let survivors = vec![
        Survivor::new("src/engine/store/memory.rs", 59, 47, "`&&` → `||`"),
        Survivor::new("src/engine/store/memory.rs", 59, 80, "`&&` → `||`"),
    ];
    let report = Report::of(&policy, Score::new(8, 2), survivors);

    assert_eq!(report.verdict(), Verdict::Passed);
    assert_eq!(report.score().percent(), Some(80));
    assert_eq!(report.survivors().len(), 2);
    assert_eq!(
        report.survivors().first().map(Survivor::place),
        Some("src/engine/store/memory.rs:59:47".to_owned()),
        "the survivors travel with the score, because a score nobody can act on is a score that gets
         ignored"
    );
    assert_eq!(
        report.summary(),
        "8/10 mutant(s) killed (80%)",
        "the line a CI job prints"
    );

    // The other two verdicts read differently on purpose: the failure names the threshold it missed,
    // and the inconclusive one names the evidence it does not have.
    let below = Report::of(&policy, Score::new(3, 10), Vec::new());
    assert_eq!(below.verdict(), Verdict::Failed);
    assert_eq!(
        below.summary(),
        "3/13 mutant(s) killed (23%), below the threshold"
    );
    let thin = Report::of(&policy, Score::new(3, 1), Vec::new());
    assert_eq!(thin.verdict(), Verdict::Unproven);
    assert_eq!(
        thin.summary(),
        "4 mutant(s) judged (75%), which is not enough evidence"
    );
    let nothing = Report::of(&policy, Score::new(0, 0), Vec::new());
    assert_eq!(nothing.verdict(), Verdict::Unproven);
    assert_eq!(
        nothing.summary(),
        "0 mutant(s) judged (nothing judged), which is not enough evidence"
    );
}

#[test]
fn the_suite_is_scored_against_a_table_of_planted_defects() {
    // What a run catches is the property's business, so the case asks a run for *something* and then
    // checks that the score says exactly what the walks said. A score that disagreed with the walks
    // it came from would be the one number in this step nobody could act on.
    let mutants = planted();
    let (seed, seen) = (0..64)
        .find_map(|seed| {
            let seen = killed_by_hand(seed, 40, &mutants);
            (seen > 0).then_some((seed, seen))
        })
        .unwrap_or_else(|| {
            panic!(
                "no walk of 40 steps from any of 64 seeds noticed a planted defect: the run either\n                 never reaches the end of a job's life or does not report what it caught"
            )
        });
    let score = caught(seed, 40, &[&TerminalIsFinal], &mutants);
    assert_eq!(
        score,
        Score::new(seen, 4 - seen),
        "`caught` is the walk of `jobq/3.19-property-invariants` run once per mutant, and the score it\n         answers has to be the count those walks produced"
    );
    assert_eq!(score.judged(), 4, "every planted defect was judged");

    // And the score is what a policy is held to, with the verdict and the line a run prints.
    let report = Report::of(
        &MutationPolicy::new(80, 4),
        score,
        vec![
            Survivor::new("src/engine/state.rs", 1, 1, "`complete` → `failed`"),
            Survivor::new("src/engine/state.rs", 2, 1, "the budget is ignored"),
        ],
    );
    assert_eq!(report.verdict(), Verdict::Failed);
    assert_eq!(
        report.summary(),
        format!("{seen}/4 mutant(s) killed (25%), below the threshold"),
        "the share is a quarter: one planted defect out of four"
    );
    assert_eq!(report.survivors().len(), 2);
    assert_eq!(
        MutationPolicy::new(25, 4).judge(score),
        Verdict::Passed,
        "25% meets a policy of 25, and the same score failed the one above: the policy is a decision,\n         and the score is only evidence"
    );
    assert_eq!(
        caught(seed, 40, &[&TerminalIsFinal], &mutants),
        score,
        "the same seed scores the same run: a score that moved between two runs of one seed would be\n         a score nobody could compare"
    );

    // The blind spot, whatever the run: `TerminalIsFinal` is about *leaving* the end of a job's life,
    // and a model that fails a completed job, or that retries past the budget, stays inside it. A
    // score of 100% against this table would mean the suite noticed something it cannot notice.
    for seed in 0..8 {
        for length in [1, 40, 200] {
            assert!(
                walk(&CallsItFailed, seed, length, &TerminalIsFinal).is_none(),
                "seed {seed} over {length} steps saw a defect this property does not talk about"
            );
            assert!(
                walk(&RetriesForever, seed, length, &TerminalIsFinal).is_none(),
                "seed {seed} over {length} steps saw the ignored budget"
            );
        }
    }

    // One more fact worth having before trusting a score: a walk ends a job's life at most once. Both
    // terminal states absorb everything that arrives afterwards, so a run that reached `succeeded`
    // never reaches `failed` — and the two defects that reopen a finished job can never be caught by
    // one run. A score belongs to a run, not to a suite, which is why the seed travels with it.
    for seed in 0..8 {
        let reopened = caught(
            seed,
            200,
            &[&TerminalIsFinal],
            &[&ReopensFailed, &ReopensSucceeded],
        );
        assert!(
            reopened.killed() <= 1,
            "seed {seed} caught both reopened ends of a job: {reopened:?}"
        );
    }
}

#[test]
fn a_suite_that_cannot_fail_kills_nothing() {
    // The cheap invariant, the expensive lesson: `true` is a property that cannot answer "no", and a
    // run against it is a perfect green score of nothing. This is the reason the step before this
    // one asks for a counter-example rather than for a green build.
    let score = caught(0, 40, &[&Holds], &planted());
    assert_eq!(
        score,
        Score::new(0, 4),
        "an invariant that always holds catches no planted defect"
    );
    assert_eq!(score.percent(), Some(0));
    assert_eq!(
        MutationPolicy::new(0, 4).judge(score),
        Verdict::Passed,
        "every mutant survived, and a policy that asks for nothing says that is fine: the score is
         the evidence, and the policy is the decision"
    );
    assert_eq!(
        MutationPolicy::new(1, 4).judge(score),
        Verdict::Failed,
        "one mutant is all it takes to fail a policy of 1%"
    );
}
