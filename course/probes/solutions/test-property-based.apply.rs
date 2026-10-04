//! probe = "test/property-based@apply"
//! title = "A walk, an invariant, and the counter-example it owes you"
//! hint = "Check the invariant after every step, not at the end; on failure shrink the sequence until no single op can be removed or made smaller, and report the seed so the failure can be replayed."
//!
//! Step `jobq/3.19-property-invariants` replaces examples with properties: instead of writing the
//! three sequences somebody thought of, you state what must always be true and let a walk look for a
//! sequence that makes it false. Three things make that worth doing, and each one is a decision:
//!
//! * **the invariant is checked after every step.** A property checked only at the end of a walk
//!   misses exactly the interesting bugs — the ones where an op takes the system somewhere illegal
//!   and a *later* op puts it back. A queue whose depth goes to nine and back to zero passes an
//!   end-of-walk check and fails in production;
//! * **a failure comes with the sequence that caused it.** "The invariant failed" is not a bug
//!   report. [`run`] answers with the seed, the sequence the generator produced and the *shrunk* one
//!   — because the walk's own sequence is forty ops long and nine of them are irrelevant;
//! * **shrinking stops when it cannot see anything else to try.** [`shrink`] removes one op at a
//!   time and tries smaller spellings of the ops that have one, until neither helps. That is
//!   1-minimality, which is what a delta-debugger actually delivers: the report is small enough to
//!   read, and the claim "this is the shortest sequence that fails" would be a different (and false)
//!   claim;
//! * **the seed is enough to replay the failure.** A walk that cannot be re-run is a rumour, which
//!   is why the generator is deterministic and why [`Rng`] is a parameter rather than a thread-local
//!   random source.
//!
//! ## What you write
//!
//! * [`generate`] — one sequence of ops from the generator and the alphabet;
//! * [`holds`] — walk one sequence against a fresh subject, checking the invariant after **every**
//!   step, and answer with the violation or with nothing;
//! * [`shrink`] — the smallest failing sequence this minimiser can find;
//! * [`run`] — many cases, and the counter-example the first failing one owes.
//!
//! [`Rng`], [`Op`], [`ALPHABET`], [`Subject`], [`Violation`] and [`Outcome`] are given: a probe has
//! no `proptest` (the course meets it in T4·4.13, and a probe with a dependency is not a probe), so
//! the generator is a deterministic hash the tests can state exactly, and the *subject* — the model
//! whose invariant is being checked — is supplied by the caller here exactly as the macro supplies it
//! in the real step.
//!
//! ## What the tests ask
//!
//! A property that holds reports how many cases and steps were walked. A sequence that ends in a good
//! state is still a counter-example if it passed through a bad one, which is what "after every step"
//! buys. The reported sequence is enough to replay the failure from the reported seed, the shrunk
//! sequence is no longer than the original and still fails, and **no single op of it can be removed
//! or made smaller** — checked by brute force over the shrunk sequence, so a shrinker that gives up
//! early fails the test rather than quietly returning something long. Two runs of one seed are the
//! same run, and two seeds are not.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies and **no shrinker library**: `proptest` shrinks with a strategy that
//! knows how to make an integer smaller, and this file shrinks with the one thing it can always
//! do — delete an op, or spell it smaller. The consequence, stated rather than hidden: the result is
//! 1-minimal and not globally minimal, and a sequence whose failure only disappears when *two* ops
//! are removed together will not be reduced to its shortest form.
//!
//! Also missing on purpose: no coverage-guided search (the walk is uniform over the alphabet, which
//! is enough for the invariants this step states and does not pretend to be a fuzzer —
//! `test/fuzzing@apply` is T4's step), no parallelism, and no persistence of found counter-examples:
//! a property that has found a bug is a failing test to write down by hand, and the seed in the
//! report is how it is written.

/// The generator: a deterministic hash, so a failure walked today is a failure walked tomorrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// A generator for `seed`. The same seed is the same sequence of numbers, always.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next number: splitmix64, which is small, fast and good enough to shake a model.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `bound`, or 0 when there is nothing to choose from.
    pub fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            0
        } else {
            self.next_u64() % bound
        }
    }
}

/// One step of the walk.
///
/// A fixed alphabet rather than arbitrary bytes, because a shrinker that has to guess how to make a
/// random byte smaller is a different problem. `Add` carries a number so that "smaller spelling"
/// means something concrete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Add `n` units of work.
    Add(u8),
    /// Complete one.
    Remove,
    /// Throw away everything outstanding.
    Clear,
}

/// The ops a sequence is built from. Deliberately small: an alphabet is the shape of the walk, and a
/// walk over 256 ops finds a failure nobody can read.
pub const ALPHABET: [Op; 4] = [Op::Add(1), Op::Add(9), Op::Remove, Op::Clear];

/// A smaller spelling of an op, when it has one.
///
/// The second half of shrinking: deleting an op makes a sequence shorter, and this makes it *smaller*
/// without changing its shape. `None` means there is nothing smaller to try.
#[must_use]
pub fn smaller(op: Op) -> Option<Op> {
    match op {
        Op::Add(n) if n > 1 => Some(Op::Add(1)),
        _ => None,
    }
}

/// The model the walk drives, and the invariant it must keep.
///
/// The subject is the caller's, exactly as it is in the real step: the harness cannot know what a
/// job queue has to guarantee, and a probe that invented a subject would be testing its own example.
pub trait Subject {
    /// A fresh subject. The generator is handed over because the initial state is allowed to be
    /// random — a failure that needs a particular starting point is a real failure.
    fn new(rng: &mut Rng) -> Self;

    /// Do one op.
    fn apply(&mut self, op: Op);

    /// The invariant, in the subject's own words.
    ///
    /// # Errors
    ///
    /// A sentence a human can act on.
    fn invariant(&self) -> Result<(), String>;
}

/// Where a walk broke its invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Which step broke it, counting from 1.
    pub at_step: usize,
    /// What the subject said.
    pub message: String,
}

/// What one walk found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Every case held everywhere, and here is how much was walked.
    Held {
        /// How many cases the generator produced.
        cases: usize,
        /// How many ops were applied, over all of them.
        steps: usize,
    },
    /// A counter-example, small enough to read.
    Failed {
        /// The seed the failing case came from: enough to replay it without re-deriving it.
        seed: u64,
        /// The sequence the generator produced.
        original: Vec<Op>,
        /// The smallest sequence [`shrink`] could find that still fails.
        minimal: Vec<Op>,
        /// What the invariant said when it broke.
        message: String,
        /// Which step of `original` broke it.
        at_step: usize,
    },
}

/// One sequence of `steps` ops, from `rng` and the alphabet.
#[must_use]
pub fn generate(rng: &mut Rng, steps: usize) -> Vec<Op> {
    (0..steps)
        .map(|_| ALPHABET[rng.below(ALPHABET.len() as u64) as usize])
        .collect()
}

/// Walk `ops` against a fresh subject built from `seed`, checking the invariant after every step.
///
/// `None` means the sequence held everywhere. The check is inside the loop on purpose: a sequence
/// that ends well and passes through something illegal is the bug this whole exercise is aimed at.
#[must_use]
pub fn holds<S: Subject>(seed: u64, ops: &[Op]) -> Option<Violation> {
    let mut subject = S::new(&mut Rng::new(seed));
    for (index, op) in ops.iter().enumerate() {
        subject.apply(*op);
        if let Err(message) = subject.invariant() {
            return Some(Violation {
                at_step: index + 1,
                message,
            });
        }
    }
    None
}

/// The smallest failing sequence this minimiser can find, starting from a sequence that already
/// fails.
///
/// Two moves, applied to a fixpoint: drop one op, or replace one op with its [`smaller`] spelling.
/// A move is kept only when the sequence still fails, so the answer is a counter-example by
/// construction — and 1-minimal, which is the honest name for "no single op can be removed or made
/// smaller".
#[must_use]
pub fn shrink<S: Subject>(seed: u64, ops: &[Op]) -> Vec<Op> {
    let mut best = ops.to_vec();
    loop {
        let mut changed = false;
        for index in 0..best.len() {
            let Some(smaller) = smaller(best[index]) else {
                continue;
            };
            let mut candidate = best.clone();
            candidate[index] = smaller;
            if holds::<S>(seed, &candidate).is_some() {
                best = candidate;
                changed = true;
                break;
            }
        }
        if changed {
            continue;
        }
        for index in 0..best.len() {
            let mut candidate = best.clone();
            candidate.remove(index);
            if holds::<S>(seed, &candidate).is_some() {
                best = candidate;
                changed = true;
                break;
            }
        }
        if !changed {
            return best;
        }
    }
}

/// Walk `cases` sequences of `steps` ops each, and answer with the first counter-example.
///
/// Each case gets its own seed derived from `seed` and the case number, and the failing case's seed
/// is what the report carries: the case can then be replayed with one call to [`holds`], which is the
/// difference between a bug report and a rumour.
#[must_use]
pub fn run<S: Subject>(seed: u64, cases: usize, steps: usize) -> Outcome {
    let mut walked = 0;
    for case in 0..cases {
        let case_seed = seed ^ (case as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let mut rng = Rng::new(case_seed);
        let original = generate(&mut rng, steps);
        if let Some(violation) = holds::<S>(case_seed, &original) {
            let minimal = shrink::<S>(case_seed, &original);
            return Outcome::Failed {
                seed: case_seed,
                original,
                minimal,
                message: violation.message,
                at_step: violation.at_step,
            };
        }
        walked += steps;
    }
    Outcome::Held {
        cases,
        steps: walked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A depth that must never exceed three, and an op that puts it back to zero.
    ///
    /// The `Clear` is the whole point: `[Add(9), Add(9), Clear]` ends at zero and passes through
    /// nine, so a walk that checks the invariant at the end of the sequence does not find it.
    struct Depth {
        depth: i64,
    }

    impl Subject for Depth {
        fn new(_rng: &mut Rng) -> Self {
            Self { depth: 0 }
        }

        fn apply(&mut self, op: Op) {
            match op {
                Op::Add(n) => self.depth += i64::from(n),
                Op::Remove => self.depth -= 1,
                Op::Clear => self.depth = 0,
            }
        }

        fn invariant(&self) -> Result<(), String> {
            if self.depth > 3 {
                Err(format!("the depth reached {}", self.depth))
            } else {
                Ok(())
            }
        }
    }

    /// A subject whose invariant cannot fail: the shape of a property that is decoration, kept here
    /// so the "everything held" path has something to walk.
    struct Counting {
        ops: usize,
    }

    impl Subject for Counting {
        fn new(_rng: &mut Rng) -> Self {
            Self { ops: 0 }
        }

        fn apply(&mut self, _op: Op) {
            self.ops += 1;
        }

        fn invariant(&self) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn a_property_that_holds_says_how_much_of_the_space_it_walked() {
        let outcome = run::<Counting>(7, 64, 40);
        assert_eq!(
            outcome,
            Outcome::Held {
                cases: 64,
                steps: 64 * 40
            },
            "a property that holds is only worth something with the size of the walk attached"
        );
    }

    #[test]
    fn a_sequence_that_ends_well_is_still_a_counter_example() {
        let sequence = [Op::Add(1), Op::Add(1), Op::Add(1), Op::Add(1), Op::Clear];
        assert_eq!(
            holds::<Depth>(1, &sequence),
            Some(Violation {
                at_step: 4,
                message: "the depth reached 4".to_owned()
            }),
            "the invariant is checked after every step: by the end of this sequence the depth is \
             back to zero, and the fourth op is where it went over the bound"
        );
        assert_eq!(
            holds::<Depth>(1, &[Op::Add(1), Op::Add(1), Op::Clear]),
            None,
            "and a sequence that stays inside the bound holds, so the check is not vacuous"
        );
    }

    #[test]
    fn the_reported_seed_replays_the_failure_it_came_from() {
        let Outcome::Failed {
            seed,
            original,
            at_step,
            message,
            ..
        } = run::<Depth>(7, 64, 40)
        else {
            panic!("this subject has a counter-example inside forty ops of this alphabet")
        };

        let replayed = holds::<Depth>(seed, &original).expect("the seed replays the failure");
        assert_eq!(replayed.at_step, at_step);
        assert_eq!(replayed.message, message);
        assert!(at_step <= original.len());
        assert!(!original.is_empty());
    }

    #[test]
    fn the_counter_example_is_one_no_single_op_can_improve() {
        let Outcome::Failed {
            seed,
            original,
            minimal,
            ..
        } = run::<Depth>(11, 32, 50)
        else {
            panic!("expected a counter-example")
        };

        assert!(
            holds::<Depth>(seed, &minimal).is_some(),
            "a shrink that produces a sequence which does not fail is worse than no shrink: \
             {minimal:?}"
        );
        assert!(
            !minimal.is_empty(),
            "the empty sequence holds, so it is never a counter-example"
        );

        // Brute force the two moves the shrinker is allowed to make. If either one still fails, the
        // shrinker stopped early — which is the failure mode of every shrinker ever written.
        for index in 0..minimal.len() {
            let mut removed = minimal.clone();
            removed.remove(index);
            assert_eq!(
                holds::<Depth>(seed, &removed),
                None,
                "removing op {index} ({:?}) still fails, so the sequence is not minimal: {minimal:?}",
                minimal[index]
            );
            if let Some(smaller) = smaller(minimal[index]) {
                let mut replaced = minimal.clone();
                replaced[index] = smaller;
                assert_eq!(
                    holds::<Depth>(seed, &replaced),
                    None,
                    "op {index} could be spelled smaller, and the failure stayed: {minimal:?}"
                );
            }
        }

        assert!(
            minimal.len() < original.len(),
            "the walk produced {} ops and the report has to be readable: {minimal:?}",
            original.len()
        );
    }

    #[test]
    fn shrinking_only_ever_makes_things_shorter_or_smaller() {
        // A sequence that does not fail is returned as it was: a shrinker must not invent a
        // counter-example, and must not depend on the caller having checked first.
        let held = [Op::Add(1), Op::Clear, Op::Remove];
        assert_eq!(
            shrink::<Depth>(3, &held),
            held.to_vec(),
            "nothing to shrink: this sequence holds"
        );

        let failing = [Op::Add(9), Op::Clear, Op::Add(9), Op::Add(9)];
        let minimal = shrink::<Depth>(3, &failing);
        assert_eq!(
            minimal,
            vec![Op::Add(9)],
            "one op over the bound is already a counter-example, and `Add(1)` in its place would \
             hold: the shrinker keeps the failure and throws away the noise around it"
        );
    }

    #[test]
    fn the_walk_and_its_counter_example_are_reproducible() {
        let first = run::<Depth>(1234, 16, 30);
        let again = run::<Depth>(1234, 16, 30);
        assert_eq!(
            first, again,
            "a walk whose result changes between runs is a rumour: the generator is seeded, and \
             the seed is in the report"
        );

        let other = run::<Depth>(4321, 16, 30);
        let (
            Outcome::Failed { original, .. },
            Outcome::Failed {
                original: different,
                ..
            },
        ) = (&first, &other)
        else {
            panic!("both seeds find a counter-example in this subject");
        };
        assert_ne!(
            original, different,
            "two seeds are two walks, or the generator is not generating"
        );
    }
}
