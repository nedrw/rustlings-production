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

/// The build directory a *sandboxed* run shares between its commands.
///
/// Not the project's (that is a throwaway copy) and not the sandbox (deleted when the command ends):
/// the point is to keep what cargo built for the next run. So it is the user's cache directory, one
/// per machine — and a run that cannot work out where that is simply does without, because a missing
/// optimisation must never break a check.
#[must_use]
pub fn shared_target_dir() -> Option<PathBuf> {
    let base = cache_base(std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("HOME"))?;
    let dir = base.join("kiln").join("target");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Where a cache belongs, from the two variables that say so (tested without touching the
/// environment, which a test cannot do and should not try to).
fn cache_base(
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(xdg) = xdg
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg));
    }
    let home = home?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".cache"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cache_directory_comes_from_the_variable_that_says_so() {
        use std::ffi::OsString;
        assert_eq!(
            cache_base(Some(OsString::from("/tmp/cache")), None),
            Some(PathBuf::from("/tmp/cache")),
            "XDG_CACHE_HOME wins when it is set"
        );
        assert_eq!(
            cache_base(None, Some(OsString::from("/home/learner"))),
            Some(PathBuf::from("/home/learner/.cache")),
            "and `$HOME/.cache` is the fallback"
        );
        assert_eq!(
            cache_base(Some(OsString::new()), Some(OsString::from("/home/learner"))),
            Some(PathBuf::from("/home/learner/.cache")),
            "an empty variable names nothing"
        );
        assert_eq!(
            cache_base(None, None),
            None,
            "no home, no cache: a run without one still runs"
        );
    }

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
