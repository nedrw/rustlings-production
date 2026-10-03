//! Spec for `mailer/1.10-domain-types`: the only way to make an `Email`, a `SubscriberName` or a
//! `Password` is to parse one, and what parsing does is what this file judges.
//!
//! `kiln run` copies this file into the project as `tests/spec_domain_types.rs`; it is the course's
//! file, not the learner's (plan §3 D2). The assertions live in a **probe crate** this spec writes,
//! compiles and runs: the domain types are library API, and this file has to *fail* — not fail to
//! compile — on a tree where `lib.rs` does not declare them yet, so it cannot name them itself. The
//! probe depends on the project by path, which is the same trick step `mailer/1.5-errors` uses for
//! the error enum: a second crate is the only honest way to ask "can somebody outside match on
//! this?".
//!
//! What the probe asserts, in one breath: accepted values are normalised and parsing them again is a
//! no-op; every refused sample is refused *by name* (the field, and what it expected) with exit code
//! `2` and without repeating the value; a password survives as it was typed, never appears in a
//! message and never comes out of `{:?}`; a generated set of addresses all round-trip while their
//! mutations are all refused. The generated set is written by hand on purpose — the course's specs
//! have no dependencies, and `proptest` belongs to T4·4.13, where properties become the subject
//! instead of a tool.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

/// The crate the probe is written into: one package, one dependency on the project by path.
const PROBE_MANIFEST: &str = r#"
[package]
name = "mailer-domain-probe"
version = "0.0.0"
edition = "2024"

[dependencies]
mailer = { path = "PROJECT" }

[workspace]
"#;

/// The probe's library target: the tests live in `tests/`. Nothing here prints, and nothing here is
/// a test — the whole point is that the assertions run *against the learner's crate*, not this one.
const PROBE_LIBRARY: &str =
    "//! The domain types, judged from outside the crate that defines them.\n";

/// The probe's tests: the domain contract, executed against the project by path.
const PROBE_TESTS: &str = r##"
//! The domain types, judged from outside the crate that defines them.

use std::process::ExitCode;

use mailer::domain::{
    Email, MAX_EMAIL_LENGTH, MAX_EMAIL_LOCAL_LENGTH, MAX_NAME_LENGTH, MAX_PASSWORD_LENGTH,
    MIN_PASSWORD_LENGTH, Password, SubscriberName,
};

/// One value a parser has to accept, and what it has to become.
struct Accepted<'a> {
    /// What a caller sent.
    input: &'a str,
    /// What the type has to hold afterwards.
    stored: &'a str,
}

/// One value a parser has to refuse, and what a message about it has to say.
struct Refused<'a> {
    /// What a caller sent.
    input: &'a str,
    /// The field the message has to name.
    field: &'static str,
    /// A fragment of what the message has to say the value should have been.
    expected: &'static str,
}

/// Parse an email and complain in the course's words when it is refused.
fn email(input: &str) -> Email {
    Email::parse(input).unwrap_or_else(|error| panic!("`{input}` has to be accepted: {error}"))
}

/// Parse a name and complain in the course's words when it is refused.
fn name(input: &str) -> SubscriberName {
    SubscriberName::parse(input)
        .unwrap_or_else(|error| panic!("`{input}` has to be accepted: {error}"))
}

/// Parse a password and complain in the course's words when it is refused.
fn password(input: &str) -> Password {
    Password::parse(input).unwrap_or_else(|error| panic!("a password has to be accepted: {error}"))
}

#[test]
fn the_limits_are_the_ones_the_course_documents() {
    // The boundaries are part of the contract, so they are checked where they are defined: a
    // service that accepts a 300-byte address is a service that will meet a 254-byte limit in a
    // database or a mail server later, at the worst possible moment.
    assert_eq!(MAX_EMAIL_LENGTH, 254, "the email limit changed");
    assert_eq!(MAX_EMAIL_LOCAL_LENGTH, 64, "the local-part limit changed");
    assert_eq!(MAX_NAME_LENGTH, 100, "the name limit changed");
    assert_eq!(MIN_PASSWORD_LENGTH, 12, "the password minimum changed");
    assert_eq!(MAX_PASSWORD_LENGTH, 128, "the password maximum changed");
}

#[test]
fn an_accepted_value_is_normalised_and_parsing_it_again_is_a_no_op() {
    let addresses = [
        Accepted {
            input: "Ada@Example.COM",
            stored: "ada@example.com",
        },
        Accepted {
            input: "  grace@example.org  ",
            stored: "grace@example.org",
        },
        Accepted {
            input: "ada+newsletter@sub.example.co.uk",
            stored: "ada+newsletter@sub.example.co.uk",
        },
        Accepted {
            input: "x-y_z.1@ex-ample.com",
            stored: "x-y_z.1@ex-ample.com",
        },
        Accepted {
            input: "ada@xn--bcher-kva.example",
            stored: "ada@xn--bcher-kva.example",
        },
    ];
    for Accepted { input, stored } in addresses {
        let address = email(input);
        assert_eq!(
            address.as_str(),
            stored,
            "`{input}` has to be stored as `{stored}`: the database compares bytes, so the form a \
             parser produces is the form a subscriber is found by"
        );
        assert_eq!(
            address.to_string(),
            stored,
            "`Display` and `as_str` have to agree, or two places print two different addresses"
        );
        assert_eq!(
            email(address.as_str()),
            address,
            "parsing what was stored has to give the same value: normalisation is a function with \
             fixed points, not a thing that happens once"
        );
    }

    let names = [
        Accepted {
            input: "Ada Lovelace",
            stored: "Ada Lovelace",
        },
        Accepted {
            input: "  Ada   Lovelace  ",
            stored: "Ada Lovelace",
        },
        Accepted {
            input: "Ana María",
            stored: "Ana María",
        },
    ];
    for Accepted { input, stored } in names {
        let parsed = name(input);
        assert_eq!(parsed.as_str(), stored, "`{input}` has to be stored as `{stored}`");
        assert_eq!(parsed.to_string(), stored, "`Display` and `as_str` have to agree");
        assert_eq!(name(parsed.as_str()), parsed, "parsing a stored name changes nothing");
    }
}

#[test]
fn every_refused_sample_is_refused_by_name() {
    // 65 characters before the `@`: one over the limit, and still an address.
    let long_local = format!("{}@example.com", "a".repeat(65));
    let long_address = format!("{}@example.com", "a".repeat(250));
    let long_name = "a".repeat(101);
    let short_password = "a".repeat(11);
    let long_password = "a".repeat(129);
    let addresses: Vec<Refused> = vec![
        Refused { input: "", field: "email", expected: "exactly one `@`" },
        Refused { input: "   ", field: "email", expected: "exactly one `@`" },
        Refused { input: "not-an-address", field: "email", expected: "exactly one `@`" },
        Refused { input: "ada@@example.com", field: "email", expected: "exactly one `@`" },
        Refused { input: "@example.com", field: "email", expected: "a local part of 1 to 64" },
        Refused { input: "ada@", field: "email", expected: "a domain of labels" },
        Refused { input: "ada@example", field: "email", expected: "a domain of labels" },
        Refused { input: "ada@.com", field: "email", expected: "a domain of labels" },
        Refused { input: "ada@example.com.", field: "email", expected: "a domain of labels" },
        Refused { input: "ada@example..com", field: "email", expected: "a domain of letters" },
        Refused { input: "ada@-example.com", field: "email", expected: "a domain of letters" },
        Refused { input: "ada@exam_ple.com", field: "email", expected: "a domain of letters" },
        Refused { input: "ada example@test.com", field: "email", expected: "a local part of letters" },
        Refused { input: "ada@exa mple.com", field: "email", expected: "a domain of letters" },
        Refused { input: ".ada@example.com", field: "email", expected: "a local part of letters" },
        Refused { input: "ada.@example.com", field: "email", expected: "a local part of letters" },
        Refused { input: "ada..lovelace@example.com", field: "email", expected: "a local part of letters" },
        Refused { input: "ada@例え.com", field: "email", expected: "a domain of letters" },
        Refused { input: &long_local, field: "email", expected: "a local part of 1 to 64" },
        Refused { input: &long_address, field: "email", expected: "at most 254 bytes" },
    ];
    for sample in &addresses {
        let error = Email::parse(sample.input)
            .err()
            .unwrap_or_else(|| panic!("`{}` has to be refused", sample.input));
        let message = error.to_string();
        assert!(
            message.contains(sample.field),
            "a refusal has to name the field (`{}`), it said: {message}",
            sample.field
        );
        assert!(
            message.contains(sample.expected),
            "a refusal has to say what the value should have been (`{}`), it said: {message}",
            sample.expected
        );
        assert_eq!(
            error.exit_code(),
            ExitCode::from(2),
            "a value the caller sent is the caller's to fix: {message}"
        );
        assert!(
            !message.contains(sample.input) || sample.input.trim().is_empty(),
            "a refusal must not repeat the value it refused: {message}"
        );
    }

    let names = [
        Refused { input: "", field: "name", expected: "1 to 100 characters" },
        Refused { input: "   ", field: "name", expected: "1 to 100 characters" },
        Refused { input: "Ada\nLovelace", field: "name", expected: "without control characters" },
        Refused { input: "Ada\tLovelace", field: "name", expected: "without control characters" },
        Refused { input: &long_name, field: "name", expected: "1 to 100 characters" },
    ];
    for sample in &names {
        let error = SubscriberName::parse(sample.input)
            .err()
            .unwrap_or_else(|| panic!("`{}` has to be refused", sample.input));
        assert!(error.to_string().contains(sample.field), "{error}");
        assert!(error.to_string().contains(sample.expected), "{error}");
        assert_eq!(error.exit_code(), ExitCode::from(2), "{error}");
    }

    let passwords = [
        Refused { input: "", field: "password", expected: "12 to 128 characters" },
        Refused { input: &short_password, field: "password", expected: "12 to 128 characters" },
        Refused { input: &long_password, field: "password", expected: "12 to 128 characters" },
        Refused { input: "twelve chars\nand more", field: "password", expected: "without control characters" },
    ];
    for sample in &passwords {
        let error = Password::parse(sample.input)
            .err()
            .unwrap_or_else(|| panic!("a refused password stayed refused"));
        assert!(error.to_string().contains(sample.field), "{error}");
        assert!(error.to_string().contains(sample.expected), "{error}");
        assert_eq!(error.exit_code(), ExitCode::from(2), "{error}");
        assert!(
            !error.to_string().contains(sample.input) || sample.input.trim().is_empty(),
            "a refusal must not repeat a value that may be a secret: {error}"
        );
    }
}

#[test]
fn a_password_keeps_its_spaces_and_never_comes_out_of_a_debug_line() {
    const SECRET: &str = "  correct horse battery staple  ";
    let secret = password(SECRET);
    assert_eq!(
        secret.as_str(),
        SECRET,
        "a password is not trimmed, collapsed or lower-cased: it is compared with what the \
         subscriber types next time"
    );

    let debug = format!("{secret:?}");
    assert!(
        !debug.contains(SECRET.trim()),
        "`Debug` reaches log lines and panics (step `mailer/1.6-logging`): {debug}"
    );
    assert!(
        debug.contains("redacted"),
        "a redacted value has to say that it is one, otherwise nobody can tell a missing secret \
         from a hidden one: {debug}"
    );

    let error = Password::parse("short")
        .err()
        .expect("five characters is not a password");
    assert!(
        !error.to_string().contains("short"),
        "the message must not quote what was sent: {error}"
    );
}

#[test]
fn a_generated_set_of_addresses_round_trips_and_their_mutations_are_refused() {
    let mut generator = Generator(0x2545_F491_4F6C_DD1D);
    let mut accepted = 0;
    for _ in 0..400 {
        let local = generator.local_part();
        let domain = format!("{}.{}", generator.label(), generator.tld());
        let address = format!("{local}@{domain}");

        let parsed = Email::parse(&address)
            .unwrap_or_else(|error| panic!("generated `{address}` has to be accepted: {error}"));
        assert_eq!(parsed.as_str(), address, "a canonical address has to survive as it is");
        let again = Email::parse(parsed.as_str())
            .unwrap_or_else(|error| panic!("`{}` was accepted once: {error}", parsed.as_str()));
        assert_eq!(
            again, parsed,
            "parsing a stored address has to be a no-op: {address}"
        );
        assert!(address.len() <= MAX_EMAIL_LENGTH, "the generator made an address that is too long");
        accepted += 1;

        // Every mutation of an accepted address has to be refused: a property is only worth
        // something if the values just outside it are refused too. Each of these moves one letter of
        // the address grammar — and none of them is the address itself, or a change that is legal.
        let (local, domain) = address.split_once('@').expect("the generator adds an `@`");
        let long_label = "x".repeat(MAX_EMAIL_LOCAL_LENGTH + 3);
        let mutations = [
            // the `@` itself, gone, doubled or replaced
            format!("{local}{domain}"),
            format!("{local} {domain}"),
            format!("{local}@@{domain}"),
            // a side of the `@` went missing
            format!("@{domain}"),
            format!("{local}@"),
            // a dot at the wrong end of the domain
            format!("{local}@.{domain}"),
            format!("{local}@{domain}."),
            // a label that starts or ends with `-`, holds a space, or is too long
            format!("{local}@-{domain}"),
            format!("{local}@{domain}-"),
            format!("{local}@{domain}. example"),
            format!("{local}@{long_label}"),
            // the dot that makes it a domain at all, and one too many
            format!("{local}@{}", domain.replacen('.', "", 1)),
            format!("{local}@{}", domain.replace('.', "..")),
        ];
        for mutation in mutations {
            assert!(
                Email::parse(&mutation).is_err(),
                "`{mutation}` is not an address, and it has to be refused (the address it came \
                 from was `{address}`)"
            );
        }
    }
    assert_eq!(accepted, 400, "the generator has to make the set this test names");
}

/// A deterministic source of values: the same seed gives the same set on every machine, and a
/// failure can be reproduced from the address it printed (T4·4.13 replaces this with `proptest` and
/// a shrinking strategy; here the set only has to be wide, not clever).
struct Generator(u64);

impl Generator {
    /// The next number, xorshift64* — small, determined, and good enough for names.
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// One value in `0..bound`.
    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    /// A run of characters from `alphabet`, `1..=high` long.
    fn run(&mut self, alphabet: &[u8], high: u64) -> String {
        let length = 1 + self.below(high);
        (0..length)
            .map(|_| alphabet[self.below(alphabet.len() as u64) as usize] as char)
            .collect()
    }

    /// A local part: no dot at either end, never two dots in a row.
    fn local_part(&mut self) -> String {
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_+-";
        let mut local = self.run(ALPHABET, 20);
        if self.below(2) == 0 {
            local.push('.');
            local.push_str(&self.run(ALPHABET, 10));
        }
        local
    }

    /// A domain label: letters and digits, never a `-` at either end.
    fn label(&mut self) -> String {
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        self.run(ALPHABET, 12)
    }

    /// A top-level label: letters only, so the generated address is one this service would accept.
    fn tld(&mut self) -> String {
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
        self.run(ALPHABET, 5)
    }
}
"##;

/// A directory the probe is written into, removed when the test ends.
struct Dir(PathBuf);

impl Dir {
    /// A fresh directory, named after the test that asked for it.
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mailer-domain-{label}-{}-{unique}",
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

    /// Write a file below the directory, creating what it needs.
    fn write(&self, name: &str, contents: &str) {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("{} is creatable: {error}", parent.display()));
        }
        std::fs::write(&path, contents)
            .unwrap_or_else(|error| panic!("{} is writable: {error}", path.display()));
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The learner's project — the crate the probe has to compile against.
fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn the_domain_types_hold_up_from_outside_the_crate() {
    let dir = Dir::new("probe");
    dir.write(
        "Cargo.toml",
        &PROBE_MANIFEST.replace("PROJECT", &project().display().to_string()),
    );
    dir.write("src/lib.rs", PROBE_LIBRARY);
    dir.write("tests/domain.rs", PROBE_TESTS);

    let output = Command::new("cargo")
        .args(["test", "--quiet", "--offline"])
        .current_dir(dir.path())
        // The probe compiles the project again, so it gets a target directory of its own: sharing
        // the project's would rebuild it with different settings, and building in the temporary
        // directory would throw the result away after every run (step `mailer/1.5-errors` does the
        // same for its own probe).
        .env(
            "CARGO_TARGET_DIR",
            project().join("target/kiln-domain-probe"),
        )
        .output()
        .unwrap_or_else(|error| panic!("cargo has to be on PATH: {error}"));

    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let tests = PROBE_TESTS.matches("#[test]").count();
    assert!(
        output.status.success(),
        "the domain types have to hold up from another crate. A refusal here is the compiler or a \
         test: make an `Email`, a `SubscriberName` and a `Password` that can only be built by \
         parsing, and let the probe check what they do:\n{printed}"
    );
    assert!(
        printed.contains(&format!("{tests} passed; 0 failed")),
        "the probe's tests have to run, all {tests} of them, and pass: nothing was judged by a \
         suite that skipped its work:\n{printed}"
    );
    assert!(
        printed.contains("0 ignored"),
        "a probe test nobody runs is a promise nobody keeps:\n{printed}"
    );
}
