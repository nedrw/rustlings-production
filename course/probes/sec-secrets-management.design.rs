//! probe = "sec/secrets-management@design"
//! title = "Two keys valid, one preferred, and a window that is arithmetic"
//! hint = "Keep the key you replaced verifying until the longest token lifetime has passed since its successor took over, sign with the newest open key, and refuse a retirement one millisecond under that instant."
//!
//! Step `lakelet/4.23-secret-rotation` is about the one thing `sec/secrets-management@apply` could
//! not answer and `sec/token-lifetime@apply` made inevitable: *the key that signs changes while the
//! service runs*, and it has to change without a gap. A directory of key files is the primitive;
//! the policy around it is four decisions, and every one of them is a way a deployment locks its
//! own clients out:
//!
//! * **the directory is the switch, and an empty one is not "off".** No `keys/` directory below the
//!   store means authentication never was turned on, which is what keeps every harness the earlier
//!   steps shipped still connecting. A `keys/` directory that *exists* and holds no `*.key` file
//!   is the other answer — a refusal — because a deployment that meant to authenticate and would
//!   have let everybody in is the one failure a ring must never have, and it is the bug this step
//!   exists for;
//! * **the key that signs is the newest open one, so a rotation is an edit to a file** rather than
//!   a moment in the daemon: nothing to flip, no restart to schedule, and nothing that can be
//!   half-done. A tie is broken by the id so that two machines reading the same directory agree,
//!   and a retired key never signs — retiring the newest one hands the signing back to the key
//!   below it;
//! * **a key may only leave after the longest lifetime this deployment may issue has passed since
//!   something newer took the signing over.** The bound is [`Window::MAX_TOKEN_LIFETIME_MILLIS`]
//!   and not "the lifetime our clients usually ask for": a bound built from the usual lifetime
//!   holds for every client except the one that asked for the longest token, which is exactly the
//!   client it strands;
//! * **retiring one millisecond early is a refusal with the instant named, not a warning.** The
//!   client it would strand is not in this process and cannot be asked, and a warning leaves the
//!   key file written — so the refusal is the last thing between an operator and a lockout.
//!
//! ## What you write
//!
//! * [`KeyRing::load`] — the ring as a store's `keys/` directory gives it to you: the two facts
//!   that mean "authentication is off" and "refuse";
//! * [`KeyRing::add`] — an operator's `--add`: one key file's text onto the ring, in the ring's own
//!   order, and never over a key that is already there;
//! * [`KeyRing::preferred`] — the key that signs;
//! * [`KeyRing::retire`] — an operator's `--retire`: the end of one key's window, or the refusal
//!   that keeps a live token alive;
//! * [`KeyRing::earliest_safe_retirement`] — the number the whole step exists for;
//! * [`KeyRing::accepts`] — the ring's typed answer about one token at one instant.
//!
//! [`KeyId`], [`Key`] and its strict file reader, [`Token`] and its payload, [`Window`], [`Refusal`]
//! and the redacting `Debug` on a key are given: a key file's format and the refusal vocabulary are
//! the project's, and a probe that let you invent them would be checking a different deployment.
//!
//! ## What the tests ask
//!
//! An absent directory loads an empty ring that signs nothing, an empty directory refuses, and a
//! directory one of whose files will not read refuses rather than loading the rest. The ring is
//! oldest first whichever order the directory hands its files over in, a key already on it is never
//! replaced, and a key never prints its secret. The key that signs is the newest open one, a tie is
//! the greater id, and a retired key does not sign. A rotation moves the key named by a *new* token
//! and nothing about an old one: the bytes of a token signed before it are the bytes it had, and
//! the key that stopped signing still verifies. `earliest_safe_retirement` is the successor's
//! `not_before` plus [`Window::MAX_TOKEN_LIFETIME_MILLIS`], and `None` for the key that signs. A
//! retirement one millisecond under that instant is refused with the instant named and nothing
//! written; the instant itself is accepted and takes the last token the old key signed with it; and
//! a retirement is written once. Both directions of a window that has not opened are `NotYetValid`,
//! and an expiry names both of its instants.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no filesystem and no tag**. The project's ring is files in
//! `<store>/keys/`, read *per connection* by a running daemon — that is what makes a rotation take
//! effect without a restart — and that directory is one this file cannot make and clean up: it
//! arrives as [`RingSource`], which is the same decision with the reading taken out. What this file
//! is, and what it is the whole of, is **the arithmetic and the ring's decisions**: which key is in
//! hand, whether it may still sign or verify, and when it may leave.
//!
//! The *tag* is not here. The project's `verify` also compares an HMAC-SHA-256 over the bytes that
//! travel, with `subtle::ConstantTimeEq` rather than `==`, and re-checking HMAC from memory is not
//! what a three-minute probe is for (`sec/hmac-signing@apply` makes the same argument in the same
//! course). So [`Token`] carries the bytes a tag would cover as an opaque `rest`, and
//! [`KeyRing::accepts`] reads the key and the window and nothing else: it is the `verify` a
//! deployment with a trustworthy MAC would have, and the difference is stated rather than hidden.
//! The project's payload also carries the client id ahead of the key id; this one names the key
//! first, because the question here is which key.

use std::fmt;

/// The directory below a store that holds the ring.
///
/// The project's own word for it (`keys::DIRECTORY`): with this directory below the store, a
/// connection's first payload has to be a token, and without it authentication is off.
pub const DIRECTORY: &str = "keys";

/// The suffix of one key file: one file per key, named after the key.
pub const SUFFIX: &str = "key";

/// The window arithmetic of this deployment, and the longest lifetime it may issue.
///
/// Given, because two of the decisions below are built from one number and the project states it
/// once: a lifetime above it is refused where a token is issued, and
/// [`KeyRing::earliest_safe_retirement`] is a takeover instant plus it.
pub struct Window;

impl Window {
    /// The longest lifetime a token of this deployment may have, in milliseconds: an hour.
    pub const MAX_TOKEN_LIFETIME_MILLIS: u64 = 3_600_000;

    /// The lifetime a token may be issued with, or the refusal that keeps it under the window.
    ///
    /// Given, and the two refusals are the project's own sentences: they are the reason the
    /// arithmetic has a ceiling to be arithmetic about.
    ///
    /// # Errors
    ///
    /// [`Refusal::Invalid`] for a lifetime of zero, and for one above
    /// [`Window::MAX_TOKEN_LIFETIME_MILLIS`].
    pub fn lifetime(asked_for: u64) -> Result<u64, Refusal> {
        if asked_for == 0 {
            return Err(invalid(
                "a token's lifetime is at least one millisecond: a token that expires at the \
                 instant it was issued is one nobody can use",
            ));
        }
        if asked_for > Self::MAX_TOKEN_LIFETIME_MILLIS {
            return Err(invalid(format!(
                "a lifetime of {asked_for} ms is above this deployment's longest of {} ms, and a \
                 token a client may hold for longer than that is a key that cannot be retired \
                 safely",
                Self::MAX_TOKEN_LIFETIME_MILLIS
            )));
        }
        Ok(asked_for)
    }
}

/// The name of one key: what `id=` in its file says, and what its file is called.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId(String);

impl KeyId {
    /// `name` as a key id.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self(name.to_owned())
    }

    /// The name, as a file name and a listing spell it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One key on the ring, as one `*.key` file states it.
#[derive(Clone, PartialEq, Eq)]
pub struct Key {
    /// The key's name.
    id: KeyId,
    /// The bytes the tag is computed under. Held, never printed, and not used in this file: the
    /// MAC is not here (see the note at the top).
    secret: Vec<u8>,
    /// The first instant the key may sign at.
    not_before_millis: u64,
    /// The instant the key was retired at, or nothing while it is open.
    not_after_millis: Option<u64>,
}

impl Key {
    /// Read one key file.
    ///
    /// Given, and strict on purpose: this file is edited by hand, by an operator, at three in the
    /// morning, and a refusal that does not say which line is a hunt. What this probe re-checks is
    /// what the ring *does* with the keys rather than how the file is spelled — the step's own case
    /// for the format is `a_key_file_is_read_the_way_a_person_writes_it_and_a_broken_one_names_the_file`.
    ///
    /// # Errors
    ///
    /// [`Refusal::Invalid`] for a line that is not `name=value`, a name that is not one of the
    /// four, a number that is not one, a secret that is not hex, and a window that ends before it
    /// starts.
    pub fn parse(text: &str) -> Result<Self, Refusal> {
        let mut id: Option<KeyId> = None;
        let mut secret: Option<Vec<u8>> = None;
        let mut not_before_millis: Option<u64> = None;
        let mut not_after_millis: Option<u64> = None;

        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let number = index + 1;
            let Some((name, value)) = line.split_once('=') else {
                return Err(invalid(format!(
                    "line {number}: `{line}` is not `name=value`"
                )));
            };
            let value = value.trim();
            match name.trim() {
                "id" => id = Some(KeyId::new(value)),
                "secret" => {
                    let bytes = secret_bytes(value)
                        .map_err(|said| invalid(format!("line {number}: the secret is {said}")))?;
                    secret = Some(bytes);
                }
                "not_before_millis" => not_before_millis = Some(millis(number, value)?),
                "not_after_millis" => not_after_millis = Some(millis(number, value)?),
                other => {
                    return Err(invalid(format!(
                        "line {number}: `{other}` is not a field of a key: the fields are `id`, \
                         `secret`, `not_before_millis` and `not_after_millis`"
                    )));
                }
            }
        }

        let Some(id) = id else {
            return Err(invalid("the key names no `id`".to_owned()));
        };
        let Some(secret) = secret else {
            return Err(invalid(format!("`{id}` names no `secret`")));
        };
        let Some(not_before_millis) = not_before_millis else {
            return Err(invalid(format!(
                "`{id}` names no `not_before_millis`: a window that never opened is not a window"
            )));
        };
        if let Some(end) = not_after_millis
            && end <= not_before_millis
        {
            return Err(invalid(format!(
                "`{id}` has an inverted window: it starts at {not_before_millis} and ends at \
                 {end}, so it holds no instant at all"
            )));
        }
        Ok(Self {
            id,
            secret,
            not_before_millis,
            not_after_millis,
        })
    }

    /// The key's name.
    #[must_use]
    pub fn id(&self) -> &KeyId {
        &self.id
    }

    /// The first instant this key may sign at.
    #[must_use]
    pub fn not_before_millis(&self) -> u64 {
        self.not_before_millis
    }

    /// The instant this key was retired at, or nothing while it is open.
    #[must_use]
    pub fn not_after_millis(&self) -> Option<u64> {
        self.not_after_millis
    }

    /// `true` while no end has been written for this key: a retired key is still a key the ring
    /// verifies with, and it is no longer one this ring signs with.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.not_after_millis.is_none()
    }

    /// The bytes a tag is computed under.
    #[must_use]
    pub fn secret(&self) -> &[u8] {
        &self.secret
    }
}

/// `Debug` is written by hand: a key that printed its secret would put that secret in a log
/// aggregator, a bug report and a screen share, and a key file is where key material most often
/// escapes.
impl fmt::Debug for Key {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Key")
            .field("id", &self.id)
            .field("secret", &"<redacted>")
            .field("not_before_millis", &self.not_before_millis)
            .field("not_after_millis", &self.not_after_millis)
            .finish()
    }
}

/// Why the ring says no.
///
/// The first four are the project's own typed refusals, in the order [`KeyRing::accepts`] checks
/// them, and each one is a class a script branches on rather than a sentence it greps. The rest are
/// the refusals this file's two acts need; the project answers them from the command and from
/// `KeyRing::load`, which is the same class in the same place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The ring holds no key with this id: this token was signed by a key this deployment does not
    /// have.
    UnknownKey {
        /// The id the token names.
        id: KeyId,
    },
    /// The window has not opened yet: the token was issued before its key's window, or the
    /// connection arrived before the token was issued. The sentence names the instant that has not
    /// happened.
    NotYetValid {
        /// The first instant that window holds.
        not_before_millis: u64,
    },
    /// The key that signed this token was retired: the ring no longer verifies it. Checked before
    /// expiry on purpose — a client whose key is gone has to be re-keyed, and that is the fact an
    /// operator needs first.
    KeyRetired {
        /// The key that left.
        id: KeyId,
        /// The instant it left at.
        retired_millis: u64,
    },
    /// The token's own window has closed.
    Expired {
        /// The instant the token expired at.
        expires_millis: u64,
        /// The instant the connection arrived at.
        now_millis: u64,
    },
    /// The `keys/` directory exists and holds no `*.key` file: a deployment that meant to
    /// authenticate, which is not the same thing as authentication being off.
    EmptyRing,
    /// The id is already on the ring. A key is added and never replaced: swapping a secret under a
    /// live client locks out every token signed with the one it holds.
    AlreadyOnTheRing {
        /// The key that is already there.
        id: KeyId,
    },
    /// The end of this key's window is already written: a retirement is written once, and the
    /// instant it says is the one the clients were re-keyed against.
    AlreadyRetired {
        /// The key that already has an end.
        id: KeyId,
        /// The instant it was retired at.
        retired_millis: u64,
    },
    /// A retirement under the instant the arithmetic allows: a token this key signed could still be
    /// live, and the client holding it is not in this process to be asked.
    TooEarly {
        /// The key that may not leave yet.
        id: KeyId,
        /// The earliest instant it may leave at.
        safe_at: u64,
    },
    /// Nothing newer has taken the signing over from this key, so no instant after which its tokens
    /// are all gone is knowable from the ring — not even one an operator names.
    NoSuccessor {
        /// The key that is still signing.
        id: KeyId,
    },
    /// The text is not what the format says. The project answers these as `Error::Invalid`, which
    /// `lakeletq rotate` reports as its own class; they are a variant of the one refusal type here
    /// because a probe has one error type.
    Invalid {
        /// One sentence saying what is wrong with the text.
        detail: String,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKey { id } => write!(
                formatter,
                "the ring holds no key `{id}`: this token was signed by a key this deployment does \
                 not have"
            ),
            Self::NotYetValid { not_before_millis } => write!(
                formatter,
                "the window this token belongs to opens at {not_before_millis}, which has not \
                 happened yet"
            ),
            Self::KeyRetired { id, retired_millis } => write!(
                formatter,
                "key `{id}` was retired at {retired_millis}, so the ring no longer verifies tokens \
                 it signed"
            ),
            Self::Expired {
                expires_millis,
                now_millis,
            } => write!(
                formatter,
                "the token expired at {expires_millis} and this connection arrived at {now_millis}"
            ),
            Self::EmptyRing => formatter.write_str(
                "the ring directory exists and holds no key file: a deployment that meant to \
                 authenticate. Remove the directory to turn authentication off",
            ),
            Self::AlreadyOnTheRing { id } => write!(
                formatter,
                "`{id}` is already on the ring: a key is added and never replaced, because every \
                 client holding a token signed with the secret it has would be locked out"
            ),
            Self::AlreadyRetired { id, retired_millis } => write!(
                formatter,
                "`{id}` was retired at {retired_millis} already: a retirement is written once"
            ),
            Self::TooEarly { id, safe_at } => write!(
                formatter,
                "`{id}` cannot be retired yet: a token it signed could still be live until \
                 {safe_at}, which is the last instant it could have signed plus this deployment's \
                 longest token lifetime"
            ),
            Self::NoSuccessor { id } => write!(
                formatter,
                "`{id}` is the key this ring signs with, and nothing newer has taken that over: \
                 add its successor first"
            ),
            Self::Invalid { detail } => formatter.write_str(detail),
        }
    }
}

/// A token: the payload of a connection's first frame.
///
/// The project's payload is the client id, the key id, the two instants and the tag, inside
/// `lakelet/4.1-ingest-protocol`'s frame. This file's miniature is that payload without the client
/// id in front and without the tag: **the first field names the key**, then the window, then the
/// bytes a tag would cover. The first field is the point — a reader has to know which key to check
/// the rest against before it can check anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The key that signed it.
    key_id: KeyId,
    /// The instant it was issued at.
    issued_millis: u64,
    /// The instant it stops being good.
    expires_millis: u64,
    /// The bytes a tag covers. Opaque: this file reads them and never interprets them.
    rest: Vec<u8>,
}

impl Token {
    /// A token `key_id` signed, good from `issued_millis` until `expires_millis`, over `rest`.
    #[must_use]
    pub fn new(key_id: KeyId, issued_millis: u64, expires_millis: u64, rest: &[u8]) -> Self {
        Self {
            key_id,
            issued_millis,
            expires_millis,
            rest: rest.to_vec(),
        }
    }

    /// The key that signed it: the first field of the payload.
    #[must_use]
    pub fn key_id(&self) -> &KeyId {
        &self.key_id
    }

    /// The instant it was issued at.
    #[must_use]
    pub fn issued_millis(&self) -> u64 {
        self.issued_millis
    }

    /// The instant it stops being good.
    #[must_use]
    pub fn expires_millis(&self) -> u64 {
        self.expires_millis
    }

    /// The bytes a tag covers.
    #[must_use]
    pub fn rest(&self) -> &[u8] {
        &self.rest
    }

    /// The payload a connection sends as its first frame.
    ///
    /// Given, and the format is the point: a length-prefixed key id, the two instants as
    /// little-endian numbers, and then the rest. Lengths and all, because the bytes that travel are
    /// what both ends agree on (`relay/2.11-hmac-signing`'s rule, in this project's frame).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let id = self.key_id.as_str().as_bytes();
        let mut bytes = Vec::with_capacity(4 + id.len() + 16 + self.rest.len());
        bytes.extend_from_slice(&(id.len() as u16).to_le_bytes());
        bytes.extend_from_slice(id);
        bytes.extend_from_slice(&self.issued_millis.to_le_bytes());
        bytes.extend_from_slice(&self.expires_millis.to_le_bytes());
        bytes.extend_from_slice(&self.rest);
        bytes
    }

    /// The payload of a connection's first frame, read as a token.
    ///
    /// Given, and structural: whether these bytes are a token at all, before any ring has an
    /// opinion about them. A payload arrives from a socket, so every length in it is a claim, and
    /// this reader takes as it goes rather than indexing its way past the end of the payload.
    ///
    /// # Errors
    ///
    /// [`Refusal::Invalid`] for a field that runs past the end of the payload, an id that is not
    /// UTF-8, and a window that ends before it starts.
    pub fn decode(payload: &[u8]) -> Result<Self, Refusal> {
        let mut cursor = Cursor::new(payload);
        let length = usize::from(cursor.u16()?);
        let key_id = std::str::from_utf8(cursor.take(length)?)
            .map_err(|error| invalid(format!("the key id is not UTF-8: {error}")))?;
        let issued_millis = cursor.u64()?;
        let expires_millis = cursor.u64()?;
        if expires_millis < issued_millis {
            return Err(invalid(format!(
                "the token expires at {expires_millis} and says it was issued at {issued_millis}: \
                 no instant is both"
            )));
        }
        Ok(Self {
            key_id: KeyId::new(key_id),
            issued_millis,
            expires_millis,
            rest: cursor.rest().to_vec(),
        })
    }
}

/// A reader over a token's payload, which never reads past the end of it.
///
/// Given: it is a byte cursor, not a decision, and the same one the project's reader is written
/// with for the same reason — a payload arrives from a peer, so a length in it is a claim rather
/// than a fact.
struct Cursor<'a> {
    /// The payload.
    bytes: &'a [u8],
    /// How far into it this reader has got.
    at: usize,
}

impl<'a> Cursor<'a> {
    /// A reader at the start of `bytes`.
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// The next `count` bytes.
    fn take(&mut self, count: usize) -> Result<&'a [u8], Refusal> {
        let end = self.at.checked_add(count).ok_or_else(|| {
            invalid(format!(
                "a field of {count} byte(s) cannot fit in a payload of {} byte(s)",
                self.bytes.len()
            ))
        })?;
        let slice = self.bytes.get(self.at..end).ok_or_else(|| {
            invalid(format!(
                "a field of {count} byte(s) starts at {} and the payload is {} byte(s) long",
                self.at,
                self.bytes.len()
            ))
        })?;
        self.at = end;
        Ok(slice)
    }

    /// The next two bytes, little endian.
    fn u16(&mut self) -> Result<u16, Refusal> {
        let bytes = self.take(2)?;
        let mut array = [0u8; 2];
        array.copy_from_slice(bytes);
        Ok(u16::from_le_bytes(array))
    }

    /// The next eight bytes, little endian.
    fn u64(&mut self) -> Result<u64, Refusal> {
        let bytes = self.take(8)?;
        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(array))
    }

    /// Everything that is left.
    fn rest(&self) -> &'a [u8] {
        self.bytes.get(self.at..).unwrap_or_default()
    }
}

/// What the daemon found where the store keeps its ring.
///
/// The project reads a directory per connection; a probe cannot make a store, so the directory
/// arrives as the two facts that decide everything — is it there, and what is in it. The order a
/// directory lists its files in is not a promise, and this type does not pretend otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingSource<'a> {
    /// There is no `keys/` directory ([`DIRECTORY`]) below the store: authentication never was
    /// turned on.
    NoDirectory,
    /// That directory is there, and these are the key files (`<id>.<SUFFIX>`) it holds, as an
    /// operator wrote them, in the order the directory handed them over. The order is not a
    /// promise: a directory is not sorted, and the ring is.
    Directory(&'a [&'a str]),
}

/// The ring: the keys a store authenticates with, and the acts an operator has on them.
///
/// Sorted by `(not_before_millis, id)` and never reordered: a listing prints the ring oldest first,
/// the preferred key is the last open one, and two machines reading the same directory agree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyRing {
    /// Every key on the ring, oldest `not_before` first, ties by id.
    keys: Vec<Key>,
}

impl KeyRing {
    /// Every key on the ring, oldest `not_before` first, ties by id.
    #[must_use]
    pub fn keys(&self) -> &[Key] {
        &self.keys
    }

    /// `true` while the ring holds a key at all: a ring that exists authenticates, whatever its
    /// keys' windows say.
    #[must_use]
    pub fn is_open(&self) -> bool {
        !self.keys.is_empty()
    }

    /// The ring below a store's `keys/` directory.
    ///
    /// # Errors
    ///
    /// [`Refusal::EmptyRing`] when the directory is there and holds no key file, and
    /// [`Refusal::Invalid`] when one of the files it holds will not read.
    pub fn load(source: RingSource<'_>) -> Result<Self, Refusal> {
        let _ = source;
        todo!(
            "sec/secrets-management@design: the ring a store's `keys/` directory is, and the \
             difference between a store without one and one whose is empty"
        )
    }

    /// One key file's text onto the ring: an operator's `--add`.
    ///
    /// The key's window is what the file says, and the ring is put back in its own order.
    ///
    /// # Errors
    ///
    /// [`Refusal::Invalid`] when the text is not a key file, and [`Refusal::AlreadyOnTheRing`]
    /// when the id is already there.
    pub fn add(&mut self, text: &str) -> Result<KeyId, Refusal> {
        let _ = (self, text);
        todo!(
            "sec/secrets-management@design: one key file onto the ring, in the ring's order and \
               never over a key that is already there"
        )
    }

    /// The key that signs: the open key with the latest `not_before`, ties by id.
    ///
    /// A property of the files rather than of the clock, which is why the answer does not depend on
    /// an instant: it is what makes a rotation an edit to a file rather than a moment in the daemon.
    #[must_use]
    pub fn preferred(&self) -> Option<&Key> {
        todo!("sec/secrets-management@design: the newest open key, ties by id")
    }

    /// Write the end of one key's window: an operator's `--retire`.
    ///
    /// `at` is the instant the operator asked for, or `None` for the earliest instant the
    /// arithmetic allows — which is what `lakeletq rotate --retire` with no `--at` means. The answer
    /// is the instant the key now carries.
    ///
    /// # Errors
    ///
    /// [`Refusal::UnknownKey`] when the ring does not hold the id, [`Refusal::AlreadyRetired`] when
    /// its end is already written, [`Refusal::TooEarly`] under the safe instant, and
    /// [`Refusal::NoSuccessor`] when nothing newer has taken the signing over.
    pub fn retire(&mut self, id: &KeyId, at: Option<u64>) -> Result<u64, Refusal> {
        let _ = (self, id, at);
        todo!(
            "sec/secrets-management@design: the end of one key's window, refused under the instant \
             the arithmetic allows"
        )
    }

    /// The instant after which no token this key signed can still be live, or `None` when the ring
    /// cannot prove one.
    ///
    /// `None` is the honest answer for the key that signs: nothing has taken the signing over from
    /// it, so it may still sign and no instant after which its tokens are all gone is knowable from
    /// the ring. A key the ring does not hold has no window either.
    #[must_use]
    pub fn earliest_safe_retirement(&self, id: &KeyId) -> Option<u64> {
        let _ = id;
        todo!(
            "sec/secrets-management@design: the last instant this key could have signed, plus this \
             deployment's longest token lifetime"
        )
    }

    /// This ring's verdict on one token at one instant.
    ///
    /// The checks are in the order a reader would ask them, and the order is part of the answer —
    /// a token whose key has left *and* whose window has closed is a retirement, because a client
    /// whose key is gone has to be re-keyed first. The answer is the key that signed it.
    ///
    /// # Errors
    ///
    /// One of the four token refusals, in that order: [`Refusal::UnknownKey`],
    /// [`Refusal::NotYetValid`], [`Refusal::KeyRetired`], [`Refusal::Expired`].
    pub fn accepts(&self, token: &Token, at: u64) -> Result<KeyId, Refusal> {
        let _ = (self, token, at);
        todo!(
            "sec/secrets-management@design: is this a key I have, is its window open, has it been \
             retired, and is the token's own window still open"
        )
    }
}

/// One [`Refusal::Invalid`].
fn invalid(detail: impl Into<String>) -> Refusal {
    Refusal::Invalid {
        detail: detail.into(),
    }
}

/// One `u64` field of a key file, with the line it was on.
fn millis(line: usize, text: &str) -> Result<u64, Refusal> {
    text.parse::<u64>().map_err(|_| {
        invalid(format!(
            "line {line}: `{text}` is not a number of milliseconds since the Unix epoch"
        ))
    })
}

/// The bytes a secret written as hex means.
///
/// The one hex rule of this project's ring, used by the file reader and by the operator's `--add`:
/// a secret is key material, and "it looked close enough" is not a thing a MAC can be built from.
fn secret_bytes(secret_hex: &str) -> Result<Vec<u8>, String> {
    let text = secret_hex.trim();
    if text.is_empty() {
        return Err("empty: a secret is hex and this one is nothing".to_owned());
    }
    if !text.len().is_multiple_of(2) {
        return Err(format!(
            "`{text}`, which has an odd number of digits: a byte is two of them"
        ));
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    let mut digits = text.bytes();
    while let (Some(high), Some(low)) = (digits.next(), digits.next()) {
        let Some(high) = nibble(high) else {
            return Err(format!("`{text}`: `{}` is not a hex digit", high as char));
        };
        let Some(low) = nibble(low) else {
            return Err(format!("`{text}`: `{}` is not a hex digit", low as char));
        };
        bytes.push(high * 16 + low);
    }
    Ok(bytes)
}

/// One hex digit's value.
fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An instant every fixture counts from: 2023-11-14T22:13:20Z, a round number and not a date.
    const BASE: u64 = 1_700_000_000_000;

    /// How long a token lives where a case is not about its lifetime.
    const LIFETIME: u64 = 60_000;

    /// A secret of `label`'s own: hex, distinct per label so two keys cannot share one.
    fn secret(label: &str) -> String {
        let mut text = String::new();
        for index in 0..32u8 {
            let byte = label.as_bytes().iter().fold(index, |value, byte| {
                value.wrapping_mul(31).wrapping_add(*byte)
            });
            text.push_str(&format!("{byte:02x}"));
        }
        text
    }

    /// One key file, written the way `lakeletq rotate --add` writes it.
    fn key_file(id: &str, not_before_millis: u64, not_after_millis: Option<u64>) -> String {
        let mut text = format!(
            "id={id}\nsecret={}\nnot_before_millis={not_before_millis}\n",
            secret(id)
        );
        if let Some(end) = not_after_millis {
            text.push_str(&format!("not_after_millis={end}\n"));
        }
        text
    }

    /// The ring a store whose `keys/` directory holds these keys has.
    fn ring_of(keys: &[(&str, u64)]) -> KeyRing {
        let files: Vec<String> = keys
            .iter()
            .map(|(id, from)| key_file(id, *from, None))
            .collect();
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        KeyRing::load(RingSource::Directory(&refs)).expect("a directory with keys in it loads")
    }

    /// The names of a ring's keys, in the ring's order.
    fn names(ring: &KeyRing) -> Vec<&str> {
        ring.keys().iter().map(|key| key.id().as_str()).collect()
    }

    /// An absent ring is authentication off; a directory that holds no key is the other answer.
    #[test]
    fn an_absent_ring_is_authentication_off_and_a_directory_with_no_key_refuses() {
        let off = KeyRing::load(RingSource::NoDirectory).expect("no directory is not an error");
        assert!(
            !off.is_open(),
            "a store with no `keys/` directory never turned authentication on: {:?}",
            off.keys()
        );
        assert!(
            off.keys().is_empty(),
            "and it holds no key: {:?}",
            off.keys()
        );
        assert!(off.preferred().is_none(), "so nothing signs on it");

        let empty = KeyRing::load(RingSource::Directory(&[]));
        assert_eq!(
            empty,
            Err(Refusal::EmptyRing),
            "a directory that exists and holds no key is a deployment that meant to authenticate, \
             and a ring that lets everybody in is the one failure a ring must not have"
        );

        let good = key_file("k1", BASE, None);
        let broken = "id=k2\nsecret=not-hex\nnot_before_millis=1\n";
        assert!(
            KeyRing::load(RingSource::Directory(&[good.as_str()])).is_ok(),
            "one key file that reads is a ring"
        );
        assert!(
            KeyRing::load(RingSource::Directory(&[good.as_str(), broken])).is_err(),
            "and a ring one of whose files will not read is not a ring holding the other one: \
             failing open is the other half of the empty-directory bug"
        );
    }

    /// The ring's order is the arithmetic's, and a key is added and never replaced.
    #[test]
    fn the_ring_is_oldest_first_and_a_key_already_on_it_is_never_replaced() {
        let older = key_file("k1", BASE, None);
        let newer = key_file("k2", BASE + 1_000_000, None);
        let mut ring = KeyRing::load(RingSource::Directory(&[newer.as_str(), older.as_str()]))
            .expect("two keys load");
        assert_eq!(
            names(&ring),
            ["k1", "k2"],
            "a directory hands its files over in whatever order it likes, and the ring does not: \
             the listing is oldest first, which is the order a rotation happens in"
        );
        assert!(
            !format!("{ring:?}").contains(&secret("k1")),
            "and the ring does not print a secret when it is printed: a key that did would put that \
             secret in a log aggregator"
        );

        assert_eq!(
            ring.add(&key_file("k3", BASE + 500_000, None)),
            Ok(KeyId::new("k3")),
            "a key that is not on the ring is added"
        );
        assert_eq!(
            names(&ring),
            ["k1", "k3", "k2"],
            "and it lands in its place rather than at the end: the ring's order is the arithmetic's, \
             not the order an operator typed in"
        );
        assert_eq!(
            ring.add(&key_file("k3", BASE + 900_000, None)),
            Err(Refusal::AlreadyOnTheRing {
                id: KeyId::new("k3")
            }),
            "a key is added and never replaced: swapping the secret under a live client locks out \
             every token signed with the one it holds"
        );
        assert_eq!(
            ring.keys()[1].not_before_millis(),
            BASE + 500_000,
            "and the refused add left the key it refused exactly where it was"
        );
    }

    /// The key that signs: newest open, ties by id, and never a retired one.
    #[test]
    fn the_key_that_signs_is_the_newest_open_one_and_a_retired_key_never_signs() {
        let two = ring_of(&[("k1", BASE), ("k2", BASE + 1_000_000)]);
        assert_eq!(
            two.preferred().map(Key::id).map(KeyId::as_str),
            Some("k2"),
            "the open key with the latest `not_before` is the one that signs, which is what makes a \
             new token name the new key"
        );

        let tied = ring_of(&[("k1", BASE), ("k3", BASE)]);
        assert_eq!(
            tied.preferred().map(Key::id).map(KeyId::as_str),
            Some("k3"),
            "two keys that arrived at the same instant are still one order, and it is the ring's \
             rather than a directory listing's: the greater id signs"
        );

        let retired = key_file("k2", BASE + 1_000_000, Some(BASE + 2_000_000));
        let open = key_file("k1", BASE, None);
        let mixed =
            KeyRing::load(RingSource::Directory(&[retired.as_str(), open.as_str()])).expect("load");
        assert_eq!(
            mixed.preferred().map(Key::id).map(KeyId::as_str),
            Some("k1"),
            "a retired key never signs, so retiring the newest one hands the signing back to the key \
             below it rather than leaving the ring with a preferred key it cannot use"
        );

        let all = KeyRing::load(RingSource::Directory(&[
            key_file("k1", BASE, Some(BASE + 2_000_000)).as_str(),
            key_file("k2", BASE + 1_000, Some(BASE + 2_000_000)).as_str(),
        ]))
        .expect("a ring whose every key was retired still loads");
        assert!(all.preferred().is_none(), "and none of them signs");
        assert!(
            all.is_open(),
            "the ring still authenticates, because it holds keys: it is the signing that has stopped"
        );
    }

    /// A rotation moves the key a new token names, and nothing about an old one.
    #[test]
    fn a_new_token_names_the_key_that_signs_and_a_rotation_leaves_an_old_one_alone() {
        let mut ring = ring_of(&[("k1", BASE)]);
        let issued = BASE + 1_000;
        let before = Token::new(
            ring.preferred().expect("one key signs").id().clone(),
            issued,
            issued + LIFETIME,
            b"opaque",
        );
        let payload = before.encode();
        assert_eq!(before.key_id().as_str(), "k1");
        assert_eq!(
            &payload[..2],
            &(before.key_id().as_str().len() as u16).to_le_bytes(),
            "the payload opens with the length of the key id"
        );
        assert_eq!(
            &payload[2..4],
            b"k1",
            "and the key id is the first thing in it: a reader has to know which key to check the \
             rest against before it can check anything"
        );

        assert_eq!(
            ring.add(&key_file("k2", issued, None)),
            Ok(KeyId::new("k2")),
            "the successor is added, which is the whole rotation"
        );
        let after = Token::new(
            ring.preferred().expect("the new key signs").id().clone(),
            issued + 1,
            issued + 1 + LIFETIME,
            b"opaque",
        );
        assert_eq!(
            after.key_id().as_str(),
            "k2",
            "a new token names the key that took the signing over"
        );
        assert_eq!(
            before.encode(),
            payload,
            "and nothing about the old one moved: a rotation edits a file rather than rewriting the \
             tokens already out there"
        );
        assert_eq!(
            ring.accepts(&before, issued + 1),
            Ok(KeyId::new("k1")),
            "the key that no longer signs still verifies what it signed, which is the window a \
             retirement has to respect"
        );
    }

    /// The number the whole step exists for, and the key that has none.
    #[test]
    fn the_window_is_the_takeover_plus_the_longest_lifetime_and_none_for_the_key_that_signs() {
        let ring = ring_of(&[("k1", BASE), ("k2", BASE + 1_000_000)]);
        let safe = BASE + 1_000_000 + Window::MAX_TOKEN_LIFETIME_MILLIS;
        assert_eq!(
            ring.earliest_safe_retirement(&KeyId::new("k1")),
            Some(safe),
            "the last instant k1 could have signed is the instant k2 took the signing over, and its \
             tokens are live until then plus the longest lifetime this deployment may issue"
        );
        assert_eq!(
            Window::MAX_TOKEN_LIFETIME_MILLIS,
            3_600_000,
            "an hour, stated once: a bound built from the lifetime clients usually ask for is a \
             bound that fails for the one client that asked for the longest token"
        );
        assert_eq!(
            ring.earliest_safe_retirement(&KeyId::new("k2")),
            None,
            "the key that signs has no such instant: nothing has taken the signing over from it, so \
             the ring says it cannot prove a bound rather than guessing one"
        );
        assert_eq!(
            ring.earliest_safe_retirement(&KeyId::new("nobody")),
            None,
            "a key this ring does not hold has no window either"
        );
    }

    /// The arithmetic is a refusal with the instant in it, and the refused call writes nothing.
    #[test]
    fn a_retirement_one_millisecond_early_is_refused_and_the_sentence_names_the_instant() {
        let mut ring = ring_of(&[("k1", BASE), ("k2", BASE + 1_000_000)]);
        let safe = BASE + 1_000_000 + Window::MAX_TOKEN_LIFETIME_MILLIS;
        let refused = ring
            .retire(&KeyId::new("k1"), Some(safe - 1))
            .expect_err("one millisecond under the safe instant is a refusal");
        assert_eq!(
            refused,
            Refusal::TooEarly {
                id: KeyId::new("k1"),
                safe_at: safe
            },
            "a retirement that strands a live token is refused rather than warned about, because \
             the client holding that token is not in this process and cannot be asked"
        );
        assert!(
            refused.to_string().contains(&safe.to_string()),
            "and the one sentence names the instant the operator has to wait for: {refused}"
        );
        assert_eq!(
            ring.keys()[0].not_after_millis(),
            None,
            "the refused call left the key open: a warning would have written the file"
        );
        assert_eq!(
            ring.retire(&KeyId::new("nobody"), None),
            Err(Refusal::UnknownKey {
                id: KeyId::new("nobody")
            }),
            "and a key the ring does not hold cannot be retired"
        );
    }

    /// The instant itself is allowed, and it is exactly the last instant of the last token.
    #[test]
    fn retiring_at_the_safe_instant_is_accepted_and_takes_the_last_token_with_it() {
        let mut ring = ring_of(&[("k1", BASE)]);
        let issued = BASE + 1_000_000;
        let last = Token::new(
            KeyId::new("k1"),
            issued,
            issued + Window::MAX_TOKEN_LIFETIME_MILLIS,
            b"opaque",
        );
        assert_eq!(
            ring.add(&key_file("k2", issued, None)),
            Ok(KeyId::new("k2")),
            "the successor arrives at the instant the token was issued"
        );
        let safe = issued + Window::MAX_TOKEN_LIFETIME_MILLIS;
        assert_eq!(
            ring.earliest_safe_retirement(&KeyId::new("k1")),
            Some(safe),
            "so k1 stops signing at that instant and its tokens are live until `safe`"
        );
        assert_eq!(
            ring.retire(&KeyId::new("k1"), None),
            Ok(safe),
            "`--retire` with no `--at` means the earliest safe instant, and the safe instant itself \
             is allowed: it is the boundary and not one millisecond under it"
        );

        let left = ring.accepts(&last, safe - 1);
        let right = ring.accepts(&last, safe);
        assert_eq!(
            left,
            Ok(KeyId::new("k1")),
            "one millisecond before its end the last token k1 signed is still good: left {left:?} / \
             right {right:?}"
        );
        assert_eq!(
            right,
            Err(Refusal::KeyRetired {
                id: KeyId::new("k1"),
                retired_millis: safe
            }),
            "and at the instant itself it is a retirement rather than an expiry, because a client \
             whose key is gone has to be re-keyed first: left {left:?} / right {right:?}"
        );
        assert_eq!(
            ring.retire(&KeyId::new("k1"), None),
            Err(Refusal::AlreadyRetired {
                id: KeyId::new("k1"),
                retired_millis: safe
            }),
            "a retirement is written once"
        );
        assert_eq!(
            ring.preferred().map(Key::id).map(KeyId::as_str),
            Some("k2"),
            "and the ring never went a moment without a key that signs"
        );
    }

    /// A key the ring does not hold, and both directions of a window that has not opened.
    #[test]
    fn accepts_refuses_a_key_it_does_not_hold_and_a_window_that_has_not_opened_both_ways() {
        let ring = ring_of(&[("k1", BASE)]);
        let stranger = Token::new(KeyId::new("nobody"), BASE, BASE + LIFETIME, b"opaque");
        assert_eq!(
            ring.accepts(&stranger, BASE),
            Err(Refusal::UnknownKey {
                id: KeyId::new("nobody")
            }),
            "a token signed by a key this deployment does not have is refused as that, before \
             anything about its window is read"
        );

        let early = Token::new(KeyId::new("k1"), BASE - 5_000, BASE + LIFETIME, b"opaque");
        assert_eq!(
            ring.accepts(&early, BASE),
            Err(Refusal::NotYetValid {
                not_before_millis: BASE
            }),
            "a key whose window has not opened cannot have signed, so a token that claims it did is \
             not a token"
        );
        let token = Token::new(KeyId::new("k1"), BASE + 10_000, BASE + LIFETIME, b"opaque");
        let too_early = ring.accepts(&token, BASE + 5_000);
        assert_eq!(
            too_early,
            Err(Refusal::NotYetValid {
                not_before_millis: BASE + 10_000
            }),
            "and the other direction is the same class and a different instant: a connection that \
             arrives before the token was issued is inside a window that opens at the token's own \
             instant: {too_early:?}"
        );
        assert_eq!(
            ring.accepts(&token, BASE + 10_000),
            Ok(KeyId::new("k1")),
            "the first instant of the window is inside it"
        );
    }

    /// The token's own edge, and the sentence a stranded operator reads.
    #[test]
    fn an_expired_token_names_both_instants_and_a_retired_key_names_the_instant_it_left() {
        let ring = ring_of(&[("k1", BASE)]);
        let token = Token::new(
            KeyId::new("k1"),
            BASE + 1_000,
            BASE + 1_000 + LIFETIME,
            b"opaque",
        );
        assert!(
            ring.accepts(&token, BASE + LIFETIME).is_ok(),
            "one millisecond before the token's own edge it is still good"
        );
        let expired = ring.accepts(&token, BASE + 1_000 + LIFETIME);
        assert_eq!(
            expired,
            Err(Refusal::Expired {
                expires_millis: BASE + 1_000 + LIFETIME,
                now_millis: BASE + 1_000 + LIFETIME
            }),
            "and at the edge it is expired: the sentence carries the instant it expired at and the \
             instant the connection arrived at, because an operator reading it has to know which of \
             the two clocks moved"
        );
        let said = expired
            .as_ref()
            .expect_err("the token's own window has closed")
            .to_string();
        assert!(
            said.contains(&(BASE + 1_000 + LIFETIME).to_string()),
            "and the instant is in the sentence a log line carries: {said}"
        );

        let mut retired = ring_of(&[("k1", BASE)]);
        assert_eq!(
            retired.add(&key_file("k2", BASE + 1_000, None)),
            Ok(KeyId::new("k2"))
        );
        let safe = BASE + 1_000 + Window::MAX_TOKEN_LIFETIME_MILLIS;
        assert_eq!(retired.retire(&KeyId::new("k1"), None), Ok(safe));
        let stranded = retired
            .accepts(&token, safe)
            .expect_err("the key that signed it has left");
        assert_eq!(
            stranded,
            Refusal::KeyRetired {
                id: KeyId::new("k1"),
                retired_millis: safe
            },
            "a token whose key has left is a retirement even though its own window closed long \
             before: the class is what a script branches on"
        );
        assert!(
            stranded.to_string().contains(&safe.to_string()),
            "and the sentence names the instant the key left, which is the one an operator acts on: \
             {stranded}"
        );
    }

    /// The format: the key first, the rest carried, and the ring accepting what it wrote.
    #[test]
    fn a_token_round_trips_through_its_payload_and_the_ring_accepts_what_it_wrote() {
        let ring = ring_of(&[("k1", BASE)]);
        let rest: &[u8] = b"the bytes a tag covers, which this file does not read";
        let token = Token::new(KeyId::new("k1"), BASE, BASE + LIFETIME, rest);
        let payload = token.encode();
        let back = Token::decode(&payload).expect("what `encode` wrote is a token");
        assert_eq!(
            back, token,
            "a payload a reader cannot read back is not a format"
        );
        assert_eq!(
            back.rest(),
            rest,
            "and the rest travels byte for byte: a reader that dropped it would throw away exactly \
             the bytes a tag is computed over"
        );
        assert_eq!(back.issued_millis(), BASE);
        assert_eq!(back.expires_millis(), BASE + LIFETIME);
        assert_eq!(
            ring.accepts(&back, BASE),
            Ok(KeyId::new("k1")),
            "and what survived the wire is the token the ring that signed it accepts: the payload \
             and the verdict are one format"
        );

        assert!(
            Token::decode(&payload[..3]).is_err(),
            "a payload that stops in the middle of the key id is not a token at all"
        );
        assert!(
            Token::decode(&Vec::new()).is_err(),
            "and a first payload of nothing is not one either: `lakelet/4.6-ingest-backpressure` \
             and `lakelet/4.17-soak` connect without a token, and the ring is what tells a client \
             that sent one apart from a client that sent half of one"
        );
    }
}
