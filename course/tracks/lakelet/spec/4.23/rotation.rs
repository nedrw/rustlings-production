//! Spec for `lakelet/4.23-secret-rotation`: a ring of keys, and a rotation with no gap.
//!
//! `kiln run` copies this file into the project as `tests/spec_rotation.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Sixteen cases, and the shape of them is the point:
//!
//! * **an empty ring is authentication off.** The directory is the switch: a store with no `keys`
//!   directory takes a batch as its first payload, exactly as it did before this step, which is
//!   what keeps `lakelet/4.6-ingest-backpressure`'s and `lakelet/4.17-soak`'s harnesses — both
//!   connect and send batches — true. A directory that *exists* and holds no key is the other
//!   answer: a refusal, because fail-open is the one thing a ring must never do;
//! * **the first payload is the token.** It is a payload inside `lakelet/4.1-ingest-protocol`'s
//!   frame — the framing does not change, and the case that says so reads
//!   [`frame::VERSION`], [`frame::OVERHEAD`] and [`frame::MAX_PAYLOAD`] and scans a token frame
//!   cleanly — and a token this daemon refuses *closes the connection*, counted where 4.6 counts
//!   refusals, because a client that never got in is a fact a deployment has to be able to read
//!   off its own log;
//! * **the refusals are typed and the sentences carry the numbers.** Expired names the instant it
//!   expired and the instant it arrived; a retired key names the instant it left; a tag that is
//!   one byte different is `Tag`, and it is refused through a constant-time comparison
//!   (`relay/2.11-hmac-signing`'s lesson, in this project);
//! * **the rotation window is arithmetic this spec can check.** A key may leave only after the
//!   longest lifetime this deployment may issue has passed since something newer took the signing
//!   over, `--retire` refuses one millisecond earlier and names the instant, and the retirement at
//!   exactly that instant is accepted *and* is the moment the last token the old key signed stops
//!   being good;
//! * **a rotation takes effect on a running daemon.** The ring is read per connection, so a client
//!   that connected under the old key and one that connects after the new key arrived both work,
//!   with no restart in between — which is the whole step;
//! * **the operator's command is the interface.** `rotate --add` writes a key the ring then loads,
//!   `--list` prints the ring in a deterministic order with each key's window, whether it signs,
//!   and the instant it may leave, `--retire` is refused rather than allowed to strand a token,
//!   and every other mistake is the class `lakelet/4.10-downsampling` assigned it;
//! * **the page cannot drift from the code.** `docs/rotation.md` carries a fenced block that is
//!   one command's whole output, and the case re-runs that command and compares — the rule
//!   `lakelet/4.16-profiling` and `lakelet/4.19-backup-restore` set for their own pages.
//!
//! The library cases read no clock at all: every instant in them is a number the case chose. The
//! cases that judge the daemon *as a process* cannot — `service::ingest` is this project's one
//! clock, and a token it is asked to verify has to be minted near the machine's instant — so those
//! read the machine's instant once, open their keys a minute before it and issue tokens that live a
//! minute past it, and then judge by counts. A case that decided anything by how long a run took is
//! not a case this course writes.

mod support;

use std::path::{Path, PathBuf};

use lakelet::engine::spool::FILE;
use lakelet::engine::wal::segment;
use lakelet::protocol::batch::{self, Record};
use lakelet::protocol::frame::{self, Decoded, Frame};
use lakelet::service::keys::{self, KeyId, KeyRing, Refusal, Token, Window};

use support::StoreDir;
use support::Var;
use support::service::Service;

/// The page this step delivers, relative to the project.
const PAGE: &str = "docs/rotation.md";

/// The directory below a store that holds the ring.
const RING: &str = "keys";

/// An instant every fixture counts from: 2023-11-14T22:13:20Z, a round number and not a date.
const BASE: u64 = 1_700_000_000_000;

/// How long a token lives where a case is not about its lifetime.
const LIFETIME: u64 = 60_000;

/// The longest lifetime this deployment may issue, read from the module that states it.
fn longest() -> u64 {
    Window::MAX_TOKEN_LIFETIME_MILLIS
}

/// A secret of `label`'s own: 32 bytes of hex, distinct per label so two keys cannot share one.
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

/// The ring's directory in `store`.
fn ring_dir(store: &StoreDir) -> PathBuf {
    store.path().join(RING)
}

/// One key file, written the way `lakeletq rotate --add` writes it.
fn plant(store: &StoreDir, id: &str, secret: &str, not_before: u64, not_after: Option<u64>) {
    let directory = ring_dir(store);
    std::fs::create_dir_all(&directory)
        .unwrap_or_else(|error| panic!("{} cannot be created: {error}", directory.display()));
    let id = KeyId::new(id);
    let path = keys::key_file(&directory, &id);
    let text = keys::render(&id, secret, not_before, not_after);
    std::fs::write(&path, text)
        .unwrap_or_else(|error| panic!("{} cannot be written: {error}", path.display()));
}

/// The ring a daemon at `store` would read right now.
fn read_ring(store: &StoreDir) -> KeyRing {
    KeyRing::load(ring_dir(store))
        .unwrap_or_else(|error| panic!("the ring of {} loads: {error}", store.path().display()))
}

/// One record, named by its index.
///
/// The series name grows with the index, so two frames of this fixture have two different lengths
/// and a case can tell the daemon's lines about them apart.
fn record(index: usize) -> Record {
    Record {
        series: format!("rot,host=h{}", "0".repeat(index)),
        ts_millis: BASE as i64 + index as i64,
        value: index as f64,
    }
}

/// The payload of one batch frame.
fn payload(index: usize) -> Vec<u8> {
    batch::encode(&[record(index)])
}

/// Frames on the wire: what a client writes in one call.
fn wire(payloads: &[Vec<u8>]) -> Vec<u8> {
    payloads
        .iter()
        .flat_map(|payload| Frame::new(payload.clone()).encode())
        .collect()
}

/// The records a third party reads out of the store's log.
fn records(store: &StoreDir) -> Vec<Vec<u8>> {
    let bytes = store
        .read(FILE)
        .unwrap_or_else(|| panic!("{} holds a log", store.path().display()));
    segment::scan(&bytes)
        .unwrap_or_else(|error| panic!("the log the daemon wrote is a log: {error}"))
        .records()
        .to_vec()
}

/// One `key=<number>` field of a daemon log line.
fn field(line: &str, key: &str) -> u64 {
    let (_, rest) = line.split_once(&format!("{key}=")).unwrap_or_else(|| {
        panic!(
            "the line `{line}` carries no `{key}=` — the daemon reports what a connection came to \
             as fields, which is what a collector and this spec both read"
        )
    });
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits
        .parse()
        .unwrap_or_else(|error| panic!("`{key}=` in `{line}` is not a number: {error}"))
}

/// Run one `lakeletq` command line inside this process, which is where the contract lives.
fn lakeletq(arguments: &[&str]) -> lakelet::cli::Outcome {
    lakelet::cli::run(
        arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect::<Vec<String>>(),
    )
}

/// The page this case reads, from the project it is part of.
fn page() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(PAGE);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("{PAGE} is the page this step delivers, and it cannot be read: {error}")
    })
}

/// One instant a command line was given, as the text a script would type.
fn typed(millis: u64) -> String {
    millis.to_string()
}

/// Every `.rs` file below `directory`, for the checks that read the sources as text.
fn collect_rust(directory: &Path, found: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rust(&path, found)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
    Ok(())
}
#[test]
fn an_empty_ring_is_authentication_off_and_a_batch_is_a_first_payload() {
    // The switch, and the reason it is a directory rather than a configuration key: a deployment
    // that never turned authentication on is a deployment with no keys. This is also the case
    // that keeps `lakelet/4.6-ingest-backpressure`'s and `lakelet/4.17-soak`'s harnesses true —
    // they connect to a store with no ring and send batches, and the first payload they send is a
    // batch and not a token.
    let service = Service::start("rot-off", &[]);
    assert!(
        !service.store().path().join(RING).exists(),
        "this store has no ring, which is the state the case is about"
    );
    service.send(&wire(&[payload(0), payload(1)]));
    let closed = service.wait_for_line("connection closed accepted=2");
    assert!(
        closed.contains("rejected=0"),
        "with no key on the ring there is nothing to refuse, and the connection says so: {closed}"
    );
    assert_eq!(
        records(service.store()),
        vec![payload(0), payload(1)],
        "both frames are on the file, decoded as the batches they are: a daemon that took the \
         first payload of every connection for a token would have written nothing"
    );
}

#[test]
fn a_ring_with_one_key_accepts_a_token_that_key_signed_and_its_batches_arrive() {
    let now = wall_now();
    let service = Service::start("rot-one", &[]);
    plant(service.store(), "k1", &secret("k1"), now - 60_000, None);
    let token = read_ring(service.store())
        .issue("client-1", LIFETIME, now)
        .unwrap_or_else(|error| panic!("a ring with one open key signs: {error}"));
    service.send(&wire(&[token.encode(), payload(0), payload(1)]));
    let closed = service.wait_for_line("connection closed accepted=2");
    assert!(
        closed.contains("rejected=0"),
        "the token is not a frame this daemon stores, and the two batches behind it are: {closed}"
    );
    assert_eq!(
        records(service.store()),
        vec![payload(0), payload(1)],
        "the frames after the token are the ones on the file: the token was read and not stored"
    );
}

#[test]
fn a_token_whose_tag_is_one_byte_different_is_refused_and_counted() {
    let now = wall_now();
    let service = Service::start("rot-tag", &[]);
    plant(service.store(), "k1", &secret("k1"), now - 60_000, None);
    let token = read_ring(service.store())
        .issue("client-1", LIFETIME, now)
        .unwrap_or_else(|error| panic!("a ring with one open key signs: {error}"));
    // The tag is the last sixty-four bytes of the payload, and one hex digit of it moves.
    let mut payload = token.encode();
    let last = payload.len() - 1;
    payload[last] = if payload[last] == b'0' { b'1' } else { b'0' };
    service.send(&wire(&[payload, batch_payload()]));

    let refusal = service.wait_for_line("token refused");
    assert!(
        refusal.contains("tag"),
        "the refusal is about the tag, and it is a sentence rather than a code: {refusal}"
    );
    // Where 4.6 counts refusals: the connection's own line, which is what an operator reads and
    // what the daemon's counters are added up from. A client that never got in has to show up.
    let closed = service.wait_for_line("connection closed accepted=0 rejected=1");
    assert!(
        closed.contains("bytes=0"),
        "nothing the client sent was stored, and the connection says which counters moved: {closed}"
    );
    assert_eq!(
        records(service.store()),
        Vec::<Vec<u8>>::new(),
        "a connection refused at its first payload writes nothing at all"
    );
}

#[test]
fn an_expired_token_names_both_instants_and_a_window_that_has_not_opened_is_not_yet_valid() {
    let store = StoreDir::new("rot-expired");
    plant(&store, "k1", &secret("k1"), BASE, None);
    let ring = read_ring(&store);
    let token = ring
        .issue("client-1", LIFETIME, BASE)
        .unwrap_or_else(|error| panic!("a ring with one open key signs: {error}"));
    let expires = token.expires_millis();

    let refused = ring
        .verify(&token, expires + 1)
        .expect_err("a token past its instant is not good");
    assert_eq!(
        refused,
        Refusal::Expired {
            expires_millis: expires,
            now_millis: expires + 1,
        },
        "the refusal is typed, and it carries both numbers"
    );
    let sentence = refused.to_string();
    assert!(
        sentence.contains(&expires.to_string()) && sentence.contains(&(expires + 1).to_string()),
        "and the sentence a log line carries names both instants — when it expired and when the \
         connection arrived: {sentence}"
    );
    assert!(
        ring.verify(&token, expires - 1).is_ok(),
        "one millisecond earlier the same token is good, which is what makes the bound an instant \
         rather than a rounding"
    );

    // A key whose window has not opened. The operator moved `not_before` forward after tokens were
    // issued with it, which is a hand-edit and exactly the class of thing this reader is for.
    plant(&store, "k1", &secret("k1"), BASE + 10_000, None);
    let moved = read_ring(&store)
        .verify(&token, BASE + 20_000)
        .expect_err("a token claiming to be signed before its key's window opened is not good");
    assert_eq!(
        moved,
        Refusal::NotYetValid {
            not_before_millis: BASE + 10_000,
        },
        "the window it is too early for is the key's, and the refusal says which instant"
    );

    // And a token from a clock that is ahead of this daemon's: the same variant, naming the
    // instant the token was issued at, because a fleet whose clocks disagree is refused by the
    // direction it disagrees in rather than by a separate rule.
    let ahead = read_ring(&store)
        .issue("client-1", LIFETIME, BASE + 30_000)
        .unwrap_or_else(|error| panic!("the key's window is open at that instant: {error}"));
    assert_eq!(
        read_ring(&store).verify(&ahead, BASE + 20_000),
        Err(Refusal::NotYetValid {
            not_before_millis: BASE + 30_000,
        }),
        "an instant that has not arrived is not yet valid either"
    );

    // A key that has not opened cannot sign, which is the issue-side half of the same rule: a
    // daemon that minted a token its own reader would refuse is a daemon that locks out clients
    // it just let in.
    let unopened = StoreDir::new("rot-unopened");
    plant(&unopened, "soon", &secret("soon"), BASE + 10_000, None);
    let refused_issue = read_ring(&unopened)
        .issue("client-1", LIFETIME, BASE)
        .expect_err("a key that has not opened cannot sign");
    assert!(
        refused_issue
            .to_string()
            .contains(&(BASE + 10_000).to_string()),
        "and the refusal names the instant the window opens: {refused_issue}"
    );
}

#[test]
fn a_token_signed_by_a_retired_key_is_refused_and_its_sentence_names_the_instant() {
    let store = StoreDir::new("rot-retired");
    plant(&store, "k1", &secret("k1"), BASE, None);
    let token = read_ring(&store)
        .issue("client-1", 600_000, BASE + 1_000)
        .unwrap_or_else(|error| panic!("the only key is open and signs: {error}"));
    // A successor arrives, and the old key's end is written by hand — far too early, which is the
    // retirement `lakeletq rotate --retire` exists to refuse and the file an operator's typo
    // leaves behind.
    plant(&store, "k2", &secret("k2"), BASE + 2_000, None);
    plant(&store, "k1", &secret("k1"), BASE, Some(BASE + 3_000));
    let retired = BASE + 3_000;
    let arrived = BASE + 4_000;
    assert!(
        token.expires_millis() > arrived,
        "the token is still live when the client arrives, which is what makes this a gap and not \
         an expiry"
    );

    let refused = read_ring(&store)
        .verify(&token, arrived)
        .expect_err("a token whose key is gone is not good");
    assert_eq!(
        refused,
        Refusal::KeyRetired {
            id: KeyId::new("k1"),
            retired_millis: retired,
        },
        "a live token from a retired key is refused *as a retirement*, and not as an expiry"
    );
    assert!(
        refused.to_string().contains(&retired.to_string()),
        "and the sentence names the instant the key left, which is the number an operator has to \
         go and look at: {refused}"
    );
}

#[test]
fn issue_signs_with_the_key_that_is_preferred() {
    let store = StoreDir::new("rot-preferred");
    plant(&store, "k1", &secret("k1"), BASE, None);
    plant(&store, "k2", &secret("k2"), BASE + 1_000, None);
    let ring = read_ring(&store);
    assert_eq!(
        ring.preferred().map(|key| key.id().as_str()),
        Some("k2"),
        "the key with the latest `not_before` is the one that signs"
    );
    let token = ring
        .issue("client-1", LIFETIME, BASE + 2_000)
        .unwrap_or_else(|error| panic!("a ring with two open keys signs: {error}"));
    assert_eq!(
        token.key_id().as_str(),
        "k2",
        "so a new token names the new key, and `lakelet/4.23-secret-rotation`'s central claim — \
         which key signed this — is visible in the token itself"
    );

    // Two keys that arrived at the same instant are still one order, and it is the ring's rather
    // than a map's: the greater id wins, so both machines that read these files agree.
    plant(&store, "k3", &secret("k3"), BASE + 1_000, None);
    assert_eq!(
        read_ring(&store).preferred().map(|key| key.id().as_str()),
        Some("k3"),
        "a tie is broken by the id, deterministically"
    );

    // And a retired key never signs: retiring the newest one hands the signing back to the key
    // below it rather than leaving the ring with a preferred key that cannot be used.
    plant(
        &store,
        "k3",
        &secret("k3"),
        BASE + 1_000,
        Some(BASE + 600_000),
    );
    assert_eq!(
        read_ring(&store).preferred().map(|key| key.id().as_str()),
        Some("k2"),
        "a key with an end written for it is not the one that signs"
    );

    // A ring whose every key was retired cannot sign at all, and says so instead of minting a
    // token nothing would verify.
    plant(&store, "k1", &secret("k1"), BASE, Some(BASE + 600_000));
    plant(
        &store,
        "k2",
        &secret("k2"),
        BASE + 1_000,
        Some(BASE + 600_000),
    );
    let all_retired = read_ring(&store);
    assert!(
        all_retired.preferred().is_none(),
        "a ring whose every key was retired has no key that signs"
    );
    assert!(
        all_retired.is_open(),
        "the ring still authenticates — it holds keys — and it is the signing that has stopped"
    );
    let refused = all_retired
        .issue("client-1", LIFETIME, BASE + 700_000)
        .expect_err("nothing on this ring is open");
    assert!(
        refused.to_string().contains("no key"),
        "and the refusal names the fact rather than the key: {refused}"
    );
}

#[test]
fn the_rotation_window_is_arithmetic_and_a_retirement_one_millisecond_early_is_refused() {
    let store = StoreDir::new("rot-window");
    plant(&store, "k1", &secret("k1"), BASE, None);
    plant(&store, "k2", &secret("k2"), BASE + 1_000_000, None);
    let ring = read_ring(&store);
    let safe = BASE + 1_000_000 + longest();
    assert_eq!(
        ring.earliest_safe_retirement(&KeyId::new("k1")),
        Some(safe),
        "the instant after which no token k1 signed can still be live is the last instant it could \
         have signed — the instant k2 took the signing over — plus the longest lifetime this \
         deployment may issue"
    );
    assert_eq!(
        ring.earliest_safe_retirement(&KeyId::new("k2")),
        None,
        "and the key that signs has no such instant: nothing has taken the signing over from it, \
         so it may still sign, and the ring says it cannot prove a bound rather than guessing one"
    );
    assert_eq!(
        ring.earliest_safe_retirement(&KeyId::new("nobody")),
        None,
        "a key this ring does not hold has no window either"
    );

    // The operator's half: one millisecond under that instant is a refusal, with the instant named
    // and nothing on the answer stream — a retirement that strands a live token is the failure
    // this step is about, and a warning would leave the file written.
    let directory = store.path().display().to_string();
    let early = typed(safe - 1);
    let refused = lakeletq(&[
        "rotate", "--store", &directory, "--retire", "k1", "--at", &early,
    ]);
    assert_eq!(
        refused.code(),
        4,
        "a retirement before the safe instant is `the store refused`: {refused:?}"
    );
    assert_eq!(refused.stdout(), "", "nothing is answered: {refused:?}");
    assert!(
        refused.stderr().contains(&safe.to_string()),
        "and the one sentence names the instant the operator has to wait for: {refused:?}"
    );
    assert!(
        read_ring(&store).keys()[0].not_after_millis().is_none(),
        "the refused call left the key file exactly as it found it"
    );
}

#[test]
fn retiring_at_the_safe_instant_is_accepted_and_takes_the_last_token_with_it() {
    let store = StoreDir::new("rot-exact");
    plant(&store, "k1", &secret("k1"), BASE, None);
    // The longest token this deployment may issue, signed by k1 while k1 is still the key that
    // signs — the worst case a rotation has to survive.
    let issued = BASE + 1_000_000;
    let token = read_ring(&store)
        .issue("client-1", longest(), issued)
        .unwrap_or_else(|error| {
            panic!("the longest lifetime this build issues is one it issues: {error}")
        });
    let safe = issued + longest();
    assert_eq!(token.expires_millis(), safe);

    // k2 arrives at the instant the token was issued, so k1 stops signing there and its tokens are
    // live until `safe` and not one instant longer.
    plant(&store, "k2", &secret("k2"), issued, None);
    assert_eq!(
        read_ring(&store).earliest_safe_retirement(&KeyId::new("k1")),
        Some(safe)
    );

    let directory = store.path().display().to_string();
    let retired = lakeletq(&["rotate", "--store", &directory, "--retire", "k1"]);
    assert_eq!(
        retired.code(),
        0,
        "the safe instant is accepted, and it is what a retirement with no `--at` means: {retired:?}"
    );
    assert!(
        retired.stdout().contains(&format!("not_after={safe}")),
        "the answer is the ring's own line for the key it just retired: {retired:?}"
    );
    assert!(
        retired.stdout().contains("preferred=false") && retired.stdout().contains("id=k1"),
        "and k1 is no longer the key that signs: {retired:?}"
    );

    let after = read_ring(&store);
    assert!(
        after.verify(&token, safe - 1).is_ok(),
        "one millisecond before the safe instant the last token k1 signed is still good: the \
         retirement is at the boundary and not before it"
    );
    assert_eq!(
        after.verify(&token, safe),
        Err(Refusal::KeyRetired {
            id: KeyId::new("k1"),
            retired_millis: safe,
        }),
        "and at the instant itself it is refused as a retirement, which is the instant the token \
         expired at: a retirement placed any earlier would have refused a live client"
    );
    assert!(
        after.verify(&token, safe + 1).is_err(),
        "and afterwards, for the rest of time"
    );
    let signed = after
        .issue("client-2", LIFETIME, safe + 1)
        .unwrap_or_else(|error| panic!("the new key signs: {error}"));
    assert_eq!(
        signed.key_id().as_str(),
        "k2",
        "and the client that arrives after the rotation is signed by the new key, so the ring \
         never went a moment without one"
    );
}

/// The payload of one batch the daemon stores where a case is about the token and not the data.
fn batch_payload() -> Vec<u8> {
    payload(7)
}

/// One source file, read as text, for the cases that judge what a file *says*.
fn read(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{} is part of the project: {error}", path.display()))
}

/// The machine's instant, for the cases that judge the daemon *as a process*.
///
/// The daemon is the one place in this project that reads a clock (`service::ingest`, once per
/// connection), so a token it is asked to verify has to be minted at an instant near this one: the
/// cases below open keys a minute ago and issue tokens that live for a minute, and every judgement
/// they make is a count. What a case decides therefore does not depend on how fast the machine is —
/// only on whether the token's window covers the second the daemon reads, and a minute of margin is
/// not a thing a spec run can spend.
fn wall_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

#[test]
fn a_rotation_takes_effect_on_a_running_daemon_without_a_restart() {
    let now = wall_now();
    let mut service = Service::start("rot-live", &[]);
    // The first key arrives *after* the daemon started, and the client it authenticates connects
    // afterwards: a daemon that read its ring once at startup has an empty one, takes the token for
    // a batch and stores nothing that verifies.
    plant(service.store(), "k1", &secret("k1"), now - 60_000, None);
    let first = read_ring(service.store())
        .issue("client-1", LIFETIME, now - 1_000)
        .unwrap_or_else(|error| panic!("k1 is open and signs: {error}"));
    assert_eq!(first.key_id().as_str(), "k1");
    service.send(&wire(&[first.encode(), payload(0)]));
    let closed = service.wait_for_line(&format!(
        "connection closed accepted=1 rejected=0 bytes={}",
        payload(0).len()
    ));
    assert!(
        closed.contains("accepted=1"),
        "the client under the old key is served: {closed}"
    );

    // The rotation: the successor appears while the daemon runs, and the ring is read per
    // connection, so the very next client is signed by it.
    plant(service.store(), "k2", &secret("k2"), now, None);
    let second = read_ring(service.store())
        .issue("client-2", LIFETIME, now)
        .unwrap_or_else(|error| panic!("k2 took the signing over: {error}"));
    assert_eq!(
        second.key_id().as_str(),
        "k2",
        "a client connecting after the new key arrived is signed by the new key"
    );
    service.send(&wire(&[second.encode(), payload(1)]));
    let closed = service.wait_for_line(&format!(
        "connection closed accepted=1 rejected=0 bytes={}",
        payload(1).len()
    ));
    assert!(
        closed.contains("accepted=1"),
        "and it is served without the daemon being restarted: {closed}"
    );

    // And the key that is still on the ring verifies as well — two keys valid, one of them
    // preferred — which is what makes the rotation a window rather than a cut.
    service.send(&wire(&[first.encode(), payload(2)]));
    let closed = service.wait_for_line(&format!(
        "connection closed accepted=1 rejected=0 bytes={}",
        payload(2).len()
    ));
    assert!(
        closed.contains("accepted=1"),
        "the old key's still-live token is accepted after the rotation: {closed}"
    );
    assert!(
        service.is_running(),
        "no restart, no signal and no downtime: the same process served all three connections"
    );
    assert_eq!(
        records(service.store()).len(),
        3,
        "and the three batches those connections carried are all on the file"
    );
}

#[test]
fn a_token_round_trips_through_its_payload_and_that_payload_is_a_frame() {
    let store = StoreDir::new("rot-layout");
    plant(&store, "k1", &secret("k1"), BASE, None);
    let ring = read_ring(&store);
    let token = ring
        .issue("client-7", LIFETIME, BASE + 1_000)
        .unwrap_or_else(|error| panic!("the only key is open and signs: {error}"));
    let payload = token.encode();
    let client = "client-7".len();

    // The layout the module writes down, field by field and in order: two lengths, two ids, two
    // instants and sixty-four bytes of hex tag.
    assert_eq!(payload.len(), 2 + client + 2 + 2 + 8 + 8 + 64);
    assert_eq!(&payload[0..2], &(client as u16).to_le_bytes());
    assert_eq!(&payload[2..2 + client], b"client-7");
    assert_eq!(&payload[2 + client..4 + client], &2u16.to_le_bytes());
    assert_eq!(&payload[4 + client..6 + client], b"k1");
    let issued = 6 + client;
    assert_eq!(&payload[issued..issued + 8], &(BASE + 1_000).to_le_bytes());
    assert_eq!(
        &payload[issued + 8..issued + 16],
        &(BASE + 1_000 + LIFETIME).to_le_bytes()
    );
    let tag = &payload[issued + 16..];
    assert!(
        tag.iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)),
        "the tag is lower-case hex, which is what makes a payload a person can read at all"
    );
    assert_eq!(
        token.signed_bytes().len() + 64,
        payload.len(),
        "the tag covers the bytes before it and is not one of them: what is signed is what is sent"
    );

    let decoded = Token::decode(&payload).unwrap_or_else(|refusal| {
        panic!("a token this build wrote is a token this build reads: {refusal}")
    });
    assert_eq!(
        decoded, token,
        "every field survives the round trip, tag included"
    );

    // The token travels inside `lakelet/4.1-ingest-protocol`'s frame, and that framing did not
    // change for it: a token is a payload, and the three constants the protocol is built around are
    // the ones it was built around.
    assert_eq!(frame::VERSION, 1);
    assert_eq!(frame::OVERHEAD, 9);
    assert_eq!(frame::MAX_PAYLOAD, 1_048_576);
    let framed = Frame::new(payload.clone()).encode();
    match frame::decode(&framed) {
        Decoded::Frame(read, used) => {
            assert_eq!(
                read.payload, payload,
                "the frame carries the token, byte for byte"
            );
            assert_eq!(
                used,
                framed.len(),
                "and scans cleanly, with nothing left over"
            );
        }
        other => panic!("a token frame is a frame of this protocol: {other:?}"),
    }

    // A payload that is *almost* a token is not one, and the reader says so rather than reading
    // past the end of it or building a token out of half a tag.
    for cut in [1, 3, 64] {
        let short = payload[..payload.len() - cut].to_vec();
        assert!(
            matches!(Token::decode(&short), Err(Refusal::Malformed { .. })),
            "a payload {cut} byte(s) short of a token is malformed"
        );
    }
    assert!(
        matches!(Token::decode(&payload[..6]), Err(Refusal::Malformed { .. })),
        "and so is a prefix of one: no field runs past the end of what arrived"
    );
}

#[test]
fn rotate_add_writes_a_key_the_ring_loads_and_list_prints_the_ring_in_order() {
    let store = StoreDir::new("rot-cli");
    let directory = store.path().display().to_string();
    let younger = format!("{}", BASE + 3_000_000);
    let added = lakeletq(&[
        "rotate",
        "--store",
        &directory,
        "--add",
        "--id",
        "new",
        "--secret",
        &secret("new"),
        "--from",
        &younger,
    ]);
    assert_eq!(added.code(), 0, "adding a key answers: {added:?}");
    assert_eq!(added.stderr(), "", "and diagnoses nothing: {added:?}");
    assert_eq!(
        added.stdout(),
        format!(
            "id=new not_before={} not_after=none preferred=true safe_retire=none\n",
            BASE + 3_000_000
        ),
        "the answer is the ring's own line for the key it just wrote, and the one key this ring \
         holds has nothing newer above it to be retired after"
    );
    let loaded = read_ring(&store);
    assert_eq!(
        loaded.keys().len(),
        1,
        "the file it wrote is a file the daemon's own reader loads"
    );
    assert_eq!(loaded.keys()[0].id().as_str(), "new");
    assert!(loaded.keys()[0].is_open());

    // A key is added and never replaced: swapping a secret under a live client is the failure this
    // step is about, and it is refused rather than done.
    let again = lakeletq(&[
        "rotate",
        "--store",
        &directory,
        "--add",
        "--id",
        "new",
        "--secret",
        &secret("other"),
        "--from",
        &younger,
    ]);
    assert_eq!(
        again.code(),
        4,
        "a key already on the ring is refused: {again:?}"
    );
    assert_eq!(again.stdout(), "", "nothing is answered: {again:?}");
    assert!(
        again.stderr().contains("new"),
        "and the sentence names the key that is already there: {again:?}"
    );
    assert_eq!(
        read_ring(&store).keys()[0].not_before_millis(),
        BASE + 3_000_000,
        "the refused call left the key file exactly as it found it"
    );

    // A second key, older, so the listing has an order to show: oldest first, which is the order a
    // rotation happens in.
    let older = lakeletq(&[
        "rotate",
        "--store",
        &directory,
        "--add",
        "--id",
        "old",
        "--secret",
        &secret("old"),
        "--from",
        &typed(BASE),
    ]);
    assert_eq!(older.code(), 0, "{older:?}");

    let listed = lakeletq(&["rotate", "--store", &directory, "--list"]);
    assert_eq!(listed.code(), 0, "{listed:?}");
    let lines: Vec<&str> = listed.stdout().lines().collect();
    assert_eq!(
        lines,
        vec![
            format!(
                "id=old not_before={} not_after=none preferred=false safe_retire={}",
                BASE,
                BASE + 3_000_000 + longest()
            ),
            format!(
                "id=new not_before={} not_after=none preferred=true safe_retire=none",
                BASE + 3_000_000
            ),
        ],
        "one line per key, oldest first, each with its window, whether it signs, and the instant \
         it may leave: {listed:?}"
    );

    let asked = lakeletq(&["rotate", "--store", &directory, "--list", "--json"]);
    assert_eq!(asked.code(), 0, "{asked:?}");
    let parsed: serde_json::Value = serde_json::from_str(asked.stdout())
        .unwrap_or_else(|error| panic!("`--json` answers one JSON value: {error}: {asked:?}"));
    assert_eq!(
        parsed,
        serde_json::json!([
            {
                "id": "old",
                "not_before": BASE,
                "not_after": serde_json::Value::Null,
                "preferred": false,
                "safe_retire": BASE + 3_000_000 + longest(),
            },
            {
                "id": "new",
                "not_before": BASE + 3_000_000,
                "not_after": serde_json::Value::Null,
                "preferred": true,
                "safe_retire": serde_json::Value::Null,
            },
        ]),
        "--json is the same objects in the same order, with `null` where a value is absent: {asked:?}"
    );
}

#[test]
fn rotate_classes_every_mistake_the_way_the_cli_contract_does() {
    let store = StoreDir::new("rot-classes");
    let directory = store.path().display().to_string();
    plant(&store, "k1", &secret("k1"), BASE, None);
    plant(&store, "k2", &secret("k2"), BASE + 1_000_000, None);

    // 3: the thing asked about is not there — a store, and a key.
    let nowhere = store.path().join("elsewhere").display().to_string();
    let gone = lakeletq(&["rotate", "--store", &nowhere, "--list"]);
    assert_eq!(gone.code(), 3, "a store that is not there is `3`: {gone:?}");
    assert_eq!(gone.stdout(), "", "{gone:?}");
    let absent = lakeletq(&["rotate", "--store", &directory, "--retire", "nobody"]);
    assert_eq!(
        absent.code(),
        3,
        "a key that is not on the ring is the thing asked about not being there: {absent:?}"
    );
    assert!(
        absent.stderr().contains("nobody"),
        "and the sentence names it: {absent:?}"
    );

    // 2: the call was wrong — five shapes of it, each refused before anything is opened.
    let nothing = lakeletq(&["rotate", "--store", &directory]);
    assert_eq!(
        nothing.code(),
        2,
        "no operation is a wrong call: {nothing:?}"
    );
    let two = lakeletq(&["rotate", "--store", &directory, "--list", "--add"]);
    assert_eq!(two.code(), 2, "two operations is a wrong call: {two:?}");
    let secret_typo = lakeletq(&[
        "rotate", "--store", &directory, "--add", "--id", "k9", "--secret", "not-hex",
    ]);
    assert_eq!(
        secret_typo.code(),
        2,
        "a secret that is not hex: {secret_typo:?}"
    );
    let instant_typo = lakeletq(&[
        "rotate", "--store", &directory, "--retire", "k1", "--at", "soon",
    ]);
    assert_eq!(
        instant_typo.code(),
        2,
        "an instant that is not a number: {instant_typo:?}"
    );
    let escaping = lakeletq(&[
        "rotate",
        "--store",
        &directory,
        "--add",
        "--id",
        "../k9",
        "--secret",
        &secret("k9"),
    ]);
    assert_eq!(
        escaping.code(),
        2,
        "a name that would walk out of the ring's directory is a wrong call: {escaping:?}"
    );
    assert!(
        !store.path().join("..").join("k9.key").exists(),
        "and nothing was written outside the ring"
    );

    // 4: the store refused — and the default `--at` is the earliest instant a retirement is
    // allowed at, so retiring the key that signs is refused until its successor is there.
    let signing = lakeletq(&["rotate", "--store", &directory, "--retire", "k2"]);
    assert_eq!(
        signing.code(),
        4,
        "the key nothing newer has taken the signing over from cannot be retired: {signing:?}"
    );
    assert_eq!(signing.stdout(), "", "{signing:?}");
    assert!(
        signing.stderr().contains("successor") || signing.stderr().contains("newer"),
        "and the sentence says what to do about it: {signing:?}"
    );
    let twice = lakeletq(&[
        "rotate",
        "--store",
        &directory,
        "--retire",
        "k1",
        "--at",
        &typed(BASE + 1_000_000 + longest()),
    ]);
    assert_eq!(twice.code(), 0, "{twice:?}");
    let again = lakeletq(&["rotate", "--store", &directory, "--retire", "k1"]);
    assert_eq!(
        again.code(),
        4,
        "a key that is already retired is not retired again: {again:?}"
    );
    assert!(
        again
            .stderr()
            .contains(&(BASE + 1_000_000 + longest()).to_string()),
        "and the sentence names the instant it left at: {again:?}"
    );
}

/// The two instants the page's example ring was built with, and the ids it shows.
const PAGE_OLD: u64 = 1_700_000_000_000;
const PAGE_NEW: u64 = 1_700_003_000_000;

#[test]
fn the_page_carries_the_ring_the_command_prints() {
    let text = page();
    let lowered = text.to_ascii_lowercase();
    for placeholder in ["todo", "tbd", "fixme"] {
        assert!(
            !lowered.contains(placeholder),
            "{PAGE} still says `{placeholder}`: a page with a blank in it is a page nobody has \
             written down"
        );
    }
    // The four things a rotation page is for, in the page's own words.
    assert!(
        lowered.contains("authentication is off"),
        "{PAGE} does not say what an empty ring means: a deployment with no keys does not \
         authenticate, and that is the switch rather than an accident"
    );
    assert!(
        lowered.contains("subtle"),
        "{PAGE} does not name the constant-time comparison: `==` on a tag leaks where a guess \
         diverged, and that sentence is one line of the lesson"
    );
    for missing in ["hsm", "revocation", "generate"] {
        assert!(
            lowered.contains(missing),
            "{PAGE} does not say what this step does not do (`{missing}`): the honest half of a \
             security page is the list of things it is not"
        );
    }
    assert!(
        text.contains(&longest().to_string()),
        "{PAGE} does not carry the longest lifetime this deployment may issue ({}), which is the \
         number the whole rotation window is arithmetic on",
        longest()
    );

    // The block is one command's whole answer, for the ring the page states, re-run here.
    let block = quoted(&text);
    let store = StoreDir::new("rot-page");
    let directory = store.path().display().to_string();
    for (id, secret, from) in [
        ("old", secret("page-old"), PAGE_OLD),
        ("new", secret("page-new"), PAGE_NEW),
    ] {
        let added = lakeletq(&[
            "rotate",
            "--store",
            &directory,
            "--add",
            "--id",
            id,
            "--secret",
            &secret,
            "--from",
            &typed(from),
        ]);
        assert_eq!(
            added.code(),
            0,
            "the page's ring is one `--add` writes: {added:?}"
        );
    }
    let listed = lakeletq(&["rotate", "--store", &directory, "--list"]);
    assert_eq!(listed.code(), 0, "{listed:?}");
    let printed: Vec<String> = listed.stdout().lines().map(str::to_owned).collect();
    assert_eq!(
        printed, block,
        "{PAGE}'s block is not what `lakeletq rotate --list` prints: the page's numbers are a \
         measurement, and the way a page full of measurements stays true is that the command which \
         produced them is re-run and compared. Edit the page from a run, never from an intention."
    );
}

/// The block `docs/rotation.md` quotes, which is a ring as the command prints it.
///
/// The lines are what identify it — `id=… safe_retire=…` is how `--list` answers, and the page also
/// shows a *key file*, which also has an `id=` line — and the sentence above it has to name the
/// command, because a block of numbers with no command behind it is a block nobody can re-run.
fn quoted(page: &str) -> Vec<String> {
    let lines: Vec<&str> = page.lines().collect();
    let anchor = lines
        .iter()
        .position(|line| line.starts_with("id=") && line.contains("safe_retire="))
        .unwrap_or_else(|| {
            panic!(
                "{PAGE} does not quote a ring: a reader has to be able to see the keys an operator \
                 reads, and no line of the page is one"
            )
        });
    let opening = lines[..anchor]
        .iter()
        .rposition(|line| line.trim_start().starts_with("```"))
        .unwrap_or_else(|| {
            panic!(
                "{PAGE} has a ring's line at {anchor} outside a fenced block: a reader has to be \
                 able to copy the block out of the page"
            )
        });
    let closing = anchor
        + lines[anchor..]
            .iter()
            .position(|line| line.trim_start().starts_with("```"))
            .unwrap_or_else(|| panic!("{PAGE} opens a fenced block it never closes"));
    assert!(
        lines[..opening]
            .iter()
            .rev()
            .any(|line| line.contains("lakeletq rotate --list")),
        "{PAGE} does not say which command produced the block it carries: state `lakeletq rotate \
         --list` in the sentence above it"
    );
    lines[opening + 1..closing]
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
}

#[test]
fn the_ring_reads_no_clock_and_the_daemon_reads_one_in_one_place() {
    let module = read("src/service/keys.rs");
    for forbidden in [
        "SystemTime",
        "Instant",
        "UNIX_EPOCH",
        "std::env",
        "env::var",
    ] {
        assert!(
            !module.contains(forbidden),
            "src/service/keys.rs names `{forbidden}`: every function there that depends on an \
             instant takes one as a number, which is what makes `what does this ring do at this \
             instant` a question a case answers with a value rather than a wall clock"
        );
    }

    // The other half of the same rule: the clock the daemon does read is read in one place, so a
    // reader knows where to look for it.
    let mut service = Vec::new();
    collect_rust(Path::new("src/service"), &mut service)
        .expect("src/service is part of the project");
    let reading: Vec<String> = service
        .iter()
        .filter(|path| read(path).contains("SystemTime::now"))
        .map(|path| path.display().to_string())
        .collect();
    assert_eq!(
        reading,
        vec!["src/service/ingest.rs".to_owned()],
        "the daemon's clock is a connection's arrival instant, and it is read where the connection \
         is judged and nowhere else"
    );

    // And the layer rule `src/service/mod.rs` states: the engine may not use the service, and a
    // ring is a service-module type.
    let mut engine = Vec::new();
    collect_rust(Path::new("src/engine"), &mut engine).expect("src/engine is part of the project");
    assert!(
        !engine.is_empty(),
        "the engine is the bigger half of this crate"
    );
    for path in &engine {
        let text = read(path);
        assert!(
            !text.contains("crate::service") && !text.contains("service::keys"),
            "{} names the service layer: the engine is what runs in a load generator, a test and a \
             repair tool, and a ring that reached into it would be a socket's business inside the \
             store's",
            path.display()
        );
    }
}

#[test]
fn the_two_harnesses_that_came_before_this_step_still_send_batches_with_no_token() {
    // The claim `lakelet/4.23-secret-rotation` has to keep: 4.6's overload scenario and 4.17's soak
    // both drive the daemon in this process and send batches with no token, and the step that keeps
    // them working is the empty ring — not a change in them.
    for (harness, drives) in [
        ("tests/sim_backpressure.rs", "Queue::new"),
        ("tests/soak.rs", "ingest::serve"),
    ] {
        let text = read(harness);
        assert!(
            text.contains(drives),
            "{harness} drives `{drives}`, which is a path this step changed"
        );
        for forbidden in ["service::keys", "KeyRing", "Token::", "key_file"] {
            assert!(
                !text.contains(forbidden),
                "{harness} speaks the ring's language now (`{forbidden}`): those harnesses connect \
                 and send batches, and a step that made them present a token would have moved work \
                 no earlier step's contract allows to move"
            );
        }
    }

    // And the daemon they drive is still the daemon: a bound, a policy, a burst, and books that add
    // up. `sample` keeps one arrival in four, so the shape of the answer is the same whatever the
    // machine's speed is.
    let mut service = Service::start(
        "rot-harness",
        &[
            Var::new("LAKELET_QUEUE_CAPACITY", "2"),
            Var::new("LAKELET_QUEUE_POLICY", "sample"),
        ],
    );
    service.send(&wire(&[payload(0), payload(1), payload(2), payload(3)]));
    let closed = service.wait_for_line("connection closed");
    let accepted = field(&closed, "accepted");
    let rejected = field(&closed, "rejected");
    assert_eq!(
        accepted + rejected,
        4,
        "every frame a client with no token sends is written or refused, exactly as before this \
         step: {closed}"
    );
    assert!(
        accepted >= 1,
        "and a deployment with no ring still stores data: {closed}"
    );
    assert!(
        service.is_running(),
        "an overload is not a reason to die, and neither is a step about authentication"
    );
}

#[test]
fn a_key_file_is_read_the_way_a_person_writes_it_and_a_broken_one_names_the_file() {
    let store = StoreDir::new("rot-files");
    let directory = ring_dir(&store);
    std::fs::create_dir_all(&directory)
        .unwrap_or_else(|error| panic!("{} cannot be created: {error}", directory.display()));

    // A file a person wrote: a comment, a blank line, spacing around the `=`, and a secret in
    // upper case, which is the same bytes.
    let hand = format!(
        "# the ring of this store\n\nid = hand \nsecret = {}\nnot_before_millis = 1000\n",
        secret("hand").to_ascii_uppercase()
    );
    std::fs::write(directory.join("hand.key"), hand)
        .unwrap_or_else(|error| panic!("the hand-written key cannot be written: {error}"));
    let ring = KeyRing::load(&directory).unwrap_or_else(|error| {
        panic!("a key file a person wrote is one this ring reads: {error}")
    });
    assert_eq!(ring.keys().len(), 1);
    assert_eq!(ring.keys()[0].id().as_str(), "hand");
    assert_eq!(ring.keys()[0].not_before_millis(), 1_000);
    assert!(ring.keys()[0].is_open());
    let token = ring
        .issue("client-1", LIFETIME, 2_000)
        .unwrap_or_else(|error| panic!("the key it read signs: {error}"));
    assert!(
        ring.verify(&token, 3_000).is_ok(),
        "and a secret written in upper case is the same secret"
    );

    // Every way a file can be wrong, and every one of them names the file: an operator editing
    // this directory by hand is the person these sentences are written for.
    let broken = [
        (
            "hex.key",
            "id=hex\nsecret=zzzz\nnot_before_millis=1000\n".to_owned(),
            "zzzz",
        ),
        (
            "window.key",
            format!(
                "id=window\nsecret={}\nnot_before_millis=2000\nnot_after_millis=1000\n",
                secret("window")
            ),
            "inverted",
        ),
        (
            "named.key",
            format!(
                "id=other\nsecret={}\nnot_before_millis=1000\n",
                secret("named")
            ),
            "other",
        ),
        (
            "unknown.key",
            format!(
                "id=unknown\nsecret={}\nnot_before_millis=1000\nnot_after=1000\n",
                secret("unknown")
            ),
            "not_after",
        ),
        ("empty.key", "# a key with nothing in it\n".to_owned(), "id"),
    ];
    for (name, text, needle) in broken {
        std::fs::write(directory.join(name), &text)
            .unwrap_or_else(|error| panic!("{name} cannot be written: {error}"));
        let refused = KeyRing::load(&directory)
            .err()
            .unwrap_or_else(|| panic!("{name} is not a key this build can read"));
        let sentence = refused.to_string();
        assert!(
            sentence.contains(name),
            "the refusal names the file an operator has to go and look at: {sentence}"
        );
        assert!(
            sentence.contains(needle),
            "and it says what is wrong with it (`{needle}`): {sentence}"
        );
        std::fs::remove_file(directory.join(name))
            .unwrap_or_else(|error| panic!("{name} cannot be removed: {error}"));
    }

    // A directory that exists and holds no key is a refusal rather than an empty ring: fail-open is
    // the one answer this must not give.
    std::fs::remove_file(directory.join("hand.key"))
        .unwrap_or_else(|error| panic!("the hand-written key cannot be removed: {error}"));
    let empty = KeyRing::load(&directory)
        .expect_err("a ring that exists with nothing in it is the state this rule is about");
    assert!(
        empty.to_string().contains("no `*.key`") && empty.to_string().contains(RING),
        "and the sentence names the directory and says what an operator should do about it: {empty}"
    );

    // A directory that is *not there* is the switch in its other position: no keys, no
    // authentication, and no error.
    let missing = directory.join("nowhere");
    let off = KeyRing::load(&missing)
        .unwrap_or_else(|error| panic!("a store with no ring has no keys and no error: {error}"));
    assert!(!off.is_open());
    assert!(off.keys().is_empty());
    assert!(
        off.preferred().is_none(),
        "an empty ring has no key that signs"
    );
    assert!(
        off.earliest_safe_retirement(&KeyId::new("hand")).is_none(),
        "and it holds no key to answer about"
    );
}
