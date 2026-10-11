//! probe = "store/tiering@apply"
//! title = "The object lands before the local file goes"
//! hint = "Put the object first and remove the local file second, count what the manifest states, check a fetched object's length before you write it, and never read a local file a pass has already moved."
//!
//! Step `lakelet/4.21-cold-tiering` is the oldest trick in storage: most of what a telemetry store holds is never read again, and it does not have to be on the expensive machine. `store/retention@apply` is the sibling this step's rule copies with one word changed — retention removes, tiering *moves* — and `ops/backup-restore@apply` argued the ordering family this step is the third member of. Four decisions, and each one is judged by a case rather than by a paragraph.
//!
//! 1. **the object lands first, and the local file goes second.** A crash in the window leaves the segment in *both* places and the two are the same bytes, so the store is readable with the tier and without it. The other order has a window in which the manifest names a segment whose bytes are in neither place, and that is not a cold store, it is a lost one. Nothing may be removed on a `put` that did not answer — and a `put` whose answer was lost on the way back has landed, which is why the pass stops rather than repeating the removal;
//! 2. **the tier keeps no manifest of its own.** What is in the tier is answered by asking it, paged: a map from ordinal to key beside the store would be a second file that can disagree with the tier, and a store that trusted it would refuse to open for a reason nobody could see. The price is stated rather than hidden — a tier that has lost its listing is a tier that holds nothing, which is a truth a caller can act on — and it is why a hydrate refuses a key its listing does not hold;
//! 3. **the rule is a segment's own last record, and the pass is applied at a `now` a caller hands in.** A segment is cold exactly when the last timestamp the manifest states for it is below `now - window`; a segment is one file moved as one object, and the newest record in it decides whether anybody reads that file again. A segment whose range nobody stated is never cold, and nothing in a tier reads a clock;
//! 4. **a hydrate checks the length before it writes, and a complete store is asked nothing.** The tier's copy of a segment is a copy of something with a stated length, and a copy that is not that length is not the segment: a file written first and checked afterwards is a file the store's own open then refuses, for a reason nobody can trace to the tier. And the common case is the cheap one — a store whose files are all the stated length asks the tier for no listing and no object at all, which is what makes a hydrate affordable before every cold read.
//!
//! ## What you write
//!
//! * [`object_key`] — the key a segment's bytes live under: the file the store gives it, under the port's prefix;
//! * [`Policy::is_cold`] — the rule above, at the boundary it is judged on;
//! * [`listing`] — the loop that walks a port's pages, and the refusal of an endpoint that never advances;
//! * [`sweep`] — every cold segment into the tier, object first and local file second;
//! * [`hydrate`] — every segment whose file is gone or is not the stated length, back from the tier.
//!
//! The port ([`Objects`], [`Cursor`], [`Page`]), the store's local side ([`Local`], [`SegmentInfo`]), the vocabulary a pass reports in ([`Report`], [`Error`], [`Listing`]) and the in-file port ([`Memory`]) are given. The port is the interface a second backend is written against, so it is not this file's to invent; [`Local`] exists to be the *other* side of the window, and its removals are journalled for the same reason the port's calls are.
//!
//! ## What the tests ask
//!
//! A key is the store's own path under the prefix, and a prefix with a trailing slash is one prefix. The policy boundary is one millisecond wide, and a segment whose range nobody stated is never cold. A sweep's journal, in order, is one `put` followed by one `remove` per cold segment, with the warm ones untouched — and a `put` that never reached the tier leaves the local file alone while a `put` whose answer was lost leaves the segment in *both* places, which is the window read from both sides. A sweep that has already moved a segment asks the tier nothing. A hydrate brings back exactly the files that are gone and a second hydrate asks nothing at all. A `get` of an object the port never saw is a miss rather than an empty file, an object that is not the manifest's length is refused and leaves no file behind, and a listing walks more than one page — while a listing that answers the same cursor twice is refused rather than followed for ever.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no device and no S3**: the box is a map rather than a directory, so there is nothing to `fsync` and no directory entry to wait for, and the tier is an in-file port rather than `src/engine/tier/s3.rs` over a socket. What the step's own suite does that this file cannot is *run twice*: its cases are written once and run against a directory and against an S3-shaped endpoint, with `KILN_BACKEND` choosing which and `conformance.toml` naming the two — and a tier that works against a directory and breaks against a bucket is exactly the bug that buys. Here the port is the same in-process one both sides of every case use, which is what makes the *orderings* checkable at all: the journal is the evidence, and a map can be stopped between two writes where a real device cannot.
//!
//! Missing on purpose, beyond the device: the ordering's real cause (a `put` that waits for the bytes *and* for the name that makes them an object), the refusal `Segments::open` gives a store whose cold segments are out of the box — and the way back from it, one copy per segment by hand, because a key is the store's own path — the keys being a function of a segment's identity rather than of an instant, and the byte-level round trip (this file compares the bytes it moved, and a store opened from a directory afterwards is the step's own case, not a probe's).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

/// How many keys a listing asks an endpoint for in one page.
///
/// A thousand, and it is the number a *client* asks for rather than the number it gets: an endpoint
/// may answer fewer, this file's own port answers as many as it was built to, and `next` is the only
/// field that says there is more.
pub const PAGE_KEYS: usize = 1000;

/// The operations a pass journals, on either side of the window the ordering is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// The tier was asked to write an object.
    Put,
    /// The tier was asked for an object.
    Get,
    /// The tier was asked for one page of a listing.
    List,
    /// The tier was asked to remove an object.
    Delete,
    /// The store's own file was removed from the box.
    Remove,
}

impl Op {
    /// The word this operation is spelled with in a journal.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::Get => "get",
            Self::List => "list",
            Self::Delete => "delete",
            Self::Remove => "remove",
        }
    }
}

/// One operation a pass carried out, and whether it reached the tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// What was asked.
    pub op: Op,
    /// The key, the prefix or the file it named.
    pub key: String,
    /// `true` when the operation reached the tier. A `put` whose answer was lost on the way back
    /// has landed; one that never reached the endpoint has not, and the pass cannot tell the two
    /// apart — which is the whole reason the ordering is what it is.
    pub landed: bool,
}

/// The journal both sides of the window write to, in the order they wrote.
///
/// Given, because it is the evidence and not the work. A machine cannot be asked to die at a chosen
/// instruction, so the ordering argument is made checkable here instead: the store's own removals
/// and the port's calls land in one list, in order, and a case can read the order off it. It is
/// shared by reference count because the port is reached through `&dyn Objects` and the store's
/// local side through `&mut Local`, and the journal is the one thing both are handed.
#[derive(Debug, Clone, Default)]
pub struct Journal {
    calls: Rc<RefCell<Vec<Call>>>,
}

impl Journal {
    /// An empty journal.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Write one operation down.
    pub fn record(&self, op: Op, key: &str, landed: bool) {
        self.calls.borrow_mut().push(Call {
            op,
            key: key.to_owned(),
            landed,
        });
    }

    /// Everything journalled so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<Call> {
        self.calls.borrow().clone()
    }

    /// The journal as lines: `<op> <key>`, with a `!` after the operation when it did not land.
    ///
    /// The rendering a case asserts on, because the order of two writes is the claim and `put
    /// segments/0.lks` followed by `remove segments/0.lks` is that claim written down.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .map(|call| {
                let mark = if call.landed { "" } else { "!" };
                format!("{}{mark} {}", call.op.as_str(), call.key)
            })
            .collect()
    }
}

/// Where a listing continues: a *key*, not an offset.
///
/// An offset counts what a prefix held when the walk began, so a key that arrives in the middle of
/// a walk makes the next page skip one and a key that leaves makes it repeat one. The tier is
/// exactly the thing that changes while somebody walks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor(String);

impl Cursor {
    /// A cursor that says "after this key".
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The key it names.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One page of a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The keys this page answers, in key order.
    pub keys: Vec<String>,
    /// The cursor for the page after this one, or `None` when this is the last.
    pub next: Option<Cursor>,
}

/// Every key a port holds under one prefix, and how many pages it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    keys: Vec<String>,
    pages: u64,
}

impl Listing {
    /// The keys, in the order the port answered them.
    #[must_use]
    pub fn keys(&self) -> &[String] {
        &self.keys
    }

    /// How many pages the walk asked for. `1` is a listing that fitted in one answer.
    #[must_use]
    pub const fn pages(&self) -> u64 {
        self.pages
    }

    /// Whether the walk saw this key.
    #[must_use]
    pub fn holds(&self, key: &str) -> bool {
        self.keys.iter().any(|held| held == key)
    }
}

/// What a tier can be asked, and nothing that knows what a segment is.
///
/// The port is the whole reason the rule and the two passes can be written once and run against two
/// implementations, and it says the four things a tier can be asked: `put` (idempotent — the same key
/// and the same bytes leave the same object), `get` (absence is `None`, not an error), `delete`
/// (idempotent, because a pass that deletes what is already gone has reached the state it asked for)
/// and `list`, which answers **one page**.
pub trait Objects {
    /// The prefix this port's store lives under.
    fn prefix(&self) -> &str;

    /// Write the object.
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), Error>;

    /// The object's bytes, or `None` when the tier does not hold it.
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, Error>;

    /// Remove the object.
    fn delete(&self, key: &str) -> Result<(), Error>;

    /// One page under `prefix`, continuing after `cursor` when one is given.
    fn list(&self, prefix: &str, cursor: Option<Cursor>) -> Result<Page, Error>;
}

/// What a pass refuses with, phrased for the operator who reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The tier could not carry the request out: a socket that died, an endpoint that never
    /// answered, a listing that does not advance. Nothing about *this* request was learned.
    Transport {
        /// What happened.
        detail: String,
    },
    /// The store's own file could not be read or written.
    Local {
        /// What happened.
        detail: String,
    },
    /// The tier does not hold the object the manifest names.
    Missing {
        /// The key that is not there.
        key: String,
    },
    /// The tier's copy of a segment is not the length the manifest states: it is not the segment.
    Damaged {
        /// The key the copy came from.
        key: String,
        /// The length the manifest states.
        stated: u64,
        /// The length the tier answered with.
        found: u64,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport { detail } => write!(f, "the tier could not answer: {detail}"),
            Self::Local { detail } => write!(f, "the store's own file: {detail}"),
            Self::Missing { key } => write!(f, "the tier does not hold `{key}`"),
            Self::Damaged { key, stated, found } => write!(
                f,
                "`{key}` is {found} bytes and the manifest states {stated}: a copy that is not that \
                 length is not the segment"
            ),
        }
    }
}

/// What the manifest states about one segment: the three numbers a pass reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentInfo {
    /// Where the segment sits in the store's own order; its file is `segments/<index>.lks`.
    pub index: u64,
    /// The length the manifest states for it, which is the length its file has.
    pub bytes: u64,
    /// The last timestamp in it, or `None` when the manifest states no range for it.
    pub last_millis: Option<u64>,
}

/// What one pass looked at and did, counted from the *manifest* rather than from a directory.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    moved: u64,
    fetched: u64,
    kept: u64,
    bytes: u64,
    pages: u64,
}

impl Report {
    /// How many segments this pass put in the tier and removed from the box.
    #[must_use]
    pub const fn moved(&self) -> u64 {
        self.moved
    }

    /// How many segments this pass fetched back.
    #[must_use]
    pub const fn fetched(&self) -> u64 {
        self.fetched
    }

    /// How many segments this pass looked at and left alone: the policy did not call them cold, or
    /// their file is already where this pass would have put it.
    #[must_use]
    pub const fn kept(&self) -> u64 {
        self.kept
    }

    /// The bytes of the segments it moved or fetched, from the manifest's own numbers.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// How many pages a listing it walked answered: `0` when it asked for none.
    #[must_use]
    pub const fn pages(&self) -> u64 {
        self.pages
    }
}

/// What "cold" means: a window, and the reading a pass is applied at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    cold_after_millis: u64,
}

impl Policy {
    /// The window the build ships with: a week.
    pub const DEFAULT: Policy = Policy {
        cold_after_millis: 7 * 24 * 60 * 60 * 1_000,
    };

    /// A policy that calls a segment cold once its last record is older than this many
    /// milliseconds.
    #[must_use]
    pub const fn new(cold_after_millis: u64) -> Self {
        Self { cold_after_millis }
    }

    /// The window.
    #[must_use]
    pub const fn cold_after_millis(&self) -> u64 {
        self.cold_after_millis
    }

    /// The instant a segment's last record is compared against: `now - window`, and zero when the
    /// reading is below the window, which moves nothing rather than everything.
    ///
    /// Given: arithmetic, not a decision.
    #[must_use]
    pub const fn cutoff(&self, now_millis: u64) -> u64 {
        now_millis.saturating_sub(self.cold_after_millis)
    }

    /// Whether this segment is cold at this reading.
    #[must_use]
    pub fn is_cold(&self, info: &SegmentInfo, now_millis: u64) -> bool {
        todo!()
    }
}

/// The store's local side: the segment files the manifest names, and the journal both sides write.
///
/// Given, because it stands in for the device and for `lakelet/4.8-compaction`'s open: the files are
/// a map so that a pass can be stopped between two writes, and every removal is journalled so that
/// the order of the two writes is a fact a case can read.
#[derive(Debug)]
pub struct Local {
    manifest: Vec<SegmentInfo>,
    files: BTreeMap<u64, Vec<u8>>,
    journal: Journal,
}

impl Local {
    /// A store whose manifest names these segments and whose directory holds no files yet.
    #[must_use]
    pub fn new(manifest: Vec<SegmentInfo>, journal: &Journal) -> Self {
        Self {
            manifest,
            files: BTreeMap::new(),
            journal: journal.clone(),
        }
    }

    /// Write a segment's file, as the store's own write path did.
    pub fn seed(&mut self, index: u64, bytes: &[u8]) {
        self.files.insert(index, bytes.to_vec());
    }

    /// The manifest.
    #[must_use]
    pub fn manifest(&self) -> &[SegmentInfo] {
        &self.manifest
    }

    /// Whether the segment's file is on the box.
    #[must_use]
    pub fn holds(&self, index: u64) -> bool {
        self.files.contains_key(&index)
    }

    /// The length of the segment's file as the device has it, or `None` when there is no file.
    ///
    /// The question a hydrate asks first, and the reason it is the *device's* answer rather than the
    /// manifest's: a file that is not the stated length is as absent as one that is gone.
    #[must_use]
    pub fn length_of(&self, index: u64) -> Option<u64> {
        self.files.get(&index).map(|bytes| bytes.len() as u64)
    }

    /// The bytes of the segment's file.
    ///
    /// # Errors
    ///
    /// [`Error::Local`] when there is no such file: a pass that is about to remove a file reads it,
    /// and a file that is not there is not a segment to move.
    pub fn read(&self, index: u64) -> Result<Vec<u8>, Error> {
        self.files.get(&index).cloned().ok_or_else(|| Error::Local {
            detail: format!("{} is not on this box", file_of(index)),
        })
    }

    /// Remove the segment's file, and journal the removal.
    ///
    /// Idempotent in the same way the port's `delete` is: a file that is already gone is the state
    /// this call asked for.
    pub fn remove(&mut self, index: u64) -> Result<(), Error> {
        self.journal.record(Op::Remove, &file_of(index), true);
        self.files.remove(&index);
        Ok(())
    }

    /// Write the segment's bytes into the file the manifest already names.
    pub fn write(&mut self, index: u64, bytes: &[u8]) -> Result<(), Error> {
        self.files.insert(index, bytes.to_vec());
        Ok(())
    }
}

/// A port whose write does not come back: the two sides of the window a crash inside a pass has.
///
/// A machine cannot be asked to die at a chosen instruction, so the realistic version of the same
/// state is asked for instead — a `put` that landed and whose answer was lost on the way back, or
/// one that never reached the endpoint at all. The two are indistinguishable to the pass, which is
/// exactly why it may not repeat the removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fate {
    /// The next `put` is carried out, and its answer never comes back.
    Lost,
    /// The next `put` never reaches the tier.
    Unreached,
}

/// The tier this file ships: a map of objects, a request journal, and a page size.
///
/// Given: an in-file implementation is what makes a probe possible at all, and it is deliberately
/// *only* the four calls the port names — no key validation, no directories, no device. It can also
/// be told to lose the answer to one `put` ([`Memory::lose_next_put`]) and to answer a page smaller
/// than the listing it holds, which are the two things the step's own suite has to arrange with a
/// wrapper and a stub.
#[derive(Debug)]
pub struct Memory {
    prefix: String,
    objects: RefCell<BTreeMap<String, Vec<u8>>>,
    page: usize,
    journal: Journal,
    fate: RefCell<Option<Fate>>,
}

impl Memory {
    /// A port for one store's prefix, journalling to `journal`.
    #[must_use]
    pub fn new(prefix: &str, journal: &Journal) -> Self {
        Self {
            prefix: prefix.to_owned(),
            objects: RefCell::new(BTreeMap::new()),
            page: PAGE_KEYS,
            journal: journal.clone(),
            fate: RefCell::new(None),
        }
    }

    /// The same port, answering at most `keys` keys per page.
    ///
    /// The endpoint's own bound rather than the client's: a client that stopped at the end of the
    /// first page would report a third of a tier as the whole of it.
    #[must_use]
    pub fn with_page(mut self, keys: usize) -> Self {
        self.page = keys;
        self
    }

    /// Make the next `put` answer the way a crash inside the pass's window does.
    pub fn lose_next_put(&self, fate: Fate) {
        *self.fate.borrow_mut() = Some(fate);
    }

    /// Every key it holds, in order.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.objects.borrow().keys().cloned().collect()
    }

    /// One object, for a case that has to look.
    #[must_use]
    pub fn object(&self, key: &str) -> Option<Vec<u8>> {
        self.objects.borrow().get(key).cloned()
    }

    /// Put an object without journalling it: how a case plants something the pass never wrote.
    pub fn seed(&self, key: &str, bytes: &[u8]) {
        self.objects
            .borrow_mut()
            .insert(key.to_owned(), bytes.to_vec());
    }
}

impl Objects for Memory {
    fn prefix(&self) -> &str {
        &self.prefix
    }

    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), Error> {
        let fate = self.fate.borrow_mut().take();
        let landed = fate != Some(Fate::Unreached);
        self.journal.record(Op::Put, key, landed);
        if landed {
            self.objects
                .borrow_mut()
                .insert(key.to_owned(), bytes.to_vec());
        }
        match fate {
            None => Ok(()),
            Some(Fate::Lost) => Err(Error::Transport {
                detail: format!("the endpoint took `{key}` and the answer never came back"),
            }),
            Some(Fate::Unreached) => Err(Error::Transport {
                detail: format!("the endpoint never saw `{key}`"),
            }),
        }
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        self.journal.record(Op::Get, key, true);
        Ok(self.object(key))
    }

    fn delete(&self, key: &str) -> Result<(), Error> {
        self.journal.record(Op::Delete, key, true);
        self.objects.borrow_mut().remove(key);
        Ok(())
    }

    fn list(&self, prefix: &str, cursor: Option<Cursor>) -> Result<Page, Error> {
        self.journal.record(Op::List, prefix, true);
        let after = cursor.as_ref().map(Cursor::as_str);
        let remaining: Vec<String> = self
            .keys()
            .into_iter()
            .filter(|key| key.starts_with(prefix))
            .filter(|key| after.is_none_or(|last| key.as_str() > last))
            .collect();
        let keys: Vec<String> = remaining.iter().take(self.page).cloned().collect();
        let next = (remaining.len() > keys.len())
            .then(|| keys.last().map(|last| Cursor::new(last.clone())))
            .flatten();
        Ok(Page { keys, next })
    }
}

/// The store's own path for a segment: `segments/<ordinal>.lks`.
///
/// Given: the store's layout is `lakelet/4.7-columnar-segments`'s decision rather than this step's.
/// What this step adds is that a *key* is that same path under the port's prefix, which is what
/// makes a store whose process has stopped recoverable one copy per segment.
#[must_use]
pub fn file_of(ordinal: u64) -> String {
    format!("segments/{ordinal}.lks")
}

/// The key a segment's bytes live under: the file the store gives it, under the port's prefix.
///
/// The key is a function of the segment's identity, so two `put`s of one segment wrote one object —
/// which is the argument that makes a retry safe. A prefix with a trailing separator is one prefix,
/// because the alternative is an empty name in the middle of a key.
#[must_use]
pub fn object_key(prefix: &str, ordinal: u64) -> String {
    todo!()
}

/// Every key a port holds under `prefix`, walking its pages.
///
/// The loop lives here rather than in a caller for two reasons: "the endpoint said there is more" is
/// a fact only the loop can act on, and an endpoint that answers the same cursor twice is not making
/// progress — a walk that took its word for it would ask for ever.
///
/// # Errors
///
/// Everything [`Objects::list`] refuses, and [`Error::Transport`] when the cursor does not advance.
pub fn listing(objects: &dyn Objects, prefix: &str) -> Result<Listing, Error> {
    todo!()
}

/// Move every cold segment out of the box and into the tier.
///
/// For every segment the manifest names that the policy calls cold: read the file, `put` the object,
/// and only then remove the local file. A cold segment whose file is *already* gone has been moved
/// by a pass before this one — there is nothing to upload and nothing to ask the tier — which is
/// what makes a pass owned by a schedule safe to run twice.
///
/// # Errors
///
/// The port's own refusals, and [`Error::Local`] when a file a pass has to read is not there.
pub fn sweep(
    local: &mut Local,
    objects: &dyn Objects,
    policy: &Policy,
    now_millis: u64,
) -> Result<Report, Error> {
    todo!()
}

/// Put back every segment of `local` whose file is missing, or is not the length the manifest states.
///
/// The length is checked *before* anything is written: a file written first and checked afterwards
/// is a file the store's own open then refuses, for a reason nobody can trace to the tier. A store
/// whose files are all the stated length is asked nothing at all — no listing, no request — which is
/// what makes a hydrate affordable before every cold read.
///
/// # Errors
///
/// [`Error::Missing`] when the tier does not hold the segment, [`Error::Damaged`] when what it holds
/// is not the length the manifest states, [`Error::Local`] when a file cannot be written, and the
/// port's own refusals.
pub fn hydrate(local: &mut Local, objects: &dyn Objects) -> Result<Report, Error> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reading every case applies a policy at.
    const NOW: u64 = 1_800_000_000_000;

    /// One hour, in the milliseconds a policy is written in.
    const HOUR_MS: u64 = 3_600_000;

    /// A policy that calls a segment cold once its last record is an hour old.
    fn policy() -> Policy {
        Policy::new(HOUR_MS)
    }

    /// The manifest of a four-segment store: 0 and 1 last held a record eight hours before [`NOW`],
    /// 2 and 3 a second before it. Every segment states four bytes, which is the length of its file.
    fn segments() -> Vec<SegmentInfo> {
        (0..4)
            .map(|index| SegmentInfo {
                index,
                bytes: 4,
                last_millis: Some(if index < 2 {
                    NOW - 8 * HOUR_MS
                } else {
                    NOW - 1_000
                }),
            })
            .collect()
    }

    /// The bytes of one segment's file, which is also what the manifest states.
    fn bytes_of(index: u64) -> Vec<u8> {
        format!("seg{index}").into_bytes()
    }

    /// A store with all four files on the box, a port that holds nothing yet, and the journal both
    /// of them write to.
    fn fixture() -> (Local, Memory, Journal) {
        let journal = Journal::new();
        let mut local = Local::new(segments(), &journal);
        for index in 0..4 {
            local.seed(index, &bytes_of(index));
        }
        let port = Memory::new("lakelet-tier", &journal);
        (local, port, journal)
    }

    /// A port whose listing never advances: a page that names itself as the page after it.
    struct Stuck {
        prefix: String,
    }

    impl Objects for Stuck {
        fn prefix(&self) -> &str {
            &self.prefix
        }

        fn put(&self, _key: &str, _bytes: &[u8]) -> Result<(), Error> {
            Ok(())
        }

        fn get(&self, _key: &str) -> Result<Option<Vec<u8>>, Error> {
            Ok(None)
        }

        fn delete(&self, _key: &str) -> Result<(), Error> {
            Ok(())
        }

        fn list(&self, _prefix: &str, cursor: Option<Cursor>) -> Result<Page, Error> {
            let asked =
                cursor.map_or_else(|| "a/1".to_owned(), |cursor| cursor.as_str().to_owned());
            Ok(Page {
                keys: vec!["a/1".to_owned()],
                next: Some(Cursor::new(asked)),
            })
        }
    }

    #[test]
    fn the_key_is_the_stores_own_path_under_the_ports_prefix() {
        assert_eq!(
            object_key("lakelet-tier", 2),
            "lakelet-tier/segments/2.lks",
            "a key is the file the store gives the segment, under the port's prefix"
        );
        assert_eq!(
            object_key("lakelet-tier/", 2),
            "lakelet-tier/segments/2.lks",
            "a prefix with a trailing separator is one prefix: the alternative is an empty name in \
             the middle of a key"
        );
        assert_eq!(
            object_key("", 2),
            "segments/2.lks",
            "an empty prefix is the store's own path, which is what lets a store be recovered one \
             copy per segment"
        );
        assert_ne!(
            object_key("lakelet-a", 0),
            object_key("lakelet-b", 0),
            "and two stores' objects are two objects: a key that ignored the prefix would have the \
             second store's sweep overwrite the first's"
        );
    }

    #[test]
    fn the_policy_boundary_is_one_millisecond_wide() {
        let window = policy();
        let cutoff = NOW - HOUR_MS;
        let on = SegmentInfo {
            index: 0,
            bytes: 4,
            last_millis: Some(cutoff),
        };
        let past = SegmentInfo {
            index: 1,
            bytes: 4,
            last_millis: Some(cutoff - 1),
        };
        assert!(
            !window.is_cold(&on, NOW),
            "a segment whose last record is exactly the cutoff is warm: the comparison is `last < \
             cutoff`, the same half-open rule `store/retention@apply` uses"
        );
        assert!(
            window.is_cold(&past, NOW),
            "one millisecond older and it is cold: the boundary is one millisecond wide, which is \
             what makes it a rule rather than a rounding"
        );
        assert!(
            !window.is_cold(
                &SegmentInfo {
                    index: 2,
                    bytes: 4,
                    last_millis: None,
                },
                NOW * 2
            ),
            "a segment whose range nobody stated is never cold: a pass does not move bytes on a \
             number nobody wrote down"
        );
        assert!(
            !window.is_cold(
                &SegmentInfo {
                    index: 3,
                    bytes: 4,
                    last_millis: Some(0),
                },
                0
            ),
            "and a pass applied at a reading below its own window moves nothing rather than \
             everything"
        );
    }

    #[test]
    fn a_sweep_puts_every_object_before_it_removes_the_local_file() {
        let (mut local, port, journal) = fixture();
        let report = sweep(&mut local, &port, &policy(), NOW)
            .expect("a store whose files are on this box is one a sweep can carry out");

        assert_eq!(
            journal.lines(),
            vec![
                "put lakelet-tier/segments/0.lks",
                "remove segments/0.lks",
                "put lakelet-tier/segments/1.lks",
                "remove segments/1.lks",
            ],
            "the journal is the ordering: every object lands before the local file that named it \
             goes, and the other order has a window in which the segment is in neither place"
        );
        assert_eq!(
            (report.moved(), report.kept()),
            (2, 2),
            "the two cold segments move and the two warm ones are looked at and left alone"
        );
        assert_eq!(
            report.bytes(),
            8,
            "and the bytes counted are the manifest's own numbers: two segments of four bytes"
        );
        assert_eq!(
            port.keys(),
            vec![
                "lakelet-tier/segments/0.lks".to_owned(),
                "lakelet-tier/segments/1.lks".to_owned()
            ],
            "the cold segments are in the tier, under the keys the manifest's identity gives them"
        );
        assert!(
            !local.holds(0) && !local.holds(1),
            "and they are off the box: the files a moved segment named are gone"
        );
        assert!(
            local.holds(2) && local.holds(3),
            "while the warm ones were never read, and are still where they were"
        );
    }

    #[test]
    fn a_put_that_never_reached_the_tier_leaves_the_local_file_alone() {
        let (mut local, port, journal) = fixture();
        port.lose_next_put(Fate::Unreached);
        let outcome = sweep(&mut local, &port, &policy(), NOW);
        let Err(error) = outcome else {
            panic!("a pass whose object never landed is not a pass that moved anything")
        };
        assert!(
            matches!(error, Error::Transport { .. }),
            "an endpoint that never saw the request is the tier's own refusal rather than a \
             refusal of the store: {error}"
        );
        assert_eq!(
            journal.lines(),
            vec!["put! lakelet-tier/segments/0.lks"],
            "the journal holds the attempt and says nothing landed: the pass cannot tell a lost \
             answer from a request that never arrived, which is why the removal is what follows the \
             answer and not the attempt"
        );
        assert!(
            port.keys().is_empty(),
            "the tier holds nothing: {:?}",
            port.keys()
        );
        assert!(
            local.holds(0),
            "and nothing was deleted on an upload that did not land: the segment is exactly where \
             it was"
        );

        // The store was readable throughout, so the pass that runs next is a pass over an
        // untouched store.
        let report = sweep(&mut local, &port, &policy(), NOW)
            .expect("the second pass is a pass over a store that never lost anything");
        assert_eq!(
            (report.moved(), report.kept()),
            (2, 2),
            "the segments the first pass left alone are the ones it moves"
        );
    }

    #[test]
    fn a_put_whose_answer_was_lost_leaves_the_segment_in_both_places() {
        let (mut local, port, journal) = fixture();
        port.lose_next_put(Fate::Lost);
        let Err(error) = sweep(&mut local, &port, &policy(), NOW) else {
            panic!("a `put` whose answer never came back is not a `put` that came back")
        };
        assert!(matches!(error, Error::Transport { .. }), "{error}");

        assert_eq!(
            port.object("lakelet-tier/segments/0.lks").as_deref(),
            Some(&bytes_of(0)[..]),
            "the object landed, which is what the endpoint's silence hid"
        );
        assert!(
            local.holds(0),
            "and the local file is still there: the window between the two writes is readable from \
             both sides, and that is the whole argument for this order"
        );
        assert!(
            journal
                .lines()
                .iter()
                .all(|line| !line.starts_with("remove")),
            "nothing was removed on a `put` that did not answer: {:?}",
            journal.lines()
        );

        // The door out of the window: the next pass uploads the same object again — one key, one
        // object, the same bytes — and only then removes the file.
        let report = sweep(&mut local, &port, &policy(), NOW)
            .expect("a store that is in both places is a store a pass can finish moving");
        assert_eq!(
            report.moved(),
            2,
            "the segment in the window is moved by the second pass"
        );
        assert_eq!(
            port.keys(),
            vec![
                "lakelet-tier/segments/0.lks".to_owned(),
                "lakelet-tier/segments/1.lks".to_owned()
            ],
            "the object is in the tier exactly once: the key is the identity of what was written, \
             which is what makes repeating the request safe"
        );
        assert!(
            !local.holds(0),
            "and the local file is gone only now that the tier has answered"
        );
    }

    #[test]
    fn a_sweep_that_already_moved_a_segment_asks_the_tier_nothing() {
        let (mut local, port, journal) = fixture();
        sweep(&mut local, &port, &policy(), NOW)
            .expect("the first pass moves the two cold segments");
        let before = journal.lines();

        let report = sweep(&mut local, &port, &policy(), NOW)
            .expect("a pass over a store it has already swept is a pass that finds nothing to do");
        assert_eq!(
            (report.moved(), report.kept()),
            (0, 4),
            "no segment moves the second time: the two whose files are gone have been moved, and \
             the two whose files are there are warm"
        );
        assert_eq!(
            journal.lines(),
            before,
            "and the tier is asked nothing at all: a pass that re-uploaded what it had already \
             moved would be reading a file that is not there"
        );
        assert_eq!(report.bytes(), 0, "nothing moved, so nothing was counted");
    }

    #[test]
    fn a_hydrate_brings_back_what_is_gone_and_asks_nothing_when_nothing_is() {
        let (mut local, port, journal) = fixture();
        sweep(&mut local, &port, &policy(), NOW)
            .expect("the sweep is what takes the files off the box");

        let report = hydrate(&mut local, &port)
            .expect("a tier that holds the segments is a tier a store comes back from");
        assert_eq!(
            (report.fetched(), report.kept()),
            (2, 2),
            "the two segments whose files were gone come back, and the two that never left are \
             looked at and kept"
        );
        assert_eq!(
            report.pages(),
            1,
            "the walk asked the tier for one page of two keys, and the report says so"
        );
        assert!(
            local.holds(0) && local.holds(1),
            "the files are back where the manifest names them"
        );
        assert_eq!(
            local.read(0).expect("the file came back"),
            bytes_of(0),
            "and they are the bytes that went: a hydrate answers record for record"
        );

        let before = journal.lines();
        let again = hydrate(&mut local, &port).expect("a complete store hydrates trivially");
        assert_eq!(
            again.fetched(),
            0,
            "nothing is fetched when nothing is missing"
        );
        assert_eq!(
            again.pages(),
            0,
            "and a complete store is asked nothing at all — no listing, no object — which is what \
             makes a hydrate cheap enough to run before every cold read"
        );
        assert_eq!(
            journal.lines(),
            before,
            "which the journal proves rather than asserts: {:?}",
            journal.lines()
        );
    }

    #[test]
    fn a_get_of_an_object_the_port_never_saw_is_a_miss_and_not_an_empty_file() {
        let (mut local, port, _journal) = fixture();
        local
            .remove(0)
            .expect("taking a file off the box is what a pass did");
        let outcome = hydrate(&mut local, &port);
        let Err(error) = outcome else {
            panic!("a segment whose bytes are in neither place is not a store that came back")
        };
        assert_eq!(
            error,
            Error::Missing {
                key: "lakelet-tier/segments/0.lks".to_owned()
            },
            "the tier does not hold it, and a `get` of an object nobody wrote is a miss rather than \
             an empty file"
        );
        assert!(
            !local.holds(0),
            "and nothing was written for it: an empty file where a segment belongs is a segment \
             the store would then read"
        );
    }

    #[test]
    fn an_object_that_is_not_the_length_the_manifest_states_is_refused_and_leaves_no_file_behind() {
        let (mut local, port, journal) = fixture();
        // The manifest states four bytes, the file on the box holds two, and the tier's copy holds
        // three: a copy that is not the stated length is not the segment, whichever side it is on.
        port.seed("lakelet-tier/segments/0.lks", b"seg");
        local.seed(0, b"se");

        let outcome = hydrate(&mut local, &port);
        let Err(error) = outcome else {
            panic!("a copy that is not the length the manifest states is not the segment")
        };
        assert_eq!(
            error,
            Error::Damaged {
                key: "lakelet-tier/segments/0.lks".to_owned(),
                stated: 4,
                found: 3
            },
            "the refusal names both numbers, because the two numbers are the whole of the claim"
        );
        assert!(
            !local.holds(0),
            "and the file that was already wrong is removed rather than left: a store that kept it \
             would refuse to open later, for a reason nobody could trace to the tier"
        );
        assert_eq!(
            journal.lines(),
            vec![
                "list lakelet-tier",
                "get lakelet-tier/segments/0.lks",
                "remove segments/0.lks",
            ],
            "the length is checked before anything is written: the walk asks what the tier holds, \
             the object is fetched, and the file that was already wrong is removed rather than left"
        );
    }

    #[test]
    fn a_listing_walks_every_page_and_a_page_that_never_advances_is_refused() {
        let journal = Journal::new();
        let port = Memory::new("lakelet-tier", &journal).with_page(2);
        for ordinal in 0..5 {
            port.seed(&object_key("lakelet-tier", ordinal), b"x");
        }
        let walked = listing(&port, "lakelet-tier")
            .expect("a port that answers cursor after cursor is a port a walk can finish");
        assert_eq!(
            walked.keys().len(),
            5,
            "a page smaller than the tier still answers all of it: the loop is what walks the \
             pages, and a client that stopped at the first answer would report a third of the tier \
             as the whole of it — {:?}",
            walked.keys()
        );
        assert_eq!(
            walked.pages(),
            3,
            "five keys two at a time is three pages: {pages}",
            pages = walked.pages()
        );
        assert!(
            walked.keys().windows(2).all(|pair| pair[0] < pair[1]),
            "and the keys are in key order, which is the order an S3-shaped endpoint answers in: \
             {:?}",
            walked.keys()
        );

        let stuck = Stuck {
            prefix: "lakelet-tier".to_owned(),
        };
        let outcome = listing(&stuck, "lakelet-tier");
        let Err(error) = outcome else {
            panic!("an endpoint that answers the same cursor twice is not making progress")
        };
        assert!(
            matches!(error, Error::Transport { .. }),
            "a walk that took its word for it would ask for ever, so the loop refuses instead: \
             {error}"
        );
    }
}
