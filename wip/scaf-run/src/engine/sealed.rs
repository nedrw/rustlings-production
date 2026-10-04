//! The seal that keeps the store port the engine's own.
//!
//! `Store` is a promise in two directions (see the port's own documentation): the engine promises to
//! use nothing but those five methods, and a backend promises the behaviour the conformance suite
//! asserts (step `jobq/3.5-store-port-conformance`). The engine can only certify the backends it
//! ships — `memory` today, `postgres` next — so it must be impossible for a program outside this
//! crate to hand the engine a backend nobody has held to that suite. That is what this module is
//! for.
//!
//! This is the sealed-trait pattern: `Store` has `Sealed` as a supertrait, `Sealed` is `pub` so the
//! port may name it, and this module is private so no other crate can reach the name, let alone
//! implement it. A tool that diffs two releases cannot see the difference — it is a bound, not a
//! name — but a program that tries to implement `Store` finds out at compile time.

/// Implemented by every backend this crate certifies.
///
/// Naming it is not possible outside `jobq`, and that is the point: `impl Store for MyStore` is
/// refused with "the trait bound `MyStore: sealed::Sealed` is not satisfied", which is the compiler
/// saying what the port's documentation says in prose — a backend the suite has not seen is not a
/// backend the engine will run.
pub trait Sealed {}
