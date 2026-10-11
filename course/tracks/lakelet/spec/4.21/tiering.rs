//! Spec for `lakelet/4.21-cold-tiering`: one suite, two implementations of the tier's port.
//!
//! `kiln run` copies this file into the project as `tests/spec_tiering.rs`; it is the course's file,
//! not the learner's (plan §3 D2). Twenty-seven cases, and the shape of them is the point:
//!
//! * **one suite, not two.** The fixture (`tests/support/tier.rs`) builds the tier's port from
//!   `KILN_BACKEND`, which `conformance.toml` and the `conformance@2` gate are what name: `local` is
//!   `src/engine/tier/filesystem.rs` on a directory, `s3` is `src/engine/tier/s3.rs` against an
//!   in-process endpoint (`tests/s3_stub.rs`). Every case below is written once and run twice, and no
//!   case about the *rule* — cold, the ordering, hydration — may depend on which implementation it
//!   happens to be on. That is the whole claim of the gate: a tier that works against a directory
//!   and breaks against a bucket has a bug the developer's own machine cannot see;
//! * **and the half that is about the client runs in both.** A signature, a payload hash, a retry, a
//!   request's shape, a page token: those are questions about `http.rs` and `s3.rs` rather than about
//!   the port, and the fixture builds an S3-shaped endpoint on *every* run for them. What
//!   `KILN_BACKEND` decides is which implementation the cases below are run *through* — and leaving
//!   the client's half out of the `local` run would leave it out of the mutation walk, which runs
//!   this suite exactly once;
//! * **the port is blobs.** `put`/`get`/`delete`/`list`, absence as `None`, `delete` idempotent, and
//!   a listing that **pages**: a page is a continuation of the page before it, a small page still
//!   answers everything because the loop (`tier::listing`) is the thing that walks it, and the keys
//!   are a function of the segment's identity rather than of an instant;
//! * **the rule is a value.** A segment is cold exactly when its last stated timestamp is below
//!   `now - cold_after_millis`, the boundary is one millisecond wide, and a segment whose age nobody
//!   wrote down is never moved;
//! * **the ordering is the safety argument, and it is judged.** `sweep` puts the object first and
//!   removes the local file second, so a crash in the window leaves the segment in both places and
//!   the store readable; the case that fails a delete after a successful put is what makes that a
//!   test rather than a paragraph;
//! * **hydration is what keeps the engine's readers untouched.** A store whose cold segments are out
//!   of the box is not openable (`Segment` open refuses the file that is not there, and the refusal
//!   names it), and `hydrate` is what puts the bytes back — checking the length the manifest states
//!   *before* it writes, and leaving nothing behind when the tier's copy disagrees. What it takes is
//!   the store's **directory** rather than a live store, so a store whose process has stopped with
//!   its segments in the tier is put back before it is opened instead of being unopenable;
//! * **the HTTP half is judged where it exists.** Five cases are the S3-shaped half and say so: the
//!   signed request is pinned against a vector from the vendor's own documentation, the payload hash
//!   is the hash of the body that was sent, a `5xx` is retried and a `403` is not, the request shape
//!   is asserted field by field out of the endpoint's log — which is what `outbound/http-client`
//!   at *design* depth means — and the second page is asked for with the token the first page
//!   returned;
//! * **the engine boundary holds with a port in it.** `src/engine/tier/**` names no service-layer
//!   module and no command line, and nothing in it reads a clock: a pass is applied at a `now` a
//!   caller hands it, and the instant a signature carries is a value in a config.

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lakelet::engine::compaction::{self, SegmentInfo, Segments};
use lakelet::engine::tier::{
    Cursor, Error, Objects, Page, Policy, Report, http, hydrate, listing, object_key, s3, sweep,
};
use lakelet::protocol::batch::Record;

use support::tier::{self, Tier};

/// How many segments the fixture's store holds where a case is about a store.
const SEGMENTS: u64 = 4;

/// How many keys a case plants where it is about the listing.
const KEYS: u64 = 6;

/// A policy that calls every segment cold: `now` is always past the store's last record.
const ALL_COLD: u64 = 0;

/// The instant a case applies a policy at when it wants the whole store cold.
fn after_the_store() -> u64 {
    tier::EPOCH + 8 * tier::HOUR
}

/// The canonical request of the vendor's own signed-request example, byte for byte.
///
/// It is here rather than computed because that is what makes the signature case evidence: a pinned
/// value this build produced would only say "this build still does what it did", while this one says
/// the SigV4 this build implements is the SigV4 the vendor documents. `GET /` with an empty body on
/// `example.amazonaws.com`, dated `20150830T123600Z`, in `us-east-1` for a service called
/// `service` — the request the vendor's "get-vanilla" test case signs.
const CANONICAL: &str = "GET\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\n\
                         host;x-amz-date\n\
                         e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// The signature the vendor's documentation gives for [`CANONICAL`].
const SIGNATURE: &str = "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31";

/// The access key of the vendor's own example.
const EXAMPLE_ACCESS: &str = "AKIDEXAMPLE";

/// The secret of the vendor's own example.
const EXAMPLE_SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

/// The sha256 of the body `b"lakelet"`, as hex: a payload hash a case can state rather than derive.
const LAKELET_SHA256: &str = "78697ff2d723760d969caec48b72ccb915ecfb9e433b3be7449effe8638f7647";

/// `true` when this run is the S3-shaped backend's.
fn is_s3(tier: &Tier) -> bool {
    tier.backend() == "s3"
}

/// The path of one of the step's own source files.
fn source(relative: &str) -> String {
    std::fs::read_to_string(relative).unwrap_or_else(|error| {
        panic!("{relative} cannot be read: {error} — this case is about it")
    })
}

/// Every source file of the tier module, as `(path, text)`.
fn tier_sources() -> Vec<(&'static str, String)> {
    [
        "src/engine/tier/mod.rs",
        "src/engine/tier/filesystem.rs",
        "src/engine/tier/http.rs",
        "src/engine/tier/s3.rs",
    ]
    .into_iter()
    .map(|path| (path, source(path)))
    .collect()
}

/// Every file below `root`, as paths relative to it, in order.
///
/// A store's *shape* on the device: what a pass that promised to write nothing durable has to leave
/// alone, and what unchanged is the evidence for that promise.
fn stored(root: &Path) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut pending: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if let Ok(relative) = path.strip_prefix(root) {
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found.sort();
    found
}

/// The report one pass answered, or a panic naming the pass that refused.
fn ran(what: &str, outcome: Result<Report, Error>) -> Report {
    outcome.unwrap_or_else(|error| {
        panic!("{what} is a pass this store and tier can carry out: {error}")
    })
}

/// A tier whose write does not answer: how a crash inside a pass's window is made deterministic.
///
/// `lakelet/4.21`'s ordering argument is about a machine that dies between two writes, and a machine
/// cannot be asked to die at a chosen instruction. What *can* be asked for is a tier whose write does
/// not come back, which is the same state from the pass's point of view — and it is the realistic
/// version of the window rather than an artificial one: a `put` that was carried out and whose answer
/// was lost on the way back is the ordinary case a client's retry rule exists for. `writes` decides
/// which side of the window the pass stops on: a write that landed and did not answer, or one that
/// never reached the tier at all.
struct Losing<'a> {
    /// The port that does the work.
    inner: &'a dyn Objects,
    /// `true` when the object reaches the tier before the answer is lost.
    writes: bool,
}

impl Objects for Losing<'_> {
    fn prefix(&self) -> &str {
        self.inner.prefix()
    }

    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), Error> {
        if self.writes {
            self.inner.put(key, bytes)?;
        }
        Err(Error::Transport {
            detail: format!(
                "the endpoint {} `{key}` and the answer never came back",
                if self.writes { "took" } else { "never saw" }
            ),
        })
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        self.inner.get(key)
    }

    fn delete(&self, key: &str) -> Result<(), Error> {
        self.inner.delete(key)
    }

    fn list(&self, prefix: &str, cursor: Option<Cursor>) -> Result<Page, Error> {
        self.inner.list(prefix, cursor)
    }
}

/// A port whose listing never advances: a page that names itself as the page after it.
///
/// A client that took such an endpoint's word for it would ask forever, and a client that stopped
/// quietly would report a third of a tier as the whole of it. Neither is an answer, which is what
/// the case below establishes.
struct Stuck {
    /// The prefix it answers under.
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
        let token =
            Cursor::new(cursor.map_or_else(|| "a/1".to_owned(), |asked| asked.as_str().to_owned()));
        Ok(Page {
            keys: vec!["a/1".to_owned()],
            next: Some(token),
        })
    }
}

/// The report a case reads, as `name=value` lines: one line per counter, so a failure says which.
fn lines(report: &Report) -> String {
    [
        format!("moved={}", report.moved()),
        format!("fetched={}", report.fetched()),
        format!("kept={}", report.kept()),
        format!("bytes={}", report.bytes()),
        format!("pages={}", report.pages()),
    ]
    .join(" ")
}

#[test]
fn put_then_get_answers_the_same_bytes_and_an_unwritten_key_is_absent() {
    let fixture = Tier::start("put-get");
    let port = fixture.objects();
    port.put("a/one", b"the bytes")
        .unwrap_or_else(|error| panic!("a port takes a key and a body: {error}"));
    assert_eq!(
        port.get("a/one")
            .unwrap_or_else(|error| panic!("a key that was just written reads back: {error}")),
        Some(b"the bytes".to_vec()),
        "the bytes a `put` was handed are the bytes a `get` answers, on {}",
        fixture.backend()
    );
    assert_eq!(
        port.get("a/two").unwrap_or_else(|error| panic!(
            "asking for a key nobody wrote is not a \
            refusal: {error}"
        )),
        None,
        "an object nobody wrote is absence rather than an error: `hydrate` asks about keys that \
         were never there on every cold store, and a tier that answered a refusal would make the \
         ordinary case look like a fault"
    );
    assert_ne!(
        port.get("a/one").unwrap_or_default(),
        port.get("a/two").unwrap_or_default(),
        "and the two are told apart: absent is `None`, not an empty object"
    );
}

#[test]
fn delete_removes_and_deleting_twice_is_not_a_failure() {
    let fixture = Tier::start("delete");
    let port = fixture.objects();
    port.put("a/one", b"bytes")
        .unwrap_or_else(|error| panic!("a port takes a key and a body: {error}"));
    port.delete("a/one")
        .unwrap_or_else(|error| panic!("a key that is there can be removed: {error}"));
    assert_eq!(
        port.get("a/one")
            .unwrap_or_else(|error| panic!("a removed key reads back: {error}")),
        None,
        "a `delete` removes what it names"
    );
    port.delete("a/one").unwrap_or_else(|error| {
        panic!(
            "removing a key that is already gone is the state the call asked for rather than a \
             failure — a pass that deleted twice, or one whose object a bucket policy had already \
             removed, is not a pass that failed: {error}"
        )
    });
}

#[test]
fn a_listing_answers_one_prefix_in_order_and_a_page_continues_the_one_before_it() {
    let fixture = Tier::start("listing-pages");
    let port = fixture.objects();
    // Written out of order, and with a sibling prefix that must not appear: the order a listing
    // answers in is the port's, not the order the keys were written in.
    for key in [
        "segments/0000000002.lks",
        "index/0000000000.lki",
        "segments/0000000000.lks",
        "segments/0000000001.lks",
    ] {
        port.put(key, b"x")
            .unwrap_or_else(|error| panic!("`{key}` is a key this port takes: {error}"));
    }
    let first = port
        .list("segments/", None)
        .unwrap_or_else(|error| panic!("the port lists a prefix: {error}"));
    assert_eq!(
        first.keys,
        vec![
            "segments/0000000000.lks".to_owned(),
            "segments/0000000001.lks".to_owned()
        ],
        "a page holds the keys under the prefix, in key order, and only those — the index key is \
         another prefix and the keys were written in another order"
    );
    let Some(next) = first.next.clone() else {
        panic!(
            "the first page is not the last one: the port holds more keys under the prefix than \
             one page holds ({}), and a page with no `next` would be a listing that stopped early",
            tier::PAGE
        );
    };
    let second = port
        .list("segments/", Some(next))
        .unwrap_or_else(|error| panic!("a page's own cursor asks for the page after it: {error}"));
    assert_eq!(
        second.keys,
        vec!["segments/0000000002.lks".to_owned()],
        "the second page continues the first: no key is repeated and none is skipped"
    );
    assert_eq!(second.next, None, "and the last page says it is the last");
    let all = listing(&*port, "segments/")
        .unwrap_or_else(|error| panic!("the loop walks a port's pages: {error}"));
    assert_eq!(
        all.keys(),
        vec![
            "segments/0000000000.lks".to_owned(),
            "segments/0000000001.lks".to_owned(),
            "segments/0000000002.lks".to_owned()
        ],
        "and the loop's answer is every key under the prefix, in order: the two pages are one \
         listing"
    );
    assert_eq!(
        all.pages(),
        2,
        "in two pages, because one holds {}",
        tier::PAGE
    );
}

#[test]
fn a_small_page_still_answers_everything_and_the_walk_counts_its_pages() {
    let fixture = Tier::start("listing-loop");
    let port = fixture.objects();
    let prefix = fixture.prefix();
    let mut written: Vec<String> = Vec::new();
    for ordinal in 0..KEYS {
        let key = object_key(prefix, ordinal);
        port.put(&key, b"x")
            .unwrap_or_else(|error| panic!("`{key}` is a key this port takes: {error}"));
        written.push(key);
    }
    let all = listing(&*port, prefix)
        .unwrap_or_else(|error| panic!("the loop walks every page: {error}"));
    written.sort();
    assert_eq!(
        all.keys(),
        written,
        "a listing whose page is smaller than the tier answers the whole tier: {KEYS} keys, {} to \
         a page",
        tier::PAGE
    );
    assert_eq!(
        all.pages(),
        KEYS / tier::PAGE as u64,
        "and it took the number of pages the tier needs: {} keys over {} to a page is {} requests",
        KEYS,
        tier::PAGE,
        KEYS / tier::PAGE as u64
    );
    if is_s3(&fixture)
        && let Some(endpoint) = fixture.stub()
    {
        // The corroboration only the S3-shaped run has, and the reason this case is worth more
        // there than a count the loop computed itself: this is what the endpoint was asked, and the
        // endpoint answers a page **shorter** than the `max-keys` the client asked for. A client
        // that stopped at the end of the first page would report one third of the tier.
        assert_eq!(
            endpoint.calls_of("GET").len() as u64,
            all.pages(),
            "the endpoint saw one `ListObjectsV2` per page the loop read: {:?}",
            endpoint
                .calls_of("GET")
                .iter()
                .map(|call| call.line())
                .collect::<Vec<String>>()
        );
    }
}

#[test]
fn keys_are_the_segments_identity_and_two_stores_do_not_collide() {
    let fixture = Tier::start("keys");
    assert_eq!(
        object_key(fixture.prefix(), 7),
        object_key(fixture.prefix(), 7),
        "a segment's key is a function of its identity: the same segment gives the same key on \
         every run and every machine, which is what makes the key the idempotency key a `put` may \
         be repeated under"
    );
    assert_ne!(
        object_key("store-a", 7),
        object_key("store-b", 7),
        "and two stores handed one tier do not collide: the prefix is part of the key, so store \
         A's segment 7 and store B's segment 7 are two objects"
    );
    assert!(
        object_key("store-a", 7).ends_with("segments/0000000007.lks"),
        "the key is the name the store gives the segment's file, under the port's prefix — an \
         operator looking for the object of segment 7 finds the ordinal the manifest lists, \
         zero-padded the way a directory sorts: {}",
        object_key("store-a", 7)
    );

    let mine = fixture.port("store-a");
    let theirs = fixture.port("store-b");
    let store = fixture.store(2);
    let report = ran(
        "a sweep of two cold segments",
        sweep(&store, &*mine, &Policy::new(ALL_COLD), after_the_store()),
    );
    assert_eq!(
        report.moved(),
        2,
        "both segments are cold: {}",
        lines(&report)
    );
    let held = fixture.held("store-a");
    assert_eq!(
        held,
        vec![object_key("store-a", 0), object_key("store-a", 1)],
        "and what the tier holds is exactly the keys the function names"
    );
    assert!(
        fixture.held("store-b").is_empty(),
        "a second store's share of the same tier is empty: nothing store A wrote is visible under \
         store B's prefix, and the two cannot overwrite each other"
    );
    assert_eq!(
        theirs
            .get(&object_key("store-b", 0))
            .unwrap_or_else(|error| panic!("a key nobody wrote is not a refusal: {error}")),
        None,
        "nor can store B read store A's object by deriving its own key for the same ordinal"
    );
}

#[test]
fn sweep_moves_the_cold_segments_and_leaves_the_warm_ones_alone() {
    let fixture = Tier::start("sweep-cold");
    let store = fixture.store(SEGMENTS);
    let port = fixture.objects();
    let policy = Policy::new(2 * tier::HOUR);
    let now = tier::EPOCH + 4 * tier::HOUR;
    let report = ran(
        "a sweep of a store of four segments",
        sweep(&store, &*port, &policy, now),
    );
    assert_eq!(
        (report.moved(), report.kept()),
        (2, 2),
        "the two segments whose last record is older than the cutoff leave and the two that are \
         not stay: {} against a cutoff of {}",
        lines(&report),
        policy.cutoff(now)
    );
    let expected: u64 = store
        .manifest()
        .segments()
        .iter()
        .take(2)
        .map(|info| info.bytes)
        .sum();
    assert_eq!(
        report.bytes(),
        expected,
        "and the bytes a sweep reports are what the manifest says the segments it moved weigh, not \
         what a directory listing happens to hold: the manifest is the store"
    );
    for ordinal in 0..SEGMENTS {
        let path = tier::file_of(fixture.store_dir(), ordinal);
        assert_eq!(
            path.is_file(),
            ordinal >= 2,
            "{} is {} and it is {} for segment {ordinal}, whose records are {} the cutoff",
            path.display(),
            if ordinal >= 2 { "still here" } else { "gone" },
            if ordinal >= 2 {
                "a segment that moves"
            } else {
                "a segment that stays"
            },
            if ordinal >= 2 {
                "not older than"
            } else {
                "older than"
            }
        );
    }
    assert_eq!(
        fixture.held(fixture.prefix()),
        vec![
            object_key(fixture.prefix(), 0),
            object_key(fixture.prefix(), 1)
        ],
        "the tier holds the two that left, and only them"
    );
}

#[test]
fn the_round_trip_answers_record_for_record() {
    let fixture = Tier::start("round-trip");
    let store = fixture.store(SEGMENTS);
    let port = fixture.objects();
    let before: Vec<Record> = fixture.answers(&store);
    let out = ran(
        "a sweep of every segment",
        sweep(&store, &*port, &Policy::new(ALL_COLD), after_the_store()),
    );
    assert_eq!(
        out.moved(),
        SEGMENTS,
        "every segment is cold: {}",
        lines(&out)
    );
    let back = ran(
        "a hydrate of everything a sweep moved",
        hydrate(fixture.store_dir(), &*port),
    );
    assert_eq!(
        (back.fetched(), back.kept()),
        (SEGMENTS, 0),
        "every segment comes back and none of them was already right: {}",
        lines(&back)
    );
    assert_eq!(
        fixture.answers(&store),
        before,
        "the store answers the same records after the round trip as before it — record for record, \
         not a count"
    );
    // And a *second* process's view of the same directory agrees again: hydration is what makes the
    // directory match the manifest, so an open after it is an open of the store it was.
    let reopened = Segments::open(fixture.store_dir())
        .unwrap_or_else(|error| panic!("a hydrated store opens: {error}"));
    assert_eq!(
        tier::answers(&reopened),
        before,
        "and a store opened from the directory alone answers the same records, which is the claim \
         the ordering exists to keep"
    );
}

#[test]
fn the_window_between_the_object_and_the_delete_is_readable_from_both_sides() {
    let fixture = Tier::start("ordering");
    let store = fixture.store(2);
    let port = fixture.objects();
    let before: Vec<Record> = fixture.answers(&store);
    let file = tier::file_of(fixture.store_dir(), 0);
    let bytes =
        std::fs::read(&file).unwrap_or_else(|error| panic!("the segment is readable: {error}"));

    // The window itself: the object has landed and the pass has not yet removed the local file. A
    // crash there leaves the segment in *both* places, and both are the same bytes because the `put`
    // is what put them there — so the store is readable with the tier and without it.
    let losing = Losing {
        inner: &*port,
        writes: true,
    };
    let outcome = sweep(&store, &losing, &Policy::new(ALL_COLD), after_the_store());
    let Err(error) = outcome else {
        panic!(
            "the sweep is expected to stop: its tier never answers the write, and a pass that \
             reported a move here would be a pass that counted a write nothing confirmed"
        );
    };
    assert!(
        error.to_string().contains("never came back"),
        "the refusal is the tier's own, unaltered: {error}"
    );
    let key = object_key(fixture.prefix(), 0);
    assert_eq!(
        port.get(&key)
            .unwrap_or_else(|error| panic!("the tier answers about its own key: {error}")),
        Some(bytes.clone()),
        "the object is in the tier, because the `put` was carried out before the answer was lost"
    );
    assert!(
        file.is_file(),
        "and the local file is still where it was, because the delete had not happened yet: the \
         ordering is what leaves the segment in both places rather than in neither"
    );
    let reopened = Segments::open(fixture.store_dir())
        .unwrap_or_else(|error| panic!("a store whose files are still local opens: {error}"));
    assert_eq!(
        tier::answers(&reopened),
        before,
        "so a store opened from the directory in that window answers every record it answered \
         before the pass began — and a pass that removed the file first would have left a manifest \
         naming a segment that is nowhere"
    );

    // The other side of the window, and the other half of the same claim: a write that never reached
    // the tier at all leaves the segment exactly where it was, and the store readable.
    let never = Losing {
        inner: &*port,
        writes: false,
    };
    let other = Tier::start("ordering-never");
    let store = other.store(2);
    let untouched: Vec<Record> = other.answers(&store);
    assert!(
        sweep(&store, &never, &Policy::new(ALL_COLD), after_the_store()).is_err(),
        "a write that never arrived is a refusal too, not a move"
    );
    assert!(
        other.held(other.prefix()).is_empty(),
        "and nothing is in the tier: a pass that removed the local file after a write that never \
         landed would have lost the segment from both places"
    );
    let reopened = Segments::open(other.store_dir())
        .unwrap_or_else(|error| panic!("a store whose files were never touched opens: {error}"));
    assert_eq!(
        tier::answers(&reopened),
        untouched,
        "the store answers every record it answered before, because every file is where it was"
    );
}

#[test]
fn hydrate_brings_back_a_store_whose_files_are_gone_and_asks_nothing_when_they_are_not() {
    let fixture = Tier::start("hydrate-missing");
    let store = fixture.store(3);
    let port = fixture.objects();
    let before: Vec<Record> = fixture.answers(&store);
    // A store whose files are all where the manifest says asks the tier nothing: not a listing, not
    // a request. That is what makes a hydrate cheap enough to run before every read.
    let complete = ran(
        "a hydrate of a complete store",
        hydrate(fixture.store_dir(), &*port),
    );
    assert_eq!(
        (complete.fetched(), complete.kept(), complete.pages()),
        (0, 3, 0),
        "nothing is fetched, three segments are already right, and no page of listing was read: {}",
        lines(&complete)
    );
    if let Some(endpoint) = fixture.stub() {
        assert!(
            endpoint.calls_of("GET").is_empty(),
            "and the endpoint was asked nothing at all: {:?}",
            endpoint
                .calls_of("GET")
                .iter()
                .map(|call| call.line())
                .collect::<Vec<String>>()
        );
    }
    ran(
        "a sweep of every segment",
        sweep(&store, &*port, &Policy::new(ALL_COLD), after_the_store()),
    );
    assert!(
        !tier::file_of(fixture.store_dir(), 0).is_file(),
        "the sweep is what took the files out of the box, so the hydrate below is the one that has \
         work to do"
    );
    let back = ran(
        "a hydrate of what the sweep moved",
        hydrate(fixture.store_dir(), &*port),
    );
    assert_eq!(
        (back.fetched(), back.kept()),
        (3, 0),
        "every missing segment comes back: {}",
        lines(&back)
    );
    assert_eq!(
        back.bytes(),
        store.manifest().bytes(),
        "and they weigh what the manifest says"
    );
    assert_eq!(
        fixture.answers(&store),
        before,
        "the answers are record for record what they were before anything left"
    );
    let again = ran("a second hydrate", hydrate(fixture.store_dir(), &*port));
    assert_eq!(
        (again.fetched(), again.pages()),
        (0, 0),
        "and a second hydrate asks the tier nothing, because nothing is missing any more: {}",
        lines(&again)
    );
}

#[test]
fn hydrate_refuses_an_object_that_is_not_the_length_the_manifest_states() {
    let fixture = Tier::start("hydrate-damaged");
    let store = fixture.store(2);
    let port = fixture.objects();
    let before: Vec<Record> = fixture.answers(&store);
    let bytes = std::fs::read(tier::file_of(fixture.store_dir(), 0))
        .unwrap_or_else(|error| panic!("the segment's file is readable: {error}"));
    let neighbour = std::fs::read(tier::file_of(fixture.store_dir(), 1))
        .unwrap_or_else(|error| panic!("the second segment's file is readable: {error}"));
    let stated = store
        .manifest()
        .segments()
        .first()
        .map(|info| info.bytes)
        .unwrap_or_default();
    ran(
        "a sweep of every segment",
        sweep(&store, &*port, &Policy::new(ALL_COLD), after_the_store()),
    );
    let key = object_key(fixture.prefix(), 0);
    port.put(&key, b"short").unwrap_or_else(|error| {
        panic!("the tier takes an object that is the wrong length: {error}")
    });
    let outcome = hydrate(fixture.store_dir(), &*port);
    let Err(error) = outcome else {
        panic!(
            "a segment whose bytes are not the length the manifest states is refused rather than \
             read: a hydrate that accepted it would write a file the store's own open then refuses, \
             for a reason nobody could trace to the tier"
        );
    };
    match &error {
        Error::Damaged {
            key: named,
            stated: said,
            found,
        } => {
            assert_eq!(named, &key, "the refusal names the object: {error}");
            assert_eq!((*said, *found), (stated, 5), "and both lengths: {error}");
        }
        other => panic!("a length that disagrees is a `Damaged` refusal, not {other:?}"),
    }
    assert!(
        !tier::file_of(fixture.store_dir(), 0).is_file(),
        "and no file is left behind: the bad object never became a segment file, so the next open \
         refuses a file that is *missing* rather than one nobody can explain"
    );

    // The other half of the same rule: a local file that is the wrong length is repaired from the
    // tier's copy rather than left to be refused. One file is restored whole so that the case can
    // tell "came back" from "was already right".
    port.put(&key, &bytes)
        .unwrap_or_else(|error| panic!("the tier takes the segment back: {error}"));
    std::fs::write(tier::file_of(fixture.store_dir(), 0), b"short")
        .unwrap_or_else(|error| panic!("a wrong-length file is what this case plants: {error}"));
    std::fs::write(tier::file_of(fixture.store_dir(), 1), &neighbour)
        .unwrap_or_else(|error| panic!("the second segment's file is put back whole: {error}"));
    let repaired = ran(
        "a hydrate of a store with one wrong-length file",
        hydrate(fixture.store_dir(), &*port),
    );
    assert_eq!(
        (repaired.fetched(), repaired.kept()),
        (1, 1),
        "the file that disagreed comes back and the one that did not stays: {}",
        lines(&repaired)
    );
    assert_eq!(
        fixture.answers(&store),
        before,
        "and the store answers every record again: the tier's copy of a segment is the manifest's"
    );
}

#[test]
fn a_stopped_store_whose_segments_are_cold_is_hydrated_by_path_and_then_opens() {
    let fixture = Tier::start("stopped-home");
    // The store is written, swept, and **dropped**: what is left is a directory, which is the state
    // a process that stopped leaves behind. The sweep is the pass that came before it.
    let before: Vec<Record> = {
        let store = fixture.store(2);
        let before = fixture.answers(&store);
        let port = fixture.objects();
        ran(
            "a sweep of every segment",
            sweep(&store, &*port, &Policy::new(ALL_COLD), after_the_store()),
        );
        before
    };
    // Nothing is left under `segments/`, and an open says so. That is the state a stopped store is
    // in, and it is what the hydrate below has to undo rather than work around.
    let refusal = Segments::open(fixture.store_dir())
        .err()
        .unwrap_or_else(|| {
            panic!(
                "a store whose segments are in the tier opened without them: a reader that \
                 answered from a manifest naming files nobody has would answer a hole"
            )
        });
    let text = refusal.to_string();
    assert!(
        text.contains("0000000000.lks"),
        "the refusal names the file that is missing rather than saying a store is broken: {text}"
    );
    assert!(
        text.contains("does not exist"),
        "in the words the store's own reader uses for a file nobody kept: {text}"
    );
    assert!(
        !before.is_empty(),
        "the records that are not answerable yet are real ones, not an empty store: {before:?}"
    );

    // There is no live store to hand a hydrate — the process that owned this one is gone — so what
    // it is handed is the store's **directory**, and the manifest comes in through the read-only
    // door (`compaction::Manifest::read`): nothing is opened, nothing is created and nothing is
    // deleted. The files the manifest names and the directory does not hold are what comes back.
    let port = fixture.objects();
    let back = ran(
        "a hydrate of the directory a stopped store left behind",
        hydrate(fixture.store_dir(), &*port),
    );
    assert_eq!(
        (back.fetched(), back.kept()),
        (2, 0),
        "every segment the manifest names comes back, and nothing was left where it was: {}",
        lines(&back)
    );
    assert_eq!(
        back.bytes(),
        compaction::Manifest::read(fixture.store_dir())
            .expect("the store's manifest, read without opening the store")
            .bytes(),
        "and what came back weighs what the manifest says the store weighs"
    );
    let reopened = Segments::open(fixture.store_dir()).unwrap_or_else(|error| {
        panic!(
            "a store whose files are back under the names the manifest lists opens — and the open \
             is what checks them against the lengths it states: {error}"
        )
    });
    assert_eq!(
        tier::answers(&reopened),
        before,
        "and it answers every record it answered before the sweep, because what came back is what \
         left"
    );
}

#[test]
fn the_way_back_for_a_stopped_store_is_one_object_per_segment_it_names() {
    let fixture = Tier::start("stopped-sync");
    let port = fixture.objects();
    {
        let store = fixture.store(2);
        ran(
            "a sweep of every segment",
            sweep(&store, &*port, &Policy::new(ALL_COLD), after_the_store()),
        );
    }
    // The keys this step derives *are* the store's own paths, and that is what makes the way back a
    // `sync` rather than a translation: the object a segment is in is the file the manifest names
    // for it, so a hydrate writes each one back where it belongs with nothing to look up — and an
    // operator with a bucket, a shell and no hydrate at all does the same thing by hand.
    for ordinal in 0..2 {
        let key = object_key(fixture.prefix(), ordinal);
        assert_eq!(
            tier::file_of(fixture.store_dir(), ordinal)
                .strip_prefix(fixture.store_dir())
                .ok()
                .map(|path| path.to_string_lossy().replace('\\', "/")),
            Some(
                key.trim_start_matches(&format!("{}/", fixture.prefix()))
                    .to_owned()
            ),
            "the object's key, minus the port's prefix, is the path the manifest names: that is what \
             makes the way back a `sync` rather than a translation"
        );
    }
    let home = ran(
        "a hydrate of the directory a stopped store left behind",
        hydrate(fixture.store_dir(), &*port),
    );
    assert_eq!(
        (home.fetched(), home.kept()),
        (2, 0),
        "one object per segment the manifest names is what comes back, and nothing was left where \
         it was: {}",
        lines(&home)
    );
    for info in compaction::Manifest::read(fixture.store_dir())
        .expect("the store's manifest, read without opening the store")
        .segments()
    {
        assert_eq!(
            std::fs::metadata(tier::file_of(fixture.store_dir(), info.index))
                .expect("every segment the manifest names is back")
                .len(),
            info.bytes,
            "segment {} is not back at the length the manifest states, under the name the manifest \
             lists for it",
            info.index
        );
    }
    let again = ran("a second hydrate", hydrate(fixture.store_dir(), &*port));
    assert_eq!(
        (again.fetched(), again.pages()),
        (0, 0),
        "and a second hydrate asks the tier nothing, because nothing is missing any more: {}",
        lines(&again)
    );
    assert_eq!(
        fixture.held(fixture.prefix()).len(),
        2,
        "the tier still holds both objects, and nothing in the way back removed them: a store that \
         came home is a store whose tier may be swept again"
    );
}

#[test]
fn the_policy_boundary_is_one_millisecond_wide() {
    let fixture = Tier::start("policy-boundary");
    let window = 1_000u64;
    let policy = Policy::new(window);
    // A segment whose last record is *on* the cutoff stays, and the one a millisecond older goes:
    // the rule is `last < cutoff`, the same half-open comparison `lakelet/4.9-retention` uses, and a
    // policy case that could not tell those two apart would be a policy case about nothing.
    let cutoff = tier::last_of(2);
    let on = SegmentInfo::spanning(7, 100, 3, cutoff - 500, cutoff);
    let past = SegmentInfo::spanning(8, 100, 3, cutoff - 500, cutoff - 1);
    assert!(
        !policy.is_cold(&on, cutoff + window),
        "a segment whose last record is exactly the cutoff is warm: {} against {}",
        cutoff,
        policy.cutoff(cutoff + window)
    );
    assert!(
        policy.is_cold(&past, cutoff + window),
        "one millisecond older and it is cold: the boundary is one millisecond wide, which is what \
         makes it a rule rather than a rounding"
    );
    assert!(
        !policy.is_cold(&SegmentInfo::new(9, 100, 3), cutoff * 2),
        "and a segment whose age nobody stated is never cold: a pass does not move bytes on a \
         number nobody wrote down"
    );
    assert_eq!(
        policy.cutoff(window - 1),
        0,
        "a `now` below the window answers a cutoff of zero, which is below every timestamp this \
         store can hold — so a policy applied at it moves nothing rather than everything"
    );

    // The same rule, where it decides a file: the store's segment 2 has its last record exactly on
    // the cutoff, and it stays on the box while the two before it leave.
    let store = fixture.store(SEGMENTS);
    let port = fixture.objects();
    let report = ran(
        "a sweep at the boundary",
        sweep(&store, &*port, &policy, cutoff + window),
    );
    assert_eq!(
        (report.moved(), report.kept()),
        (2, 2),
        "the two segments below the cutoff move and the one exactly on it does not: {} against {}",
        lines(&report),
        tier::last_of(2)
    );
    assert!(
        tier::file_of(fixture.store_dir(), 2).is_file(),
        "and the file of the segment on the boundary is the one a case can point at"
    );
}

#[test]
fn the_signed_request_is_the_vendors_own_vector() {
    let fixture = Tier::start("signature-vector");
    let bucket = fixture.bucket(fixture.prefix());
    // The request the vendor's own signed-request example makes, in this build's vocabulary — and
    // the signature below is the one the vendor's documentation states for it. A pinned value this
    // build produced would only say "this build still does what it did"; this one says the SigV4 it
    // implements is the SigV4 an endpoint implements.
    let example = s3::Bucket::new(s3::Config {
        endpoint: "example.amazonaws.com".to_owned(),
        bucket: "example".to_owned(),
        region: "us-east-1".to_owned(),
        service: "service".to_owned(),
        date: "20150830T123600Z".to_owned(),
        access_key: EXAMPLE_ACCESS.to_owned(),
        secret_key: EXAMPLE_SECRET.to_owned(),
    })
    .unwrap_or_else(|error| panic!("the example's config is one a tier can use: {error}"));
    let request = http::Request::new("GET", "/")
        .with_header("host", "example.amazonaws.com")
        .with_header("x-amz-date", "20150830T123600Z");
    let signed: Vec<String> = example.signed_headers(&request);
    let names: Vec<&str> = signed.iter().map(String::as_str).collect();
    assert_eq!(
        request.canonical_request(&names),
        CANONICAL,
        "the canonical request is the one the vendor's example signs, byte for byte — method, \
         canonical URI, canonical query, the signed headers, the signed-header list and the payload \
         hash, in that order"
    );
    let authorization = example.authorization(&request);
    assert!(
        authorization.contains(&format!("Signature={SIGNATURE}")),
        "and the signature over it is the value the vendor's documentation gives: {authorization}"
    );
    assert!(
        authorization.contains(&format!(
            "Credential={EXAMPLE_ACCESS}/20150830/us-east-1/service/aws4_request"
        )),
        "with the credential scope the date, the region, the service and `aws4_request`: \
         {authorization}"
    );
    assert_eq!(
        example.config().scope(),
        "20150830/us-east-1/service/aws4_request",
        "which is the scope as a value, so a case can read it without parsing a header"
    );

    // And the signed headers are the ones sent: the request this port makes to the endpoint carries
    // a `host`, an `x-amz-date`, a payload hash — and no authorization when it is signed.
    let key = object_key(fixture.prefix(), 0);
    bucket
        .put(&key, b"bytes")
        .unwrap_or_else(|error| panic!("the endpoint takes an object: {error}"));
    let Some(endpoint) = fixture.stub() else {
        return; // A real endpoint has no request log; the vector above is what this case owns.
    };
    let Some(call) = endpoint.calls_of("PUT").into_iter().next() else {
        panic!("the endpoint logged the `put` the case just made")
    };
    let sent: BTreeSet<String> = call
        .headers
        .iter()
        .map(|(name, _)| name.to_ascii_lowercase())
        .filter(|name| name != "authorization" && name != "content-length")
        .collect();
    let Some(header) = call.header("authorization") else {
        panic!(
            "the request carried an authorization header: {}",
            call.line()
        )
    };
    let listed = header
        .split("SignedHeaders=")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .map(|list| {
            list.split(';')
                .map(str::to_owned)
                .collect::<BTreeSet<String>>()
        })
        .unwrap_or_default();
    assert_eq!(
        listed, sent,
        "the signed-header list is every header the request carried and no other — a header that \
         was signed and not sent, or sent and not signed, is a 403 from an endpoint that checked: \
         {header}"
    );
    assert!(
        listed.contains("host")
            && listed.contains("x-amz-date")
            && listed.contains("x-amz-content-sha256"),
        "including the three every S3-shaped request has to sign: {header}"
    );
}

#[test]
fn the_payload_hash_names_the_body_that_was_sent() {
    let fixture = Tier::start("payload-hash");
    let bucket = fixture.bucket(fixture.prefix());
    let body = http::Request::new("PUT", "/x/y").with_body(b"lakelet".to_vec());
    assert_eq!(
        body.payload_hash(),
        LAKELET_SHA256,
        "the hash a request names its payload by is the sha256 of its own body, in hex"
    );
    let same = http::Request::new("PUT", "/x/y").with_body(b"lakelet".to_vec());
    let other = http::Request::new("PUT", "/x/y").with_body(b"lakeleu".to_vec());
    assert_eq!(
        bucket.authorization(&body),
        bucket.authorization(&same),
        "the same request signs the same bytes: `x-amz-date` is a value in the config and nothing \
         here reads a clock"
    );
    assert_ne!(
        bucket.authorization(&body),
        bucket.authorization(&other),
        "and one byte of body changes the signature, because the payload's hash is part of the \
         string that is signed"
    );
}

#[test]
fn a_500_is_retried_and_the_object_is_there_exactly_once() {
    let fixture = Tier::start("retry-500");
    let Some(endpoint) = fixture.stub() else {
        return; // A real endpoint cannot be told to fail on demand.
    };
    // A request log is the in-process endpoint's; a deployment's endpoint says nothing back.
    let port = fixture.bucket(fixture.prefix());
    let key = object_key(fixture.prefix(), 0);
    endpoint.fail_next(1, 500);
    port.put(&key, b"the object").unwrap_or_else(|error| {
        panic!("the second attempt answers, so the `put` succeeds: {error}")
    });
    let attempts = endpoint.calls_of("PUT");
    assert_eq!(
        attempts.len(),
        2,
        "the endpoint saw two attempts: the first it answered `500` to, and the second it carried \
         out: {:?}",
        attempts
            .iter()
            .map(|call| call.line())
            .collect::<Vec<String>>()
    );
    assert_eq!(
        attempts.first().map(|call| call.line()),
        attempts.last().map(|call| call.line()),
        "and the two are the same request, byte for byte — a retry that renamed its object would be \
         a tier that kept two of them"
    );
    assert_eq!(
        endpoint.object(&key).as_deref(),
        Some(&b"the object"[..]),
        "the object is there"
    );
    assert_eq!(
        endpoint.keys(),
        vec![key],
        "and it is there exactly once: the key is the identity of what is written, which is what \
         makes repeating the request safe"
    );
    assert_eq!(
        port.get(&object_key(fixture.prefix(), 0))
            .unwrap_or_else(|error| panic!("the object reads back: {error}"))
            .as_deref(),
        Some(&b"the object"[..]),
        "a success after a failure is reported as a success, not as the failure that came first"
    );
}

#[test]
fn a_403_is_not_retried_and_its_refusal_names_the_signature() {
    let fixture = Tier::start("refuse-403");
    let Some(endpoint) = fixture.stub() else {
        return;
    };
    // A `403` is earned or injected: only the in-process endpoint can do the second.
    let port = fixture.bucket(fixture.prefix());
    let key = object_key(fixture.prefix(), 0);
    endpoint.fail_next(1, 403);
    let outcome = port.get(&key);
    let Err(error) = outcome else {
        panic!("a refused request is not an answer: the endpoint answered 403")
    };
    match &error {
        Error::Refused { status, .. } => assert_eq!(*status, 403, "{error}"),
        other => panic!("a 403 is a `Refused`, not {other:?}"),
    }
    let text = error.to_string();
    assert!(
        text.contains("the signature was refused"),
        "the refusal an operator reads says what a 403 usually is: {text}"
    );
    assert!(
        text.contains("20261008/eu-central-1/s3/aws4_request"),
        "and quotes the scope the request carried, which is the first thing to check: {text}"
    );
    assert_eq!(
        endpoint.calls_of("GET").len(),
        1,
        "and it was asked once: a 4xx is an answer, and a second attempt gets the same one — \
         spending an attempt on it is an attempt the next request needs"
    );
}

#[test]
fn the_operations_are_the_requests_an_s3_endpoint_means() {
    let fixture = Tier::start("request-shape");
    let Some(endpoint) = fixture.stub() else {
        return;
    };
    // The log is the in-process endpoint's; a deployment's endpoint says nothing back.
    let port = fixture.bucket(fixture.prefix());
    let key = object_key(fixture.prefix(), 0);
    port.put(&key, b"bytes")
        .unwrap_or_else(|error| panic!("a `put` reaches the endpoint: {error}"));
    port.get(&key)
        .unwrap_or_else(|error| panic!("a `get` reaches the endpoint: {error}"));
    port.delete(&key)
        .unwrap_or_else(|error| panic!("a `delete` reaches the endpoint: {error}"));
    listing(&port, fixture.prefix())
        .unwrap_or_else(|error| panic!("a listing reaches the endpoint: {error}"));

    let mut seen: Vec<String> = Vec::new();
    for call in endpoint.calls() {
        seen.push(call.line());
        // Every operation is addressed path-style: the bucket is the first path segment and the key
        // is the rest of it, which is what an endpoint that is not AWS itself speaks.
        assert!(
            call.path.starts_with(&format!("/{}", tier::BUCKET)),
            "every request names the bucket in its path: {}",
            call.line()
        );
    }
    assert!(
        seen.contains(&format!("PUT /{}/{key}", tier::BUCKET)),
        "`PutObject` is a PUT to the object's own path: {seen:?}"
    );
    assert!(
        seen.contains(&format!("GET /{}/{key}", tier::BUCKET)),
        "`GetObject` is a GET of it: {seen:?}"
    );
    assert!(
        seen.contains(&format!("DELETE /{}/{key}", tier::BUCKET)),
        "`DeleteObject` is a DELETE of it: {seen:?}"
    );

    let Some(list) = endpoint
        .calls_of("GET")
        .into_iter()
        .find(|call| call.key().is_none())
    else {
        panic!("the listing is a GET on the bucket itself, with no key: {seen:?}")
    };
    assert_eq!(
        list.query_of("list-type").as_deref(),
        Some("2"),
        "`ListObjectsV2` says so in the query, because the version-one shape is what an endpoint \
         answers without it: {}",
        list.line()
    );
    assert_eq!(
        list.query_of("prefix").as_deref(),
        Some(fixture.prefix()),
        "the prefix is the one the store's keys live under: {}",
        list.line()
    );
    let keys = list
        .query_of("max-keys")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    assert!(
        keys > 0,
        "and the request says how many keys it wants in one page, which is what a client has to be \
         able to state: {}",
        list.line()
    );

    // The headers a signed request is made of, on every one of them.
    for call in endpoint.calls() {
        for header in [
            "authorization",
            "x-amz-content-sha256",
            "host",
            "x-amz-date",
        ] {
            assert!(
                call.header(header).is_some(),
                "`{header}` is on every request this tier makes, and `{}` has no such header",
                call.line()
            );
        }
    }
    assert!(
        endpoint
            .calls_of("GET")
            .iter()
            .all(|call| call.header("x-amz-content-sha256").is_some()),
        "including the reads, which sign an empty payload like everything else"
    );
}

#[test]
fn the_second_page_used_the_token_the_first_page_returned() {
    let fixture = Tier::start("page-token");
    let Some(endpoint) = fixture.stub() else {
        return;
    };
    // A token is the endpoint's word for "there is more", and the stub is the endpoint here.
    let port = fixture.bucket(fixture.prefix());
    let prefix = fixture.prefix();
    for ordinal in 0..KEYS {
        port.put(&object_key(prefix, ordinal), b"x")
            .unwrap_or_else(|error| panic!("segment {ordinal}'s object is written: {error}"));
    }
    // This case walks the pages by hand rather than with `tier::listing`, because what it is about
    // is the token itself: which request carried which one, in which order.
    let mut keys: Vec<String> = Vec::new();
    let mut tokens: Vec<String> = Vec::new();
    let mut cursor: Option<Cursor> = None;
    let mut first_page: Vec<String> = Vec::new();
    loop {
        let page = port
            .list(prefix, cursor.clone())
            .unwrap_or_else(|error| panic!("the endpoint lists a prefix: {error}"));
        assert!(
            page.keys.iter().all(|key| !keys.contains(key)),
            "no key is repeated across a page boundary: {:?} after {keys:?}",
            page.keys
        );
        keys.extend(page.keys.clone());
        if cursor.is_none() {
            first_page = page.keys.clone();
        }
        let Some(next) = page.next else {
            break;
        };
        tokens.push(next.as_str().to_owned());
        cursor = Some(next);
    }
    let mut expected: Vec<String> = (0..KEYS)
        .map(|ordinal| object_key(prefix, ordinal))
        .collect();
    expected.sort();
    assert_eq!(
        keys, expected,
        "walking the pages by their tokens answers every key under the prefix, once each and in \
         order"
    );
    assert_eq!(
        tokens.len() as u64,
        KEYS / tier::PAGE as u64 - 1,
        "{} keys over {} to a page is one token per page but the last",
        KEYS,
        tier::PAGE
    );
    assert_eq!(
        tokens.first().map(String::as_str),
        first_page.last().map(String::as_str),
        "and the first token is a *key* rather than an offset — the last key of the page before it — \
         so a key that arrives in the middle of a walk cannot shift the pages after it"
    );

    let listings: Vec<String> = endpoint
        .calls_of("GET")
        .into_iter()
        .filter(|call| call.key().is_none())
        .map(|call| call.line())
        .collect();
    let carried: Vec<String> = endpoint
        .calls_of("GET")
        .into_iter()
        .filter(|call| call.key().is_none())
        .filter_map(|call| call.query_of("continuation-token"))
        .collect();
    assert_eq!(
        carried, tokens,
        "and the endpoint was asked with them in order: every page after the first carried the token \
         the page before returned, and no request repeated one: {listings:?}"
    );
}

#[test]
fn a_key_that_is_not_a_path_below_the_root_is_refused() {
    let fixture = Tier::start("key-paths");
    // The directory implementation is built here rather than through the fixture's port, because
    // what this case is about is a thing only *it* has: a root, and keys that could walk out of it.
    // An S3-shaped endpoint has no such thing — a `..` in a key is a name with dots in it — so this
    // is a rule of the boring implementation, judged on both runs.
    let mut root = std::path::PathBuf::from(fixture.store_dir());
    root.push("tier-root-for-keys");
    let directory = lakelet::engine::tier::Directory::new(&root).with_prefix("store");
    for key in [
        "",
        "/etc/passwd",
        "a/",
        "..",
        "../outside",
        "a/../../outside",
        "a//b",
        "./a",
    ] {
        let outcome = directory.put(key, b"bytes");
        let Err(error) = outcome else {
            panic!(
                "`{key}` is not a key below a root: a port that joined it to its root would write \
                 outside the tier, which is a port that can be told to write anywhere"
            );
        };
        match &error {
            Error::Invalid { detail } => assert!(
                detail.contains(key) || key.is_empty(),
                "and the refusal names the key it refused: {detail}"
            ),
            other => panic!("a key that is not a path is an `Invalid` refusal, not {other:?}"),
        }
    }
    directory
        .put("store/segments/0000000007.lks", b"bytes")
        .unwrap_or_else(|error| panic!("a key that is a path below the root is taken: {error}"));
    let file = directory
        .path_of_key("store/segments/0000000007.lks")
        .unwrap_or_else(|error| panic!("the key's file can be named: {error}"));
    assert!(
        file.starts_with(&root) && file.is_file(),
        "and the file it names is below the root and there: {}",
        file.display()
    );
    assert!(
        !root
            .parent()
            .is_some_and(|parent| parent.join("outside").exists()),
        "nothing was written outside the root: a refusal is not a written-then-moved file"
    );
}

#[test]
fn a_body_with_no_content_length_ends_at_the_close() {
    let fixture = Tier::start("bare-body");
    let Some(endpoint) = fixture.stub() else {
        // A deployment's endpoint is S3's, and S3 always says how long its body is.
        return;
    };
    let port = fixture.bucket(fixture.prefix());
    let key = object_key(fixture.prefix(), 0);
    let bytes = b"a body whose length the head never stated";
    port.put(&key, bytes)
        .unwrap_or_else(|error| panic!("a `put` reaches the endpoint: {error}"));
    // From here the endpoint answers the HTTP/1.0 way: no `content-length`, a body, and then the
    // close. S3 does not do this; a proxy that strips the header, or an endpoint older than the
    // header, does — and the client that trusted only the head would answer an empty object and
    // call it read.
    endpoint.without_length();
    let read = port
        .get(&key)
        .unwrap_or_else(|error| panic!("a body with no stated length is still an answer: {error}"));
    assert_eq!(
        read.as_deref(),
        Some(&bytes[..]),
        "the body was read to the close, which is the only other thing a body can mean — not a \
         prefix of it, and not nothing"
    );
    assert_eq!(
        endpoint.calls_of("GET").len(),
        1,
        "and the answer was read once: nothing about a missing length is a reason to ask again"
    );
}

#[test]
fn a_tier_root_that_is_not_there_yet_holds_nothing_rather_than_refusing() {
    let fixture = Tier::start("root-that-is-not-there");
    // The local implementation's root is where a `put` would create the first file. A tier nobody
    // has written to yet is a tier that holds nothing — not a tier that is broken — and asking a
    // question of it is not an error.
    let root = std::path::PathBuf::from(fixture.store_dir()).join("tier-nobody-wrote-here");
    assert!(
        !root.exists(),
        "this case is about a root that is not there: {}",
        root.display()
    );
    let directory = lakelet::engine::tier::Directory::new(&root).with_prefix("store");
    let page = directory
        .list("store/", None)
        .unwrap_or_else(|error| panic!("an empty tier lists an empty page: {error}"));
    assert!(
        page.keys.is_empty() && page.next.is_none(),
        "and the page says so: {:?}",
        page.keys
    );
    assert_eq!(
        directory
            .get("store/segments/0000000000.lks")
            .unwrap_or_else(|error| panic!(
                "a key nobody put there is absence, not a refusal: {error}"
            )),
        None,
        "an object nobody wrote is `None` on this implementation too"
    );
    assert!(
        !root.exists(),
        "and asking created nothing: a question about a tier is not a write to it"
    );
}

#[test]
fn a_client_refuses_an_endpoint_it_cannot_speak_to() {
    let https = http::Client::new("https://minio.example:9000");
    let Err(error) = https else {
        panic!(
            "this client speaks http and nothing else: a tier that quietly spoke TLS-less https \
             would send a request a bucket refuses with a signature error nobody can read"
        );
    };
    match &error {
        Error::Invalid { detail } => assert!(
            detail.contains("https"),
            "and the refusal names the scheme it cannot speak: {detail}"
        ),
        other => panic!("a scheme this client cannot speak is `Invalid`, not {other:?}"),
    }
    let bare = http::Client::new("127.0.0.1:9000")
        .unwrap_or_else(|error| panic!("`host:port` is an authority: {error}"));
    assert_eq!(
        bare.authority(),
        "127.0.0.1:9000",
        "and it is the `Host` header too"
    );
    let defaulted = http::Client::new("http://bucket.example")
        .unwrap_or_else(|error| panic!("a scheme with no port takes the scheme's: {error}"));
    assert_eq!(
        defaulted.authority(),
        "bucket.example:80",
        "a port that is not written is 80, because this client speaks http and that is http's"
    );
    let path = http::Client::new("http://bucket.example/a/b");
    let Err(error) = path else {
        panic!(
            "an endpoint is an authority rather than a URL: the path a request names is built from \
             the bucket and the key, and an endpoint with a path in it would be silently ignored"
        );
    };
    assert!(
        error.to_string().contains("not an authority"),
        "and the refusal says which of the two it was: {error}"
    );
}

#[test]
fn a_listing_that_never_advances_is_refused_rather_than_followed_for_ever() {
    let stuck = Stuck {
        prefix: "a/".to_owned(),
    };
    let outcome = listing(&stuck, "a/");
    let Err(error) = outcome else {
        panic!(
            "an endpoint that answers the same cursor twice is not making progress, and a loop that \
             took its word for it would ask forever"
        );
    };
    match &error {
        Error::Transport { detail } => {
            assert!(
                detail.contains("a/1"),
                "the refusal names the cursor a walk would have repeated: {detail}"
            );
        }
        other => panic!("a walk that cannot end is a transport failure, not {other:?}"),
    }
    assert_eq!(
        stuck.prefix(),
        "a/",
        "and a port answers for the prefix it serves, which is what a pass derives its keys under"
    );
}

#[test]
fn the_tier_stays_inside_the_engine_and_the_boundary_holds_with_a_port_in_it() {
    for (path, text) in tier_sources() {
        for forbidden in ["crate::service", "crate::cli"] {
            assert!(
                !text.contains(forbidden),
                "{path} names `{forbidden}`: the engine is what runs in a test, a load generator \
                 and a repair tool, and a tier that reached the service layer or the command line \
                 would be a module that cannot be judged without a socket or a terminal \
                 (`lakelet/4.1-ingest-protocol`'s boundary, which a port does not suspend)"
            );
        }
    }
    // The port is the one thing this step adds to the engine's shape, and it is a *trait* with two
    // implementations: a module with a port in it is a module whose rule can be judged against more
    // than one store, which is what `conformance.toml` declares and the gate runs.
    let ports = source("src/engine/tier/mod.rs");
    assert!(
        ports.contains("pub trait Objects"),
        "src/engine/tier/mod.rs declares the port every implementation answers to"
    );
    assert!(
        source("src/engine/tier/filesystem.rs").contains("impl Objects for Directory")
            && source("src/engine/tier/s3.rs").contains("impl Objects for Bucket"),
        "and both implementations are implementations of it, rather than two shapes that happen to \
         have methods with the same names"
    );
    assert!(
        source("src/engine/mod.rs").contains("pub mod tier;"),
        "the module is declared by the engine, inside the block a scaffold may write in"
    );
}

#[test]
fn the_engine_reads_no_clock_in_the_tiers_files() {
    for (path, text) in tier_sources() {
        for forbidden in [
            "std::time",
            "SystemTime",
            "Instant",
            "std::env",
            "std::process",
            "std::thread",
            "rand",
        ] {
            assert!(
                !text.contains(forbidden),
                "{path} contains `{forbidden}`: nothing in the tier reads a clock, an environment \
                 variable, a thread's identity or a seeded generator. A pass is applied at a `now` a \
                 caller hands it, and the instant a signature carries is a value in a config — which \
                 is what makes two runs of the same request sign the same bytes and a policy case \
                 say the same thing at midnight (`lakelet/4.16-profiling` made the same decision \
                 about measurement)"
            );
        }
    }
    let ports = source("src/engine/tier/mod.rs");
    assert!(
        ports.contains("now_millis: u64"),
        "the sweep takes the instant it is applied at (`now_millis: u64`), so a schedule is a \
         caller's decision rather than a `now()` this engine calls"
    );
    assert!(
        ports.contains("pub fn hydrate(directory: impl AsRef<Path>, objects: &dyn Objects)")
            && !ports.contains("objects: &dyn Objects, now"),
        "and hydration has no instant at all: putting a segment's bytes back is not a decision about \
         time. What it takes is the store's directory rather than a live store, so a store whose \
         process stopped is hydrated before it is opened"
    );
    assert!(
        source("src/engine/tier/s3.rs").contains("pub date: String"),
        "while the S3-shaped config carries the date its signatures are dated with"
    );
}

#[test]
fn a_report_counts_what_a_pass_looked_at_and_the_tier_holds_no_map_of_its_own() {
    let fixture = Tier::start("report-and-map");
    let store = fixture.store(3);
    let port = fixture.objects();
    let before: Vec<Record> = fixture.answers(&store);
    let store_files = stored(fixture.store_dir());
    let warm = ran(
        "a sweep that finds nothing cold",
        sweep(
            &store,
            &*port,
            &Policy::new(365 * 24 * tier::HOUR),
            after_the_store(),
        ),
    );
    assert_eq!(
        (warm.moved(), warm.fetched(), warm.bytes(), warm.pages()),
        (0, 0, 0, 0),
        "a policy nothing is cold under moves nothing and reads no page: {}",
        lines(&warm)
    );
    assert_eq!(
        warm.kept(),
        3,
        "and says how many segments it left alone: {}",
        lines(&warm)
    );
    assert!(
        fixture.held(fixture.prefix()).is_empty(),
        "the tier is empty, which is the answer `list` gives — there is no file anywhere that says \
         what the tier holds, and a tier that lost its listing is a tier that holds nothing rather \
         than a store that cannot be read"
    );
    assert_eq!(
        stored(fixture.store_dir()),
        store_files,
        "and the store's own directory holds the files it held before: the tier writes nothing \
         durably — no map from an ordinal to a key, no second manifest, no index of what is cold. \
         What the tier holds is answered by asking it (`list`), which is why a tier that lost its \
         listing is a tier that holds nothing rather than a store that cannot be read"
    );
    ran(
        "a sweep of every segment",
        sweep(&store, &*port, &Policy::new(ALL_COLD), after_the_store()),
    );
    let manifest_before = std::fs::read(fixture.store_dir().join(compaction::MANIFEST))
        .unwrap_or_else(|error| panic!("the store has a manifest: {error}"));
    ran(
        "a hydrate of the swept store",
        hydrate(fixture.store_dir(), &*port),
    );
    assert_eq!(
        std::fs::read(fixture.store_dir().join(compaction::MANIFEST))
            .unwrap_or_else(|error| panic!("the store has a manifest: {error}")),
        manifest_before,
        "and neither pass rewrites it: `segments.manifest` keeps the layout an earlier step pinned, \
         and a segment that left the box is still a segment the manifest names"
    );
    assert_eq!(
        fixture.answers(&store),
        before,
        "so the store's own account of itself never changed — only where its bytes were"
    );
}
