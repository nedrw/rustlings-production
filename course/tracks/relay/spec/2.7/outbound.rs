//! Spec for `relay/2.7-outbound-client`: one request to somebody else's server, with a bound on how
//! long it may take.
//!
//! `kiln run` copies this file into the project as `tests/spec_outbound.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Every test in it talks to a **real server**, not to a mock:
//! `tests/support/target.rs` is a scripted HTTP target on a port of its own, and it answers slowly,
//! answers nothing, redirects, closes, or behaves — the five things a subscriber does to a client
//! that has no deadlines.
//!
//! Two properties run through the whole file:
//!
//! * **every wait has a bound**, and the three bounds are different questions: the connection, one
//!   read, and the whole attempt. A test that only bounds the whole attempt cannot tell a client
//!   that cut a silent target at its read deadline from one that waited for the attempt deadline;
//! * **what the client does with an answer is a decision, not a pass-through.** `Outcome` is the
//!   only thing the delivery layer needs to know, and it is the difference between a delivery that
//!   is retried and one that stops.
//!
//! A note for whoever authors the next step: this file builds `Request`s, builds a `SendRequest` for
//! the port test, and reads `Response`'s fields. Adding a field to `SendRequest` (a signing secret,
//! say) means updating this file as well, and re-pinning it with `kiln spec manifest`.

#[path = "support/target.rs"]
mod target;

use std::time::{Duration, Instant};

use chrono::Utc;
use relay::delivery::{SendError, SendRequest, Sender};
use relay::domain::{EndpointUrl, TenantId};
use relay::events::Event;
use relay::outbound::{Client, HttpSender, Outcome, Policy, Request};
use serde_json::json;
use target::{Reply, Target};
use uuid::Uuid;

/// A URL on `target`, as the client's own type.
fn url(target: &Target, path: &str) -> EndpointUrl {
    target
        .url(path)
        .parse()
        .expect("the fixture builds a URL this service accepts")
}

/// A policy that leaves everything at its default except the deadlines a test is about.
fn policy(read: Duration, deadline: Duration) -> Policy {
    Policy {
        read,
        deadline,
        ..Policy::default()
    }
}

/// What one answer to one status means, in the three words `Outcome` has.
fn meaning(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Success(_) => "success",
        Outcome::Permanent(_) => "permanent",
        Outcome::Retryable(_) => "retryable",
    }
}

/// A target that accepts the connection and then says nothing is cut at the read deadline.
#[tokio::test]
async fn a_target_that_never_answers_is_cut_at_the_read_deadline() {
    let target = Target::start(vec![Reply::silent()]);
    let client = Client::new(policy(Duration::from_millis(300), Duration::from_secs(30)));

    let started = Instant::now();
    let outcome = client.attempt(&Request::post(url(&target, "/hook"))).await;
    let took = started.elapsed();

    assert_eq!(
        meaning(&outcome),
        "retryable",
        "a subscriber that takes the connection and never answers is a failed attempt, not a \
         success and not a permanent refusal: we do not know what it thinks: {outcome:?}"
    );
    assert!(
        took < Duration::from_secs(5),
        "the read deadline is 300ms and the attempt deadline is 30s, and the attempt ended after \
         {took:?}: the *read* is not being cut. One silent subscriber then holds a worker (and its \
         database connection to that claim's row) for the whole attempt deadline, and a handful of \
         them hold every worker"
    );
    assert_eq!(
        target.requests().len(),
        1,
        "the request did arrive: the target is silent, not closed"
    );
}

/// The attempt deadline bounds the chain, not just one hop.
#[tokio::test]
async fn the_attempt_deadline_covers_a_chain_of_slow_hops() {
    let target = Target::start(vec![Reply::redirect("/next").slow(1500)]);
    // The read deadline is ten seconds and each hop answers in 1.5, so the read is never the thing
    // that cuts. The attempt deadline is half a second.
    let client = Client::new(policy(Duration::from_secs(10), Duration::from_millis(500)));

    let started = Instant::now();
    let outcome = client.attempt(&Request::post(url(&target, "/start"))).await;
    let took = started.elapsed();

    assert_eq!(
        meaning(&outcome),
        "retryable",
        "an attempt cut by its own deadline failed for a reason that may pass: {outcome:?}"
    );
    assert!(
        took < Duration::from_millis(1200),
        "the attempt deadline is 500ms and the hop answers after 1500ms, and the attempt ended \
         after {took:?}: the deadline has to cover the *attempt* — the connect, every read, and \
         every redirect — or a chain of slow hops has no bound at all, and the read deadline (10s \
         here) is what a worker ends up waiting for"
    );
}

/// A redirect chain is followed, and a `303` means `GET` at the next hop.
#[tokio::test]
async fn a_redirect_chain_is_followed_and_a_303_means_get() {
    let target = Target::start(vec![
        Reply::redirect("/one"),
        Reply::status(303).header("location", "/two"),
        Reply::ok("the end of the chain"),
    ]);
    let client = Client::new(Policy::default());

    let outcome = client.attempt(&Request::post(url(&target, "/start"))).await;
    match &outcome {
        Outcome::Success(response) => assert_eq!(
            String::from_utf8_lossy(&response.body),
            "the end of the chain",
            "the answer that counts is the one at the end of the chain"
        ),
        other => panic!("a chain that ends in a 200 is a success: {other:?}"),
    }

    let seen = target.requests();
    let paths: Vec<&str> = seen.iter().map(|request| request.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["/start", "/one", "/two"],
        "the chain was walked in order, from the URL that was asked for to the one it ended at"
    );
    let methods: Vec<&str> = seen.iter().map(|request| request.method.as_str()).collect();
    assert_eq!(
        methods,
        vec!["POST", "POST", "GET"],
        "a `303 See Other` says `the answer is over there`, and the method it means is GET: \
         forwarding a POST body to wherever a redirect points hands a payload to a target that \
         never asked for it — which is why 303 exists at all"
    );
    assert_eq!(
        target.connections(),
        1,
        "and a chain inside one target is one connection, not one per hop"
    );
    assert_eq!(
        seen[2].body, "",
        "the GET carries no body: the payload belongs to the request that was redirected, not to \
         the thing it was redirected to"
    );
}

/// A redirect that points back at a URL the attempt has already visited is refused.
#[tokio::test]
async fn a_redirect_that_points_at_itself_is_refused() {
    let target = Target::start(vec![Reply::redirect("/loop")]);
    let client = Client::new(Policy::default());

    let outcome = client.attempt(&Request::post(url(&target, "/loop"))).await;
    assert_eq!(
        meaning(&outcome),
        "permanent",
        "a target that answers every request with `go back to where you came from` is not going to \
         stop doing it: following it until the hop limit is a budget spent on a target that cannot \
         take the delivery, and retrying it spends the budget again: {outcome:?}"
    );
    assert!(
        target.requests().len() <= 3,
        "the client stopped instead of walking the loop: {} requests arrived",
        target.requests().len()
    );
}

/// A chain longer than the policy allows is cut, at the limit and not at some other number.
#[tokio::test]
async fn a_chain_longer_than_the_policy_is_cut() {
    let target = Target::start(vec![
        Reply::redirect("/a"),
        Reply::redirect("/b"),
        Reply::redirect("/c"),
        Reply::redirect("/d"),
        Reply::ok("never reached"),
    ]);
    let client = Client::new(Policy {
        redirects: 2,
        ..Policy::default()
    });

    let outcome = client.attempt(&Request::post(url(&target, "/start"))).await;
    assert_eq!(
        meaning(&outcome),
        "permanent",
        "a target that redirects more than the policy allows is refused: an unbounded chain is how \
         one delivery becomes a walk across somebody's load balancer: {outcome:?}"
    );
    assert_eq!(
        target.requests().len(),
        3,
        "the request plus exactly `redirects` hops. Fewer means the limit is being enforced \
         somewhere other than where the policy says, and more means the limit is not enforced at \
         all: {} requests arrived",
        target.requests().len()
    );
}

/// The status decides whether another attempt is worth it.
#[tokio::test]
async fn the_status_decides_whether_another_attempt_is_worth_it() {
    let cases = [
        (200, "success", "the subscriber took it"),
        (201, "success", "it took it and made something"),
        (204, "success", "it took it and has nothing to say"),
        (
            400,
            "permanent",
            "this request will not be accepted, however often it is repeated",
        ),
        (
            404,
            "permanent",
            "there is nothing there: another attempt finds the same nothing",
        ),
        (
            410,
            "permanent",
            "the subscriber is saying `never send this again`",
        ),
        (
            408,
            "retryable",
            "the subscriber timed out reading us: it is asking for another attempt",
        ),
        (
            429,
            "retryable",
            "the subscriber is asking us to come back later",
        ),
        (
            500,
            "retryable",
            "the subscriber broke while handling it, and may not be broken next time",
        ),
        (503, "retryable", "the subscriber is not ready"),
        (
            302,
            "permanent",
            "a redirect that does not say where to go: there is no next hop to take",
        ),
    ];

    for (status, expected, why) in cases {
        let target = Target::start(vec![Reply::status(status)]);
        let client = Client::new(Policy::default());
        let outcome = client.attempt(&Request::post(url(&target, "/hook"))).await;
        assert_eq!(
            meaning(&outcome),
            expected,
            "a {status} is {expected} ({why}). The worker acts on this word and nothing else, so a \
             client that answers `retryable` for everything spends on a permanent failure the \
             budget the next delivery needs, and one that answers `permanent` for everything \
             abandons subscribers that were only busy: {outcome:?}"
        );
    }
}

/// Connections are reused — until an answer says not to.
#[tokio::test]
async fn a_connection_is_reused_until_the_answer_closes_it() {
    let target = Target::start(vec![Reply::ok("one"), Reply::ok("two"), Reply::ok("three")]);
    let client = Client::new(Policy::default());
    for path in ["/one", "/two", "/three"] {
        let outcome = client.attempt(&Request::post(url(&target, path))).await;
        assert!(
            matches!(outcome, Outcome::Success(_)),
            "three ordinary requests all succeed: {outcome:?}"
        );
    }
    assert_eq!(
        target.connections(),
        1,
        "three requests to one target travelled on one connection. A worker that opens a TCP \
         connection — and, in a deployment that has one, pays for a TLS handshake — per delivery \
         spends more of its life connecting than delivering, and a subscriber sees a burst of new \
         connections instead of a stream of events"
    );

    let closing = Target::start(vec![
        Reply::ok("one").header("connection", "close"),
        Reply::ok("two").header("connection", "close"),
    ]);
    let client = Client::new(Policy::default());
    for path in ["/one", "/two"] {
        let outcome = client.attempt(&Request::post(url(&closing, path))).await;
        assert!(matches!(outcome, Outcome::Success(_)), "{outcome:?}");
    }
    assert_eq!(
        closing.connections(),
        2,
        "a subscriber that answers `connection: close` is closing the connection, and the client \
         has to take it at its word: sending the next delivery down it fails, and the delivery is \
         retried for a reason that had nothing to do with the delivery"
    );
}

/// A body ends where the answer says it ends: a length, or the close of the connection.
#[tokio::test]
async fn a_body_ends_where_the_answer_says_it_ends() {
    let target = Target::start(vec![
        Reply::ok("the body, measured only by the close").without_length(),
    ]);
    let outcome = Client::new(Policy::default())
        .attempt(&Request::post(url(&target, "/hook")))
        .await;
    match &outcome {
        Outcome::Success(response) => assert_eq!(
            String::from_utf8_lossy(&response.body),
            "the body, measured only by the close",
            "an answer with no `content-length` that closes the connection is complete: reading \
             only `content-length` bodies loses this one, and a subscriber that sends it is \
             delivered to and never recorded"
        ),
        other => panic!("that answer is a success: {other:?}"),
    }

    // `content-length` is a promise. Five bytes against a promise of a hundred is not a body of
    // five: it is an answer that never finished.
    let lying = Target::start(vec![Reply::ok("short").header("content-length", "100")]);
    let client = Client::new(policy(Duration::from_millis(300), Duration::from_secs(30)));
    let outcome = client.attempt(&Request::post(url(&lying, "/hook"))).await;
    assert_eq!(
        meaning(&outcome),
        "retryable",
        "an answer that stops five bytes into a hundred is unfinished, and unfinished is worth \
         another attempt — treating it as a short body delivers half a payload and calls it \
         delivered: {outcome:?}"
    );
}

/// Hop-by-hop headers belong to the connection, and the client owns the connection.
#[tokio::test]
async fn hop_by_hop_headers_are_the_clients_business() {
    let target = Target::start(vec![Reply::ok("taken")]);
    let client = Client::new(Policy::default());
    let request = Request::post(url(&target, "/hook"))
        .header("connection", "close")
        .header("transfer-encoding", "chunked")
        .header("keep-alive", "timeout=5")
        .header("te", "trailers")
        .header("upgrade", "h2c")
        .header("x-relay-delivery", "abc");
    let outcome = client.attempt(&request).await;
    assert!(
        matches!(outcome, Outcome::Success(_)),
        "the request is sent: {outcome:?}"
    );

    let seen = target.requests();
    let first = seen.first().expect("the target received the request");
    for name in ["transfer-encoding", "keep-alive", "te", "upgrade"] {
        assert!(
            first.header(name).is_none(),
            "`{name}` reached the wire. A hop-by-hop header describes *this* connection: copying \
             the caller's value into the next hop is a claim about a connection that no longer \
             exists, and `transfer-encoding` in particular tells the target to read a body framing \
             this client does not write. Headers: {:?}",
            first.headers
        );
    }
    assert_ne!(
        first.header("connection"),
        Some("close"),
        "the caller does not get to close a connection this client means to keep: reusing it after \
         forwarding `connection: close` is how a keep-alive pool becomes a pool of dead sockets"
    );
    assert_eq!(
        first.header("x-relay-delivery"),
        Some("abc"),
        "everything that is not the connection's business is forwarded as it was written"
    );
    assert_eq!(
        first.header("content-length"),
        Some("0"),
        "and the length of the body is the client's, because the client wrote the body"
    );
}

/// The port `relay/2.6-at-least-once` asked for: a delivery attempt, over HTTP.
#[tokio::test]
async fn the_delivery_port_is_this_client() {
    let target = Target::start(vec![
        Reply::ok("taken"),
        Reply::status(503).body("busy"),
        Reply::status(400).body("no"),
    ]);
    let sender = HttpSender::new(Policy::default());
    let delivery = Uuid::new_v4();
    let event = Event {
        id: Uuid::new_v4(),
        tenant_id: TenantId::new(),
        kind: "invoice.paid".to_owned(),
        payload: json!({ "n": 1 }),
        received_at: Utc::now(),
    };
    let request = SendRequest {
        delivery,
        event: event.clone(),
        attempt: 2,
        url: url(&target, "/hook"),
    };

    let receipt = sender
        .send(request.clone())
        .await
        .expect("a 200 is a receipt with the status in it");
    assert_eq!(
        receipt.status, 200,
        "the receipt says what the subscriber answered, so a worker (and a metric) can tell a 202 \
         from a 200"
    );
    assert!(
        matches!(
            sender.send(request.clone()).await,
            Err(SendError::Retryable(_))
        ),
        "a 503 reaches the worker as retryable, which is the only word it acts on"
    );
    assert!(
        matches!(
            sender.send(request.clone()).await,
            Err(SendError::Permanent(_))
        ),
        "and a 400 reaches it as permanent"
    );

    let seen = target.requests();
    assert_eq!(seen.len(), 3, "three attempts, three requests");
    let first = seen.first().expect("the target received the request");
    assert_eq!(first.method, "POST", "a webhook is a POST");
    assert_eq!(first.path, "/hook", "to the path the endpoint named");
    assert_eq!(
        first.header("content-type"),
        Some("application/json"),
        "and the body is JSON, which the subscriber has to be told"
    );
    let id = delivery.to_string();
    assert_eq!(
        first.header("x-relay-delivery"),
        Some(id.as_str()),
        "every attempt of one delivery carries the same delivery id. That header is the receiver's \
         half of the at-least-once contract: without it, a duplicate and a new event look identical"
    );
    let body: serde_json::Value =
        serde_json::from_str(&first.body).expect("the body is the event, as JSON");
    assert_eq!(
        body["id"],
        json!(event.id.to_string()),
        "the body carries the event's identity, which is what a subscriber keys its work on: {body}"
    );
    assert_eq!(body["kind"], json!("invoice.paid"), "{body}");
    assert_eq!(
        body["payload"],
        json!({ "n": 1 }),
        "and the payload the tenant sent, unchanged: {body}"
    );
}
