//! Spec for `jobq/3.19-property-invariants`: a property is a generator, an invariant asked after
//! every step, and the counter-example that comes out when it is false.
//!
//! `kiln run` copies this file into the project as `tests/spec_invariants.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Nine cases, and most of them are about the *harness*
//! rather than about a queue, because a property test whose harness cannot fail is the most
//! expensive kind of decoration: it is a green line that means nothing.
//!
//! * **the generator** — one seed, one sequence, on every machine and in every process, spread
//!   over the model's whole alphabet. A generator that only ever drew one event would make every
//!   case below vacuously true;
//! * **after every step, not after the last one** — the case this step is named for. An invariant
//!   that cannot hold is answered by the *first* step, and an invariant that counts the history is
//!   answered by exactly as many operations as it needs, whatever the walk's length was;
//! * **a refusal is a step** — `transition` refuses far more pairs than it allows, and a walk that
//!   stopped at the first refusal would only ever see the moves a job is meant to make;
//! * **the model is the engine's table** — every step the invariant is handed is the answer
//!   `transition` gives for that pair, and a long walk reaches every state the model has;
//! * **a planted defect is found, and the finding replays** — the same property over a model with
//!   one rule changed answers with a seed and an operation sequence, and the seed reproduces it;
//! * **shrinking** — a case with noise in it comes back without the noise, and the three sentences
//!   of that contract are checked rather than assumed.
//!
//! Nothing here starts a container and nothing waits on a clock: the model is a pure function of a
//! state and an event (`jobq/3.14-job-state-machine`), which is exactly what makes it walkable, and
//! the durability of the same rules is `jobq/3.5`–`jobq/3.16`'s business.

use std::cell::RefCell;

use jobq::engine::{
    Counterexample, EVENTS, Event, Invariant, JobState, Machine, Model, Rng, STATES, Step,
    transition,
};
use jobq::engine::{replay, shrink, walk};

/// One step as the recorder below keeps it: event, before, after, applied.
type Seen = (Event, JobState, JobState, bool);

/// An invariant that answers `yes` and remembers what it was shown.
///
/// A property run leaves nothing behind when it holds, so the cases that are about what the walk
/// *did* rather than about what it found read it off a recorder: it always says yes, the walk runs
/// to its end, and the trajectory is in the `RefCell` afterwards.
#[derive(Default)]
struct Recorder(RefCell<Vec<Seen>>);

impl Recorder {
    /// Everything recorded so far, leaving the recorder empty.
    fn take(&self) -> Vec<Seen> {
        self.0.replace(Vec::new())
    }
}

impl Invariant for Recorder {
    fn holds(&self, _history: &[Event], step: &Step) -> bool {
        self.0
            .borrow_mut()
            .push((step.event(), step.before(), step.after(), step.applied()));
        true
    }
}

/// The operations a walk of `length` steps from `seed` plays, in order.
fn generated(seed: u64, length: usize) -> Vec<Event> {
    let recorder = Recorder::default();
    assert!(
        walk(&Machine, seed, length, &recorder).is_none(),
        "the recorder never refuses a step, so this walk found nothing"
    );
    recorder
        .take()
        .into_iter()
        .map(|(event, ..)| event)
        .collect()
}

/// A property that cannot hold: the first step is the counter-example, whatever it is.
struct Never;

impl Invariant for Never {
    fn holds(&self, _history: &[Event], _step: &Step) -> bool {
        false
    }
}

/// A property about the walk itself: fewer than three operations have been played.
///
/// Its answer depends on nothing but the length of the history, which is what makes it a case
/// about *when* the invariant is asked rather than about the queue.
struct ShorterThanThree;

impl Invariant for ShorterThanThree {
    fn holds(&self, history: &[Event], _step: &Step) -> bool {
        history.len() < 3
    }
}

/// The property this queue lives on: a job that finished is never moved out of the end of its life.
///
/// Written against the step rather than against the state, because that is what it is about: the
/// event that arrives at a terminal job may be refused (the job stays where it is) and must never
/// be accepted into somewhere else.
struct TerminalIsFinal;

impl Invariant for TerminalIsFinal {
    fn holds(&self, _history: &[Event], step: &Step) -> bool {
        !(step.before().is_terminal() && step.after() != step.before())
    }
}

/// A property about the history itself: the walk never sees two completions.
struct AtMostOneCompletion;

impl Invariant for AtMostOneCompletion {
    fn holds(&self, history: &[Event], _step: &Step) -> bool {
        history
            .iter()
            .filter(|event| **event == Event::Complete)
            .count()
            < 2
    }
}

/// A model with one rule changed: a job that is out of attempts may be claimed again.
///
/// The one difference between this and `Machine` is the line below, which is the point: a planted
/// defect is the smallest thing that can tell a property that works from a property that cannot
/// fail.
struct ReopensFailed;

impl Model for ReopensFailed {
    fn step(&self, state: JobState, event: Event) -> Result<JobState, jobq::engine::Illegal> {
        if state == JobState::Failed && event == Event::Claim {
            Ok(JobState::Running)
        } else {
            transition(state, event)
        }
    }
}

/// A model that refuses everything.
///
/// `transition` is the only way to build one of this crate's refusals — `Illegal` has no public
/// constructor on purpose — so even a model that never accepts an event answers with the engine's
/// own refusal.
struct Stonewall;

impl Model for Stonewall {
    fn step(&self, _state: JobState, _event: Event) -> Result<JobState, jobq::engine::Illegal> {
        transition(JobState::Succeeded, Event::Claim)
    }
}

/// `true` when every operation of `part` appears in `whole`, in the same order.
fn is_subsequence(part: &[Event], whole: &[Event]) -> bool {
    let mut rest = whole.iter();
    part.iter().all(|wanted| rest.any(|event| event == wanted))
}

#[test]
fn the_generator_is_the_seed_and_nothing_else() {
    // Two generators built from one seed are one generator. This is the promise a counter-example
    // rests on: a failure that could not be reproduced would be a rumour, and a run that drew from
    // a clock or a hash seed would produce one.
    assert_eq!(generated(7, 200), generated(7, 200));
    assert_ne!(
        generated(7, 200),
        generated(8, 200),
        "two seeds, two walks: a generator that ignored its seed would make the seed in a finding
         decoration too"
    );

    // `below` answers inside its range, and it answers at all when there is nothing to draw from.
    let mut rng = Rng::new(0);
    for bound in [1_u64, 2, 6, 1_000] {
        for _ in 0..200 {
            let drawn = rng.below(bound);
            assert!(
                drawn < bound,
                "`below({bound})` answered {drawn}, which is not an index into a list of {bound}"
            );
        }
    }
    assert_eq!(
        Rng::new(12_345).below(0),
        0,
        "drawing from nothing has one honest answer, and it is not a panic: a library that failed
         here would fail inside somebody else's process"
    );

    // And the alphabet is reachable: every event of the model turns up in a long walk, or the
    // properties below would only ever be asked about the events this generator happens to like.
    let mut seen: Vec<Event> = Vec::new();
    for event in generated(0, 600) {
        assert!(
            EVENTS.contains(&event),
            "a walk played `{event}`, which is not one of the model's own events: the list a walk
             draws from and the list the model answers about have to be one list"
        );
        if !seen.contains(&event) {
            seen.push(event);
        }
    }
    assert_eq!(
        seen.len(),
        EVENTS.len(),
        "600 operations reached {} of the model's {} events",
        seen.len(),
        EVENTS.len()
    );
}

#[test]
fn the_invariant_is_asked_after_every_step_and_not_after_the_last_one() {
    // An invariant that cannot hold is answered by the first step. A checker that ran at the end of
    // the walk would hand back all twenty-five operations, and the operation at fault — which is
    // what the person reading the failure has to write a test for — would be buried in them.
    let found = walk(&Machine, 3, 25, &Never).expect("a property that is never true has a case");
    assert_eq!(
        found.ops().len(),
        1,
        "the counter-example is one step, not the twenty-five the walk played: {:?}",
        found.ops()
    );
    assert_eq!(
        found.seed(),
        3,
        "the finding carries the seed it came from, because that is what replays the run"
    );

    // A property that needs three operations to say no answers with exactly three, however long
    // the walk was — and a walk too short to answer is not the same thing as a property that holds.
    let found = walk(&Machine, 3, 25, &ShorterThanThree)
        .expect("the third operation is the counter-example");
    assert_eq!(found.ops().len(), 3, "{:?}", found.ops());
    assert!(
        walk(&Machine, 3, 2, &ShorterThanThree).is_none(),
        "two operations are not three: `None` here means \"the walk was too short\", and that is
         why the case above is the one that says the invariant is asked at all"
    );
    assert!(
        walk(&Machine, 3, 0, &Never).is_none(),
        "a walk of no steps has no step to refuse"
    );
}

#[test]
fn a_refused_event_is_a_step_and_not_a_stop() {
    // Every event is refused here, so the walk can only count steps. If a refusal ended the walk,
    // or dropped the event from the history, this would answer `None` or one operation.
    let found = walk(&Stonewall, 4, 9, &ShorterThanThree)
        .expect("the third refusal is the counter-example");
    assert_eq!(found.ops().len(), 3, "{:?}", found.ops());

    // And a refusal is still an answer from the model: the job stays where it was, and the flag
    // says so. A walk that treated a refusal as "the model has no opinion" would be walking a
    // model the engine does not have.
    let recorder = Recorder::default();
    assert!(walk(&Stonewall, 4, 9, &recorder).is_none());
    let seen = recorder.take();
    assert_eq!(seen.len(), 9, "nine steps were played");
    for (event, before, after, applied) in seen {
        assert!(!applied, "`{event}` was refused by this model");
        assert_eq!(
            after, before,
            "a refused event leaves the job in `{before}`, and the invariant has to see that"
        );
    }
}

#[test]
fn the_walk_is_driven_by_the_engine_s_own_table() {
    // The model is a driver of `transition`, not a second implementation of it: every step the
    // invariant is shown is the answer `transition` gives that pair. Two implementations that
    // agree today are two implementations that disagree the first time one of them is edited.
    let mut reached: Vec<JobState> = vec![JobState::Queued];
    for seed in 0..8 {
        let recorder = Recorder::default();
        assert!(
            walk(&Machine, seed, 40, &recorder).is_none(),
            "the recorder never refuses a step"
        );
        let mut state = JobState::Queued;
        for (event, before, after, applied) in recorder.take() {
            assert_eq!(
                before, state,
                "the walk handed over a step that starts somewhere else"
            );
            match transition(state, event) {
                Ok(destination) => {
                    assert!(applied, "`{event}` is allowed from `{state}`");
                    assert_eq!(
                        after, destination,
                        "the model answered `{after}` where `transition` answers `{destination}`"
                    );
                }
                Err(refusal) => {
                    assert!(
                        !applied,
                        "`transition` refuses `{event}` from `{state}`: {refusal}"
                    );
                    assert_eq!(after, before, "a refusal moves nothing");
                }
            }
            state = after;
            if !reached.contains(&state) {
                reached.push(state);
            }
        }
    }

    // And the alphabet is wide enough to reach every state the model has. A walk that never got
    // anywhere near `failed` would hold every property about the end of a job for the wrong
    // reason — the events that would have taken it there were never played.
    for state in STATES {
        assert!(
            reached.contains(&state),
            "eight walks of forty steps never reached `{state}`: reached {reached:?} — the walk's
             alphabet is missing the event that gets there"
        );
    }
}

#[test]
fn the_engine_s_own_model_keeps_the_property() {
    // The property the whole queue rests on, walked rather than argued: whatever arrives at a job
    // that has finished, it is not handed out again. Every seed, several lengths, one answer.
    for seed in 0..16 {
        for length in [0, 1, 2, 5, 40] {
            let found = walk(&Machine, seed, length, &TerminalIsFinal);
            assert!(
                found.is_none(),
                "seed {seed} over {length} steps moved a finished job: {:?}",
                found.map(|found| found.ops().to_vec())
            );
        }
    }
}

#[test]
fn a_defect_somebody_planted_is_found_and_the_finding_replays() {
    // The same property, over a model with one rule changed. This is what tells the case above
    // apart from decoration: a property that cannot fail is a property nobody has to write, and
    // the cheapest evidence that this one can is a rule somebody broke.
    let (seed, found) = (0..32)
        .find_map(|seed| {
            walk(&ReopensFailed, seed, 40, &TerminalIsFinal).map(|found| (seed, found))
        })
        .unwrap_or_else(|| {
            panic!(
                "none of 32 walks over a model that reopens a finished job noticed it: either the
                 generator does not reach the end of a job or the invariant is not being asked"
            )
        });
    assert_eq!(found.seed(), seed, "the finding names the run it came from");
    assert_eq!(
        found.ops().last(),
        Some(&Event::Claim),
        "the counter-example ends with the claim of a job that had already finished: {:?}",
        found.ops()
    );

    // And the finding replays: the same seed, walked at least as long, answers the same case. This
    // is the half of a property run that a CI log is worth: the failure is reproducible.
    let again = walk(&ReopensFailed, seed, 40, &TerminalIsFinal)
        .expect("the finding was found with this seed");
    assert_eq!(
        again.ops(),
        found.ops(),
        "the same seed replays the same case"
    );
}

#[test]
fn shrinking_takes_out_what_the_failure_does_not_need() {
    // A case with noise in it: the property is about two completions, and the operations around
    // them are not part of it. There is exactly one answer that satisfies the contract — the two
    // completions and nothing else — so this is the shape of `shrink`, not one path through it.
    let noisy = Counterexample::new(
        7,
        vec![
            Event::Claim,
            Event::Complete,
            Event::Fail { retryable: true },
            Event::Complete,
        ],
    );
    let smaller = shrink(&Machine, &AtMostOneCompletion, &noisy);
    assert_eq!(
        smaller.ops(),
        [Event::Complete, Event::Complete],
        "the smallest thing that still breaks the property is the two completions"
    );
    assert_eq!(
        smaller.seed(),
        7,
        "the seed is carried over: the case is no longer what that seed generates, and the report
         still has to say which run it came from"
    );
}

#[test]
fn a_shrunk_case_still_breaks_and_nothing_more_can_come_out_of_it() {
    // A real finding from a real walk, and the three sentences of `shrink`'s contract checked
    // rather than assumed: it still breaks, its operations are the ones the walk found in the same
    // order, and no single operation can be taken out of it any more.
    let found = (0..32)
        .find_map(|seed| walk(&Machine, seed, 60, &AtMostOneCompletion))
        .expect("a walk long enough sees two completions");
    let smaller = shrink(&Machine, &AtMostOneCompletion, &found);

    assert!(
        smaller.ops().len() <= found.ops().len(),
        "shrinking made the case longer: {:?} from {:?}",
        smaller.ops(),
        found.ops()
    );
    assert!(
        is_subsequence(smaller.ops(), found.ops()),
        "{:?} is not a subsequence of the walk's {:?}",
        smaller.ops(),
        found.ops()
    );
    assert!(
        replay(
            &Machine,
            smaller.seed(),
            smaller.ops(),
            &AtMostOneCompletion
        )
        .is_some(),
        "the shrunk case does not break the property it came from: {:?}",
        smaller.ops()
    );
    for index in 0..smaller.ops().len() {
        let mut without = smaller.ops().to_vec();
        without.remove(index);
        assert!(
            replay(&Machine, smaller.seed(), &without, &AtMostOneCompletion).is_none(),
            "operation {index} of {:?} can still be taken out and the case still breaks, so the
             shrink stopped early",
            smaller.ops()
        );
    }
}

#[test]
fn the_lists_the_walk_draws_from_are_the_model_s_own_names() {
    // `jobq/3.14-job-state-machine` exported `STATES` and `EVENTS` as data because a walk needs an
    // alphabet; a hand-written list goes stale the day a state is added, so the names are checked
    // against the encodings here rather than trusted.
    assert_eq!(STATES.len(), 4, "the model has four states");
    assert_eq!(
        EVENTS.len(),
        6,
        "six spellings: two events with no flag, and two with both answers of the one flag"
    );
    let names: Vec<&str> = STATES.iter().map(|state| state.as_str()).collect();
    assert_eq!(
        names,
        ["queued", "running", "succeeded", "failed"],
        "the alphabet the walk draws from is the alphabet the column stores"
    );
    for state in STATES {
        assert_eq!(
            JobState::parse(state.as_str()).expect("a name this list holds is a name parse knows"),
            state,
            "`{}` does not round-trip",
            state.as_str()
        );
    }

    // Both answers of the one flag the model reads are in the alphabet. Without the non-retryable
    // spelling a walk could never reach `failed`, and every property about the end of a job would
    // hold because nothing ever got there.
    let flagged: Vec<bool> = EVENTS
        .iter()
        .filter_map(|event| match event {
            Event::Fail { retryable } | Event::ClaimLapsed { retryable } => Some(*retryable),
            _ => None,
        })
        .collect();
    assert_eq!(flagged.len(), 4, "two flagged events, two answers each");
    assert!(
        flagged.contains(&true) && flagged.contains(&false),
        "{flagged:?}"
    );
}
