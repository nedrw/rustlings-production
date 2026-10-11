//! probe = "ops/backup-restore@apply"
//! title = "A copy's manifest is a log, and a restore writes the store's manifest last"
//! hint = "Write each file's bytes, wait, and only then append the entry that names it; verify every entry before writing anything; write the store's own manifest last."
//!
//! Step `lakelet/4.19-backup-restore` is about the only part of a backup that survives a bad day.
//! A copy taken while the store was being written is two stores at once; a copy whose segment lost
//! a byte in transit is a directory that looks exactly like a good one; and a copy nobody has ever
//! restored is not a backup, it is a hope with a timestamp. Four rules, and each of them is the
//! answer to a way a copy fails:
//!
//! * **the copy's manifest is a log, not a format of your own.** It grows one entry at a time and
//!   is read back the way a log is read: the prefix whose lines are all entries this build wrote,
//!   the bytes after it that nobody can prove, and where the walk stopped. A torn last entry is
//!   dropped with the bytes it occupied; a line that is corrupt stops the prefix *there*;
//! * **the log proves the list, the files prove the bytes.** Which files a copy holds is what its
//!   manifest says, and nothing about the manifest says they are *there*: an entry states a length
//!   and a checksum, and only the file can agree with them. Two passes, and they are different
//!   passes — a `recover` that read every file would report a shorter copy instead of a refusal;
//! * **a file's bytes land before the entry that names it.** Per file: write the contents, wait
//!   for the device, and only then append the entry. That ordering is what makes "the manifest
//!   says what is complete" a property of the code rather than a claim about a page cache, and a
//!   file the copy was killed inside is left partial and *unnamed*;
//! * **a restore verifies everything before it writes anything.** Verifying while writing looks
//!   like the same thing and is not: the first failure lands halfway through, and the destination
//!   is then a directory an operator will open to look at it — and opening a half-restore deletes
//!   what its manifest does not name. A refused restore leaves the destination as it found it;
//! * **the store's own `segments.manifest` is written last.** That file is the commit point
//!   (`lakelet/4.8-compaction`: a directory is a store because of it), so a restore that writes it
//!   first, or in the middle, has a commit point that means nothing;
//! * **a restore is idempotent, and refuses a store that is something else.** Re-running a restore
//!   whose first attempt was interrupted has to be safe, so a destination that already holds
//!   exactly this copy is a no-op; a destination that holds a *different* store is a refusal and
//!   not a merge, because two stores merged into one directory is a directory nobody can reason
//!   about.
//!
//! ## What you write
//!
//! * [`copy`] — a store into a directory, in the order that makes the copy's manifest true;
//! * [`recover`] — what a copy's manifest proves: the prefix, the bytes it could not prove, and
//!   where the walk stopped;
//! * [`restore`] — a copy back into a store, verifying all of it before writing any of it, the
//!   store's own manifest last;
//! * `checked` — the three comparisons one entry makes (present, the length it states, the
//!   checksum it states). Both of the other two are written against it and must not disagree.
//!
//! [`Dir`], [`Store`], [`Entry`], [`Interruption`], [`Report`], [`Outcome`] and the refusal
//! vocabulary are given: the line format is the project's, the store's file list is taken as a
//! file, and the classes a caller branches on are the interface, not the exercise.
//!
//! ## What the tests ask
//!
//! A complete copy names the store's file list first and then the segments it names in its order,
//! and every named file has the bytes and the checksum the entry states. Over every interruption
//! budget from nothing to everything, no copy ever names a file whose bytes are not all there, a
//! file the budget cut through is partial and unnamed, and a copy killed before its first byte
//! names nothing — which is not the same as no copy. A manifest cut inside its last entry drops
//! that entry with the bytes it occupied; a corrupt line stops the prefix there and discards the
//! rest, while a line whose *checksum* is wrong is a whole entry the log proves and a file that
//! disagrees. A copy missing a file, a file shorter than the manifest says, and a file with one
//! flipped byte are three refusals, and none of them writes anything. A restore writes the store's
//! manifest last and re-running it writes nothing at all; a destination that already holds a
//! different store is refused rather than merged; and a directory with no copy's manifest is a
//! typed refusal rather than an empty copy.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no filesystem and no device**: the real copy writes real files and
//! waits for the device before believing an entry, and the real restore's ordering is a claim
//! about what a machine keeps after a power cut. Here a [`Dir`] is a map of names to bytes that
//! remembers the order it was written in, which is enough to re-check the *ordering* — the whole
//! exercise is which write comes first — and not enough to re-check durability. The ordering is
//! also why a restore into an empty directory is the only shape that is safe: nothing here is
//! fsynced, because nothing here is on a disk.
//!
//! Also missing on purpose: no real checksum (`digest` is a 64-bit FNV rather than the crc32 the
//! store seals its records with, and a hand-rolled crc32 in a course file would teach nothing
//! about backups), no store format (the file list is taken as a file, so a segment's bytes are not
//! parsed, and no version or epoch travels with a copy), no log prefix (the real copy takes the
//! bytes a replay proves and not one byte past it, which is `lakelet/4.4-wal-recovery`'s rule),
//! and no timing — RPO and RTO are numbers an operator measures, not numbers a three-minute file
//! can produce.

use std::collections::BTreeMap;

/// The store's own file list, relative to the store's root.
///
/// It is a file of the store like any other and the copy takes it byte for byte — but its name is
/// the commit point, so a restore writes it last and nothing else may take its place.
pub const MANIFEST: &str = "segments.manifest";

/// The copy's own manifest: the log that says which files a copy holds, and which of them are
/// complete.
pub const BACKUP: &str = "backup.manifest";

/// A checksum of `bytes`, as sixteen lower-case hex characters.
///
/// FNV-1a 64, and given rather than written: the store seals its records with crc32
/// (`lakelet/4.4-wal-recovery`), and a hand-rolled crc32 in a course file teaches nothing about
/// backups while being wrong in ways nobody would notice. What is re-checked here is the
/// *ordering* and the two halves of verification, which is where the mistakes that matter live.
#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// One entry of the copy's manifest: a file, the bytes it states, and the checksum of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The file's name, relative to the store's root.
    pub name: String,
    /// The bytes the file has.
    pub bytes: u64,
    /// The checksum those bytes make.
    pub digest: String,
}

impl Entry {
    /// The entry a file makes.
    #[must_use]
    pub fn of(name: &str, bytes: &[u8]) -> Self {
        Self {
            name: name.to_owned(),
            bytes: bytes.len() as u64,
            digest: digest(bytes),
        }
    }

    /// This entry as one line of the log, the newline included.
    ///
    /// The format is stated once, here, because the reader and the writer are the same build and a
    /// probe is not the place to invent a format: `<name> <bytes> <checksum>`.
    #[must_use]
    pub fn line(&self) -> String {
        format!("{} {} {}\n", self.name, self.bytes, self.digest)
    }

    /// The entry one line states, or `None` when the line is not an entry this build wrote.
    #[must_use]
    pub fn parse(line: &str) -> Option<Self> {
        let mut fields = line.split(' ');
        let name = fields.next()?;
        let bytes = fields.next()?.parse().ok()?;
        let digest = fields.next()?;
        if name.is_empty() || fields.next().is_some() || !is_checksum(digest) {
            return None;
        }
        Some(Self {
            name: name.to_owned(),
            bytes,
            digest: digest.to_owned(),
        })
    }
}

/// `true` when these characters are one of [`digest`]'s sixteen lower-case hex ones.
fn is_checksum(text: &str) -> bool {
    text.len() == 16
        && text
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
}

/// How far a copy is allowed to get: the death this step is about, as a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Interruption {
    /// The copy runs to the end.
    #[default]
    Complete,
    /// Stop once this many bytes of file contents have been written.
    AfterBytes(u64),
}

impl Interruption {
    /// The budget in bytes, or `None` when the copy is not interrupted at all.
    #[must_use]
    pub const fn budget(self) -> Option<u64> {
        match self {
            Self::Complete => None,
            Self::AfterBytes(bytes) => Some(bytes),
        }
    }
}

/// What a copy did, as the counts an operator reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// Files of the store anything was written for, the one a budget cut through included.
    pub files: u64,
    /// Bytes of those files' contents written.
    pub bytes: u64,
    /// `true` when the budget stopped the copy.
    pub interrupted: bool,
}

/// What a restore wrote: the files of the store it put back, and their bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// Files written into the destination.
    pub files: u64,
    /// Bytes of their contents written.
    pub bytes: u64,
}

/// Where the walk of a copy's manifest stopped.
///
/// The same three answers a store's own log gives a replay (`lakelet/4.4-wal-recovery`'s
/// `segment::scan`), because a copy's manifest is that log and not a second format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The manifest ended where the log says it does.
    End,
    /// The walk stopped inside a line: the copy was killed while appending one.
    Torn,
    /// The walk stopped at a line that is there and does not verify, or is not a line this build
    /// wrote.
    Corrupt,
}

/// What a copy's manifest proves: the entries of its prefix, and what could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    /// The entries of the prefix, in the order the manifest states them.
    pub entries: Vec<Entry>,
    /// The bytes of the manifest nobody can prove: the copy's incomplete tail.
    pub discarded: u64,
    /// Where the walk stopped.
    pub stop: Stop,
}

/// What one entry's file is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Damage {
    /// The copy does not hold the file the entry names.
    Missing {
        /// The file.
        name: String,
    },
    /// The file is a different length from the one the entry states.
    Short {
        /// The file.
        name: String,
        /// What the entry says.
        expected: u64,
        /// What the file has.
        found: u64,
    },
    /// The file's bytes make another checksum.
    Checksum {
        /// The file.
        name: String,
        /// What the entry says.
        expected: String,
        /// What the bytes make.
        found: String,
    },
}

impl std::fmt::Display for Damage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { name } => write!(formatter, "{name} is not in the copy"),
            Self::Short {
                name,
                expected,
                found,
            } => write!(
                formatter,
                "{name} is {found} byte(s) and the copy says {expected}"
            ),
            Self::Checksum {
                name,
                expected,
                found,
            } => write!(
                formatter,
                "{name} checksums to {found} and the copy says {expected}"
            ),
        }
    }
}

/// Why a copy or a restore was refused. Each class is its own answer, because a script branches on
/// it and an operator reads one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// There is no copy's manifest here at all, which is not the same as a copy that names nothing.
    NoManifest {
        /// The file that is not there.
        name: String,
    },
    /// The destination already holds a store that is not this copy.
    Occupied {
        /// The file that makes it a store.
        name: String,
    },
    /// The copy is not what its own manifest says it is.
    Damaged(Damage),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoManifest { name } => write!(formatter, "there is no {name} here: no copy"),
            Self::Occupied { name } => write!(
                formatter,
                "the destination already holds a store ({name}) and this is not it"
            ),
            Self::Damaged(damage) => write!(formatter, "the copy is damaged: {damage}"),
        }
    }
}

/// A store to copy: the file list, the segments it names, and whatever else the directory holds.
///
/// A store is not a directory listing: it is the files its own `segments.manifest` names, and a
/// file in `segments/` that the list does not name is an orphan a crash left behind. Both are here
/// so that a copy that takes the listing instead of the list is a case that fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    manifest: Vec<u8>,
    segments: Vec<(String, Vec<u8>)>,
    orphans: Vec<(String, Vec<u8>)>,
}

impl Store {
    /// A store of these segments, with its file list built from their names in this order.
    #[must_use]
    pub fn of(segments: &[(&str, &[u8])]) -> Self {
        let manifest: String = segments
            .iter()
            .map(|(name, bytes)| format!("{name} {}\n", bytes.len()))
            .collect();
        Self {
            manifest: manifest.into_bytes(),
            segments: segments
                .iter()
                .map(|(name, bytes)| ((*name).to_owned(), bytes.to_vec()))
                .collect(),
            orphans: Vec::new(),
        }
    }

    /// The same store with a file its file list does not name: an orphan a crash left behind.
    #[must_use]
    pub fn with_orphan(mut self, name: &str, bytes: &[u8]) -> Self {
        self.orphans.push((name.to_owned(), bytes.to_vec()));
        self
    }

    /// The file list, taken as a file like any other.
    #[must_use]
    pub fn manifest(&self) -> &[u8] {
        &self.manifest
    }

    /// The segments the file list names, in the order it names them, with their bytes.
    #[must_use]
    pub fn segments(&self) -> Vec<(&str, &[u8])> {
        self.segments
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect()
    }

    /// Everything the store's directory holds, in name order, orphans included.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .segments
            .iter()
            .chain(self.orphans.iter())
            .map(|(name, _)| name.clone())
            .collect();
        names.push(MANIFEST.to_owned());
        names.sort();
        names
    }
}

/// A directory in memory: the files written into it, and the order they were written in.
///
/// Given because a probe has no device. The real copy writes real files and waits for the device
/// before it appends the entry that names them; what that buys is the *order* of the calls, which
/// is what this records and the whole of what the copy is judged on here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dir {
    files: BTreeMap<String, Vec<u8>>,
    writes: Vec<String>,
}

impl Dir {
    /// An empty directory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Write a whole file, replacing anything of that name.
    pub fn put(&mut self, name: &str, bytes: &[u8]) {
        self.writes.push(name.to_owned());
        self.files.insert(name.to_owned(), bytes.to_vec());
    }

    /// Write after what is already there, which is how a log grows.
    pub fn append(&mut self, name: &str, bytes: &[u8]) {
        self.writes.push(name.to_owned());
        self.files
            .entry(name.to_owned())
            .or_default()
            .extend_from_slice(bytes);
    }

    /// Take a file away, which is how a copy that lost one is modelled.
    pub fn remove(&mut self, name: &str) -> Option<Vec<u8>> {
        self.files.remove(name)
    }

    /// The bytes of one file, or `None` when there is no such file.
    #[must_use]
    pub fn read(&self, name: &str) -> Option<&[u8]> {
        self.files.get(name).map(Vec::as_slice)
    }

    /// `true` when the directory holds that file.
    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.files.contains_key(name)
    }

    /// Every file it holds, in name order.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }

    /// One name per write, in the order the writes happened: what a device would have seen.
    #[must_use]
    pub fn writes(&self) -> &[String] {
        &self.writes
    }
}

/// Copy the store at `source` into `destination`, stopping where `interruption` says.
///
/// The order is the argument: the store's own file list first (it is the file that says which
/// files the store is), then every segment it names in its order, and per file — the bytes, and
/// only then the entry that names them. The copy's own manifest is created empty before the first
/// file, so a copy killed before its first byte names nothing, which is not the same as no copy.
///
/// A file the budget cuts through is left partial and unnamed, and the copy is over: the entry is
/// what says a file is complete, and an entry for a partial file would be a copy that lies about
/// the only thing it is for.
#[must_use]
pub fn copy(source: &Store, destination: &mut Dir, interruption: Interruption) -> Report {
    destination.put(BACKUP, b"");
    let budget = interruption.budget();
    let mut files: u64 = 0;
    let mut written: u64 = 0;
    let mut queue: Vec<(&str, &[u8])> = vec![(MANIFEST, source.manifest())];
    queue.extend(source.segments());
    for (name, bytes) in queue {
        if let Some(budget) = budget {
            let room = budget.saturating_sub(written);
            if room == 0 {
                return Report {
                    files,
                    bytes: written,
                    interrupted: true,
                };
            }
            if room < bytes.len() as u64 {
                let fits = &bytes[..room as usize];
                destination.put(name, fits);
                written = written.saturating_add(room);
                return Report {
                    files: files.saturating_add(1),
                    bytes: written,
                    interrupted: true,
                };
            }
        }
        destination.put(name, bytes);
        destination.append(BACKUP, Entry::of(name, bytes).line().as_bytes());
        files = files.saturating_add(1);
        written = written.saturating_add(bytes.len() as u64);
    }
    Report {
        files,
        bytes: written,
        interrupted: false,
    }
}

/// What the copy at `directory` proves: the entries its manifest states, and the bytes nobody can
/// prove.
///
/// The walk is over the *log* and not over the files: a line that is stated in full and reads as an
/// entry this build wrote is in the prefix, and whether the file behind it is there, is the length
/// the entry states and makes the checksum it states is [`restore`]'s business. The two are
/// deliberately separate passes — a `recover` that read every file would answer a *shorter copy*
/// where the truth is a damaged one, and a restore of it would then quietly write less than the
/// copy promised.
///
/// A copy with no manifest at all is [`Error::NoManifest`], never an empty list: "no copy here" is
/// a fact a script has to be able to tell from "a copy that names nothing".
///
/// # Errors
///
/// [`Error::NoManifest`] when there is no copy's manifest in the directory.
pub fn recover(directory: &Dir) -> Result<Loaded, Error> {
    let Some(bytes) = directory.read(BACKUP) else {
        return Err(Error::NoManifest {
            name: BACKUP.to_owned(),
        });
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        // A manifest this build did not write: there is no prefix to be had at all, so all of it
        // is discarded.
        return Ok(Loaded {
            entries: Vec::new(),
            discarded: bytes.len() as u64,
            stop: Stop::Corrupt,
        });
    };
    let mut entries: Vec<Entry> = Vec::new();
    let mut proven: usize = 0;
    for line in text.split_inclusive('\n') {
        if !line.ends_with('\n') {
            return Ok(Loaded {
                entries,
                discarded: (text.len() - proven) as u64,
                stop: Stop::Torn,
            });
        }
        let Some(entry) = Entry::parse(line.trim_end_matches('\n')) else {
            return Ok(Loaded {
                entries,
                discarded: (text.len() - proven) as u64,
                stop: Stop::Corrupt,
            });
        };
        proven += line.len();
        entries.push(entry);
    }
    Ok(Loaded {
        entries,
        discarded: 0,
        stop: Stop::End,
    })
}

/// Restore the copy at `source` into `destination`, or refuse and leave it as it was.
///
/// One order, and it is the whole of the safety argument: load the copy's manifest, refuse a
/// destination that holds a store which is not this copy, verify **every** entry before writing
/// anything, and only then write — the store's own [`MANIFEST`] last, because that file is the
/// commit point. A destination that holds every file this copy names is already this copy, and the
/// restore is a no-op — re-running a restore whose first attempt was interrupted has to be safe,
/// and a copy that names nothing asks for nothing.
///
/// # Errors
///
/// [`Error::NoManifest`] when the source is not a copy, [`Error::Occupied`] when the destination
/// holds another store, and [`Error::Damaged`] — naming the file, and both numbers where there are
/// two — when the copy is not what its own manifest says.
pub fn restore(source: &Dir, destination: &mut Dir) -> Result<Outcome, Error> {
    let loaded = recover(source)?;
    if destination.has(MANIFEST) {
        // A restored store is exactly what this copy says, so it is not a reason to refuse; a
        // store that is anything else is, because merging two stores is not a restore.
        if loaded
            .entries
            .iter()
            .all(|entry| checked(destination, entry).is_ok())
        {
            return Ok(Outcome { files: 0, bytes: 0 });
        }
        return Err(Error::Occupied {
            name: MANIFEST.to_owned(),
        });
    }
    // Every entry, before anything is written: this is the pass that makes a refused restore leave
    // the destination exactly as it found it.
    for entry in &loaded.entries {
        checked(source, entry).map_err(Error::Damaged)?;
    }
    let mut outcome = Outcome { files: 0, bytes: 0 };
    let mut commit: Option<&Entry> = None;
    for entry in &loaded.entries {
        if entry.name == MANIFEST {
            commit = Some(entry);
            continue;
        }
        let bytes = checked(source, entry).map_err(Error::Damaged)?;
        destination.put(&entry.name, &bytes);
        outcome.files = outcome.files.saturating_add(1);
        outcome.bytes = outcome.bytes.saturating_add(bytes.len() as u64);
    }
    if let Some(entry) = commit {
        let bytes = checked(source, entry).map_err(Error::Damaged)?;
        destination.put(&entry.name, &bytes);
        outcome.files = outcome.files.saturating_add(1);
        outcome.bytes = outcome.bytes.saturating_add(bytes.len() as u64);
    }
    Ok(outcome)
}

/// The bytes of one entry's file, or the reason this copy is not what its manifest says.
///
/// Three comparisons, and each of them is a way a copy can be wrong that nothing else notices:
/// the file is there, it is the length the entry states, and its bytes make the checksum the entry
/// states. The name is looked up rather than followed — a copy is a map of names to bytes here,
/// and an entry that named a path out of the copy would write wherever it pointed.
fn checked(directory: &Dir, entry: &Entry) -> Result<Vec<u8>, Damage> {
    let Some(bytes) = directory.read(&entry.name) else {
        return Err(Damage::Missing {
            name: entry.name.clone(),
        });
    };
    if bytes.len() as u64 != entry.bytes {
        return Err(Damage::Short {
            name: entry.name.clone(),
            expected: entry.bytes,
            found: bytes.len() as u64,
        });
    }
    let found = digest(bytes);
    if found != entry.digest {
        return Err(Damage::Checksum {
            name: entry.name.clone(),
            expected: entry.digest.clone(),
            found,
        });
    }
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: &str = "segments/0001.seg";
    const TWO: &str = "segments/0002.seg";

    /// A store of two segments.
    fn store() -> Store {
        Store::of(&[
            (ONE, b"the bytes of the first segment"),
            (TWO, b"the bytes of the second segment"),
        ])
    }

    /// The bytes of every file a copy of `source` takes.
    fn store_bytes(source: &Store) -> u64 {
        source.manifest().len() as u64
            + source
                .segments()
                .iter()
                .map(|(_, bytes)| bytes.len() as u64)
                .sum::<u64>()
    }

    /// A complete copy of `source`, into a directory the caller keeps.
    fn copied(source: &Store) -> Dir {
        let mut destination = Dir::new();
        let report = copy(source, &mut destination, Interruption::Complete);
        assert!(!report.interrupted, "a complete copy is not interrupted");
        destination
    }

    /// Where the newlines are in a manifest: the end of each entry.
    fn newlines(manifest: &[u8]) -> Vec<usize> {
        manifest
            .iter()
            .enumerate()
            .filter(|(_, byte)| **byte == b'\n')
            .map(|(at, _)| at)
            .collect()
    }

    /// The manifest a copy wrote, as bytes.
    fn manifest_of(directory: &Dir) -> Vec<u8> {
        directory
            .read(BACKUP)
            .expect("a copy writes its own manifest")
            .to_vec()
    }

    #[test]
    fn a_complete_copy_names_the_stores_files_and_every_named_file_verifies() {
        let source = store();
        let destination = copied(&source);
        let loaded = recover(&destination).expect("a copy writes its own manifest");
        let named: Vec<&str> = loaded
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            named,
            [MANIFEST, ONE, TWO],
            "the file list first — it is the file that says which files the store is — then the \
             segments in the order it names them"
        );
        assert_eq!(loaded.discarded, 0, "a complete copy has nothing unproven");
        assert_eq!(
            loaded.stop,
            Stop::End,
            "the walk ended because the manifest ended, not because a line stopped it"
        );
        for entry in &loaded.entries {
            let bytes = destination
                .read(&entry.name)
                .expect("every named file is in the copy");
            assert_eq!(
                bytes.len() as u64,
                entry.bytes,
                "an entry states the length its file has: {}",
                entry.name
            );
            assert_eq!(
                digest(bytes),
                entry.digest,
                "and the checksum its file has: {}",
                entry.name
            );
        }
        assert_eq!(
            destination.read(MANIFEST),
            Some(source.manifest()),
            "the store's own file list travels byte for byte: it is the file a restore ends with"
        );
    }

    #[test]
    fn no_copy_ever_names_a_file_whose_bytes_are_not_all_there() {
        let source = store();
        let total = store_bytes(&source);
        for budget in 0..=total {
            let mut destination = Dir::new();
            let report = copy(&source, &mut destination, Interruption::AfterBytes(budget));
            assert_eq!(
                report.interrupted,
                budget < total,
                "a budget short of the store's bytes stops the copy, and the whole of them does \
                 not: {budget} of {total}"
            );
            if budget == 0 {
                assert_eq!(
                    report.files, 0,
                    "a copy killed before its first byte wrote nothing of the store"
                );
                assert_eq!(report.bytes, 0);
            }
            let loaded = recover(&destination).expect(
                "the copy's own manifest is created before the first file, so a copy killed \
                 before its first byte names nothing — which is not the same as no copy",
            );
            for entry in &loaded.entries {
                let bytes = destination
                    .read(&entry.name)
                    .expect("a named file is in the copy");
                assert_eq!(
                    bytes.len() as u64,
                    entry.bytes,
                    "a file is named only after all of its bytes are there: {} at budget {}",
                    entry.name,
                    budget
                );
                assert_eq!(digest(bytes), entry.digest);
            }
            assert!(
                loaded.entries.len() <= 3,
                "a copy of a three-file store names at most three files: {:?}",
                loaded.entries
            );
        }

        // The file the budget cut through is partial and unnamed: an entry for it would be a copy
        // that lies about the one thing a manifest is for.
        let inside = source.manifest().len() as u64 + 4;
        let mut destination = Dir::new();
        let report = copy(&source, &mut destination, Interruption::AfterBytes(inside));
        assert!(report.interrupted);
        let whole = source.segments()[0].1.len() as u64;
        assert!(destination.has(ONE), "the bytes that fit were written");
        assert!(
            destination
                .read(ONE)
                .is_some_and(|bytes| (bytes.len() as u64) < whole),
            "and what is there is a part of the file, not all of it"
        );
        let loaded = recover(&destination).expect("a manifest is there");
        assert_eq!(
            loaded
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<&str>>(),
            [MANIFEST],
            "the file list was named, the segment the budget cut through was not"
        );
    }

    #[test]
    fn a_torn_last_entry_is_dropped_with_the_bytes_it_occupied() {
        let source = store();
        let mut destination = copied(&source);
        let manifest = manifest_of(&destination);
        let ends: Vec<usize> = newlines(&manifest);
        assert_eq!(
            ends.len(),
            3,
            "three entries: the file list and two segments"
        );
        // Five bytes into the third entry, as a machine killed while appending it leaves them.
        let cut = ends[1] + 1 + 5;
        destination.put(BACKUP, &manifest[..cut]);

        let loaded = recover(&destination).expect("a manifest is there");
        assert_eq!(
            loaded
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<&str>>(),
            [MANIFEST, ONE],
            "the entries the copy stated in full are the prefix"
        );
        assert_eq!(
            loaded.discarded,
            (cut - (ends[1] + 1)) as u64,
            "the walk reports the byte(s) that are there and nobody can prove, and reports them as \
             nobody's rather than as an entry"
        );
        assert_eq!(
            loaded.stop,
            Stop::Torn,
            "and the walk says why it ended: the manifest ends inside a line"
        );
    }

    #[test]
    fn a_corrupt_line_stops_the_prefix_and_a_wrong_checksum_is_the_files_business() {
        let source = store();
        let mut destination = copied(&source);
        let manifest = manifest_of(&destination);
        let ends: Vec<usize> = newlines(&manifest);
        assert_eq!(
            ends.len(),
            3,
            "three entries: the file list and two segments"
        );

        // A byte of the second line that is no longer an entry this build wrote: the log proves the
        // line before it, and nothing after it is the copy's.
        let mut corrupt = manifest.clone();
        corrupt[ends[1] - 1] = b'x';
        destination.put(BACKUP, &corrupt);
        let loaded = recover(&destination).expect("a manifest is there");
        assert_eq!(
            loaded.entries.len(),
            1,
            "a corrupt line stops the prefix there, so this copy holds the file list and nothing \
             else: {:?}",
            loaded.entries
        );
        assert_eq!(
            loaded.stop,
            Stop::Corrupt,
            "which is the corrupt stop a store's own log gives a replay of it"
        );
        assert_eq!(
            loaded.discarded,
            (corrupt.len() - (ends[0] + 1)) as u64,
            "and everything the copy stated after that line is discarded with it"
        );
        assert!(loaded.entries.iter().all(|entry| entry.name != ONE));

        // The same byte flipped to another hex digit instead: every line is an entry this build
        // wrote, so the log proves all three, and it is the *file* that disagrees. A prefix that
        // stopped here would be a copy that quietly holds less than it says.
        let mut wrong = manifest.clone();
        wrong[ends[1] - 1] = if wrong[ends[1] - 1] == b'0' {
            b'1'
        } else {
            b'0'
        };
        destination.put(BACKUP, &wrong);
        let loaded = recover(&destination).expect("a manifest is there");
        assert_eq!(
            loaded.entries.len(),
            3,
            "every line of this manifest is an entry this build wrote: what disagrees is the file \
             behind one of them"
        );
        assert_eq!(loaded.stop, Stop::End);
        let mut target = Dir::new();
        match restore(&destination, &mut target) {
            Err(Error::Damaged(Damage::Checksum { ref name, .. })) => assert_eq!(
                name, ONE,
                "the refusal names the file whose bytes do not make the checksum the entry states"
            ),
            other => panic!(
                "a line whose checksum is wrong is a whole entry and a file that disagrees, not \
                 {other:?}"
            ),
        }
        assert!(
            target.writes().is_empty(),
            "and the copy writes nothing at all"
        );
    }

    #[test]
    fn a_copy_that_is_not_what_it_says_restores_nothing() {
        let source = store();

        let mut missing = copied(&source);
        missing.remove(TWO);
        let mut destination = Dir::new();
        assert_eq!(
            restore(&missing, &mut destination),
            Err(Error::Damaged(Damage::Missing {
                name: TWO.to_owned()
            })),
            "the copy names a file it does not hold, and that is the whole of what a copy is"
        );
        assert!(
            destination.writes().is_empty(),
            "a refused restore leaves the destination exactly as it found it"
        );

        let mut cut = copied(&source);
        let bytes = cut.read(TWO).expect("the copy holds it").to_vec();
        cut.put(TWO, &bytes[..bytes.len() - 3]);
        let mut destination = Dir::new();
        match restore(&cut, &mut destination) {
            Err(Error::Damaged(Damage::Short {
                ref name,
                expected,
                found,
            })) => {
                assert_eq!(name, TWO, "the refusal names the file");
                assert_eq!(expected, bytes.len() as u64);
                assert_eq!(found, (bytes.len() - 3) as u64);
            }
            other => panic!("a cut file is a short file, not {other:?}"),
        }
        assert!(destination.writes().is_empty());

        let mut flipped = copied(&source);
        let mut bytes = flipped.read(ONE).expect("the copy holds it").to_vec();
        bytes[0] ^= 1;
        flipped.put(ONE, &bytes);
        let mut destination = Dir::new();
        match restore(&flipped, &mut destination) {
            Err(Error::Damaged(Damage::Checksum { ref name, .. })) => {
                assert_eq!(
                    name, ONE,
                    "one flipped byte is a checksum refusal, and it names the file"
                );
            }
            other => panic!("one flipped byte is a checksum refusal, not {other:?}"),
        }
        assert!(destination.writes().is_empty());
    }

    #[test]
    fn a_restore_writes_the_stores_manifest_last_and_running_it_again_writes_nothing() {
        let source = store();
        let copy_of = copied(&source);
        let mut destination = Dir::new();
        let first =
            restore(&copy_of, &mut destination).expect("a copy restores into an empty store");
        assert_eq!(
            (first.files, first.bytes),
            (3, store_bytes(&source)),
            "every file the copy names is written once, and the store's bytes are the bytes it \
             wrote"
        );
        assert_eq!(
            destination.writes().last().map(String::as_str),
            Some(MANIFEST),
            "and the store's own manifest is the last thing written: it is the commit point that \
             makes the directory a store, so anything after it is behind a store that exists"
        );
        assert_eq!(
            destination.names(),
            [MANIFEST, ONE, TWO],
            "the destination holds the store the copy named and nothing else: the copy's own log \
             is not a file of the store, and the store's manifest is one of the files it names"
        );

        let writes = destination.writes().len();
        let again = restore(&copy_of, &mut destination)
            .expect("a store that is already exactly this copy is not a reason to refuse");
        assert_eq!(
            (again.files, again.bytes),
            (0, 0),
            "a restore into a store that is already that copy is a no-op, which is what makes it \
             safe to re-run after a failure"
        );
        assert_eq!(
            destination.writes().len(),
            writes,
            "and it wrote nothing at all"
        );
    }

    #[test]
    fn a_restore_refuses_a_destination_that_holds_another_store() {
        let source = store();
        let copy_of = copied(&source);
        let mut destination = Dir::new();
        destination.put(MANIFEST, b"segments/0007.seg 12\n");
        destination.put("segments/0007.seg", b"another store");
        let writes = destination.writes().len();
        assert_eq!(
            restore(&copy_of, &mut destination),
            Err(Error::Occupied {
                name: MANIFEST.to_owned()
            }),
            "a restore into a destination that already holds a store refuses rather than merging: \
             two stores merged is a directory nobody can reason about"
        );
        assert_eq!(
            destination.writes().len(),
            writes,
            "and a refusal leaves the destination as it was"
        );
    }

    #[test]
    fn a_directory_with_no_copy_is_a_refusal_and_not_an_empty_copy() {
        let nothing = Dir::new();
        assert_eq!(
            recover(&nothing),
            Err(Error::NoManifest {
                name: BACKUP.to_owned()
            }),
            "no manifest at all is a typed refusal: a script has to be able to tell it from a copy \
             that names nothing"
        );
        let mut destination = Dir::new();
        assert!(matches!(
            restore(&nothing, &mut destination),
            Err(Error::NoManifest { .. })
        ));
        assert!(destination.writes().is_empty(), "and it wrote nothing");
    }

    #[test]
    fn a_copy_takes_the_file_list_and_not_the_directory_listing() {
        let source = store().with_orphan("segments/0009.seg", b"a crash's leftover");
        assert!(
            source
                .names()
                .iter()
                .any(|name| name == "segments/0009.seg"),
            "the store's directory holds the orphan: what a copy follows is the file list, not \
             the listing"
        );
        let destination = copied(&source);
        let loaded = recover(&destination).expect("a copy writes its own manifest");
        assert_eq!(
            loaded
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<&str>>(),
            [MANIFEST, ONE, TWO],
            "a file in `segments/` that the file list does not name is an orphan a crash left \
             behind: it is not part of what a store is, so it is not part of a copy"
        );
        assert!(
            !destination.has("segments/0009.seg"),
            "and it is not in the copy either: {:?}",
            destination.names()
        );
        assert_eq!(
            destination.names(),
            [BACKUP, MANIFEST, ONE, TWO],
            "the copy is its own log, the store's file list, and the segments that list names"
        );
    }
}
