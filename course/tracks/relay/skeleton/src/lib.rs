//! `relay` — a webhook delivery service.
//!
//! An event arrives over HTTP, is stored, and is delivered to every endpoint subscribed to it, with
//! a signature the receiver can verify. The library is the service; `src/main.rs` is a shell around
//! it, and every decision lives here, where a test can reach it without starting a process.
//!
//! The modules below are the shape of the service, and step `relay/2.1-project-skeleton` is where
//! they start doing something: the wiring is done and the behaviour is not. Read the specs — they
//! say what each module owes, from outside.

pub mod auth;
pub mod config;
pub mod db;
pub mod domain;
pub mod error;
pub mod http;
pub mod logging;

/// Usage text, printed by `--help` and after a bad argument.
pub const USAGE: &str = "\
usage: relay [OPTIONS]

OPTIONS:
    --config <FILE>          read settings from a TOML file
    --listen-host <HOST>     address to bind (default 127.0.0.1)
    --listen-port <PORT>     port to bind (default 8080; 0 asks the kernel)
    --check-config           print the effective settings and exit
    --help                   print this help and exit
    --version                print the version and exit
";
