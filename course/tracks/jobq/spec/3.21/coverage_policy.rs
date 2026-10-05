//! Spec for `jobq/3.21-coverage-policy`: what a coverage number is about, and what it is not.
//!
//! `kiln run` copies this file into the project as `tests/spec_coverage_policy.rs`; it is the
//! course's file, not the learner's (plan §3 D2). The cases are about four decisions, and none of
//! them is about measuring:
//!
//! * **a line is not a decision.** Two places on the same line, two questions, and a suite that
//!   reports one number has reported the wrong one for an error arm nobody takes;
//! * **a number is about one module.** An inventory refuses a place from elsewhere and a run of
//!   one module refuses it too, so the arithmetic answers `Foreign` rather than a percentage the
//!   moment the two sides disagree about what they describe. A percentage over a whole library is
//!   the defect this step exists to make unwritable;
//! * **the threshold is a decision about what a case can reach.** The ceiling is what the walkable
//!   places allow, a perfect run scores exactly the ceiling, and a policy above it is a rule nobody
//!   can meet rather than a target somebody is failing;
//! * **the answer is a list.** `uncovered` is the places still to write a case for, in the order
//!   the inventory was written; "78%" is not something a person can act on.
//!
//! The page the step delivers (`docs/coverage.md`) is read here as well: a policy is a decision
//! somebody wrote down, and a page that states a different scope or threshold than
//! `CoveragePolicy::standard()` is a page that has stopped being true (authoring.md §6.6 — a
//! structural check, and the reason for it is written where it happens).

use std::path::PathBuf;

use jobq::engine::{CoveragePolicy, Exempt, Inventory, Kind, POLICY_SCOPE, Site, Verdict, Walked};

/// The page this step delivers, relative to the project.
const PAGE: &str = "docs/coverage.md";
/// The module the cases below build inventories of.
const SCOPE: &str = "src/engine/coverage.rs";
/// A module that is *not* the scope, for the refusals.
const OTHER: &str = "src/engine/store/memory.rs";

/// A branch place in [`SCOPE`].
fn site(line: usize) -> Site {
    Site::branch(SCOPE, line)
}

/// The page, read from the project it is part of.
fn page() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(PAGE);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("{PAGE} is the page this step delivers, and it cannot be read: {error}")
    })
}

/// An inventory of `SCOPE` with one branch place per line.
fn inventory(lines: impl IntoIterator<Item = usize>) -> Inventory {
    let mut inventory = Inventory::of(SCOPE);
    for line in lines {
        inventory
            .add(site(line))
            .expect("a place in the scope is what an inventory is for");
    }
    inventory
}

/// A run of `SCOPE` that reached one branch place per line.
fn walked(lines: impl IntoIterator<Item = usize>) -> Walked {
    let mut walked = Walked::of(SCOPE);
    for line in lines {
        walked
            .visit(&site(line))
            .expect("a place in the scope is what a run of it reaches");
    }
    walked
}

#[test]
fn a_line_and_a_branch_are_two_questions() {
    let statement = Site::line(SCOPE, 40);
    let decision = Site::branch(SCOPE, 40);

    assert_eq!(statement.kind(), Kind::Line);
    assert_eq!(decision.kind(), Kind::Branch);
    assert_ne!(
        statement, decision,
        "the same line answers two different questions — it ran, and one way out of it was taken —
         and a set of places that could not tell them apart would report the second number by
         counting the first"
    );
    assert_eq!(Kind::Line.as_str(), "line");
    assert_eq!(Kind::Branch.as_str(), "branch");
    assert_eq!(Kind::Branch.to_string(), "branch");
    assert_eq!(statement.path(), SCOPE);
    assert_eq!(statement.line_number(), 40);
    assert_eq!(statement.place(), "src/engine/coverage.rs:40");
    assert_eq!(decision.to_string(), "src/engine/coverage.rs:40");
}

#[test]
fn an_inventory_is_about_one_module() {
    let mut inventory = Inventory::of(SCOPE);
    assert_eq!(inventory.scope(), SCOPE);
    inventory.add(site(10)).expect("a place in the scope");

    let refused = inventory.add(Site::branch(OTHER, 10)).expect_err(
        "a place from another module is not a bigger percentage, it is a different question",
    );
    assert_eq!(refused.scope(), SCOPE);
    assert_eq!(refused.path(), OTHER);
    assert!(
        refused.to_string().contains(OTHER),
        "the refusal has to name what it refused: {refused}"
    );
    assert_eq!(
        inventory.sites().len(),
        1,
        "a refused place is not recorded — an inventory that absorbed it would report a percentage
         about a tree rather than about the module it claims to describe"
    );

    // The scope is a module, not a string prefix: `src/engine` does not contain `src/engineers`.
    let mut module = Inventory::of("src/engine");
    assert!(
        module
            .add(Site::branch("src/engineers/queue.rs", 3))
            .is_err(),
        "`src/engineers/queue.rs` is not in `src/engine`, and a prefix test that said so would be
         the same defect with a longer name"
    );
    assert!(module.add(Site::branch(OTHER, 3)).is_ok());
    assert_eq!(module.sites().len(), 1);
}

#[test]
fn a_walk_is_a_set_of_places_and_not_a_count_of_hits() {
    let mut run = walked([10]);
    assert_eq!(run.scope(), SCOPE);
    run.visit(&site(10))
        .expect("visiting the same place twice is not an error");
    assert_eq!(
        run.len(),
        1,
        "a line executed a thousand times in a loop is one place: a share that counted executions
         would be a share about the test data rather than about the suite"
    );
    assert!(run.reached(&site(10)));
    assert!(!run.reached(&site(20)));
    assert!(!run.is_empty());

    run.visit(&Site::line(SCOPE, 10))
        .expect("the line is a place of its own");
    assert_eq!(
        run.len(),
        2,
        "the line is a different place from the branch on it"
    );
    assert!(!run.reached(&Site::line(SCOPE, 20)));

    let refused = run
        .visit(&Site::branch(OTHER, 10))
        .expect_err("a run of one module is not evidence about another");
    assert_eq!(refused.scope(), SCOPE);
    assert_eq!(refused.path(), OTHER);
    assert_eq!(run.len(), 2);
    assert!(Walked::of(SCOPE).is_empty());
}

#[test]
fn the_share_is_a_share_of_the_inventory() {
    let mut inventory = inventory([10, 20, 30, 40]);
    inventory
        .add(Site::line(SCOPE, 99))
        .expect("a line of the same module");
    let mut run = walked([10, 20, 30]);
    run.visit(&Site::line(SCOPE, 99))
        .expect("the line is in the scope");

    assert_eq!(inventory.total(Kind::Branch), 4);
    assert_eq!(inventory.total(Kind::Line), 1);
    assert_eq!(inventory.walkable(Kind::Branch), 4);
    assert_eq!(
        inventory
            .share(Kind::Branch, &run)
            .expect("the run is of this module"),
        Some(75),
        "three of four branches"
    );
    assert_eq!(
        inventory
            .share(Kind::Line, &run)
            .expect("the run is of this module"),
        Some(100),
        "a line the run visited does not make the branch on it taken — the two are different
         denominators, and the second one is about the suite"
    );
    assert_eq!(
        inventory
            .uncovered(Kind::Branch, &run)
            .expect("the run is of this module")
            .len(),
        1
    );

    let foreign = inventory
        .share(Kind::Branch, &Walked::of(OTHER))
        .expect_err("a run of another module is not a number about this one");
    assert_eq!(foreign.scope(), SCOPE);
    assert_eq!(foreign.path(), OTHER);
    assert!(
        inventory
            .uncovered(Kind::Branch, &Walked::of(OTHER))
            .is_err()
    );
}

#[test]
fn a_module_with_nothing_to_walk_has_no_percentage() {
    let empty = Inventory::of(SCOPE);
    let run = Walked::of(SCOPE);

    assert_eq!(
        empty
            .share(Kind::Branch, &run)
            .expect("the run is of this module"),
        None,
        "`None` rather than `0`: a module with no branch in the inventory is not a module that
         failed, it is a question the inventory does not ask — the same answer
         `jobq/3.20-mutation-threshold` gives a run that judged no mutant"
    );
    assert_eq!(empty.ceiling(Kind::Branch), None);
    assert_eq!(empty.walkable(Kind::Branch), 0);
    assert_eq!(
        CoveragePolicy::of(90, Kind::Branch)
            .judge(&empty, &run)
            .expect("the run is of this module"),
        Verdict::Unproven,
        "not enough evidence to say either way, and a two-valued gate would have to call it a
         failure and send somebody to write a case for a module that has nothing to walk"
    );
    assert!(
        !CoveragePolicy::of(90, Kind::Branch).is_satisfiable(&empty),
        "a policy about a kind of place the module does not have is a rule nobody can meet"
    );
}

#[test]
fn the_ceiling_is_why_the_threshold_is_not_a_hundred() {
    let mut inventory = inventory([10, 20, 30]);
    assert_eq!(inventory.ceiling(Kind::Branch), Some(100));

    inventory
        .exempt(
            site(30),
            "the arm is unreachable: the guard above refuses every value that gets here",
        )
        .expect("an exemption in the scope");
    assert_eq!(inventory.walkable(Kind::Branch), 2);
    assert_eq!(inventory.total(Kind::Branch), 3);
    assert_eq!(
        inventory.ceiling(Kind::Branch),
        Some(66),
        "two of three is 66 percent and the division truncates: rounding up would let a policy one
         branch short of its ceiling read as met"
    );

    let perfect = walked([10, 20]);
    assert_eq!(
        inventory
            .share(Kind::Branch, &perfect)
            .expect("the run is of this module"),
        Some(66),
        "a perfect run scores the ceiling and not a hundred, which is what makes the two numbers
         comparable"
    );
    assert!(CoveragePolicy::of(66, Kind::Branch).is_satisfiable(&inventory));
    assert!(!CoveragePolicy::of(67, Kind::Branch).is_satisfiable(&inventory));
    assert_eq!(
        CoveragePolicy::of(67, Kind::Branch)
            .judge(&inventory, &perfect)
            .expect("the run is of this module"),
        Verdict::Failed,
        "a policy nobody can meet fails every run, and `is_satisfiable` is where that is found out
         before somebody spends a week on it"
    );
    assert_eq!(
        CoveragePolicy::of(66, Kind::Branch)
            .judge(&inventory, &perfect)
            .expect("the run is of this module"),
        Verdict::Passed,
        "66 meets 66: `>=`, not `>`"
    );
}

#[test]
fn an_exemption_carries_its_reason_and_is_not_a_to_do() {
    let mut inventory = inventory([10]);
    inventory
        .exempt(
            site(20),
            "the `unreachable!()` arm after an exhaustive match",
        )
        .expect("an exemption in the scope");

    assert_eq!(inventory.exemptions().len(), 1);
    let exempt = inventory.exemptions().first().expect("one exemption");
    assert_eq!(exempt.site(), &site(20));
    assert_eq!(
        exempt.reason(),
        "the `unreachable!()` arm after an exhaustive match"
    );
    assert_eq!(
        Exempt::of(site(30), "a panic path").reason(),
        "a panic path",
        "an exemption is a decision somebody has to be able to review, and the reason travels with
         the place rather than in a comment next to it"
    );

    let uncovered = inventory
        .uncovered(Kind::Branch, &Walked::of(SCOPE))
        .expect("the run is of this module");
    assert_eq!(
        uncovered.len(),
        1,
        "the exempt place is not a case to write: it is the entry a reader is meant to argue with"
    );
    assert_eq!(
        uncovered.first().map(|place| place.place()),
        Some(site(10).place())
    );

    assert!(
        inventory
            .exempt(Site::branch(OTHER, 20), "somewhere else")
            .is_err(),
        "a place from another module cannot be exempted either"
    );
    assert_eq!(inventory.exemptions().len(), 1);

    // And an exemption is a decision somebody can revise: recording the place as walkable again
    // takes the exemption back, so a place is never both a to-do and a decision.
    inventory.add(site(20)).expect("a place in the scope");
    assert!(inventory.exemptions().is_empty());
    assert_eq!(inventory.walkable(Kind::Branch), 2);
    assert_eq!(inventory.ceiling(Kind::Branch), Some(100));
}

#[test]
fn what_is_left_to_walk_is_a_list_in_the_order_it_was_written() {
    let inventory = inventory([40, 10, 30, 20]);
    let run = walked([10]);

    let uncovered = inventory
        .uncovered(Kind::Branch, &run)
        .expect("the run is of this module");
    assert_eq!(
        uncovered
            .iter()
            .map(|place| place.line_number())
            .collect::<Vec<usize>>(),
        [40, 30, 20],
        "inventory order rather than line order: the list is a to-do somebody works through, and a
         count of how many are left is not something a person can act on"
    );

    let done = walked([10, 20, 30, 40]);
    assert!(
        inventory
            .uncovered(Kind::Branch, &done)
            .expect("the run is of this module")
            .is_empty()
    );
}

#[test]
fn the_threshold_is_met_at_the_boundary_and_not_below_it() {
    let policy = CoveragePolicy::standard();
    assert_eq!(policy.threshold(), 90);
    assert_eq!(policy.kind(), Kind::Branch);

    let inventory = inventory(1..=10);
    let nine = walked(1..=9);
    assert_eq!(
        inventory
            .share(Kind::Branch, &nine)
            .expect("the run is of this module"),
        Some(90),
        "nine of ten branches"
    );
    assert_eq!(
        policy
            .judge(&inventory, &nine)
            .expect("the run is of this module"),
        Verdict::Passed,
        "90 meets 90: the comparison is `>=`, because a threshold of 90 is met by 90"
    );

    let eight = walked(1..=8);
    assert_eq!(
        inventory
            .share(Kind::Branch, &eight)
            .expect("the run is of this module"),
        Some(80)
    );
    assert_eq!(
        policy
            .judge(&inventory, &eight)
            .expect("the run is of this module"),
        Verdict::Failed,
        "one branch short is a failure rather than a rounding error"
    );
    assert!(
        policy.is_satisfiable(&inventory),
        "the project's own policy has to be one a perfect run can meet: 90 of ten branches is
         reachable"
    );
}

#[test]
fn the_page_states_the_policy_the_code_holds_to() {
    let page = page();
    let low = page.to_ascii_lowercase();
    for placeholder in ["todo", "tbd", "fixme"] {
        assert!(
            !low.contains(placeholder),
            "{PAGE} still says `{placeholder}`: the page is the deliverable, and a policy with a
             blank in it is a policy nobody has made"
        );
    }

    let header = page
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with('|') && line.contains("place"))
        .unwrap_or_else(|| {
            panic!(
                "{PAGE} has no inventory table: a policy is a claim about which places exist, and
                 the table is that claim"
            )
        });
    for column in ["place", "kind", "case"] {
        assert!(
            header.contains(column),
            "the inventory's header has no `{column}` column: a place without a `case` is a wish,
             and a `case` without a place is unverifiable. The header is {header}"
        );
    }

    let policy = CoveragePolicy::standard();
    assert!(
        page.contains(POLICY_SCOPE),
        "{PAGE} does not name `{POLICY_SCOPE}`, which is the module this policy is about — a
         percentage over a whole library is dominated by code nobody doubts, and it says nothing
         about the step"
    );
    assert!(
        page.contains(&policy.threshold().to_string()),
        "{PAGE} does not state the threshold `CoveragePolicy::standard()` holds the project to
         ({}), so the page and the code are two different policies",
        policy.threshold()
    );
    assert!(
        page.contains(policy.kind().as_str()),
        "{PAGE} does not say which of the two questions the threshold counts (`{}`), and `line` and
         `branch` are different decisions about the same code",
        policy.kind()
    );

    let rows: Vec<Vec<String>> = page
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('|'))
        .filter(|line| !line.contains("---"))
        .map(|line| {
            line.trim_matches('|')
                .trim()
                .split('|')
                .map(|cell| cell.trim().trim_matches('`').trim().to_owned())
                .collect()
        })
        .collect();
    let places: Vec<&String> = rows
        .iter()
        .flat_map(|row| row.first())
        .filter(|cell| cell.contains('/'))
        .collect();
    assert!(
        !places.is_empty(),
        "{PAGE} lists no place at all: an inventory with no entry is a policy about nothing"
    );
    for place in places {
        assert!(
            place.starts_with(POLICY_SCOPE),
            "{PAGE} lists `{place}`, which is not in `{POLICY_SCOPE}`: a place outside the scope is
             exactly what `Inventory::add` refuses, and a page that lists one is a policy about
             another module"
        );
    }
}
