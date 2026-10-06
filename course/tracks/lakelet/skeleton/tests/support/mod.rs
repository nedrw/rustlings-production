//! The fixtures a `lakelet` spec drives the daemon through.
//!
//! Same convention as the other tracks (`relay`'s `tests/support`, `jobq`'s `tests/support`): a
//! fixture is *learner-owned* code that the course's spec files use through `mod support;`, and it
//! knows nothing about the course — no metric names, no assertions, no expectations. What it does own
//! is the tedious part: a store directory of this process's own, a daemon started on a port the
//! kernel picked, and the lines it printed.

#![allow(dead_code)] // a spec uses the part of the fixture it needs, not all of it

pub mod service;

use std::path::{Path, PathBuf};

/// One environment variable a process is started with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Var {
    /// Variable name.
    pub name: String,
    /// Its value.
    pub value: String,
}

impl Var {
    /// A variable, for a `Service::start(&[…])` call.
    #[must_use]
    pub fn new(name: &str, value: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            value: value.into(),
        }
    }
}

/// A directory under the system's temporary directory that belongs to this process.
///
/// Unique per process *and* per label, so two specs running at the same time cannot share a store —
/// which would be a test that passes alone and fails in a suite. It removes itself when it is
/// dropped, so a failing spec leaves the disk as it found it.
#[derive(Debug)]
pub struct StoreDir {
    path: PathBuf,
}

impl StoreDir {
    /// Claim a directory for `label`.
    #[must_use]
    pub fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("lakelet-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("{} could not be created: {error}", path.display()));
        Self { path }
    }

    /// Where it lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read a file below it, or `None` when there is none.
    #[must_use]
    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path.join(name)).ok()
    }
}

impl Drop for StoreDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
