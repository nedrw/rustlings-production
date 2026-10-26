//! What `meshkv/5.16-chaos-injection` has to be true about.
//!
//! The step's subject is the matrix, and the cases below are the two ways a matrix goes wrong: it
//! drifts from the page operators read, or it drifts from itself (a missing row, a duplicate, an
//! empty cell).

use meshkv::chaos::{Case, MATRIX, case_for, check, covers};

/// The page's table, as the spec reads it: one row per `| fault | client sees | invariant |`.
fn page_rows() -> Vec<(String, String, String)> {
    let text = std::fs::read_to_string("docs/chaos.md")
        .expect("docs/chaos.md ships with the step — it is the step's second deliverable");
    let mut rows = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("## ") {
            inside = line.contains("The matrix");
            continue;
        }
        if !inside || !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        let [fault, sees, invariant] = cells.as_slice() else {
            continue;
        };
        if fault.is_empty() || fault.chars().all(|c| c == '-' || c == ' ') || *fault == "fault" {
            continue;
        }
        rows.push((
            (*fault).to_owned(),
            (*sees).to_owned(),
            (*invariant).to_owned(),
        ));
    }
    rows
}

#[test]
fn the_matrix_covers_every_fault_the_playbook_can_inject() {
    for fault in ["partition", "crash", "drop", "restart"] {
        assert!(covers(fault), "the matrix has no row for `{fault}`");
    }
    assert!(!covers("teleport"), "and it does not invent faults");
    assert!(case_for("partition").is_some());
    assert!(case_for("teleport").is_none());
}

#[test]
fn the_matrix_is_internally_consistent() {
    check().expect("the course's matrix");
    assert_eq!(MATRIX.len(), 4);
}

#[test]
fn every_row_answers_what_a_client_sees_and_what_must_still_hold() {
    for case in MATRIX {
        assert!(
            !case.client_sees.is_empty(),
            "{} has no client answer",
            case.fault
        );
        assert!(
            !case.invariant.is_empty(),
            "{} has no invariant",
            case.fault
        );
        assert_ne!(
            case.client_sees, case.invariant,
            "{} says the same thing in both columns",
            case.fault
        );
    }
}

#[test]
fn the_page_and_the_code_name_the_same_matrix() {
    let rows = page_rows();
    assert_eq!(
        rows.len(),
        MATRIX.len(),
        "the page has {} row(s) and the code {} case(s)",
        rows.len(),
        MATRIX.len()
    );
    for (row, case) in rows.iter().zip(MATRIX.iter()) {
        let Case {
            fault,
            client_sees,
            invariant,
        } = *case;
        assert_eq!(row.0, fault, "fault: page and code disagree");
        assert_eq!(
            row.1, client_sees,
            "`{fault}`: what a client sees disagrees"
        );
        assert_eq!(row.2, invariant, "`{fault}`: the invariant disagrees");
    }
}

#[test]
fn a_fault_the_matrix_does_not_cover_is_refused() {
    // The matrix is the vocabulary a runbook may use: a fault outside it has no expected behaviour,
    // and an operator who injects one is running an experiment, not a drill.
    assert!(!covers("split-brain"));
    assert!(case_for("split-brain").is_none());
}
