//! Throwaway directories for probes that need a file on disk, and the claim one run takes on a
//! directory two of them would otherwise share.
//!
//! The compose probe writes a minimal project and configures it; nothing of that may survive the
//! command. Kept out of the test helpers because it is real behaviour, not a test convenience.

use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::error::{KilnError, Result};

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

/// A [`Scratch`] directory, as the typed failure a command reports when the system refuses one.
///
/// The gates that copy a project or write profile data need the directory rather than the
/// `std::io::Error`, and they need it to fail loudly: a gate that silently did without its sandbox
/// would measure the wrong tree.
pub fn directory(label: &str) -> Result<Scratch> {
    Scratch::new(label).map_err(|source| KilnError::Write {
        path: std::env::temp_dir().join(format!("kiln-{label}")),
        source,
    })
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

/// A directory that outlives the run that made it, under the machine's cache.
///
/// [`directory`] is per run and deleted when the run ends; what a gate needs *between* runs — an
/// instrumented build directory, which is expensive to make and worthless the moment it is thrown
/// away — lives here instead. A directory here that more than one run may want is claimed before it
/// is used ([`claim`]).
///
/// # Errors
///
/// [`KilnError::Write`] when the machine has nowhere to put a cache.
pub fn cache(name: &str) -> Result<PathBuf> {
    let dir = cache_root()?.join(name);
    std::fs::create_dir_all(&dir).map_err(|source| KilnError::Write {
        path: dir.clone(),
        source,
    })?;
    Ok(dir)
}

/// `<cache>/kiln`, created: the directory [`cache`] puts a directory in and [`claim`] puts a lock
/// file beside.
fn cache_root() -> Result<PathBuf> {
    let base = cache_base(std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("HOME"))
        .ok_or_else(|| KilnError::Write {
            path: PathBuf::from("<cache>/kiln"),
            source: io::Error::other("neither XDG_CACHE_HOME nor HOME says where a cache belongs"),
        })?;
    let dir = base.join("kiln");
    std::fs::create_dir_all(&dir).map_err(|source| KilnError::Write {
        path: dir.clone(),
        source,
    })?;
    Ok(dir)
}

/// A directory a run takes for itself, and lets go of when it ends.
#[derive(Debug)]
#[must_use = "a claim is released when it is dropped: hold it for as long as the directory is used"]
pub struct Claim {
    lock: PathBuf,
}

impl Drop for Claim {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock);
    }
}

/// How long a claim waits for the run that holds one before giving up.
///
/// Two minutes is longer than any single gate's own build (the slowest one walks six mutants), so a
/// gate that waits this out is waiting for something that is not going to finish.
const CLAIM_WAIT: Duration = Duration::from_secs(120);

/// How often a waiting claim looks again.
const CLAIM_POLL: Duration = Duration::from_millis(250);

/// How old a lock file has to be before its owner counts as gone.
///
/// A process that is killed never runs [`Claim`]'s `Drop`, so without this rule one interrupted run
/// would strand every later one for good — the file would sit there for ever and every waiter would
/// time out on it. Half an hour is far longer than any of these gates builds for and short enough
/// that the next person to run the course does not have to know about the file.
const CLAIM_STALE: Duration = Duration::from_secs(30 * 60);

/// Claim the shared build directory `name` — the directory [`cache`] hands out for it — for as long
/// as the returned value lives.
///
/// The claim is the file `<cache>/kiln/<name>.lock` beside the directory, created with
/// `create_new(true)` so that exactly one process can hold it, holding the owner's pid and the
/// instant it was taken, and removed when the claim is dropped. Answering `None` means someone else
/// has it: the caller says so and does not build, rather than measuring a tree the other run is
/// writing over.
///
/// Why a lock rather than a build directory per run, which would need neither: what the shared
/// directory is *for* is what the run before it left there. A cold rebuild of this course's project
/// is 22 s for `coverage`, and `mutants` would pay it once per walk (handoff M2 §1.3) — the gate
/// reuses the dependencies the last run compiled, and the copy it builds is stamped fresh, so only
/// the *package* is rebuilt. A directory per run would throw away the reason the directory exists.
///
/// Two runs at once are still wrong: A copies its tree, B builds in the shared directory within the
/// next second, and A's cargo then finds artifacts newer than A's own stamped sources and serves A
/// binaries built from B's tree — a percentage or a mutant verdict about another tree, which is the
/// class of answer this milestone removed everywhere else (review finding #5, handoff M2 §5.2).
///
/// # Errors
///
/// [`KilnError::Write`] when the machine gives a cache nowhere to live, or the lock file cannot be
/// created for a reason that is not contention.
pub fn claim(name: &str) -> Result<Option<Claim>> {
    claim_within(&lock_file(name)?, CLAIM_WAIT)
}

/// The claim, waiting up to `wait` for whoever holds it.
///
/// The wait is a parameter rather than [`CLAIM_WAIT`] itself so a test can watch a claim give up in
/// milliseconds; [`claim`] is the only caller, and it is the one that decides how long a gate
/// waits.
fn claim_within(lock: &Path, wait: Duration) -> Result<Option<Claim>> {
    let deadline = Instant::now() + wait;
    loop {
        if let Some(claim) = take(lock)? {
            return Ok(Some(claim));
        }
        // The file is there. One written longer ago than [`CLAIM_STALE`] cannot belong to a running
        // process — its owner would have removed it — so it is taken over, once, and the attempt
        // that follows is immediate.
        if modified_age(lock).is_some_and(|age| age > CLAIM_STALE)
            && std::fs::remove_file(lock).is_ok()
            && let Some(claim) = take(lock)?
        {
            return Ok(Some(claim));
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        std::thread::sleep(CLAIM_POLL.min(deadline - now));
    }
}

/// Create the lock file once, with this process as its owner. `None` when someone else has it.
fn take(lock: &Path) -> Result<Option<Claim>> {
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock)
    {
        Ok(file) => file,
        Err(source) if source.kind() == ErrorKind::AlreadyExists => return Ok(None),
        Err(source) => {
            return Err(KilnError::Write {
                path: lock.to_path_buf(),
                source,
            });
        }
    };
    let owner = owner_of(std::process::id(), SystemTime::now());
    let written = file.write_all(owner.as_bytes());
    drop(file);
    if let Err(source) = written {
        // A lock file with no owner in it is worse than none: every waiter would spend the whole
        // wait on it and then report a run it cannot name.
        let _ = std::fs::remove_file(lock);
        return Err(KilnError::Write {
            path: lock.to_path_buf(),
            source,
        });
    }
    Ok(Some(Claim {
        lock: lock.to_path_buf(),
    }))
}

/// The lock file of a shared build directory `name`.
fn lock_file(name: &str) -> Result<PathBuf> {
    Ok(cache_root()?.join(format!("{name}.lock")))
}

/// One line about the run that holds a shared directory, for the gate that could not take it.
///
/// Names the lock file, the owner's pid and how long it has been held, because all three are what a
/// person needs to decide between waiting and deleting a file whose run is gone.
#[must_use]
pub fn contended(name: &str) -> String {
    lock_file(name).map_or_else(
        |_| format!("another kiln run holds <cache>/kiln/{name}.lock"),
        |lock| contention(&lock),
    )
}

/// The same line, read from a lock file the caller already knows.
fn contention(lock: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(lock) else {
        return format!(
            "another kiln run held {} until this gate gave up",
            lock.display()
        );
    };
    let owner = owner_in(&text);
    let held = held_for(lock, owner.taken).map_or_else(
        || "held for at least the whole wait".to_owned(),
        |age| format!("held for {}s", age.as_secs()),
    );
    match owner.pid {
        Some(pid) => format!(
            "another kiln run holds {} (pid {pid}, {held})",
            lock.display()
        ),
        None => format!(
            "another kiln run holds {} (its owner is not recorded, {held})",
            lock.display()
        ),
    }
}

/// What a lock file says about its owner.
struct Owner {
    /// The pid of the run that took the claim, when the file names one.
    pid: Option<u32>,
    /// The instant the file says it was taken, when it names one.
    taken: Option<SystemTime>,
}

/// What is written into a lock file: `<pid> <unix seconds>` on one line.
fn owner_of(pid: u32, at: SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    format!("{pid} {seconds}\n")
}

/// Read the owner out of a lock file's contents.
fn owner_in(text: &str) -> Owner {
    let mut words = text.split_whitespace();
    Owner {
        pid: words.next().and_then(|pid| pid.parse().ok()),
        taken: words
            .next()
            .and_then(|seconds| seconds.parse().ok())
            .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds)),
    }
}

/// How long a lock file has been held, as its own record says and then as the file system does.
///
/// The file's record is the one to report — a file system may round a modification time to the
/// second, and the record is the moment the claim was taken.
fn held_for(lock: &Path, taken: Option<SystemTime>) -> Option<Duration> {
    taken
        .and_then(|taken| SystemTime::now().duration_since(taken).ok())
        .or_else(|| modified_age(lock))
}

/// How long ago a lock file was written, as the file system records it.
///
/// The staleness rule reads this rather than the owner's own record, because it is the one fact that
/// is there even when the file was written by a process that died in the middle of writing it.
fn modified_age(lock: &Path) -> Option<Duration> {
    lock.metadata().ok()?.modified().ok()?.elapsed().ok()
}

/// Where a container build keeps what it downloads and compiles, one cache per machine.
///
/// `name` is the fixture's own convention — `container-target` for what a build compiles,
/// `container-registry` for the crates it downloads — because a step's fixture and the gate that
/// judges the same tree have to share one cache: otherwise every build pays again for the one
/// before it. Engines mount these as host directories, not as named volumes (`podman build -v`
/// takes a path), which is why they live where a person can find them.
#[must_use]
pub fn container_cache(name: &str) -> Option<PathBuf> {
    cache(name).ok()
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

    #[test]
    fn a_second_claim_waits_for_the_first_and_then_gives_up() {
        // The lock file the public `claim` writes, at a path a test can own: the name comes from the
        // cache, and the cache comes from the environment, which a test must not set.
        let temp = crate::test_support::TempDir::new("claim-held");
        let lock = temp.path().join("coverage-target.lock");
        let first = claim_within(&lock, Duration::from_secs(5))
            .unwrap()
            .expect("nobody holds it yet");

        let started = Instant::now();
        let second = claim_within(&lock, Duration::from_millis(100)).unwrap();
        assert!(second.is_none(), "a held claim is not handed out twice");
        assert!(
            started.elapsed() >= Duration::from_millis(100),
            "and the waiter waited for it rather than answering at once: {:?}",
            started.elapsed()
        );
        assert!(lock.is_file(), "waiting does not remove the holder's file");

        drop(first);
        assert!(!lock.exists(), "`Drop` removes the lock file");
        assert!(
            claim_within(&lock, Duration::from_secs(5))
                .unwrap()
                .is_some(),
            "and the file being gone is what lets the next run in"
        );
    }

    #[test]
    fn a_lock_file_whose_owner_is_long_gone_is_taken_over() {
        // A killed process cannot run `Drop`, so its file would strand every later run without the
        // age rule. The file is given an old modification time, which is what the rule reads.
        let temp = crate::test_support::TempDir::new("claim-stale");
        let lock = temp.path().join("mutants-target.lock");
        std::fs::write(&lock, "1 1759700000\n").unwrap();
        let old = SystemTime::now() - Duration::from_secs(31 * 60);
        std::fs::File::options()
            .write(true)
            .open(&lock)
            .unwrap()
            .set_modified(old)
            .unwrap();

        let claim = claim_within(&lock, Duration::from_millis(50))
            .unwrap()
            .expect("an abandoned lock is taken over");
        let rewritten = std::fs::read_to_string(&lock).unwrap();
        assert_eq!(
            owner_in(&rewritten).pid,
            Some(std::process::id()),
            "and the new owner is written into it: {rewritten:?}"
        );
        drop(claim);
        assert!(!lock.exists());
    }

    #[test]
    fn the_line_a_blocked_gate_reports_names_the_owner_and_the_age() {
        let temp = crate::test_support::TempDir::new("claim-line");
        let lock = temp.path().join("x.lock");
        let taken = SystemTime::now() - Duration::from_secs(7);
        std::fs::write(&lock, owner_of(4242, taken)).unwrap();

        let line = contention(&lock);
        assert!(line.contains(&lock.display().to_string()), "{line}");
        assert!(line.contains("pid 4242"), "{line}");
        assert!(line.contains("held for 7s"), "{line}");

        // A file nobody can read is still named rather than guessed at.
        std::fs::remove_file(&lock).unwrap();
        let gone = contention(&lock);
        assert!(gone.contains(&lock.display().to_string()), "{gone}");
    }
}
