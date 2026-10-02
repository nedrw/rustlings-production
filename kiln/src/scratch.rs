//! Throwaway directories for probes that need a file on disk.
//!
//! The compose probe writes a minimal project and configures it; nothing of that may survive the
//! command. Kept out of the test helpers because it is real behaviour, not a test convenience.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A directory under the system temporary directory, removed when it is dropped.
#[derive(Debug)]
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Create an empty directory whose name says what it is for.
    pub fn new(label: &str) -> io::Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("kiln-{label}-{}-{unique}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    /// Root of the directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scratch_directory_is_created_and_cleaned_up() {
        let path = {
            let scratch = Scratch::new("scratch-test").unwrap();
            assert!(scratch.path().is_dir());
            scratch.path().to_path_buf()
        };
        assert!(!path.exists(), "the guard removes the directory it created");
    }
}
