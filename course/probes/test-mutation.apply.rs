//! probe = "test/mutation@apply"
//! title = "Prove the specs are not decoration"
//! hint = "Mutate one place at a time, count only the mutants that were actually run, and make the report name every survivor — a mutant that did not compile is neither a kill nor a miss."
//!
//! Step `jobq/3.20-mutation-threshold` asks the question coverage cannot: **would the specs notice
//! if this line were wrong?** A green test suite over a function nobody asserts anything about is
//! decoration, and the way to find out is to break the function on purpose and see whether anything
//! goes red.
//!
//! Four decisions make the difference between a number worth printing and a number that flatters:
//!
//! 1. **one place at a time, deterministically.** Every mutant is the source with a single
//!    operator flipped, and the set of mutants is chosen by rule rather than by taste: same source
//!    in, same six mutants out, on any machine ([`sample`]);
//! 2. **a mutant that does not compile is not a kill.** It is not a survivor either: it never ran.
//!    Counting it as killed would let a broken build inflate the score, and counting it as survived
//!    would blame the specs for the mutator's own rough edges — a text-level mutator is not a
//!    parser, and some of what it produces is not a program ([`Fate::Unviable`]);
//! 3. **the denominator is the mutants that ran**, and when nothing ran the answer is *blocked*
//!    rather than a green 100%. A gate that passes because it could not do anything is the failure
//!    mode this whole course keeps coming back to;
//! 4. **every survivor is named.** `78%` tells a learner nothing they can act on; `src/store.rs:41:
//!    \`==\` -> \`!=\`` tells them which assertion is missing.
//!
//! ## What you write
//!
//! * [`sites`] — every place in a source file one of this gate's operators applies, in line and
//!   column order;
//! * [`mutate`] — the source with exactly that one place changed;
//! * [`sample`] — at most `cap` sites, chosen so that two runs over one file cannot disagree;
//! * [`score`] — the share that died, the policy, and the survivors by name.
//!
//! [`Site`], [`Operator`], [`Fate`], [`Score`] and [`Verdict`] are given: the operator catalogue is
//! the gate's (handoff-m2 §1.3) and a probe that let you invent it would be checking a different
//! gate.
//!
//! ## What the tests ask
//!
//! Every operator finds its own sites, including the two-character ones — `<=` is not a `<`
//! followed by an `=`, and `!=` is not a `!` with something after it. A digit is only a site when
//! it stands alone, because `10` -> `11` is a different program rather than a smaller one. A mutant
//! differs from its original in exactly one place. Sampling returns the same sites every time, is
//! bounded by `cap`, and includes the first and last candidate so that a mutation at the end of a
//! file is not systematically invisible. The score ignores mutants that never ran — with the same
//! results, counting them as kills passes a threshold that they must not be allowed to pass — a run
//! with nothing viable is *blocked*, the share rounds **down** so a threshold is never flattered,
//! and the report names every survivor with its file, its line and both spellings.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no compiler and no spec run**: the real gate copies the project into
//! a sandbox, applies one mutation and runs the step's own command, where "red" is a kill. Here the
//! fate of each mutant is an input, and what is re-checked is everything *around* that run — the
//! catalogue, the deterministic sample, the arithmetic and the policy — which is the half a learner
//! can still get wrong after the tool works.
//!
//! Two consequences, stated rather than hidden: the mutator is text-level, so a `-` inside `->` is a
//! site and its mutant will not compile — that is precisely why [`Fate::Unviable`] exists; and
//! nothing here runs the specs, so a mutant that stays green because the *assertion* is missing is
//! indistinguishable from one that stays green because the code was already equivalent. The second
//! is the honest limit of mutation testing itself, not of this file: an equivalent mutant is a
//! survivor with no missing assertion behind it, and the threshold is what keeps that from
//! mattering.

/// One place a single flip can be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// The file, as the report names it.
    pub file: String,
    /// Line, counting from 1.
    pub line: u32,
    /// Column, counting from 1, in bytes.
    pub column: u32,
    /// What is at that place now.
    pub original: String,
    /// What the mutant puts there instead.
    pub replacement: String,
    /// Which operator this is.
    pub operator: Operator,
}

/// The gate's operator catalogue: one flip per variant, and the pair is always an involution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// `==` becomes `!=`.
    Eq,
    /// `!=` becomes `==`.
    Ne,
    /// `<` becomes `<=`.
    Lt,
    /// `<=` becomes `<`.
    Le,
    /// `>` becomes `>=`.
    Gt,
    /// `>=` becomes `>`.
    Ge,
    /// `&&` becomes `||`.
    And,
    /// `||` becomes `&&`.
    Or,
    /// `!` is deleted.
    Not,
    /// `Ok(` becomes `Err(`.
    Ok,
    /// `Err(` becomes `Ok(`.
    Err,
    /// `Some(` becomes `None(`.
    Some,
    /// `None(` becomes `Some(`.
    None,
    /// `+` becomes `-`.
    Plus,
    /// `-` becomes `+`.
    Minus,
    /// A bare `0` becomes `1`.
    Zero,
    /// A bare `1` becomes `0`.
    One,
}

impl Operator {
    /// What this operator is called, as a report prints it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Operator::Eq => "==",
            Operator::Ne => "!=",
            Operator::Lt => "<",
            Operator::Le => "<=",
            Operator::Gt => ">",
            Operator::Ge => ">=",
            Operator::And => "&&",
            Operator::Or => "||",
            Operator::Not => "!",
            Operator::Ok => "Ok",
            Operator::Err => "Err",
            Operator::Some => "Some",
            Operator::None => "None",
            Operator::Plus => "+",
            Operator::Minus => "-",
            Operator::Zero => "0",
            Operator::One => "1",
        }
    }
}

/// What happened when the specs were run against one mutant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fate {
    /// The specs went red: the mutant is dead, and the assertion that killed it exists.
    Killed,
    /// The specs stayed green: nothing in the suite looks at this behaviour.
    Survived,
    /// The mutant never ran — it did not compile, or the run did not finish. Neither a kill nor a
    /// miss, and it must not be counted as either.
    Unviable {
        /// Why, in one line, for the report.
        reason: String,
    },
}

/// The verdict a threshold produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The share that died reached the threshold.
    Pass,
    /// It did not.
    Fail,
    /// Nothing could be scored: no viable mutant at all. A gate that passes because it could not
    /// run is the failure this variant exists to prevent.
    Blocked,
}

/// What a mutation run is worth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Score {
    /// Mutants the specs killed.
    pub killed: usize,
    /// Mutants the specs did not notice.
    pub survived: usize,
    /// Mutants that never ran, counted and not scored.
    pub unviable: usize,
    /// `killed / (killed + survived)`, as whole percent, rounded down.
    pub percent: u32,
    /// What the threshold makes of it.
    pub verdict: Verdict,
    /// Every survivor, as `file:line: \`original\` -> \`replacement\``.
    pub survivors: Vec<String>,
}

/// Every place in `source` this gate's catalogue applies, in line and column order.
///
/// Column is a byte offset within the line, counting from 1. At each position the longest matching
/// operator wins, which is the whole reason `<=` does not become a `<` followed by an `=`, and why
/// `!=` is not a `!`. A digit is a site only when neither neighbour can make it part of a longer
/// number: `10` has none, and neither does `1.0` or `a1`.
#[must_use]
pub fn sites(file: &str, source: &str) -> Vec<Site> {
    todo!()
}

/// `source` with the single change `site` describes, and nothing else touched.
#[must_use]
pub fn mutate(source: &str, site: &Site) -> String {
    todo!()
}

/// At most `cap` of `sites`, chosen so that the same list always answers the same way.
///
/// Evenly spaced over the candidate list with the first and the last always kept: a mutation at the
/// end of a file is then not systematically invisible, and the choice does not depend on how many
/// sites there happen to be.
#[must_use]
pub fn sample(sites: &[Site], cap: usize) -> Vec<Site> {
    todo!()
}

/// Score a run against `threshold` percent.
///
/// The denominator is `killed + survived`: the mutants that actually ran. [`Fate::Unviable`] is
/// counted and reported but never scored, and a run where nothing ran is [`Verdict::Blocked`]. The
/// share rounds down, so a threshold is a floor rather than a thing a rounding error can flatter.
#[must_use]
pub fn score(results: &[(Site, Fate)], threshold: u32) -> Score {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small function with one of each shape in it. Text, not a parsed tree: the gate is a text
    /// mutator, and pretending otherwise in the test would hide exactly the rough edge it has.
    const SOURCE: &str = "\
fn limit(n: u32) -> u32 {
    if n >= 10 && n != 0 {
        return 0;
    }
    n + 1
}
";

    fn site_at_line(site_list: &[Site], line: u32) -> Vec<&Site> {
        site_list.iter().filter(|site| site.line == line).collect()
    }

    #[test]
    fn every_operator_finds_its_own_sites() {
        let found = sites("src/limit.rs", SOURCE);
        let shapes: Vec<(&str, &str)> = found
            .iter()
            .map(|site| (site.original.as_str(), site.replacement.as_str()))
            .collect();

        assert!(
            shapes.contains(&(">=", ">")),
            "a two-character operator is one site, not a `>` followed by an `=`: {shapes:?}"
        );
        assert!(
            shapes.contains(&("&&", "||")),
            "and the same for `&&`: {shapes:?}"
        );
        assert!(
            shapes.contains(&("!=", "==")),
            "`!=` is not a `!` with something after it: {shapes:?}"
        );
        assert!(
            shapes.contains(&("+", "-")),
            "an arithmetic flip is a site: {shapes:?}"
        );
        assert!(
            shapes.contains(&("-", "+")),
            "including the `-` in `->`, which is a site the mutator cannot tell from a subtraction \
             and whose mutant will not compile: {shapes:?}"
        );
        assert!(
            shapes.contains(&("0", "1")) || shapes.contains(&("1", "0")),
            "a standalone literal is a site: {shapes:?}"
        );

        // `10` is not two digits to flip: `11`, `00` and `1` are different programs, and a mutant
        // that changes what the code means rather than how much it does is not evidence.
        let ten = site_at_line(&found, 2)
            .iter()
            .filter(|site| site.original == "1" || site.original == "0")
            .count();
        assert_eq!(
            ten,
            1,
            "only the `0` in `!= 0` is a literal here: the `10` is one number: {:?}",
            site_at_line(&found, 2)
        );
        let decimals = sites("src/x.rs", "let x = 1.0;\n");
        assert!(
            decimals
                .iter()
                .all(|site| site.original != "1" && site.original != "0"),
            "and neither is a digit inside a decimal: {decimals:?}"
        );

        // Line and column are where the report points, so they have to be the real ones.
        let first = found.first().expect("the first line has a site");
        assert_eq!(first.line, 1);
        assert_eq!(
            first.column, 18,
            "the `-` of `->` is the first site on line 1"
        );
        assert_eq!(first.file, "src/limit.rs");
        assert!(
            found
                .windows(2)
                .all(|pair| (pair[0].line, pair[0].column) < (pair[1].line, pair[1].column)),
            "sites come out in reading order, so the sample is stable"
        );
    }

    #[test]
    fn a_mutant_differs_from_its_original_in_exactly_one_place() {
        let found = sites("src/limit.rs", SOURCE);
        for site in &found {
            let mutant = mutate(SOURCE, site);
            assert_ne!(mutant, SOURCE, "{site:?} produced the original back");
            assert_eq!(
                mutant.lines().count(),
                SOURCE.lines().count(),
                "a text flip inside a line does not change how many lines there are: {site:?}"
            );
            // Undoing the flip has to give the original back: that is what makes the mutant a
            // mutation of *this* program.
            let undo = Site {
                replacement: site.original.clone(),
                original: site.replacement.clone(),
                ..site.clone()
            };
            assert_eq!(
                mutate(&mutant, &undo),
                SOURCE,
                "undoing {site:?} did not give the original back"
            );
        }

        assert_eq!(
            mutate(SOURCE, &found[0]),
            SOURCE.replacen("->", "+>", 1),
            "and the change is the one the site describes"
        );
    }

    #[test]
    fn sampling_is_bounded_deterministic_and_keeps_the_two_ends() {
        let source: String = (0..20).map(|_| "let a = 0;\n").collect();
        let found = sites("src/many.rs", &source);
        assert_eq!(found.len(), 20, "one site per line: {found:?}");

        let six = sample(&found, 6);
        assert_eq!(
            six.len(),
            6,
            "`cap` is a cap and also a target when there is enough"
        );
        assert_eq!(
            six.iter().map(|site| site.line).collect::<Vec<u32>>(),
            vec![1, 4, 8, 12, 16, 20],
            "evenly spaced over the candidates with both ends kept"
        );
        assert_eq!(sample(&found, 6), six, "the same question, the same answer");
        assert_eq!(
            sample(&found, 100).len(),
            20,
            "asking for more than there is gets what there is"
        );
        assert_eq!(
            sample(&found, 1)
                .iter()
                .map(|site| site.line)
                .collect::<Vec<u32>>(),
            vec![1],
            "one mutant is the first candidate, not a panic on a division"
        );
        assert!(sample(&found, 0).is_empty(), "zero mutants is a policy");
        assert!(sample(&[], 6).is_empty());
    }

    #[test]
    fn a_mutant_that_never_ran_is_neither_a_kill_nor_a_miss() {
        let found = sites("src/limit.rs", SOURCE);
        let site = found[0].clone();
        let results = vec![
            (site.clone(), Fate::Killed),
            (site.clone(), Fate::Killed),
            (site.clone(), Fate::Killed),
            (site.clone(), Fate::Killed),
            (site.clone(), Fate::Killed),
            (site.clone(), Fate::Survived),
            (
                site.clone(),
                Fate::Unviable {
                    reason: "did not compile".to_owned(),
                },
            ),
            (
                site.clone(),
                Fate::Unviable {
                    reason: "the run timed out".to_owned(),
                },
            ),
        ];

        let report = score(&results, 80);
        assert_eq!(report.killed, 5);
        assert_eq!(report.survived, 1);
        assert_eq!(report.unviable, 2, "counted, and never scored");
        assert_eq!(
            report.percent, 83,
            "five of six, rounded down: counting the two unviable mutants as kills would have said \
             87, and counting them as survivors 62"
        );
        assert_eq!(report.verdict, Verdict::Pass);
        assert_eq!(
            report.survivors,
            vec!["src/limit.rs:1: `-` -> `+`".to_owned()],
            "the one mutant that stayed green is named, because that is the line the learner has \
             to go and write an assertion about"
        );

        // The threshold is honoured from the other side too.
        assert_eq!(score(&results, 84).verdict, Verdict::Fail);
        assert_eq!(score(&results, 83).verdict, Verdict::Pass);
    }

    #[test]
    fn a_run_where_nothing_ran_is_blocked_rather_than_green() {
        let site = sites("src/limit.rs", SOURCE)[0].clone();
        let results = vec![
            (
                site.clone(),
                Fate::Unviable {
                    reason: "did not compile".to_owned(),
                },
            ),
            (
                site.clone(),
                Fate::Unviable {
                    reason: "the run timed out".to_owned(),
                },
            ),
        ];
        let report = score(&results, 0);
        assert_eq!(
            report.verdict,
            Verdict::Blocked,
            "zero percent of nothing is not a pass, even with a threshold of zero"
        );
        assert_eq!(report.percent, 0);
        assert_eq!(report.unviable, 2);
        assert!(
            report.survivors.is_empty(),
            "and nothing is blamed on the specs"
        );

        assert_eq!(
            score(&[], 80).verdict,
            Verdict::Blocked,
            "an empty run is the same answer: the gate could not do its job"
        );
    }

    #[test]
    fn the_share_rounds_down_so_a_threshold_is_never_flattered() {
        let site = sites("src/limit.rs", SOURCE)[0].clone();
        let mut results = vec![(site.clone(), Fate::Killed), (site.clone(), Fate::Killed)];
        results.push((site.clone(), Fate::Survived));
        let report = score(&results, 67);
        assert_eq!(report.killed, 2);
        assert_eq!(report.survived, 1);
        assert_eq!(
            report.percent, 66,
            "two of three is 66.6 percent, and the number a report prints is the one a threshold \
             can be compared against without a rounding surprise"
        );
        assert_eq!(
            report.verdict,
            Verdict::Fail,
            "so a threshold of 67 is not met by two out of three"
        );

        // A clean run is a hundred, and every survivor is named with its own line.
        let mut perfect = Vec::new();
        for site in sites("src/limit.rs", SOURCE) {
            perfect.push((site, Fate::Killed));
        }
        let report = score(&perfect, 100);
        assert_eq!(report.percent, 100);
        assert_eq!(report.verdict, Verdict::Pass);
        assert!(report.survivors.is_empty());
    }

    #[test]
    fn the_report_names_every_survivor_and_not_just_the_count() {
        let found = sites("src/limit.rs", SOURCE);
        let results: Vec<(Site, Fate)> = found
            .iter()
            .enumerate()
            .map(|(index, site)| {
                let fate = if index % 2 == 0 {
                    Fate::Killed
                } else {
                    Fate::Survived
                };
                (site.clone(), fate)
            })
            .collect();
        let report = score(&results, 80);
        assert_eq!(report.survivors.len(), report.survived);
        assert!(
            report
                .survivors
                .iter()
                .all(|name| name.contains("src/limit.rs:")),
            "a survivor without a file and a line is a number, not a task: {:?}",
            report.survivors
        );
        for name in &report.survivors {
            assert!(
                name.contains("`") && name.contains("->"),
                "the report says what was changed, so the reader knows what to assert: {name}"
            );
        }
    }
}
