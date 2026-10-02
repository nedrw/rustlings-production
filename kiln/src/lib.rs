//! `kiln` — the runner for the *rustlings-production* course.
//!
//! The library reads the course metadata under `course/` (manifest, capability ledger, track
//! files) and reports honestly what a step still needs. `main.rs` is a thin shell around it,
//! the same shape the course teaches in `mailer/1.1-workspace`.
//!
//! Rules this crate lives by (see `docs/plan.md` §3 D3, §10):
//!
//! * the metadata files are the single source of truth — the runner never invents content;
//! * every failure is a typed error, never a bare exit code;
//! * the lint ladder it enforces on course steps is enforced on this crate too.

// Tests may assert loudly, production code may not (plan §6: test targets are relaxed).
#![cfg_attr(
    test,
    allow(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used
    )
)]

pub mod caps;
pub mod error;
pub mod gates;
pub mod info;
