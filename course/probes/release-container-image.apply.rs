//! probe = "release/container-image@apply"
//! title = "A base image is a decision you can read"
//! hint = "`latest` is a moving target, a bare name is the same thing with fewer words, and the final stage has to say which user it runs as."
//!
//! Step `mailer/1.21-container` builds a multi-stage image: a builder with the toolchain, a
//! runtime with the binary and nothing else. Two decisions make the image reproducible and
//! survivable: every `FROM` names a version (a tag that is not `latest`, or a digest), and the
//! final stage runs as somebody who is not root.
//!
//! The Dockerfile here is text, because a probe runs offline and cannot build an image. What is
//! re-checked is the part of the review a person does by reading the file.
//!
//! ## What you write
//!
//! [`audit`] — every finding, `UnpinnedBase` in line order and `RunsAsRoot` last:
//!
//! * `FROM x` is pinned when `x` names a tag other than `latest`, or a digest (`@sha256:`).
//!   `scratch` is pinned by definition: it is not a thing that changes. A bare `FROM rust` is
//!   exactly `rust:latest` spelled shorter;
//! * the **final** stage — after the last `FROM` — must contain a `USER` line naming somebody other
//!   than `root`. A builder stage's `USER` says nothing about the image that ships.
//!
//! Lines are matched case-insensitively and a trailing `as <stage>` is ignored; `#` lines are
//! comments.
//!
//! ## What the tests ask
//!
//! A clean multi-stage file; `latest`; a bare name; no `USER` in the final stage; `USER root`; a
//! builder `USER` that does not count; and several unpinned bases arriving in line order.

/// Something the image gets wrong.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Finding {
    /// A base image whose version is not fixed.
    UnpinnedBase {
        /// One-based line number.
        line: usize,
        /// The image as written.
        image: String,
    },
    /// The final stage does not choose a non-root user.
    RunsAsRoot,
}

/// Every finding about `dockerfile`, in the order described above.
#[must_use]
pub fn audit(dockerfile: &str) -> Vec<Finding> {
    todo!("check every FROM, then the USER of the last stage")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLEAN: &str = "\
FROM rust:1.98-alpine AS builder
WORKDIR /src
RUN cargo build --release
USER builder

FROM alpine:3.20
COPY --from=builder /src/target/release/mailer /usr/local/bin/mailer
USER app
ENTRYPOINT [\"mailer\"]
";

    #[test]
    fn a_clean_multi_stage_file_has_nothing_to_report() {
        assert_eq!(audit(CLEAN), []);
    }

    #[test]
    fn latest_is_a_moving_target() {
        let dockerfile = "FROM rust:latest\nUSER app\n";
        assert_eq!(
            audit(dockerfile),
            [Finding::UnpinnedBase {
                line: 1,
                image: "rust:latest".to_owned(),
            }]
        );
    }

    #[test]
    fn a_bare_name_is_latest_spelled_shorter() {
        let dockerfile = "FROM alpine\nUSER app\n";
        assert_eq!(
            audit(dockerfile),
            [Finding::UnpinnedBase {
                line: 1,
                image: "alpine".to_owned(),
            }]
        );
    }

    #[test]
    fn scratch_is_pinned_by_definition() {
        let dockerfile = "FROM scratch\nUSER app\n";
        assert_eq!(audit(dockerfile), []);
    }

    #[test]
    fn a_digest_is_pinned() {
        let dockerfile = "FROM alpine@sha256:0123456789abcdef\nUSER app\n";
        assert_eq!(audit(dockerfile), []);
    }

    #[test]
    fn the_final_stage_has_to_name_a_user() {
        let dockerfile = "FROM rust:1.98-alpine AS builder\nUSER builder\nFROM alpine:3.20\n";
        assert_eq!(audit(dockerfile), [Finding::RunsAsRoot]);
    }

    #[test]
    fn user_root_is_still_root() {
        let dockerfile = "FROM alpine:3.20\nUSER root\n";
        assert_eq!(audit(dockerfile), [Finding::RunsAsRoot]);
    }

    #[test]
    fn every_unpinned_base_is_reported_in_line_order() {
        let dockerfile = "\
FROM rust:latest AS builder
FROM rust:latest AS tooling
FROM alpine:3.20
USER app
";
        assert_eq!(
            audit(dockerfile),
            [
                Finding::UnpinnedBase {
                    line: 1,
                    image: "rust:latest".to_owned(),
                },
                Finding::UnpinnedBase {
                    line: 2,
                    image: "rust:latest".to_owned(),
                },
            ]
        );
    }
}
