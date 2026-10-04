//! `kiln links [PATH] [--external]` — the documentation link check, seen from outside a step.
//!
//! The offline half is the `links` gate itself ([`crate::gates::links`]), one implementation for the
//! learner's page and for this report, so "the gate is green" and "this command is clean" cannot
//! disagree. The other half is `--external`, and it is a command rather than a gate on purpose
//! (plan §6): fetching a URL asks somebody else's server a question, and a step that cannot be
//! finished because a crate's documentation site is down teaches the wrong lesson about what "done"
//! means. It is opt-in, it needs `curl`, and `cargo xtask ci` never calls it.
//!
//! Exit codes: 0 nothing is gone, 1 at least one target answered with 404 or 410, 2 the fetch could
//! not run at all (no `curl`) or the tree could not be read. Codes that only mean "a server did not
//! want to answer a robot" — 403, 429, 5xx, a timeout — are printed as `?` and never make the
//! command
//! fail: a check that goes red for reasons outside the tree is the kind of red nobody follows.
//!
//! A report about the repository covers the repository: the directories holding learner projects
//! (`app/<track>`) are skipped and named, because a working copy mid-step has placeholders in it by
//! design and this command is what a maintainer runs to check the course's own pages. `cargo xtask
//! ci` skips exactly the same directories, so the two agree on what "the repository is clean"
//! means. Passing a path (`kiln links app/mailer`) checks that tree and nothing else.

use std::path::Path;
use std::time::Duration;

use crate::cmd::Session;
use crate::error::{EXIT_ERROR, EXIT_FAILED, EXIT_OK, KilnError, Result};
use crate::gates::links::{self, Scan};
use crate::process::{self, Ran};

/// Deadline for one URL: long enough for a slow CDN, short enough that a black hole does not hold the
/// whole report.
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// How the fetch identifies itself: a link check, not a browser.
const USER_AGENT: &str = "kiln-link-check/1";

/// Scan a tree and report; `--external` adds the network pass.
///
/// With no path the report covers the repository's own markdown, in the same sense `cargo xtask ci`
/// means it: the directories holding learner projects (`Course::project_roots`) are skipped and
/// named as skipped. Pointing the command at a path — `kiln links app/mailer` — prints a report
/// about exactly that tree, which is how a step's `links` gate sees it.
pub fn run(session: &Session, path: Option<&Path>, external: bool) -> Result<u8> {
    let course = session.course();
    let (root, skipped) = match path {
        Some(path) => (path.to_path_buf(), Vec::new()),
        None => (course.root().to_path_buf(), course.project_roots()),
    };
    if !root.is_dir() {
        return Err(KilnError::Invalid(format!(
            "{} is not a directory",
            root.display()
        )));
    }

    let names: Vec<&str> = skipped.iter().map(String::as_str).collect();
    let scan = links::scan_skipping(&root, &names)?;
    println!("kiln links · {}", root.display());
    println!(
        "  {} page(s) · {} link(s) · {} external target(s)",
        scan.pages(),
        scan.links(),
        scan.external().len()
    );
    for name in &skipped {
        println!(
            "  skipped {name}/ — a learner's project, judged by its step's `links` gate \
             (`kiln links {name}/<track>` checks one)"
        );
    }
    for finding in scan.findings() {
        println!("✗ {}", finding.line());
    }
    if scan.is_clean() {
        println!("✓ every target resolves without leaving this machine");
    } else {
        println!("{} link(s) do not resolve.", scan.findings().len());
    }

    if !external {
        return Ok(if scan.is_clean() {
            EXIT_OK
        } else {
            EXIT_FAILED
        });
    }
    fetch(&root, &scan, scan.is_clean())
}

/// Fetch every external target, and say what the servers answered.
fn fetch(root: &Path, scan: &Scan, offline_clean: bool) -> Result<u8> {
    let targets: Vec<&str> = scan
        .external()
        .iter()
        .map(String::as_str)
        .filter(|url| url.starts_with("http"))
        .collect();
    if targets.is_empty() {
        println!("--external: nothing to fetch.");
        return Ok(if offline_clean { EXIT_OK } else { EXIT_FAILED });
    }
    if process::locate("curl").is_none() {
        println!(
            "--external needs `curl` on PATH (macOS ships it; a slim Linux image may not). Nothing \
             was fetched."
        );
        return Ok(EXIT_ERROR);
    }

    println!();
    println!(
        "--external: fetching {} target(s), one at a time",
        targets.len()
    );
    let max = FETCH_TIMEOUT.as_secs().to_string();
    let mut fetched = (0usize, 0usize, 0usize);
    for url in targets {
        let argv = [
            "-sS",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "-L",
            "--max-time",
            max.as_str(),
            "--connect-timeout",
            "5",
            "-A",
            USER_AGENT,
            url,
        ];
        let ran = process::run("curl", &argv, root, FETCH_TIMEOUT)?;
        let (marker, detail) = match fetched_as(&ran) {
            Fetched::Healthy(code) => {
                fetched.0 += 1;
                ("✓".to_owned(), code)
            }
            Fetched::Unclear(why) => {
                fetched.1 += 1;
                ("?".to_owned(), why)
            }
            Fetched::Gone(why) => {
                fetched.2 += 1;
                ("✗".to_owned(), why)
            }
        };
        println!("  {marker} {detail} {url}");
    }
    println!();
    println!(
        "{} answered · {} unclear · {} gone (only the last of the three fails this command)",
        fetched.0, fetched.1, fetched.2
    );
    Ok(if fetched.2 == 0 { EXIT_OK } else { EXIT_FAILED })
}

/// What one fetch was worth.
#[derive(Debug, PartialEq, Eq)]
enum Fetched {
    /// An answer in the 2xx-3xx range.
    Healthy(String),
    /// An answer this check cannot call either way.
    Unclear(String),
    /// The page is not there any more.
    Gone(String),
}

/// Read `curl`'s answer.
///
/// 404 and 410 are the only codes that mean the link is dead; everything else is somebody's policy,
/// somebody's bad day, or a network that is not there. The tool's own exit status is not used for
/// this: `curl` is happy to report `000` and exit 6 when a host cannot be reached, which is a fact
/// about this machine rather than about the link.
fn fetched_as(ran: &Ran) -> Fetched {
    if ran.timed_out {
        return Fetched::Unclear(format!("no answer within {}s", FETCH_TIMEOUT.as_secs()));
    }
    let code = ran.stdout.trim();
    match code.parse::<u16>() {
        Ok(200..=399) => Fetched::Healthy(code.to_owned()),
        Ok(404 | 410) => Fetched::Gone(code.to_owned()),
        Ok(403 | 429) => Fetched::Unclear(format!("{code} — the server does not answer robots")),
        Ok(500..=599) => {
            Fetched::Unclear(format!("{code} — the server answered with its own error"))
        }
        Ok(0) | Err(_) => Fetched::Unclear(
            ran.first_stderr_line()
                .unwrap_or("no answer at all")
                .to_owned(),
        ),
        Ok(other) => Fetched::Unclear(other.to_string()),
    }
}

/// Scan a tree and report; `--external` adds the network pass.
#[cfg(test)]
mod tests {
    use super::*;

    /// A finished command with just a status code and a message.
    fn answered(stdout: &str, stderr: &str) -> Ran {
        Ran {
            code: Some(0),
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
            timed_out: false,
            elapsed: Duration::ZERO,
        }
    }

    #[test]
    fn only_a_missing_page_fails_the_command() {
        assert_eq!(
            fetched_as(&answered("200", "")),
            Fetched::Healthy("200".to_owned())
        );
        assert_eq!(
            fetched_as(&answered("301", "")),
            Fetched::Healthy("301".to_owned())
        );
        assert_eq!(
            fetched_as(&answered("404", "")),
            Fetched::Gone("404".to_owned())
        );
        assert_eq!(
            fetched_as(&answered("410", "")),
            Fetched::Gone("410".to_owned())
        );

        for unclear in ["403", "429", "500", "418"] {
            assert!(
                matches!(fetched_as(&answered(unclear, "")), Fetched::Unclear(_)),
                "{unclear} must not be called gone"
            );
        }
    }

    #[test]
    fn a_host_that_does_not_answer_is_not_a_dead_link() {
        let unreachable = Ran {
            code: Some(6),
            stdout: "000".to_owned(),
            stderr: "curl: (6) Could not resolve host: example.invalid".to_owned(),
            timed_out: false,
            elapsed: Duration::from_millis(30),
        };
        let Fetched::Unclear(why) = fetched_as(&unreachable) else {
            panic!("an unreachable host is not a dead link");
        };
        assert!(why.contains("Could not resolve host"), "{why}");

        let killed = Ran {
            timed_out: true,
            ..answered("", "")
        };
        let Fetched::Unclear(why) = fetched_as(&killed) else {
            panic!("a timeout is not a dead link");
        };
        assert!(why.contains("no answer"), "{why}");
    }
}
