//! Spec for `relay/2.11-hmac-signing`: what a receiver can tell about a request that arrives.
//!
//! `kiln run` copies this file into the project as `tests/spec_signing.rs`; it is the course's file,
//! not the learner's (plan §3 D2). Four properties:
//!
//! * **the signature is a function of bytes, not of values.** The same key, instant, delivery and
//!   body answer the same header — this file contains a *golden vector* (the exact header a
//!   documented input produces), so the format is pinned rather than described;
//! * **every part of the signed material is covered.** The body, the instant and the delivery id are
//!   in the tag, so changing any one of them is refused — including changing the body to a different
//!   but *equivalent* spelling of the same JSON, which is the failure a signature computed over
//!   re-serialised values produces;
//! * **a signature expires and is bound to one delivery.** A tag that proves "somebody with the key
//!   sent this body" is a bearer token: the delivery id is checked, not only hashed;
//! * **what the adapter sends is what a receiver can verify.** The last two tests drive
//!   `HttpSender` against a real scripted target and verify the request that arrived, once with a
//!   store a test filled in and once with the secret a tenant was actually given.
//!
//! One property this file cannot measure is time: whether the tag comparison is *constant* time is
//! not a question a test can answer by measuring a machine, and a timing assertion would be a
//! flaky test that fails on a loaded machine and passes on an unlucky one. What it checks instead
//! is the shape — that the comparison the module exposes is one function of two tags answering a
//! `bool`, that a tag which is wrong only in its last character fails the same way as one that is
//! wrong everywhere (no error that reports *where* a guess diverged), and that the comparison goes
//! through the crate that exists to do it in constant time.

mod support;

#[path = "support/keys.rs"]
mod keys;

#[path = "support/target.rs"]
mod target;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use relay::db::Db;
use relay::delivery::{SendError, SendRequest, Sender};
use relay::domain::{EndpointUrl, TenantId};
use relay::events::Event;
use relay::outbound::{HttpSender, Policy};
use relay::signing::{
    self, DbSecrets, SIGNATURE_HEADER, Secret, SignatureError, TIMESTAMP_HEADER, Window,
};
use serde_json::json;
use sqlx::PgPool;
use support::app::{self, App};
use support::client::Request;
use support::postgres::Postgres;
use support::var;
use target::{Reply, Target};
use uuid::Uuid;

use keys::Keys;

/// The tenant the database tests seed, and the key it authenticates with.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// The documented input of the golden vector, and the header it produces.
///
/// The signing string is `v1.<timestamp>.<delivery>.<body>`, and the header is that string with its
/// body replaced by the lower-case hex of `HMAC-SHA256` under the secret — the same value
/// `openssl dgst -sha256 -hmac` and a four-line Python script produce, which is the point: a
/// receiver in another language has to be able to check this without reading Rust.
const GOLDEN_SECRET: &str = "whsec_6f1a4d2b9c7e5308";
const GOLDEN_TIMESTAMP: i64 = 1_760_000_000;
const GOLDEN_DELIVERY: &str = "4b1f9c2e-7a3d-4e58-9b6f-0c2d8e5a1f74";
const GOLDEN_BODY: &str = r#"{"attempt":2,"id":"9c1d7e4a-2b6f-4d13-8f07-5a3e6b0c9d21","kind":"invoice.paid","payload":{"amount":4200},"received_at":"2025-10-09T08:53:20Z"}"#;
const GOLDEN_SIGNATURE: &str = "v1.1760000000.4b1f9c2e-7a3d-4e58-9b6f-0c2d8e5a1f74.eb6e06b1ba7f7af79718673989a105394f8d6d789ba0a3930e97e7182e8a6d65";

/// The two headers a receiver was handed, as they would arrive.
fn headers(signature: &str, timestamp: i64) -> Vec<(String, String)> {
    vec![
        (SIGNATURE_HEADER.to_owned(), signature.to_owned()),
        (TIMESTAMP_HEADER.to_owned(), timestamp.to_string()),
    ]
}

/// The delivery of the golden vector, as a value.
fn golden_delivery() -> Uuid {
    GOLDEN_DELIVERY.parse().expect("the fixture's delivery id")
}

/// A window wide enough that nothing in these tests is stale by accident.
fn window() -> Window {
    Window {
        max_age: 300,
        max_skew: 60,
    }
}

/// Signing the same request twice answers the same header, and the documented one.
#[test]
fn the_same_request_signed_twice_is_the_same_signature() {
    let secret = Secret::new(GOLDEN_SECRET);
    let signature = signing::sign(
        &secret,
        GOLDEN_TIMESTAMP,
        golden_delivery(),
        GOLDEN_BODY.as_bytes(),
    );

    assert_eq!(
        signature,
        signing::sign(
            &secret,
            GOLDEN_TIMESTAMP,
            golden_delivery(),
            GOLDEN_BODY.as_bytes()
        ),
        "a signature is a function of its inputs: a receiver that checks the same request twice \
         has to get the same answer, and a signature that changed between two calls is a signature \
         nobody can test"
    );
    assert_eq!(
        signature, GOLDEN_SIGNATURE,
        "the signature of the documented input is not the documented one. The signing string is \
         `v1.<timestamp>.<delivery>.<body bytes>`, the tag is the lower-case hex of \
         HMAC-SHA256(secret, that string), and the header is the same string with the body \
         replaced by the tag — check it against `openssl dgst -sha256 -hmac` before changing this \
         file"
    );
    assert_eq!(
        signature.split('.').count(),
        4,
        "four parts — version, timestamp, delivery, tag — and no more: `{signature}`"
    );
}

/// The bytes are the contract: a body that is not the one that was signed is refused.
#[test]
fn a_body_that_is_not_the_one_that_was_signed_is_refused() {
    let secret = Secret::new(GOLDEN_SECRET);
    let signature = signing::sign(
        &secret,
        GOLDEN_TIMESTAMP,
        golden_delivery(),
        GOLDEN_BODY.as_bytes(),
    );

    for body in [
        // One byte different.
        GOLDEN_BODY.replace("4200", "4201"),
        // The same JSON, spelled differently: another key order, another whitespace.
        GOLDEN_BODY
            .replace("{\"attempt\"", "{ \"attempt\"")
            .replace(",\"id\"", ", \"id\""),
        // Nothing at all.
        String::new(),
    ] {
        let refused = signing::verify(
            &secret,
            &headers(&signature, GOLDEN_TIMESTAMP),
            body.as_bytes(),
            golden_delivery(),
            GOLDEN_TIMESTAMP,
            window(),
        );
        assert!(
            matches!(refused, Err(SignatureError::Mismatch)),
            "a signature covers the bytes that were sent, so a body that is not those bytes has to \
             be refused — including one that is the same JSON in another spelling, which is the \
             failure a signature computed over a re-serialised value produces: {body:?} was \
             accepted as {refused:?}"
        );
    }

    // And the bytes that were signed are accepted, so the refusals above mean something.
    assert!(
        signing::verify(
            &secret,
            &headers(&signature, GOLDEN_TIMESTAMP),
            GOLDEN_BODY.as_bytes(),
            golden_delivery(),
            GOLDEN_TIMESTAMP,
            window(),
        )
        .is_ok(),
        "the request that was signed is the request that verifies"
    );
}

/// A signature for one delivery is not a signature for another.
#[test]
fn a_signature_is_for_one_delivery() {
    let secret = Secret::new(GOLDEN_SECRET);
    let other = Uuid::new_v4();
    let signature = signing::sign(
        &secret,
        GOLDEN_TIMESTAMP,
        golden_delivery(),
        GOLDEN_BODY.as_bytes(),
    );

    let refused = signing::verify(
        &secret,
        &headers(&signature, GOLDEN_TIMESTAMP),
        GOLDEN_BODY.as_bytes(),
        other,
        GOLDEN_TIMESTAMP,
        window(),
    );
    assert!(
        matches!(refused, Err(SignatureError::Mismatch)),
        "the signature names the delivery it was made for, and a receiver is told which delivery it \
         is being handed: checking only that the tag is *a* valid tag makes the signature a bearer \
         token that anybody may replay for a different delivery: {refused:?}"
    );

    // The same, with the delivery inside the signature changed rather than the one expected.
    let mut tampered = signature.clone();
    let head = tampered.rfind('.').expect("four parts") - GOLDEN_DELIVERY.len();
    tampered.replace_range(head..head + GOLDEN_DELIVERY.len(), &other.to_string());
    assert!(
        matches!(
            signing::verify(
                &secret,
                &headers(&tampered, GOLDEN_TIMESTAMP),
                GOLDEN_BODY.as_bytes(),
                other,
                GOLDEN_TIMESTAMP,
                window(),
            ),
            Err(SignatureError::Mismatch)
        ),
        "and a delivery id that somebody swapped inside the header is refused: the id is part of the \
         signed material, so changing it changes the tag"
    );
}

/// A signature has an expiry, in both directions.
#[test]
fn an_instant_outside_the_window_is_refused() {
    let secret = Secret::new(GOLDEN_SECRET);
    let signature = signing::sign(
        &secret,
        GOLDEN_TIMESTAMP,
        golden_delivery(),
        GOLDEN_BODY.as_bytes(),
    );
    let window = window();

    for (now, what) in [
        (GOLDEN_TIMESTAMP + window.max_age + 1, "an hour later"),
        (
            GOLDEN_TIMESTAMP - window.max_skew - 1,
            "with a clock far ahead",
        ),
    ] {
        let refused = signing::verify(
            &secret,
            &headers(&signature, GOLDEN_TIMESTAMP),
            GOLDEN_BODY.as_bytes(),
            golden_delivery(),
            now,
            window,
        );
        assert!(
            matches!(refused, Err(SignatureError::Stale { .. })),
            "the instant is signed so that a signature can expire ({what}): a receiver that accepts \
             any instant accepts every replay of a request it has already handled, forever: \
             {refused:?}"
        );
    }

    for now in [
        GOLDEN_TIMESTAMP,
        GOLDEN_TIMESTAMP + window.max_age,
        GOLDEN_TIMESTAMP - window.max_skew,
    ] {
        let verified = signing::verify(
            &secret,
            &headers(&signature, GOLDEN_TIMESTAMP),
            GOLDEN_BODY.as_bytes(),
            golden_delivery(),
            now,
            window,
        );
        assert!(
            verified.is_ok(),
            "and both edges of the window are inside it — a window that refuses its own boundary is \
             a window an off-by-one in a receiver disagrees with: {verified:?}"
        );
        assert!(
            matches!(verified, Ok(verified) if verified.timestamp == GOLDEN_TIMESTAMP),
            "a verified signature reports the instant it proved: {verified:?}"
        );
    }

    // A timestamp in the header that disagrees with the one inside the signature is neither stale
    // nor fresh: the two are one value, and a receiver that checked one and hashed the other has a
    // hole where a replay fits.
    let refused = signing::verify(
        &secret,
        &headers(&signature, GOLDEN_TIMESTAMP + 1),
        GOLDEN_BODY.as_bytes(),
        golden_delivery(),
        GOLDEN_TIMESTAMP,
        window,
    );
    assert!(
        matches!(refused, Err(SignatureError::Mismatch)),
        "a `{TIMESTAMP_HEADER}` that disagrees with the timestamp inside `{SIGNATURE_HEADER}` is a \
         request two clocks: {refused:?}"
    );
}

/// A version this service does not make is refused, and so is a header that has no shape.
#[test]
fn a_signature_this_service_does_not_make_is_refused() {
    let secret = Secret::new(GOLDEN_SECRET);
    let signature = signing::sign(
        &secret,
        GOLDEN_TIMESTAMP,
        golden_delivery(),
        GOLDEN_BODY.as_bytes(),
    );
    let body = GOLDEN_BODY.as_bytes();

    let unknown = signature.replacen("v1.", "v2.", 1);
    assert!(
        matches!(
            signing::verify(
                &secret,
                &headers(&unknown, GOLDEN_TIMESTAMP),
                body,
                golden_delivery(),
                GOLDEN_TIMESTAMP,
                window(),
            ),
            Err(SignatureError::UnknownVersion(_))
        ),
        "a receiver refuses a version it does not know instead of verifying it against the rules of \
         the version it does: guessing is how a format becomes unchangeable"
    );

    let malformed = [
        (String::new(), "no signature at all"),
        ("v1".to_owned(), "a version and nothing else"),
        (
            format!("v1.not-a-number.{GOLDEN_DELIVERY}.deadbeef"),
            "a timestamp that is not one",
        ),
        (
            format!("v1.{GOLDEN_TIMESTAMP}.not-a-uuid.deadbeef"),
            "a delivery that is not one",
        ),
        (
            signature
                .rsplit_once('.')
                .map(|(head, _)| head.to_owned())
                .unwrap_or_default(),
            "no tag at all",
        ),
    ];
    for (header, what) in malformed {
        assert!(
            matches!(
                signing::verify(
                    &secret,
                    &headers(&header, GOLDEN_TIMESTAMP),
                    body,
                    golden_delivery(),
                    GOLDEN_TIMESTAMP,
                    window(),
                ),
                Err(SignatureError::Malformed(_))
            ),
            "{what} is malformed rather than a failed comparison, and the difference matters to the \
             person reading a log line: `{header}`"
        );
    }
}

/// The tag comparison the module exposes does not answer where two tags diverged.
#[test]
fn a_tag_that_is_close_is_still_wrong() {
    let secret = Secret::new(GOLDEN_SECRET);
    let signature = signing::sign(
        &secret,
        GOLDEN_TIMESTAMP,
        golden_delivery(),
        GOLDEN_BODY.as_bytes(),
    );
    let (head, tag) = signature.rsplit_once('.').expect("four parts");
    assert!(signing::tags_match(tag, tag), "two identical tags match");

    // Every single-character mutation of the tag is wrong, and every one of them is wrong the same
    // way: an answer that says *where* a guess diverged is a timing channel written in words.
    let mut answers = Vec::new();
    for index in 0..tag.len() {
        let mut guess: Vec<char> = tag.chars().collect();
        let replacement = if guess[index] == '0' { '1' } else { '0' };
        guess[index] = replacement;
        let guess: String = guess.into_iter().collect();
        assert!(
            !signing::tags_match(tag, &guess),
            "a tag with one character changed is not the same tag (character {index})"
        );
        let refused = signing::verify(
            &secret,
            &headers(&format!("{head}.{guess}"), GOLDEN_TIMESTAMP),
            GOLDEN_BODY.as_bytes(),
            golden_delivery(),
            GOLDEN_TIMESTAMP,
            window(),
        );
        answers.push(format!("{refused:?}"));
    }
    let distinct: std::collections::BTreeSet<&String> = answers.iter().collect();
    assert_eq!(
        distinct.len(),
        1,
        "every one of the {} guesses failed the same way, and the answers were {distinct:?}: a \
         failure that differs by *how close* a guess was is a failure that tells the other end how \
         to guess better — which is the same leak as a comparison that stops early, said out loud",
        answers.len()
    );

    // The comparison the module offers is the one the check uses, and it is delegated to the crate
    // that exists for it: `subtle`'s `ConstantTimeEq`. This is a structural check rather than a
    // measurement on purpose — see the file's own note above.
    let source = include_str!("../src/signing.rs");
    assert!(
        source.contains("subtle"),
        "the tag comparison has to go through `subtle`, the crate whose whole purpose is comparing \
         secrets without leaking where they differ: a `==` on a tag is a comparison that stops at \
         the first wrong byte, and a spec cannot measure that without measuring the machine it runs \
         on"
    );
}

/// One database and one in-process service, for the length of one test.
struct Fixture {
    app: App,
    postgres: Postgres,
}

impl Fixture {
    /// Start a service with one tenant, over a database of its own.
    fn start() -> Self {
        let postgres = Postgres::start();
        let app = App::start(app::config(&[
            var(app::DATABASE_URL, postgres.url()),
            var(app::BOOTSTRAP_TENANTS, format!("{}={}", ACME.0, ACME.1)),
            // The subscribers of this file are a scripted target on `127.0.0.1`, and the service
            // refuses an address on its own networks unless the deployment says otherwise
            // (`relay/2.12-ssrf-defense`): this fixture is a deployment whose subscribers are
            // internal, so it says so.
            var("RELAY__DELIVERY__ALLOW_PRIVATE_TARGETS", "true"),
        ]));
        // The fixture's readiness check is the *port*, and the kernel answers a connect before the
        // service has finished starting; this file reads the tables.
        let serving = app.client().get("/healthz");
        assert_eq!(
            serving.status, 200,
            "the service has to be serving before this file asks its database anything: {}",
            serving.body
        );
        Self { app, postgres }
    }

    /// The pool the worker runs on.
    async fn pool(&self) -> PgPool {
        Db::connect(&self.postgres.url())
            .await
            .expect("the fixture's pool connects")
            .pool()
            .clone()
    }

    /// One query, as text.
    fn query(&self, sql: &str) -> String {
        self.postgres.query(sql)
    }

    /// Register `url` for the fixture's tenant, and answer the secret it was given.
    fn register(&self, url: &str) -> String {
        let answer = self.app.client().send(
            Request::post("/endpoints", Vec::new())
                .bearer(ACME.1)
                .json(&json!({ "url": url })),
        );
        assert_eq!(
            answer.status, 201,
            "the fixture registers `{url}`: {}",
            answer.body
        );
        answer
            .field("secret")
            .as_str()
            .expect("registration answers with the secret once")
            .to_owned()
    }

    /// The fixture tenant's id, as the service keys it.
    fn tenant(&self) -> TenantId {
        let id: Uuid = self
            .query("select id from tenants where name = 'acme'")
            .parse()
            .expect("the seeded tenant's id");
        TenantId::from_uuid(id)
    }
}

/// One attempt, as the worker builds it, against a target a test is listening on.
fn attempt(delivery: Uuid, tenant: TenantId, url: EndpointUrl, now: DateTime<Utc>) -> SendRequest {
    SendRequest {
        delivery,
        event: Event {
            id: Uuid::new_v4(),
            tenant_id: tenant,
            kind: "invoice.paid".to_owned(),
            payload: json!({ "amount": 4200 }),
            received_at: now,
        },
        attempt: 1,
        url,
    }
}

/// What the adapter sends is what a receiver can verify, with a store a test filled in.
#[tokio::test]
async fn what_the_adapter_sends_is_what_a_receiver_can_verify() {
    let target = Target::start(vec![Reply::ok("taken")]);
    let tenant = TenantId::new();
    let endpoint: EndpointUrl = target.url("/hook").parse().expect("a target URL");
    let secret = "whsec_a1b2c3d4e5f60718";

    let keys = Keys::new().with(tenant, endpoint.as_str(), secret);
    let sender = HttpSender::signing(Policy::default(), Arc::new(keys));
    let delivery = Uuid::new_v4();
    let sent_at = Utc::now().timestamp();
    sender
        .send(attempt(delivery, tenant, endpoint, Utc::now()))
        .await
        .expect("the subscriber took it");

    let arrived = target.requests();
    assert_eq!(arrived.len(), 1, "one attempt, one request");
    let arrived = &arrived[0];
    let header = arrived
        .header(SIGNATURE_HEADER)
        .unwrap_or_else(|| panic!("the request carries `{SIGNATURE_HEADER}`: {arrived:?}"));
    assert_eq!(
        header.split('.').next(),
        Some("v1"),
        "and it is a version the receiver knows: {header}"
    );
    let timestamp: i64 = arrived
        .header(TIMESTAMP_HEADER)
        .unwrap_or_else(|| panic!("the request carries `{TIMESTAMP_HEADER}`: {arrived:?}"))
        .parse()
        .expect("the timestamp is a whole number of seconds");
    assert!(
        (sent_at..=Utc::now().timestamp()).contains(&timestamp),
        "stamped when it was sent, not when this test started: {timestamp}"
    );

    // The receiver's half: the headers and the body as they arrived, checked against the secret it
    // was given at registration.
    let verified = signing::verify(
        &Secret::new(secret),
        &arrived.headers,
        arrived.body.as_bytes(),
        delivery,
        Utc::now().timestamp(),
        Window::default(),
    );
    assert!(
        verified.is_ok(),
        "a receiver that checks what arrived with the secret it holds has to accept it: \
         {verified:?} — body `{}`, headers {:?}",
        arrived.body,
        arrived.headers
    );
}

/// An endpoint this service has no secret for is not sent to at all.
#[tokio::test]
async fn an_endpoint_without_a_secret_is_not_sent_to_unsigned() {
    let target = Target::start(vec![Reply::ok("taken")]);
    let tenant = TenantId::new();
    let endpoint: EndpointUrl = target.url("/hook").parse().expect("a target URL");
    let sender = HttpSender::signing(Policy::default(), Arc::new(Keys::new()));

    let refused = sender
        .send(attempt(Uuid::new_v4(), tenant, endpoint, Utc::now()))
        .await;
    assert!(
        matches!(refused, Err(SendError::Permanent(_))),
        "an attempt this service cannot sign is not an attempt it makes: sending it unsigned would \
         make the signature optional, and an optional signature is not a signature. Permanent, \
         because another attempt will not find a secret: {refused:?}"
    );
    assert_eq!(
        target.requests().len(),
        0,
        "and nothing was sent: a request in the target's log here is an unverifiable webhook in \
         somebody's production"
    );
}

/// The secret a tenant was given is the secret the adapter signs with.
#[tokio::test]
async fn the_secret_a_tenant_was_given_is_the_one_the_adapter_signs_with() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    let target = Target::start(vec![Reply::ok("taken")]);
    let url = target.url("/hook");
    let secret = fixture.register(&url);
    let tenant = fixture.tenant();

    let sender = HttpSender::signing(Policy::default(), Arc::new(DbSecrets::new(pool)));
    let delivery = Uuid::new_v4();
    let endpoint: EndpointUrl = url.parse().expect("the registered URL");
    sender
        .send(attempt(delivery, tenant, endpoint, Utc::now()))
        .await
        .expect("the subscriber took it");

    let arrived = target.requests();
    assert_eq!(arrived.len(), 1, "one attempt, one request");
    let verified = signing::verify(
        &Secret::new(secret),
        &arrived[0].headers,
        arrived[0].body.as_bytes(),
        delivery,
        Utc::now().timestamp(),
        Window::default(),
    );
    assert!(
        verified.is_ok(),
        "the key the tenant was answered at registration is the key this service signs with, and \
         the lookup is where the secret lives (the endpoints table) rather than in the queue: \
         {verified:?}"
    );
    assert!(
        arrived[0].header(SIGNATURE_HEADER).is_some_and(|header| {
            header
                .rsplit_once('.')
                .is_some_and(|(head, tag)| head.ends_with(&delivery.to_string()) && tag.len() == 64)
        }),
        "and the header names the delivery it is for, with a 64-character tag: {:?}",
        arrived[0].header(SIGNATURE_HEADER)
    );
}
