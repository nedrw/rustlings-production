//! `jobq` — a job queue engine: a library that queues and runs jobs, and a daemon that drains them.
//!
//! The engine is the product and the binary is a shell around it. Everything a test can reach lives
//! in [`engine`]; [`service`] (the daemon) depends on the engine, and the engine depends on neither
//! the daemon nor anything else that runs it — that direction is what makes it reusable in a program
//! that is not this one (step `jobq/3.1-engine-boundary`).
//!
//! The modules below are the shape of the project, and the first step is where they start doing
//! something: the wiring is done and the behaviour is not. Read the specs — they say what each module
//! owes, from outside.

pub mod engine;
pub mod service;
