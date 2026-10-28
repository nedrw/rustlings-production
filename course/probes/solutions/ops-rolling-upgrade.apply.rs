//! probe = "ops/rolling-upgrade@apply"
//! title = "Every running version has to fit the schema"
//! hint = "While the old and the new code are both deployed, the schema must satisfy both: add before the new code reads, drop only after the old code is gone."
//!
//! Step `relay/2.22-rolling-upgrade` changes a table while two versions of the service are
//! serving traffic. The order that works — expand, backfill, contract — is not a style: at every
//! moment, every deployed version's queries have to run against the schema that is actually there.
//!
//! The schema and the code are data, because a probe runs offline and cannot deploy anything. What
//! is re-checked is the check a reviewer does before a rollout: does each version that is still
//! running name only columns that exist?
//!
//! ## What you write
//!
//! [`audit`] — every mismatch, sorted:
//!
//! * a column a version **selects** and the schema does not have → a finding with `what =
//!   "selects"`;
//! * a column a version **writes** and the schema does not have → a finding with `what = "writes"`;
//! * a version that names only existing columns contributes nothing.
//!
//! ## What the tests ask
//!
//! The expand phase (old and new both fit); the new code writing a column nobody migrated yet; the
//! old code selecting a column that was already dropped; one version with both mistakes; and
//! findings arriving sorted.

/// The table as it exists right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    /// The columns that are there.
    pub columns: Vec<String>,
}

/// One deployed version of the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    /// `old` or `new`, for the finding.
    pub version: &'static str,
    /// Columns it reads.
    pub selects: Vec<String>,
    /// Columns it writes.
    pub writes: Vec<String>,
}

/// One version naming a column the schema does not have.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding {
    /// Which deployed version.
    pub version: &'static str,
    /// The column.
    pub column: String,
    /// `selects` or `writes`.
    pub what: &'static str,
}

/// Every mismatch between the schema and the versions still running, sorted.
#[must_use]
pub fn audit(schema: &Schema, codes: &[Code]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for code in codes {
        for (what, columns) in [("selects", &code.selects), ("writes", &code.writes)] {
            for column in columns {
                if !schema.columns.contains(column) {
                    findings.push(Finding {
                        version: code.version,
                        column: column.clone(),
                        what,
                    });
                }
            }
        }
    }
    findings.sort();
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(columns: &[&str]) -> Schema {
        Schema {
            columns: columns.iter().map(|c| (*c).to_owned()).collect(),
        }
    }

    fn code(version: &'static str, selects: &[&str], writes: &[&str]) -> Code {
        Code {
            version,
            selects: selects.iter().map(|c| (*c).to_owned()).collect(),
            writes: writes.iter().map(|c| (*c).to_owned()).collect(),
        }
    }

    #[test]
    fn the_expand_phase_fits_both_versions() {
        let schema = schema(&["id", "email", "status", "confirmed_at"]);
        let old = code("old", &["id", "email"], &["id", "email", "status"]);
        let new = code(
            "new",
            &["id", "email", "confirmed_at"],
            &["id", "email", "status"],
        );
        assert_eq!(audit(&schema, &[old, new]), []);
    }

    #[test]
    fn new_code_before_the_column_exists_is_a_mismatch() {
        let schema = schema(&["id", "email", "status"]);
        let new = code("new", &["id", "confirmed_at"], &[]);
        assert_eq!(
            audit(&schema, &[new]),
            [Finding {
                version: "new",
                column: "confirmed_at".to_owned(),
                what: "selects",
            }]
        );
    }

    #[test]
    fn old_code_after_the_column_was_dropped_is_a_mismatch() {
        let schema = schema(&["id", "email", "status"]);
        let old = code("old", &["id", "confirmed_at"], &[]);
        assert_eq!(
            audit(&schema, &[old]),
            [Finding {
                version: "old",
                column: "confirmed_at".to_owned(),
                what: "selects",
            }]
        );
    }

    #[test]
    fn a_write_to_a_missing_column_is_a_mismatch() {
        let schema = schema(&["id"]);
        let new = code("new", &[], &["id", "confirmed_at"]);
        assert_eq!(
            audit(&schema, &[new]),
            [Finding {
                version: "new",
                column: "confirmed_at".to_owned(),
                what: "writes",
            }]
        );
    }

    #[test]
    fn one_version_can_be_wrong_twice() {
        let schema = schema(&["id"]);
        let new = code("new", &["token"], &["token"]);
        let findings = audit(&schema, &[new]);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(findings.iter().all(|f| f.column == "token"));
    }

    #[test]
    fn findings_arrive_sorted() {
        let schema = schema(&["id"]);
        let old = code("old", &["b"], &[]);
        let new = code("new", &[], &["a"]);
        let findings = audit(&schema, &[new, old]);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(
            findings.windows(2).all(|pair| pair[0] <= pair[1]),
            "{findings:?}"
        );
    }
}
