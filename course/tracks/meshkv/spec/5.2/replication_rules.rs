//! What `meshkv/5.2-replication-rules` has to be true about.
//!
//! Two deliverables, judged together: the four rules as values (`raft`), and the page that states
//! them (`docs/consensus.md`). The last three cases are the ones that make the pair a check rather
//! than two documents — the catalogue, the rules and the table have to agree, word for word.

use meshkv::raft::{Entry, FAILURES, Failure, Index, Log, Quorum, RULES, Term, committable};

fn entry(term: u64, index: Index) -> Entry {
    Entry::new(Term::new(term), index)
}

fn filled(len: usize) -> Log {
    let mut log = Log::new();
    for index in 1..=len as Index {
        log.append(entry(1, index)).expect("an entry belongs next");
    }
    log
}

#[test]
fn a_term_moves_only_forward() {
    assert_eq!(Term::zero().get(), 0);
    assert_eq!(Term::new(7).get(), 7);
    assert_eq!(
        Term::new(7).next().get(),
        8,
        "standing for election moves up one"
    );
    assert!(
        Term::new(1) < Term::new(2),
        "terms compare, or nothing can order events"
    );
    assert_eq!(Term::new(7).next().next(), Term::new(9));
}

#[test]
fn an_entry_carries_the_term_that_proposed_it_and_its_position() {
    let proposed = entry(3, 4);
    assert_eq!(proposed.term, Term::new(3));
    assert_eq!(proposed.index, 4);
}

#[test]
fn a_log_appends_at_the_next_position_and_refuses_a_gap() {
    let mut log = Log::new();
    assert!(log.is_empty(), "a new log holds nothing");
    log.append(entry(1, 1))
        .expect("the first entry belongs at 1");
    log.append(entry(1, 2)).expect("the second belongs at 2");

    let gap = log
        .append(entry(1, 4))
        .expect_err("an entry at 4 cannot follow a log of 2");
    let message = gap.to_string();
    assert!(message.contains('4') && message.contains('3'), "{message}");

    let duplicate = log.append(entry(1, 2)).expect_err("2 is already taken");
    assert!(duplicate.to_string().contains('3'), "{duplicate}");
    assert_eq!(log.len(), 2, "a refused append changes nothing");
}

#[test]
fn a_log_refuses_a_term_that_goes_backwards() {
    let mut log = Log::new();
    log.append(entry(2, 1))
        .expect("a first entry from any term");
    log.append(entry(3, 2)).expect("a newer term is fine");
    let stale = log
        .append(entry(1, 3))
        .expect_err("term 1 cannot follow term 3");
    let message = stale.to_string();
    assert!(message.contains("backwards"), "{message}");
    assert_eq!(log.len(), 2);
}

#[test]
fn the_log_reads_back_by_position() {
    let log = filled(3);
    assert_eq!(log.len(), 3);
    assert!(!log.is_empty());
    assert_eq!(log.at(1).map(|entry| entry.index), Some(1));
    assert_eq!(log.at(3).map(|entry| entry.index), Some(3));
    assert!(log.at(0).is_none(), "positions are one-based");
    assert!(log.at(4).is_none(), "the future is not written yet");
    assert_eq!(log.last().map(|entry| entry.index), Some(3));
    assert!(Log::new().last().is_none());
}

#[test]
fn a_conflicting_suffix_is_dropped_from_the_first_mismatch() {
    let mut log = filled(5);
    assert_eq!(log.truncate_from(3), 3, "3, 4 and 5 go");
    assert_eq!(log.len(), 2);
    assert_eq!(log.last().map(|entry| entry.index), Some(2));
    assert_eq!(
        log.truncate_from(99),
        0,
        "past the end there is nothing to drop"
    );
    assert_eq!(log.truncate_from(0), 0, "position 0 names no entry");
}

#[test]
fn a_majority_is_more_than_half_of_the_cluster() {
    for (size, majority) in [(1, 1), (2, 2), (3, 2), (4, 3), (5, 3), (7, 4)] {
        assert_eq!(
            Quorum::of(size).majority(),
            majority,
            "a cluster of {size} decides with {majority}"
        );
    }
    let three = Quorum::of(3);
    assert!(
        !three.is_met(1),
        "one acknowledgement is not a majority of three"
    );
    assert!(three.is_met(2), "two is");
    assert!(three.is_met(3));
    assert!(
        !Quorum::of(0).is_met(0),
        "a cluster of no nodes commits nothing — and does not panic saying so"
    );
}

#[test]
fn an_entry_from_a_previous_term_does_not_commit_even_with_every_acknowledgement() {
    let three = Quorum::of(3);
    let inherited = entry(1, 4);
    assert!(
        !committable(&inherited, Term::new(2), 3, three),
        "every node holds an entry of term 1, and the leader is in term 2: it is still not a decision"
    );
    let own = entry(2, 5);
    assert!(
        !committable(&own, Term::new(2), 1, three),
        "not yet a majority"
    );
    assert!(
        committable(&own, Term::new(2), 2, three),
        "an entry of the current term on a majority is a decision"
    );
}

fn page() -> String {
    std::fs::read_to_string("docs/consensus.md")
        .expect("docs/consensus.md ships with the step — it is the step's second deliverable")
}

/// The rows of the page's failure table, `(scenario, behaviour)`, in the order they are written.
fn catalogue_rows(page: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut inside = false;
    for line in page.lines() {
        let line = line.trim();
        if line.starts_with("## ") {
            inside = line.contains("Failure catalogue");
            continue;
        }
        if !inside || !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        let [scenario, behaviour] = cells.as_slice() else {
            continue;
        };
        if scenario.is_empty() || scenario.chars().all(|c| c == '-' || c == ' ') {
            continue;
        }
        if scenario.eq_ignore_ascii_case("scenario") {
            continue;
        }
        rows.push(((*scenario).to_owned(), (*behaviour).to_owned()));
    }
    rows
}

#[test]
fn the_page_names_every_rule_the_code_relies_on() {
    let text = page();
    for rule in RULES {
        assert!(
            text.contains(rule),
            "the page does not carry the rule `{rule}` — the code and the page have to say the same four things"
        );
    }
}

#[test]
fn the_catalogue_and_the_page_name_the_same_failures() {
    let rows = catalogue_rows(&page());
    assert_eq!(
        rows.len(),
        FAILURES.len(),
        "the page has {} row(s) and the code {} failure(s)",
        rows.len(),
        FAILURES.len()
    );
    for (row, failure) in rows.iter().zip(FAILURES.iter()) {
        let Failure { scenario, expected } = *failure;
        assert_eq!(row.0, scenario, "scenario: page and code disagree");
        assert_eq!(
            row.1, expected,
            "behaviour for `{scenario}`: page and code disagree"
        );
    }
}

#[test]
fn the_catalogue_covers_each_failure_with_a_real_behaviour() {
    assert!(
        FAILURES.len() >= 5,
        "a protocol with fewer failures than this one is not designed yet"
    );
    for failure in FAILURES {
        let Failure { scenario, expected } = failure;
        assert!(
            !scenario.is_empty() && scenario != "TODO",
            "{scenario:?} is a placeholder"
        );
        assert!(
            !expected.is_empty() && expected != "TODO",
            "{scenario}: {expected:?}"
        );
        assert!(
            expected.len() > scenario.len(),
            "the behaviour is a sentence, not a restatement"
        );
    }
}
