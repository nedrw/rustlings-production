//! probe = "test/chaos-injection@design"
//! title = "The fault matrix is data, and a page that drifted from it is a page that lies"
//! hint = "Give every fault one row with two different sentences, refuse a matrix that repeats a fault or leaves a cell empty, and compare the page with the code row by row in both directions."
//!
//! Step `meshkv/5.16-chaos-matrix` is where faults stop being one at a time. The shallower slot,
//! `test/chaos-injection@apply` (`relay/2.17`), injects a fault at the HTTP boundary and asserts one
//! outcome; this file is the step's other half: the **matrix**, the table an operator reads before
//! injecting anything. A probe cannot ship `docs/chaos.md` or a running cluster, so the page arrives
//! as rows and the domain is strings — but the rules are the ones a real matrix paid for:
//!
//! 1. **one row per fault.** A duplicate is not a second opinion, it is a fault somebody forgot:
//!    coverage is counted in distinct faults, so a matrix that names one twice has a hole in it.
//!    [`Matrix::new`] refuses the repeat and names the row.
//! 2. **the two columns answer different questions.** `client_sees` is what a caller can observe
//!    while the fault is injected; `invariant` is what a drill asserts after it. A row that says the
//!    same sentence twice has one column, and a drill written from it checks what the caller already
//!    saw.
//! 3. **an empty cell is a row nobody finished.** A fault with no expected behaviour is an
//!    experiment, not a drill, and a whitespace-only cell is empty: the matrix is a document an
//!    operator trusts without reading the code, and a blank is a place they will invent something.
//! 4. **a fault outside the matrix has no expected behaviour at all.** [`Matrix::admit`] refuses the
//!    drill at the *first* fault it cannot answer for, because a runbook that silently skips what it
//!    does not know is a runbook that reports success after doing less.
//! 5. **the page and the code are the same matrix, or one of them lies.** [`Matrix::drift`] compares
//!    them positionally and reports the first row and column that disagree, in either direction: a
//!    page that ends early, a page that names a fault the code never had, and a sentence that moved
//!    on one side. The spec is the only thing keeping a page from describing behaviour nobody
//!    implements.
//!
//! ## What you write
//!
//! * [`Matrix::new`] — the matrix those rows describe, or the first reason they are not one;
//! * [`Matrix::admit`] — a drill's list of faults, admitted whole or refused at the first fault the
//!   matrix does not cover;
//! * [`Matrix::drift`] — the first place a page and these rows disagree.
//!
//! [`Case`], [`Column`], [`Flaw`], [`Drill`] and [`Drift`] are given, with [`covers`] and
//! [`case_for`] as the vocabulary a runbook may use. The two columns and the three ways a row goes
//! wrong are the interface the page, the runbook and the spec are all written against; rebuilding
//! them in the exercise would be checking a different table.
//!
//! ## What the tests ask
//!
//! One row per fault is accepted and the four faults are the vocabulary a drill may name. A fault
//! named twice — the last row repeated, so an adjacent-only check fails — a whitespace-only cell,
//! an empty fault name, and a row whose invariant is its caller sentence with padding are each
//! refused with their own reason, and an empty matrix is refused because it answers nothing. A drill
//! that names `split-brain` after two covered faults is refused at `split-brain` and not at the
//! covered ones, while a drill of covered faults and a drill of none are admitted. The drift cases
//! live on the boundary: the page loses its last row, gains one, swaps two rows, or changes a
//! sentence, and the answer is the first position and column that differ. And the last test swaps
//! the two sides of a one-cell disagreement: the same row and the same column come back, with the
//! two sentences exchanged, which is what makes the comparison a relation rather than a verdict
//! about which side is good.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no page on disk and no cluster**. The real step reads
//! `docs/chaos.md` from the repository and compares it with the code's `MATRIX` row by row, and the
//! faults it lists are injected into a running cluster by `5.13`'s playbook. Here the page is a
//! slice of rows and a fault is a `&'static str`, so what is re-checked is the *contract*: the shape
//! a matrix must have and the comparison that keeps its two copies together. The Markdown reading,
//! the pump that injects a fault, and the outcomes themselves are not in this file — the reading is
//! `ops/runbook-drill@operate`'s habit, and the faults are `meshkv/5.14` and `5.15`'s.
//!
//! Missing on purpose: no fault is scheduled or injected, so nothing here can discover that a row's
//! sentence is *false* for the cluster — a matrix can pass every test in this file and still
//! describe a partition that behaves differently. That is what the drills are for; this file only
//! makes sure a drill that names a fault has a row, and that the row an operator reads is the row
//! the code holds.

use std::cmp::Ordering;
use std::fmt;

/// One row of the matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Case {
    /// The fault, as the playbook names it.
    pub fault: &'static str,
    /// What a caller can observe while the fault is injected.
    pub client_sees: &'static str,
    /// What the cluster must still hold afterwards.
    pub invariant: &'static str,
}

/// One cell of a row, so that a refusal can name the one it is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// The fault's name.
    Fault,
    /// The sentence a caller can act on.
    ClientSees,
    /// The assertion a drill makes.
    Invariant,
}

impl Column {
    /// The name the column has in the page.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Fault => "fault",
            Self::ClientSees => "client_sees",
            Self::Invariant => "invariant",
        }
    }
}

/// Why a list of rows is not a matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flaw {
    /// There is no row at all, so no fault has an expected behaviour.
    NoRows,
    /// One fault has two rows, which means some fault has none.
    Duplicate {
        /// The fault named twice.
        fault: &'static str,
    },
    /// A cell nobody filled in.
    Empty {
        /// The row's fault, empty when the fault name itself is the empty cell.
        fault: &'static str,
        /// Which cell is empty.
        column: Column,
    },
    /// The invariant is the caller's sentence, so there is one column where the page claims two.
    Restated {
        /// The row that says one thing twice.
        fault: &'static str,
    },
}

impl fmt::Display for Flaw {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRows => write!(formatter, "a matrix with no rows answers no fault"),
            Self::Duplicate { fault } => write!(
                formatter,
                "`{fault}` is named twice: one fault has one row, and the fault this row displaced has \
                 none"
            ),
            Self::Empty { fault, column } => write!(
                formatter,
                "the row for `{fault}` has no `{}`: a cell nobody filled in is a fault nobody answered \
                 for",
                column.name()
            ),
            Self::Restated { fault } => write!(
                formatter,
                "the row for `{fault}` says the same thing in both columns: what a caller sees and what \
                 a drill asserts are different facts"
            ),
        }
    }
}

/// Why a drill cannot be admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drill {
    /// The matrix has no row for this fault.
    Uncovered {
        /// The fault, as the drill named it.
        fault: String,
    },
}

impl fmt::Display for Drill {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Uncovered { fault } => write!(
                formatter,
                "`{fault}` has no row in the matrix: a fault with no expected behaviour is an \
                 experiment, not a drill"
            ),
        }
    }
}

/// Where a page and the code first disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drift {
    /// The page has no row at this position, though the code does.
    PageShort {
        /// The first position the page does not reach.
        at: usize,
    },
    /// The page has a row at this position that the code does not.
    PageLong {
        /// The first position the code does not reach.
        at: usize,
    },
    /// Both sides have the row, and one cell disagrees.
    Cell {
        /// The position of the row.
        at: usize,
        /// Which cell disagrees.
        column: Column,
        /// What the code says.
        code: &'static str,
        /// What the page says.
        page: &'static str,
    },
}

impl fmt::Display for Drift {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PageShort { at } => write!(
                formatter,
                "the page has no row at position {at}, but the matrix does"
            ),
            Self::PageLong { at } => write!(
                formatter,
                "the page has a row at position {at} that the matrix does not"
            ),
            Self::Cell {
                at,
                column,
                code,
                page,
            } => write!(
                formatter,
                "row {at}, `{}`: the code says `{code}` and the page says `{page}`",
                column.name()
            ),
        }
    }
}

/// The matrix: one row per fault this project injects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matrix {
    cases: Vec<Case>,
}

impl Matrix {
    /// The matrix these rows describe, or the first reason they are not one.
    ///
    /// The rows are kept in the order they were given: the page prints that order, and a drill reads
    /// it back position by position.
    pub fn new(cases: Vec<Case>) -> Result<Self, Flaw> {
        todo!()
    }

    /// The rows, in the order the page prints them.
    #[must_use]
    pub fn rows(&self) -> &[Case] {
        &self.cases
    }

    /// `true` when the matrix has a row for `fault`.
    #[must_use]
    pub fn covers(&self, fault: &str) -> bool {
        self.case_for(fault).is_some()
    }

    /// The case for `fault`, if the matrix has one.
    #[must_use]
    pub fn case_for(&self, fault: &str) -> Option<&Case> {
        self.cases.iter().find(|case| case.fault == fault)
    }

    /// Admit a drill over `faults`, or refuse it at the first fault the matrix cannot answer for.
    ///
    /// The first uncovered fault is the one reported: it is the first excuse a responder meets, and
    /// a drill is not partially admissible — a fault with no expected behaviour makes the run an
    /// experiment whether or not the other faults are covered.
    ///
    /// # Errors
    ///
    /// [`Drill::Uncovered`] naming the first requested fault with no row.
    pub fn admit(&self, faults: &[&str]) -> Result<(), Drill> {
        todo!()
    }

    /// The first place `page` and these rows disagree, in row order.
    ///
    /// The comparison is positional, because the page is the same table in the same order: rows that
    /// moved are a drift even when every row exists on both sides. When the two lists end at
    /// different lengths the shorter one is the finding and its variant says which side it was.
    #[must_use]
    pub fn drift(&self, page: &[Case]) -> Option<Drift> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASES: [Case; 4] = [
        Case {
            fault: "partition",
            client_sees: "writes sent to the isolated side are refused; the majority side keeps \
                          committing",
            invariant: "a committed write stays committed on every node once the link heals",
        },
        Case {
            fault: "crash",
            client_sees: "writes sent to the crashed node are refused until it restarts",
            invariant: "the crashed node catches up from the log when it comes back",
        },
        Case {
            fault: "drop",
            client_sees: "the dropped messages are lost; the client retries with the same idempotency \
                          key",
            invariant: "the retry is not a second write",
        },
        Case {
            fault: "restart",
            client_sees: "the node is unavailable for the length of the restart, then serves again",
            invariant: "a rolling restart never leaves fewer than a majority up",
        },
    ];

    fn matrix() -> Matrix {
        Matrix::new(CASES.to_vec()).expect("the course's matrix is a matrix")
    }

    #[test]
    fn one_row_per_fault_is_a_matrix_and_those_rows_are_the_vocabulary() {
        let matrix = matrix();

        assert_eq!(
            matrix.rows().len(),
            4,
            "four faults, four rows: coverage is counted in distinct faults"
        );
        assert!(matrix.covers("partition"), "the matrix has a partition row");
        assert!(
            !matrix.covers("teleport"),
            "a fault nobody wrote a row for is not in the vocabulary"
        );
        assert!(
            matrix.case_for("drop").is_some(),
            "a covered fault can be looked up by name"
        );
        assert_eq!(
            matrix.admit(&["partition", "crash", "drop", "restart"]),
            Ok(()),
            "a drill over the whole matrix has an expected behaviour for every fault"
        );
    }

    #[test]
    fn a_fault_named_twice_is_a_fault_somebody_forgot() {
        let mut cases = CASES.to_vec();
        cases.push(CASES[0]);

        let flaw = Matrix::new(cases).expect_err("one fault, two rows");
        assert_eq!(
            flaw,
            Flaw::Duplicate { fault: "partition" },
            "the row that repeated is the row the refusal names"
        );
        assert!(
            flaw.to_string().contains("displaced"),
            "the reason says why a repeat is a hole: {flaw}"
        );
    }

    #[test]
    fn an_empty_cell_is_a_row_nobody_finished() {
        let mut cases = CASES.to_vec();
        cases[1].client_sees = "   ";
        assert_eq!(
            Matrix::new(cases).expect_err("a blank caller answer"),
            Flaw::Empty {
                fault: "crash",
                column: Column::ClientSees,
            },
            "a cell of spaces is empty, and the refusal names the column"
        );

        let mut cases = CASES.to_vec();
        cases[2].invariant = "";
        assert_eq!(
            Matrix::new(cases).expect_err("a blank invariant"),
            Flaw::Empty {
                fault: "drop",
                column: Column::Invariant,
            },
            "the invariant is the cell a hurried page drops"
        );

        let mut cases = CASES.to_vec();
        cases[0].fault = " ";
        assert_eq!(
            Matrix::new(cases).expect_err("a row with no fault name"),
            Flaw::Empty {
                fault: " ",
                column: Column::Fault,
            },
            "a row nobody can request by name has no expected behaviour"
        );
    }

    #[test]
    fn a_row_that_says_one_thing_in_both_columns_has_one_column() {
        let mut cases = CASES.to_vec();
        cases[0].invariant = "  writes sent to the isolated side are refused; the majority side keeps \
                               committing  ";
        assert_eq!(
            Matrix::new(cases).expect_err("the two columns are the same sentence"),
            Flaw::Restated { fault: "partition" },
            "padding does not make an assertion out of a caller's sentence"
        );
    }

    #[test]
    fn a_matrix_with_no_rows_answers_no_fault() {
        assert_eq!(
            Matrix::new(Vec::new()).expect_err("an empty table"),
            Flaw::NoRows,
            "a runbook with no rows has no drill to run"
        );
    }

    #[test]
    fn a_drill_is_refused_at_the_first_fault_the_matrix_cannot_answer_for() {
        let matrix = matrix();
        assert_eq!(
            matrix.admit(&["partition", "split-brain", "teleport"]),
            Err(Drill::Uncovered {
                fault: "split-brain".to_owned(),
            }),
            "the first uncovered fault is the one a responder meets first"
        );
        assert!(
            matrix.admit(&[]).is_ok(),
            "an empty drill asks for nothing and owes nothing"
        );
        let error = matrix
            .admit(&["teleport"])
            .expect_err("a fault outside the vocabulary");
        assert!(
            error.to_string().contains("experiment"),
            "the reason says what injecting an uncovered fault is: {error}"
        );
    }

    #[test]
    fn the_page_and_the_code_are_compared_row_by_row_and_the_first_difference_is_the_finding() {
        let matrix = matrix();
        assert_eq!(
            matrix.drift(&CASES),
            None,
            "a page that is the matrix has not drifted"
        );

        let mut page = CASES.to_vec();
        page[2].invariant = "the retry writes a second time";
        assert_eq!(
            matrix.drift(&page),
            Some(Drift::Cell {
                at: 2,
                column: Column::Invariant,
                code: CASES[2].invariant,
                page: "the retry writes a second time",
            }),
            "a sentence that changed on one side is the drift, at its row and column"
        );

        let mut swapped = CASES.to_vec();
        swapped.swap(0, 1);
        assert_eq!(
            matrix.drift(&swapped),
            Some(Drift::Cell {
                at: 0,
                column: Column::Fault,
                code: "partition",
                page: "crash",
            }),
            "the page prints the matrix's order, so a moved row is not the same page"
        );

        let short = &CASES[..3];
        assert_eq!(
            matrix.drift(short),
            Some(Drift::PageShort { at: 3 }),
            "a page that lost its last row is a finding at the first position it does not reach"
        );

        let mut long = CASES.to_vec();
        long.push(Case {
            fault: "teleport",
            client_sees: "the node is somewhere else",
            invariant: "nothing was written",
        });
        assert_eq!(
            matrix.drift(&long),
            Some(Drift::PageLong { at: 4 }),
            "a page that invented a row is a finding at the first position the code does not reach"
        );
    }

    #[test]
    fn the_drift_is_a_relation_and_the_two_sides_exchange() {
        let code = matrix();
        let mut other = CASES.to_vec();
        other[1].invariant = "the crashed node forgets what it had applied";
        let page = Matrix::new(other).expect("the changed rows are still a matrix");

        let there = code.drift(page.rows());
        let back = page.drift(code.rows());
        match (&there, &back) {
            (
                Some(Drift::Cell {
                    at: there_at,
                    column: there_column,
                    code: there_code,
                    page: there_page,
                }),
                Some(Drift::Cell {
                    at: back_at,
                    column: back_column,
                    code: back_code,
                    page: back_page,
                }),
            ) => {
                assert_eq!(
                    there_at, back_at,
                    "both directions find the disagreement on the same row"
                );
                assert_eq!(
                    there_column, back_column,
                    "and in the same column, because the page is not the only side that can move"
                );
                assert_eq!(
                    (there_code, there_page),
                    (back_page, back_code),
                    "the two sides exchange: the sentence the code calls wrong is the sentence the \
                     page calls wrong"
                );
            }
            _ => panic!("both directions find one cell: {there:?} and {back:?}"),
        }

        let short = Matrix::new(CASES[..3].to_vec()).expect("three rows are a matrix");
        assert_eq!(
            code.drift(short.rows()),
            Some(Drift::PageShort { at: 3 }),
            "the longer side names the row the shorter one lacks"
        );
        assert_eq!(
            short.drift(&CASES),
            Some(Drift::PageLong { at: 3 }),
            "and the two length findings are the same row seen from the two sides"
        );
    }
}
