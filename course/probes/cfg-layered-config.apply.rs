//! probe = "cfg/layered-config@apply"
//! title = "Four layers, one winner per setting"
//! hint = "Defaults < the file < the environment < the flags, only for the settings a layer mentions — and a key nobody knows is refused, not defaulted."
//!
//! Step `mailer/1.4-config` wrote down what "it works on my machine" means: a service reads its
//! settings from four places at once, and the order they win in *is* the contract. This probe
//! re-does that step as one function.
//!
//! The file layer arrives as text — a probe has no workspace to read `mailer.toml` from, and the
//! real service reads the same bytes with `std::fs::read_to_string` before handing them to this
//! function. Everything else is exactly the course's shape.
//!
//! ## The rules
//!
//! **The order is the contract**: defaults < the file < the environment < the flags. A layer
//! changes only the settings it mentions; a setting the environment does not mention keeps what
//! the file gave it.
//!
//! **The file** is one `key = value` per line. Blank lines and lines whose first non-blank
//! character is `#` are skipped. The value is trimmed and surrounding double quotes are removed.
//! A line that is neither — one with no `=`, or one with nothing before it — is
//! [`Error::BadFile`], naming its line (counting from 1).
//!
//! **The environment** is a list of `(name, value)` pairs — exactly what `std::env::vars()` hands
//! over. Only names starting with `MAILER__` are this service's business (a leftover `PATH` must
//! not stop it): drop the prefix, lower-case the rest, and turn `__` into `.`, so
//! `MAILER__LISTEN__PORT` is `listen.port`. A name with the prefix that maps to a key no setting
//! knows is [`Error::UnknownKey`], and it is refused *by its variable name* — silently ignoring a
//! typo is how a deployment ends up on port 8080 and nobody can reach it.
//!
//! **The flags** are [`Flags`], the layer a person types on purpose. `--listen-port` is
//! `listen.port`, and its origin is the flag.
//!
//! **A key no source knows is refused** ([`Error::UnknownKey`]), in the file and in the
//! environment alike — it is never rounded down to the default.
//!
//! **A value that cannot be used is refused** ([`Error::BadValue`]) with the key, the value, and
//! where it came from — and the `expected` field is the phrase from this table, because it is what
//! the person reading the failure acts on:
//!
//! | key | accepts | `expected` when it does not |
//! | --- | --- | --- |
//! | `listen.host` | any text | — |
//! | `listen.port` | 0 to 65535 | `a port number between 0 and 65535` |
//! | `log.level` | `trace`, `debug`, `info`, `warn` or `error` | `one of trace, debug, info, warn or error` |
//! | `log.format` | `pretty` or `json` | `one of pretty or json` |
//! | `database.url` | any text | — |
//!
//! **Secrets are shown, never printed**: [`Settings::report`] answers `<unset>` for
//! `database.url` when no layer set it and `<redacted>` when one did.
//!
//! ## What you write
//!
//! [`load`] and [`Settings::report`]. The tests are the contract; the four of them cover the
//! order, the unknown key, the unusable value, and the report.

use std::fmt;

/// The settings this service runs on: the five a deployment can change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// `listen.host`: what the service binds to.
    pub listen_host: String,
    /// `listen.port`: the port it binds to.
    pub listen_port: u16,
    /// `log.level`: how much it logs.
    pub log_level: String,
    /// `log.format`: `pretty` for a person, `json` for a collector.
    pub log_format: String,
    /// `database.url`: a secret, so a setting with no safe default.
    pub database_url: Option<String>,
}

impl Settings {
    /// The defaults: the values a service starts with when no layer says anything.
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            listen_host: "127.0.0.1".to_owned(),
            listen_port: 8080,
            log_level: "info".to_owned(),
            log_format: "pretty".to_owned(),
            database_url: None,
        }
    }

    /// The `--check-config` report: one `key = value` line per setting, sorted by key.
    ///
    /// The secret is the reason this is a report rather than a `Debug` print: it is what people
    /// paste into issues, so a set URL is shown as `<redacted>` and an unset one as `<unset>`.
    #[must_use]
    pub fn report(&self) -> Vec<String> {
        todo!()
    }
}

/// The flags of the layer a person types on purpose.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flags {
    /// `--listen-port <port>`, as it was typed: the value is parsed by [`load`], so a port that
    /// cannot be used is refused like any other unusable value.
    pub listen_port: Option<String>,
}

/// Where a refused key or value was mentioned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// The configuration file.
    File,
    /// An environment variable, by its name.
    Env(String),
    /// A flag, by its name.
    Flag(&'static str),
}

/// A configuration the service cannot start with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A key no layer of this service knows.
    UnknownKey { key: String, origin: Origin },
    /// A value the setting cannot use.
    BadValue {
        key: String,
        value: String,
        expected: &'static str,
        origin: Origin,
    },
    /// A line of the file that is not blank, not a comment, and not `key = value`.
    BadFile { line: usize },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKey { key, origin } => {
                write!(
                    f,
                    "`{key}` is not a setting this service knows ({origin:?})"
                )
            }
            Self::BadValue { key, value, .. } => write!(f, "`{key}` cannot be `{value}`"),
            Self::BadFile { line } => write!(f, "line {line} is not `key = value`"),
        }
    }
}

/// Load the four layers, in the order they win.
///
/// # Errors
///
/// [`Error::UnknownKey`] for a key or variable name no setting knows, [`Error::BadValue`] for a
/// value the setting cannot use, and [`Error::BadFile`] for a file line that is not a setting.
pub fn load(file: &str, env: &[(String, String)], flags: &Flags) -> Result<Settings, Error> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file that mentions three settings, one of them a secret.
    const FILE: &str = "\
# the file layer: only the settings it mentions change\n\nlisten.host = \"0.0.0.0\"\nlisten.port = 9000\nlog.level = debug\ndatabase.url = \"postgres://mailer:from-the-file@db:5432/mailer\"\n";

    /// The environment a deployment sets, with one variable this service does not know left in it.
    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn loaded(file: &str, environment: &[(&str, &str)], flags: &Flags) -> Settings {
        load(file, &env(environment), flags).unwrap_or_else(|error| panic!("{error}"))
    }

    fn refused(file: &str, environment: &[(&str, &str)], flags: &Flags) -> Error {
        load(file, &env(environment), flags)
            .err()
            .expect("this configuration has to be refused, not loaded")
    }

    #[test]
    fn each_layer_overrides_the_one_before_it() {
        // The defaults, with nothing anywhere.
        let defaults = loaded("", &[], &Flags::default());
        assert_eq!(
            defaults,
            Settings::defaults(),
            "an empty file changes nothing"
        );
        assert_eq!(defaults.listen_host, "127.0.0.1");
        assert_eq!(defaults.listen_port, 8080);
        assert_eq!(defaults.log_level, "info");
        assert_eq!(
            defaults.log_format, "pretty",
            "a file may mention one setting and leave the rest alone"
        );
        assert_eq!(defaults.database_url, None, "a secret has no default");

        // The file wins over the defaults, and only for the keys it mentions.
        let from_file = loaded(FILE, &[], &Flags::default());
        assert_eq!(from_file.listen_host, "0.0.0.0");
        assert_eq!(from_file.listen_port, 9000);
        assert_eq!(from_file.log_level, "debug");
        assert_eq!(from_file.log_format, "pretty");

        // The environment wins over the file, and only for the keys it mentions.
        let from_env = loaded(
            FILE,
            &[
                ("MAILER__LISTEN__PORT", "9111"),
                ("MAILER__LOG__LEVEL", "warn"),
            ],
            &Flags::default(),
        );
        assert_eq!(from_env.listen_port, 9111);
        assert_eq!(from_env.log_level, "warn");
        assert_eq!(
            from_env.listen_host, "0.0.0.0",
            "a setting the environment does not mention still comes from the file"
        );

        // The flag wins over everything: it is the layer somebody typed on purpose.
        let from_flag = loaded(
            FILE,
            &[
                ("MAILER__LISTEN__PORT", "9111"),
                ("MAILER__LOG__LEVEL", "warn"),
            ],
            &Flags {
                listen_port: Some("9222".to_owned()),
            },
        );
        assert_eq!(from_flag.listen_port, 9222);
        assert_eq!(from_flag.log_level, "warn");
        assert_eq!(from_flag.listen_host, "0.0.0.0");
    }

    #[test]
    fn a_key_no_layer_knows_is_refused_instead_of_defaulted() {
        // A typo in a variable name: the service must not start on port 8080 and leave the
        // deployment wondering why nobody can reach it.
        let variable = refused("", &[("MAILER__LISTEN__PROT", "9111")], &Flags::default());
        assert_eq!(
            variable,
            Error::UnknownKey {
                key: "listen.prot".to_owned(),
                origin: Origin::Env("MAILER__LISTEN__PROT".to_owned()),
            },
            "an unknown variable is refused by name, not ignored: {variable:?}"
        );

        // The same typo in the file.
        let key = refused("listen.prot = 9111\n", &[], &Flags::default());
        assert_eq!(
            key,
            Error::UnknownKey {
                key: "listen.prot".to_owned(),
                origin: Origin::File,
            },
            "a key the file mentions and no setting knows is an error: {key:?}"
        );

        // The rest of the environment is other programmes' business, not this service's.
        let leftover = loaded(
            "",
            &[("PATH", "/usr/bin"), ("MAILER__LISTEN__PORT", "9111")],
            &Flags::default(),
        );
        assert_eq!(
            leftover.listen_port, 9111,
            "only `MAILER__` is ours to read"
        );
    }

    #[test]
    fn a_value_that_cannot_be_used_names_the_key_the_value_and_where_it_came_from() {
        // The file's origin is the file, the variable's is the variable, and the flag's is the flag.
        let from_file = refused("listen.port = 99999\n", &[], &Flags::default());
        assert_eq!(
            from_file,
            Error::BadValue {
                key: "listen.port".to_owned(),
                value: "99999".to_owned(),
                expected: "a port number between 0 and 65535",
                origin: Origin::File,
            },
            "{from_file:?}"
        );

        let from_env = refused("", &[("MAILER__LOG__FORMAT", "xml")], &Flags::default());
        assert_eq!(
            from_env,
            Error::BadValue {
                key: "log.format".to_owned(),
                value: "xml".to_owned(),
                expected: "one of pretty or json",
                origin: Origin::Env("MAILER__LOG__FORMAT".to_owned()),
            },
            "{from_env:?}"
        );

        let from_flag = refused(
            "",
            &[],
            &Flags {
                listen_port: Some("70000".to_owned()),
            },
        );
        assert_eq!(
            from_flag,
            Error::BadValue {
                key: "listen.port".to_owned(),
                value: "70000".to_owned(),
                expected: "a port number between 0 and 65535",
                origin: Origin::Flag("--listen-port"),
            },
            "the same value refused from a different layer says so: {from_flag:?}"
        );

        // A file line that is neither blank, nor a comment, nor a setting: the parser has to say
        // where it stopped, because that is the line somebody has to open.
        let no_key = refused("log.level = info\n\n= 9000\n", &[], &Flags::default());
        assert_eq!(no_key, Error::BadFile { line: 3 }, "{no_key:?}");

        let no_equals = refused(
            "log.level = info\n\nlog level info\n",
            &[],
            &Flags::default(),
        );
        assert_eq!(no_equals, Error::BadFile { line: 3 }, "{no_equals:?}");
    }

    #[test]
    fn the_report_says_whether_a_secret_is_set_without_printing_it() {
        let from_file = loaded(FILE, &[], &Flags::default());
        assert_eq!(
            from_file.report(),
            vec![
                "database.url = <redacted>".to_owned(),
                "listen.host = 0.0.0.0".to_owned(),
                "listen.port = 9000".to_owned(),
                "log.format = pretty".to_owned(),
                "log.level = debug".to_owned(),
            ],
            "one `key = value` line per setting, sorted by key"
        );
        assert!(
            from_file
                .report()
                .iter()
                .all(|line| !line.contains("from-the-file")),
            "a report is what people paste into issues: the URL must not survive into it, whichever \
             layer set it"
        );

        let unset = Settings::defaults();
        assert_eq!(
            unset.report(),
            vec![
                "database.url = <unset>".to_owned(),
                "listen.host = 127.0.0.1".to_owned(),
                "listen.port = 8080".to_owned(),
                "log.format = pretty".to_owned(),
                "log.level = info".to_owned(),
            ],
            "a secret nobody set is `<unset>`, which is not the same answer as a set one"
        );
    }
}
