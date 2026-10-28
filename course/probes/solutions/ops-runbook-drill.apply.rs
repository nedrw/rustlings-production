//! probe = "ops/runbook-drill@apply"
//! title = "The drill finds the step that was never written"
//! hint = "Walk the procedure in order: a step whose precondition no earlier step establishes is the step the runbook is missing."
//!
//! Step `mailer/1.22-runbook` writes the procedure down and then rehearses it, because a runbook is
//! evidence only once somebody has followed it. The drill's most useful finding is not a wrong
//! command: it is a step that a person would have performed from memory and the document never
//! mentions — the procedure says "restore the backup" and nothing said where the backup is.
//!
//! The procedure here is data, because a probe runs offline and cannot page anybody. What is
//! re-checked is the reading a reviewer does before trusting a runbook: every step's precondition
//! is established by a step that comes **before** it.
//!
//! ## What you write
//!
//! [`audit`] — one [`Gap`] per unmet precondition, in step order:
//!
//! * walk the steps in order, keeping the facts established so far;
//! * a step's `needs` that no earlier step `provides` is a gap naming the step and the fact;
//! * a step's own `provides` never satisfy its own `needs`, and neither does a later step: order is
//!   the whole point of a procedure.
//!
//! ## What the tests ask
//!
//! A complete runbook; a fact nobody provides; a fact provided too late; two gaps in order; a step
//! that would have to provide its own precondition; and the same fact provided twice, which is
//! harmless.

/// One step of a runbook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// What the step does.
    pub name: String,
    /// Facts it assumes are already true.
    pub needs: Vec<String>,
    /// Facts it establishes.
    pub provides: Vec<String>,
}

/// A precondition no earlier step establishes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Gap {
    /// The step that needs it.
    pub step: String,
    /// The fact nobody established first.
    pub needs: String,
}

/// Every unmet precondition, in step order.
#[must_use]
pub fn audit(steps: &[Step]) -> Vec<Gap> {
    let mut established: Vec<&str> = Vec::new();
    let mut gaps = Vec::new();
    for step in steps {
        for need in &step.needs {
            if !established.contains(&need.as_str()) {
                gaps.push(Gap {
                    step: step.name.clone(),
                    needs: need.clone(),
                });
            }
        }
        // The step's own facts become available *after* its needs were read: a procedure is a
        // sequence, and a step that would have to provide its own precondition is a missing step.
        for fact in &step.provides {
            established.push(fact);
        }
    }
    gaps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(name: &str, needs: &[&str], provides: &[&str]) -> Step {
        Step {
            name: name.to_owned(),
            needs: needs.iter().map(|n| (*n).to_owned()).collect(),
            provides: provides.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    fn gap(step_name: &str, needs: &str) -> Gap {
        Gap {
            step: step_name.to_owned(),
            needs: needs.to_owned(),
        }
    }

    #[test]
    fn a_complete_runbook_has_no_gaps() {
        let runbook = [
            step("the alert fires", &[], &["database is down"]),
            step("find the backup", &[], &["backup path"]),
            step("restore", &["backup path", "database is down"], &["database running"]),
            step("verify", &["database running"], &[]),
        ];
        assert_eq!(audit(&runbook), []);
    }

    #[test]
    fn a_fact_nobody_provides_is_a_gap() {
        let runbook = [step("restore", &["backup path"], &[])];
        assert_eq!(audit(&runbook), [gap("restore", "backup path")]);
    }

    #[test]
    fn a_fact_provided_too_late_is_still_a_gap() {
        let runbook = [
            step("restore", &["backup path"], &[]),
            step("find the backup", &[], &["backup path"]),
        ];
        assert_eq!(audit(&runbook), [gap("restore", "backup path")]);
    }

    #[test]
    fn every_gap_is_reported_in_step_order() {
        let runbook = [
            step("first", &["a"], &[]),
            step("second", &["b"], &["c"]),
        ];
        assert_eq!(
            audit(&runbook),
            [gap("first", "a"), gap("second", "b")]
        );
    }

    #[test]
    fn a_step_cannot_provide_its_own_precondition() {
        let runbook = [step("restore", &["database running"], &["database running"])];
        assert_eq!(audit(&runbook), [gap("restore", "database running")]);
    }

    #[test]
    fn providing_the_same_fact_twice_is_harmless() {
        let runbook = [
            step("first", &[], &["database running"]),
            step("second", &["database running"], &["database running"]),
            step("third", &["database running"], &[]),
        ];
        assert_eq!(audit(&runbook), []);
    }
}
