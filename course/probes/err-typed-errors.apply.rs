//! probe = "err/typed-errors@apply"
//! title = "A failure a caller can branch on"
//! hint = "A message for the person, a cause for the caller behind it, and two exit codes a script can branch on."
//!
//! Step `mailer/1.5-errors` made the failure type part of the API: every variant says what went
//! wrong, the two configuration failures keep the error behind them walkable through `source()`,
//! and every variant answers an exit code so a deploy script can tell "the caller asked for
//! something this build does not have" from "the deployment could not read its own settings".
//!
//! This probe re-does that step against the same variants. The enum is given — it is what a caller
//! matches on, and a probe that lets you rename it would not be checking an API. What you write is
//! everything that makes it usable:
//!
//! 1. `Display for Error` — one sentence per variant. These fragments have to be in the message,
//!    because they are what the person reading it acts on:
//!
//!    | variant | the message has to mention |
//!    | --- | --- |
//!    | `UnknownArgument { argument }` | the argument |
//!    | `MissingValue { flag, expected }` | the flag, and what it wanted |
//!    | `InvalidSetting { key, value, origin, .. }` | the key, the value, and where it came from |
//!    | `UnknownSetting { name }` | the name |
//!    | `UnreadableConfig { path, .. }` | the path |
//!    | `InvalidConfig { path, .. }` | the path |
//!
//! 2. `Display for SettingSource` — where a value came from, as a phrase a sentence can use:
//!    the file, the environment variable by name, the flag by name.
//! 3. `Display for ParseError` — the line the parser stopped on and what it wanted there.
//! 4. `Error::source` — `Some` for exactly the two variants that carry a cause.
//! 5. `Error::exit_code` — `2` when the caller can fix it (an unknown argument, a missing value, a
//!    setting value that cannot be used, an unknown setting), `1` when only the deployment can (a
//!    file that cannot be read, a file that cannot be parsed).
//!
//! Two things the tests will not let you get away with, because they are the two ways this shape
//! rots:
//!
//! * **flattening the cause into the message.** `source: None` and "cannot read `mailer.toml`: No
//!   such file or directory" in one string prints almost the same sentence and leaves a caller
//!   nothing to walk;
//! * **one exit code for everything.** A script that cannot tell a typo from an unreadable file
//!   either retries forever or pages somebody at three in the morning.
//!
//! The whole exercise is standard library only: `std::fmt`, `std::io`, `std::error::Error`.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// Where a setting's value came from: the part of a failure a person has to edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingSource {
    /// A value from the configuration file.
    File(PathBuf),
    /// A value from an environment variable, by its name.
    Env(String),
    /// A value from a flag somebody typed, by its name.
    Flag(&'static str),
}

/// A file the parser refused, and where it stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// The line the parser stopped on, counting from 1.
    pub line: usize,
    /// What the parser wanted there.
    pub expected: String,
}

/// Every way this service can fail.
///
/// The variants are the API: a spec, a second binary and the HTTP layer all match on them, so the
/// names and the fields are not yours to reshape. Adding a variant is a decision; folding two kinds
/// of failure into one loses the distinction the exit-code table exists for.
#[derive(Debug)]
pub enum Error {
    /// An argument this build does not have.
    UnknownArgument { argument: String },
    /// A flag that was given without its value.
    MissingValue {
        flag: &'static str,
        expected: &'static str,
    },
    /// A setting whose value this service cannot use.
    InvalidSetting {
        key: String,
        value: String,
        expected: &'static str,
        origin: SettingSource,
    },
    /// A setting no layer of this service knows.
    UnknownSetting { name: String },
    /// The configuration file could not be read. The `io::Error` is the cause.
    UnreadableConfig { path: PathBuf, source: io::Error },
    /// The configuration file could not be parsed. The [`ParseError`] is the cause.
    InvalidConfig { path: PathBuf, source: ParseError },
}

impl Error {
    /// The code the process exits with, and the reason there is a table at all: the shell is a
    /// caller too.
    ///
    /// `2` is the kind the caller fixes — they asked for something this service does not have, so
    /// retrying will not help. `1` is the kind the deployment fixes — the configuration could not
    /// be read or parsed, which is worth another attempt after somebody looks at it.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        todo!()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl fmt::Display for SettingSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl std::error::Error for Error {
    /// The failure behind this one, for the two variants that have one.
    ///
    /// "Where the value came from" is not a cause: `InvalidSetting` names a layer a person edits,
    /// and returning it here would make every caller treat a sentence as a walkable error.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        todo!()
    }
}

impl std::error::Error for ParseError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    /// A failure and every cause behind it, in the words a caller reads.
    fn describe(failure: &dyn std::error::Error) -> Vec<String> {
        let mut links = vec![failure.to_string()];
        let mut next = failure.source();
        while let Some(cause) = next {
            links.push(cause.to_string());
            next = cause.source();
        }
        links
    }

    #[test]
    fn every_failure_names_the_thing_a_person_has_to_fix() {
        let cases: Vec<(Error, Vec<&str>)> = vec![
            (
                Error::UnknownArgument {
                    argument: "--nope".to_owned(),
                },
                vec!["--nope"],
            ),
            (
                Error::MissingValue {
                    flag: "--config",
                    expected: "the path of a file",
                },
                vec!["--config", "the path of a file"],
            ),
            (
                Error::InvalidSetting {
                    key: "listen.port".to_owned(),
                    value: "70000".to_owned(),
                    expected: "a port number between 0 and 65535",
                    origin: SettingSource::Flag("--listen-port"),
                },
                vec!["listen.port", "70000", "--listen-port"],
            ),
            (
                Error::InvalidSetting {
                    key: "log.format".to_owned(),
                    value: "xml".to_owned(),
                    expected: "one of pretty or json",
                    origin: SettingSource::Env("MAILER__LOG__FORMAT".to_owned()),
                },
                vec!["log.format", "xml", "MAILER__LOG__FORMAT"],
            ),
            (
                Error::UnknownSetting {
                    name: "MAILER__LISTEN__PROT".to_owned(),
                },
                vec!["MAILER__LISTEN__PROT"],
            ),
            (
                Error::UnreadableConfig {
                    path: "absent.toml".into(),
                    source: io::Error::new(io::ErrorKind::NotFound, "No such file or directory"),
                },
                vec!["absent.toml"],
            ),
            (
                Error::InvalidConfig {
                    path: "unparseable.toml".into(),
                    source: ParseError {
                        line: 2,
                        expected: "a port number".to_owned(),
                    },
                },
                vec!["unparseable.toml"],
            ),
        ];

        for (failure, fragments) in &cases {
            let message = failure.to_string();
            assert!(
                !message.is_empty(),
                "every variant has one sentence for a human: {failure:?} prints nothing"
            );
            for fragment in fragments {
                assert!(
                    message.contains(fragment),
                    "`{message}` has to mention `{fragment}`: a failure that does not name what is \
                     wrong is a failure somebody diagnoses by guessing"
                );
            }
        }
    }

    #[test]
    fn where_a_value_came_from_is_a_phrase_of_its_own() {
        // The same sentence has to be able to say which layer to edit, so the origin prints itself.
        let origins = [
            (SettingSource::File("mailer.toml".into()), "mailer.toml"),
            (
                SettingSource::Env("MAILER__LOG__FORMAT".to_owned()),
                "MAILER__LOG__FORMAT",
            ),
            (SettingSource::Flag("--listen-port"), "--listen-port"),
        ];
        for (origin, fragment) in &origins {
            let phrase = origin.to_string();
            assert!(
                phrase.contains(fragment),
                "`{phrase}` has to name `{fragment}`: a bad value is refused with the layer that \
                 has to change"
            );
        }
    }

    #[test]
    fn the_cause_of_a_failure_stays_reachable_through_source() {
        let missing = Error::UnreadableConfig {
            path: "absent.toml".into(),
            source: io::Error::new(io::ErrorKind::NotFound, "No such file or directory"),
        };
        let chain = describe(&missing);
        assert_eq!(
            chain.len(),
            2,
            "the failure and what actually happened are two links, not one flattened sentence: \
             {chain:?}"
        );
        assert_eq!(
            chain[1], "No such file or directory",
            "the operating system's reason has to survive to the reader: {chain:?}"
        );

        let unparseable = Error::InvalidConfig {
            path: "unparseable.toml".into(),
            source: ParseError {
                line: 2,
                expected: "a port number".to_owned(),
            },
        };
        let chain = describe(&unparseable);
        assert!(
            chain.iter().any(|link| link.contains("line 2")),
            "where the parser stopped is what makes the failure fixable, so it has to survive: \
             {chain:?}"
        );
        assert_eq!(
            chain.len(),
            2,
            "the parser's refusal is the last link: nothing behind it: {chain:?}"
        );
    }

    #[test]
    fn the_two_kinds_of_failure_do_not_share_an_exit_code() {
        // What the caller can fix: code 2. A script that sees it knows not to retry.
        let caller_faults = [
            Error::UnknownArgument {
                argument: "--nope".to_owned(),
            },
            Error::MissingValue {
                flag: "--config",
                expected: "the path of a file",
            },
            Error::InvalidSetting {
                key: "listen.port".to_owned(),
                value: "70000".to_owned(),
                expected: "a port number between 0 and 65535",
                origin: SettingSource::Flag("--listen-port"),
            },
            Error::UnknownSetting {
                name: "MAILER__LISTEN__PROT".to_owned(),
            },
        ];
        for failure in &caller_faults {
            assert_eq!(
                failure.exit_code(),
                2,
                "`{failure}` is the caller's to fix: {failure:?}"
            );
        }

        // What only the deployment can fix: code 1.
        let deployment_faults = [
            Error::UnreadableConfig {
                path: "absent.toml".into(),
                source: io::Error::new(io::ErrorKind::NotFound, "gone"),
            },
            Error::InvalidConfig {
                path: "unparseable.toml".into(),
                source: ParseError {
                    line: 2,
                    expected: "a port number".to_owned(),
                },
            },
        ];
        for failure in &deployment_faults {
            assert_eq!(
                failure.exit_code(),
                1,
                "`{failure}` is the deployment's to fix: {failure:?}"
            );
        }

        let mut codes: Vec<u8> = caller_faults
            .iter()
            .chain(&deployment_faults)
            .map(Error::exit_code)
            .collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(
            codes,
            [1, 2],
            "the two kinds of failure must not share a code, or a deploy script cannot tell a typo \
             from an unreadable file"
        );
    }

    #[test]
    fn only_a_failure_that_carries_a_cause_has_one() {
        // `source()` is for the failure behind this one — the `io::Error`, the parser's refusal.
        // A chain that mixes in "where the value came from" makes every caller treat a log line as
        // a walkable cause.
        let bad_value = Error::InvalidSetting {
            key: "listen.port".to_owned(),
            value: "70000".to_owned(),
            expected: "a port number between 0 and 65535",
            origin: SettingSource::File("mailer.toml".into()),
        };
        assert!(
            bad_value.source().is_none(),
            "`{bad_value}` names a layer somebody edits; it is not an error behind this one"
        );
        assert!(
            Error::UnknownArgument {
                argument: "--nope".to_owned()
            }
            .source()
            .is_none(),
            "a variant with no cause behind it answers `None`"
        );
    }
}
