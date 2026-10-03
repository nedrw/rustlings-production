//! Spec for `mailer/1.4-config`: where a setting's value comes from, and what happens when it
//! cannot be used.
//!
//! `kiln run` copies this file into the project as `tests/spec_config.rs`; it is the course's file,
//! not the learner's (plan §3 D2). Everything here goes through the built binary, because the
//! contract of this step *is* the process's behaviour: which file it reads, which variables it
//! answers to, what `--check-config` prints and how it refuses to start.
//!
//! Every run starts from an empty environment ([`run`]), so a `MAILER__` variable left over in the
//! shell that starts the tests cannot change the answer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

/// A directory the service reads its configuration from, removed when the test ends.
struct Dir(PathBuf);

impl Dir {
    /// A fresh directory, named after the test that asked for it.
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mailer-config-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("{} is creatable: {error}", path.display()));
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Write a file below the directory and answer where it landed.
    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("{} is creatable: {error}", parent.display()));
        }
        std::fs::write(&path, contents)
            .unwrap_or_else(|error| panic!("{} is writable: {error}", path.display()));
        path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run the service in `dir`, with nothing in the environment but what the test passes.
fn run(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mailer"));
    command.args(args).current_dir(dir).env_clear();
    for (name, value) in env {
        command.env(name, value);
    }
    command
        .output()
        .unwrap_or_else(|error| panic!("the mailer binary could not be started: {error}"))
}

/// `--check-config` in `dir`, with the given environment.
fn check_config(dir: &Path, env: &[(&str, &str)]) -> Output {
    run(dir, &["--check-config"], env)
}

/// Everything the process said, whichever stream it said it on, as one string.
fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The `key = value` lines of a successful `--check-config`.
fn settings(output: &Output) -> BTreeMap<String, String> {
    assert!(
        output.status.success(),
        "`--check-config` must succeed here, it exited {:?}: {}",
        output.status.code(),
        said(output)
    );
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let mut settings = BTreeMap::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let (key, value) = line.split_once('=').unwrap_or_else(|| {
            panic!(
                "`--check-config` prints one `key = value` line per setting, this line is not one: \
                 `{line}`"
            )
        });
        settings.insert(key.trim().to_owned(), value.trim().to_owned());
    }
    assert!(
        !settings.is_empty(),
        "`--check-config` printed no settings at all: {}",
        said(output)
    );
    settings
}

/// One setting of a successful `--check-config`.
fn setting(output: &Output, key: &str) -> String {
    let settings = settings(output);
    settings
        .get(key)
        .unwrap_or_else(|| {
            panic!(
                "`--check-config` has no line for `{key}`, it printed {:?}",
                settings.keys().collect::<Vec<_>>()
            )
        })
        .clone()
}

/// A configuration that must be refused before anything starts.
fn refuse(output: &Output, expected: &[&str]) {
    assert!(
        !output.status.success(),
        "a configuration the service cannot use must exit non-zero, this one exited {:?}: {}",
        output.status.code(),
        said(output)
    );
    let text = said(output);
    assert!(
        !text.contains("panicked"),
        "the service has to report a bad setting, not panic: {text}"
    );
    for fragment in expected {
        assert!(
            text.contains(fragment),
            "the report must mention {fragment:?}, it said: {text}"
        );
    }
}

/// A file that mentions every source-able setting, so one run can show which layer won.
const FILE: &str = "\
[listen]
host = \"0.0.0.0\"
port = 9000

[log]
level = \"debug\"

[database]
url = \"postgres://mailer:from-the-file@db:5432/mailer\"
";

#[test]
fn the_defaults_are_what_the_service_documents() {
    let dir = Dir::new("defaults");
    let output = check_config(dir.path(), &[]);
    assert_eq!(setting(&output, "listen.host"), "127.0.0.1");
    assert_eq!(setting(&output, "listen.port"), "8080");
    assert_eq!(setting(&output, "log.level"), "info");
    assert_eq!(setting(&output, "log.format"), "pretty");
    assert_eq!(
        setting(&output, "database.url"),
        "<unset>",
        "a secret has no default, so a service without one must say so instead of guessing"
    );
}

#[test]
fn each_source_overrides_the_one_before_it() {
    let dir = Dir::new("layers");
    dir.write("mailer.toml", FILE);

    // The file, read from the working directory.
    let from_file = check_config(dir.path(), &[]);
    assert_eq!(setting(&from_file, "listen.host"), "0.0.0.0");
    assert_eq!(setting(&from_file, "listen.port"), "9000");
    assert_eq!(setting(&from_file, "log.level"), "debug");
    assert_eq!(
        setting(&from_file, "log.format"),
        "pretty",
        "a file may mention one setting and leave the rest at their defaults"
    );

    // The environment wins over the file, and only for the settings it mentions.
    let from_env = check_config(
        dir.path(),
        &[
            ("MAILER__LISTEN__PORT", "9111"),
            ("MAILER__LOG__LEVEL", "warn"),
        ],
    );
    assert_eq!(setting(&from_env, "listen.port"), "9111");
    assert_eq!(setting(&from_env, "log.level"), "warn");
    assert_eq!(
        setting(&from_env, "listen.host"),
        "0.0.0.0",
        "a setting the environment does not mention still comes from the file"
    );

    // The flag wins over everything, because it is the layer a person types on purpose.
    let from_flag = run(
        dir.path(),
        &["--check-config", "--listen-port", "9222"],
        &[
            ("MAILER__LISTEN__PORT", "9111"),
            ("MAILER__LOG__LEVEL", "warn"),
        ],
    );
    assert_eq!(setting(&from_flag, "listen.port"), "9222");
    assert_eq!(setting(&from_flag, "log.level"), "warn");

    // `--config` moves the file layer somewhere else; the default file is then not read at all.
    dir.write("conf/other.toml", "[listen]\nport = 9500\n");
    let elsewhere = run(
        dir.path(),
        &["--check-config", "--config", "conf/other.toml"],
        &[],
    );
    assert_eq!(setting(&elsewhere, "listen.port"), "9500");
    assert_eq!(
        setting(&elsewhere, "listen.host"),
        "127.0.0.1",
        "the file named on the command line is the file layer, not an addition to the default one"
    );
}

#[test]
fn a_setting_nobody_recognises_is_refused_instead_of_defaulted() {
    let dir = Dir::new("unknown");

    // A typo in a variable name: the service must not start on port 8080 and leave the deployment
    // wondering why nobody can reach it.
    let unknown_variable = check_config(dir.path(), &[("MAILER__LISTEN__PROT", "9111")]);
    refuse(&unknown_variable, &["MAILER__LISTEN__PROT"]);

    // The same typo in the file.
    dir.write("mailer.toml", "[listen]\nprot = 9111\n");
    let unknown_key = check_config(dir.path(), &[]);
    refuse(&unknown_key, &["prot", "mailer.toml"]);
}

#[test]
fn a_value_the_service_cannot_use_stops_it_before_it_starts() {
    let dir = Dir::new("values");

    let bad_format = check_config(dir.path(), &[("MAILER__LOG__FORMAT", "xml")]);
    refuse(&bad_format, &["log.format", "xml"]);

    let bad_port = check_config(dir.path(), &[("MAILER__LISTEN__PORT", "99999")]);
    refuse(&bad_port, &["listen.port", "99999"]);

    // A file that cannot be parsed, and one that is not there at all: both are refused, and both
    // say which file.
    dir.write("bad.toml", "[listen]\nport = \"nope\"\n");
    let unparseable = run(dir.path(), &["--check-config", "--config", "bad.toml"], &[]);
    refuse(&unparseable, &["bad.toml"]);

    // A file named on the command line has to exist: falling back to the defaults is how a
    // deployment starts with settings nobody chose.
    let missing = run(
        dir.path(),
        &["--check-config", "--config", "absent.toml"],
        &[],
    );
    refuse(&missing, &["absent.toml"]);

    // And the service itself refuses, not only `--check-config`: the configuration is loaded
    // before anything else happens.
    let service = run(dir.path(), &[], &[("MAILER__LOG__FORMAT", "xml")]);
    refuse(&service, &["log.format"]);
}

#[test]
fn the_database_url_is_shown_as_set_but_never_printed() {
    let dir = Dir::new("secret");
    const SECRET: &str = "sup3r-secret";

    let from_env = check_config(
        dir.path(),
        &[(
            "MAILER__DATABASE__URL",
            "postgres://mailer:sup3r-secret@localhost/mailer",
        )],
    );
    assert_eq!(setting(&from_env, "database.url"), "<redacted>");
    assert!(
        !said(&from_env).contains(SECRET),
        "a configuration report is what people paste into issues; the URL must not be in it: {}",
        said(&from_env)
    );

    // The file layer is redacted too: where the value came from is not what makes it a secret.
    dir.write("mailer.toml", FILE);
    let from_file = check_config(dir.path(), &[]);
    assert_eq!(setting(&from_file, "database.url"), "<redacted>");
    assert!(
        !said(&from_file).contains("from-the-file"),
        "the file layer must be redacted as well: {}",
        said(&from_file)
    );
}
