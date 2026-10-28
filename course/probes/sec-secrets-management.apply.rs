//! probe = "sec/secrets-management@apply"
//! title = "A secret that reaches the log has leaked"
//! hint = "Redact by value, longest first, and never let an empty entry match everything — the last line of defence is the one that runs on every line."
//!
//! Step `mailer/1.4-config` separates a value from the way it is spelled: the database URL comes
//! from `MAILER__DATABASE__URL` or from `mailer.toml`, and the same string must never appear in a
//! log line, an error message or a `Debug` output. The structural defences (a typed wrapper that
//! prints `***`) are the first; this probe re-checks the last one, which runs over text that was
//! written by somebody who did not know a value was secret.
//!
//! ## What you write
//!
//! [`redact`] — replace every occurrence of every secret with `[REDACTED]`:
//!
//! * matching is literal and case-sensitive: a secret is a byte string, not a pattern;
//! * **longest first**, so that `hunter2!` is not left as `[REDACTED]!` because `hunter2` was
//!   replaced inside it;
//! * **empty entries are ignored**. An empty secret that "matches everywhere" would erase the line
//!   it was supposed to protect, which is worse than the leak;
//! * a line with no secret is returned unchanged.
//!
//! ## What the tests ask
//!
//! One secret; every occurrence; a password inside a connection string; an empty entry in the list;
//! a short secret that is a prefix of a longer one; no secrets at all; and a differently-cased
//! password, which must survive because passwords are case-sensitive.

/// Replace every occurrence of every secret in `line` with `[REDACTED]`.
#[must_use]
pub fn redact(line: &str, secrets: &[&str]) -> String {
    todo!("filter the empty entries, sort longest first, replace every occurrence")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_secret_is_replaced_and_the_rest_of_the_line_survives() {
        let line = "connecting as postgres with password=hunter2";
        assert_eq!(
            redact(line, &["hunter2"]),
            "connecting as postgres with password=[REDACTED]"
        );
    }

    #[test]
    fn every_occurrence_is_replaced() {
        let line = "hunter2 and again hunter2";
        assert_eq!(
            redact(line, &["hunter2"]),
            "[REDACTED] and again [REDACTED]"
        );
    }

    #[test]
    fn a_secret_inside_a_connection_string_is_replaced() {
        let line = "DATABASE__URL=postgres://postgres:hunter2@127.0.0.1:5432/mailer";
        assert_eq!(
            redact(line, &["postgres:hunter2"]),
            "DATABASE__URL=postgres://[REDACTED]@127.0.0.1:5432/mailer"
        );
    }

    #[test]
    fn an_empty_entry_does_not_erase_the_line() {
        let line = "everything here stays";
        assert_eq!(redact(line, &["", "hunter2"]), line);
    }

    #[test]
    fn the_longest_secret_is_replaced_first() {
        // With `hunter2` first, `hunter2!` would become `[REDACTED]!` — the marker inside a value
        // that is itself still half-leaked.
        let line = "pw=hunter2!";
        assert_eq!(redact(line, &["hunter2", "hunter2!"]), "pw=[REDACTED]");
    }

    #[test]
    fn no_secrets_means_the_line_is_untouched() {
        let line = "nothing to hide";
        assert_eq!(redact(line, &[]), line);
    }

    #[test]
    fn matching_is_case_sensitive() {
        let line = "pw=Hunter2";
        assert_eq!(redact(line, &["hunter2"]), line);
    }
}
