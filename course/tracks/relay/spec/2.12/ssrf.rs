//! Spec for `relay/2.12-ssrf-defense`: only deliver where we are allowed to.
//!
//! `kiln run` copies this file into the project as `tests/spec_ssrf.rs`; it is the course's file,
//! not the learner's (plan §3 D2). The step is judged from three sides, because the three answer
//! different questions and none of them can see the other two:
//!
//! * **the registration boundary**, against a running service: a target no delivery may ever use is
//!   refused where it enters the system, with a status and a token a client can switch on — rather
//!   than accepted, delivered to (nowhere) and buried later;
//! * **the decision itself**, as a matrix over URLs and addresses: which rule each one broke, in the
//!   refusal's own words. A check that answers "no" for everything passes the first half of this
//!   file and fails the second;
//! * **the delivery path**, against a scripted target on `127.0.0.1` and a *name service this test
//!   drives*. That is the only way to judge the property the step exists for: the address that is
//!   checked is the address that is connected to, so a name that answers one thing when it is
//!   checked and another when it is dialled is never dialled twice.
//!
//! ## Reaching a target on `127.0.0.1`
//!
//! A spec may not dial the network, so every subscriber here is either a scripted target on the
//! loopback interface or a name with no address at all. The service's default configuration refuses
//! loopback — that is what the first test is about — so the tests that *do* dial a local target say
//! so, and they say it in one of the two ways the step provides: a `target::Rules` that allows
//! loopback and nothing else, or the setting a deployment uses when its subscribers are on a
//! network only it can reach (`RELAY__DELIVERY__ALLOW_PRIVATE_TARGETS=true`). Nothing here needs the
//! network to be up: an unresolved name is a *retryable* failure, and no assertion below treats it
//! as a refusal.

mod support;

#[path = "support/target.rs"]
mod target;

use std::collections::VecDeque;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use relay::config::{Config, Flags};
use relay::delivery::{SendError, SendRequest, Sender};
use relay::domain::{EndpointUrl, TenantId};
use relay::events::Event;
use relay::outbound::{Client, HttpSender, Outcome, Policy, Request};
use relay::target::{Class, Refusal, Resolver, Rules};
use serde_json::json;
use support::app::{self, App};
use support::client::Response;
use support::postgres::Postgres;
use support::var;
use target::{Reply, Target};
use uuid::Uuid;

/// The tenant these tests seed, and the key it authenticates with.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// The setting a deployment uses when its subscribers live on a network only it can reach.
const ALLOW_PRIVATE: &str = "RELAY__DELIVERY__ALLOW_PRIVATE_TARGETS";
/// A target on the loopback interface: the one class of address a test may really dial.
const LOCAL: &str = "http://127.0.0.1:9/hook";

/// A URL this service accepts, or a panic naming it.
fn url(text: &str) -> EndpointUrl {
    text.parse()
        .unwrap_or_else(|error| panic!("`{text}` has to be a URL this service parses: {error}"))
}

/// An address, for the matrices below.
fn at(text: &str) -> IpAddr {
    text.parse()
        .unwrap_or_else(|error| panic!("`{text}` has to be an address: {error}"))
}

/// Rules that allow the loopback interface and nothing else: what a test that dials a local target
/// needs, and a narrower policy than the service's own.
fn loopback_only() -> Rules {
    Rules {
        allow_loopback: true,
        ..Rules::public_only()
    }
}

/// The policy these tests deliver with: the rules they name, and a connect deadline short enough
/// that a mistake is a failure rather than a wait.
fn policy(rules: Rules) -> Policy {
    Policy {
        targets: rules,
        connect: Duration::from_secs(5),
        ..Policy::default()
    }
}

/// What one answer to one attempt means, in the words `Outcome` has.
fn meaning(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Success(_) => "success",
        Outcome::Permanent(_) => "permanent",
        Outcome::Retryable(_) => "retryable",
    }
}

/// The sentence a permanent answer carries: what the refusal said, in its own words.
fn refused(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Permanent(why) => why,
        other => panic!("a target we may not reach is refused for good, not `{other:?}`"),
    }
}

/// One delivery to `url`, as the port hands it to a sender.
fn delivery(to: &EndpointUrl) -> SendRequest {
    SendRequest {
        delivery: Uuid::new_v4(),
        event: Event {
            id: Uuid::new_v4(),
            tenant_id: TenantId::new(),
            kind: "invoice.paid".to_owned(),
            payload: json!({ "n": 1 }),
            received_at: Utc::now(),
        },
        attempt: 1,
        url: to.clone(),
    }
}

/// A name service that answers one scripted answer per call, and remembers every question.
///
/// The last answer repeats, so a test that gives one answer says "this name means that, always".
/// Injecting it is the point of the step: a real name service cannot be asked to answer one thing
/// for the check and another for the connection.
#[derive(Debug)]
struct Scripted {
    /// One answer per call, in order; the last one repeats.
    answers: Mutex<VecDeque<Vec<SocketAddr>>>,
    /// Every question that was asked, as `(host, port)`.
    asked: Mutex<Vec<(String, u16)>>,
}

impl Scripted {
    /// A name service that answers `answers`, one per call.
    fn new(answers: Vec<Vec<SocketAddr>>) -> Self {
        Self {
            answers: Mutex::new(answers.into()),
            asked: Mutex::new(Vec::new()),
        }
    }

    /// A name that always means `address`.
    fn meaning(address: SocketAddr) -> Arc<Self> {
        Arc::new(Self::new(vec![vec![address]]))
    }

    /// The questions that were asked, in order.
    fn asked(&self) -> Vec<(String, u16)> {
        self.asked
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl Resolver for Scripted {
    fn resolve(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<SocketAddr>, String>> + Send + '_>> {
        let host = host.to_owned();
        Box::pin(async move {
            self.asked
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((host, port));
            let mut answers = self
                .answers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let answer = if answers.len() > 1 {
                answers.pop_front().unwrap_or_default()
            } else {
                answers.front().cloned().unwrap_or_default()
            };
            Ok(answer)
        })
    }
}

/// A running service over a database of its own, started with the settings a test names.
fn service(settings: &[(&str, &str)]) -> (App, Postgres) {
    let postgres = Postgres::start();
    let mut env = vec![
        var(app::DATABASE_URL, postgres.url()),
        var(app::BOOTSTRAP_TENANTS, format!("{}={}", ACME.0, ACME.1)),
    ];
    for (name, value) in settings {
        env.push(var(name, *value));
    }
    let app = App::start(app::config(&env));
    // The fixture's readiness check is the *port*, and the kernel answers a connect before the
    // service has migrated or seeded anything. `readyz` answers only once the state exists, and
    // registering an endpoint needs the tenant this configuration seeded.
    let ready = app.client().get("/readyz");
    assert_eq!(
        ready.status, 200,
        "the service has to be serving before this file registers anything: {}",
        ready.body
    );
    (app, postgres)
}

/// Register a target, and answer what the service said about it.
fn register(client: &support::client::Client, target: &str) -> Response {
    client.post_json("/endpoints", ACME.1, &json!({ "url": target }))
}

/// The registration boundary refuses a target no delivery may use, and says why.
#[test]
fn the_registration_boundary_refuses_what_no_delivery_may_reach() {
    let (app, _database) = service(&[]);
    let client = app.client();

    // Every one of these is a target this service must never dial, and each is refused for its own
    // reason: an address of ours, an address of the deployment's network, the endpoint that hands
    // out cloud credentials, and two names that can only mean "here".
    let refused: [(&str, &str); 7] = [
        (LOCAL, "127.0.0.1"),
        ("http://[::1]:9/hook", "::1"),
        ("http://10.0.0.5/hook", "10.0.0.5"),
        (
            "http://169.254.169.254/latest/meta-data/",
            "169.254.169.254",
        ),
        ("http://localhost:9/hook", "localhost"),
        ("http://printer.local/hook", "printer.local"),
        ("http://someone:secret@example.test/hook", "credentials"),
    ];
    for (target, named) in refused {
        let answer = register(&client, target);
        assert_eq!(
            answer.status, 422,
            "`{target}` is not a target this service delivers to, and a URL no delivery can use is \
             refused where it enters the system — accepting it and burying the delivery later tells \
             the tenant nothing: {}",
            answer.body
        );
        let body = answer.json();
        assert_eq!(
            body.get("error"),
            Some(&json!("target_rejected")),
            "a caller switches on a stable token, not on a sentence: {body}"
        );
        let message = body
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or_default();
        assert!(
            message.contains(named),
            "the refusal has to name what it refused (`{named}`): {message}"
        );
    }

    // A name is not an address: where this one points is decided when it is resolved, at delivery
    // time, and a registration that claimed to know would be judging a name it never looked up.
    let accepted = register(&client, "http://subscriber.example.test/hook");
    assert_eq!(
        accepted.status, 201,
        "a target the service cannot judge yet is accepted, and judged when it resolves one: {}",
        accepted.body
    );

    // The same loopback target, for a deployment that says its subscribers are on a network only it
    // can reach. This is also how this step's own tests — and every earlier step's fixtures — point
    // a worker at a scripted server on `127.0.0.1`: the policy is configuration, so a test can set
    // it, and nothing about the library's own defaults has to be loosened for that.
    let (permissive, _database) = service(&[(ALLOW_PRIVATE, "true")]);
    let allowed = register(&permissive.client(), LOCAL);
    assert_eq!(
        allowed.status, 201,
        "with `{ALLOW_PRIVATE}=true` a local target is a subscriber: {}",
        allowed.body
    );
}

/// A URL whose host is a name that can only mean "here" is refused, and the name is named.
#[test]
fn a_url_that_says_here_is_refused_by_name() {
    let rules = Rules::public_only();
    let refused: [(&str, &str); 5] = [
        ("http://localhost:9/hook", "localhost"),
        ("http://LOCALHOST:9/hook", "localhost"),
        ("http://localhost.:9/hook", "localhost"),
        ("http://hooks.localhost/hook", "hooks.localhost"),
        ("http://thing.internal/hook", "thing.internal"),
    ];
    for (target, named) in refused {
        let refusal = relay::target::check(&url(target), &rules)
            .expect_err("a name that means this machine is not a subscriber");
        assert_eq!(
            refusal,
            Refusal::Name(named.to_owned()),
            "`{target}` is refused as a name, not as an address: nothing here resolves what it means"
        );
        assert!(
            refusal.to_string().contains(named),
            "and the sentence names it: {refusal}"
        );
    }
}

/// An authority that is not a host and a port is refused, including the two shapes that hide one.
#[test]
fn an_authority_that_is_not_a_host_and_a_port_is_refused() {
    let rules = Rules::permissive();

    // Credentials in the authority: everything before the `@` is a user name, so a check that reads
    // the left-hand side of the first `@` is reading the part an attacker chose.
    let refusal = relay::target::check(&url("http://example.test@127.0.0.1/hook"), &rules)
        .expect_err("an authority with credentials is not a target");
    assert!(
        matches!(refusal, Refusal::Credentials(_)),
        "credentials in a URL are their own refusal: {refusal}"
    );

    // A port that is not a number is a URL no delivery can be made to.
    let refusal = relay::target::check(&url("http://example.test:not-a-port/hook"), &rules)
        .expect_err("a port that is not a number is not a target");
    assert_eq!(refusal.kind(), "authority", "{refusal}");

    // An IPv6 literal is bracketed, and the brackets are what keep the colons inside the address
    // from being read as a port. With permissive rules the literal itself is fine, so what this
    // asserts is that it was *understood*: the host is the address, and the port is the one after
    // the bracket.
    let host = relay::target::check(&url("http://[2001:4860:4860::8888]:8443/hook"), &rules)
        .expect("a public IPv6 literal is a target");
    assert_eq!(host.literal, Some(at("2001:4860:4860::8888")), "{host:?}");
    assert_eq!(
        host.port, 8443,
        "the port is the one after the bracket: {host:?}"
    );

    // And the same shape with an address the loopback-only rules refuse: a check that split on the
    // first colon would read the host as `[` and let this through.
    let refusal = relay::target::check(&url("http://[::1]:9/hook"), &Rules::public_only())
        .expect_err("a bracketed loopback literal is still loopback");
    assert_eq!(refusal.kind(), "address", "{refusal}");
}

/// Every class of address this service refuses, in IPv4 and in the spellings of IPv6.
#[test]
fn an_address_no_delivery_may_use_is_refused_and_named() {
    let rules = Rules::public_only();
    let refused: [(&str, Class); 12] = [
        ("127.0.0.1", Class::Loopback),
        ("127.1.2.3", Class::Loopback),
        ("10.0.0.5", Class::Private),
        ("172.20.3.4", Class::Private),
        ("192.168.1.10", Class::Private),
        ("100.64.0.1", Class::Private),
        ("0.0.0.0", Class::Private),
        ("169.254.1.1", Class::LinkLocal),
        ("169.254.169.254", Class::Metadata),
        ("::1", Class::Loopback),
        ("fe80::1", Class::LinkLocal),
        ("fd00:ec2::254", Class::Metadata),
    ];
    for (text, class) in refused {
        let address = at(text);
        assert_eq!(
            Class::of(address),
            class,
            "`{text}` is classified as `{class}`"
        );
        let refusal = rules
            .check(address)
            .expect_err("an address of this class is not one we deliver to");
        assert_eq!(refusal.kind(), "address", "{refusal}");
        assert!(
            refusal.to_string().contains(text) && refusal.to_string().contains(class.as_str()),
            "the refusal names the address and the rule it broke (`{text}`, `{class}`): {refusal}"
        );
    }

    // The whole point of the `::ffff:` spelling: an IPv4 address written as IPv6 is the same
    // address, and a check that only knows the text sees neither the loopback nor the metadata
    // endpoint underneath it.
    for (text, class) in [
        ("::ffff:127.0.0.1", Class::Loopback),
        ("::ffff:10.0.0.5", Class::Private),
        ("::ffff:169.254.169.254", Class::Metadata),
    ] {
        assert_eq!(
            Class::of(at(text)),
            class,
            "`{text}` is the IPv4 address it wraps, and `{class}` is what it is"
        );
        assert!(
            rules.check(at(text)).is_err(),
            "`{text}` is refused, like the address it is"
        );
    }

    // And the addresses a delivery *is* for: none of these is refused.
    for text in [
        "93.184.216.34",
        "8.8.8.8",
        "172.32.0.1",
        "192.0.1.1",
        "2001:4860:4860::8888",
        "2606:4700:4700::1111",
    ] {
        assert_eq!(
            Class::of(at(text)),
            Class::Public,
            "`{text}` is on the public internet"
        );
        assert!(
            rules.check(at(text)).is_ok(),
            "`{text}` is a target a delivery may go to: a check that refuses everything is not a \
             check"
        );
    }
}

/// The address that is checked is the address that is connected to, once.
#[tokio::test]
async fn the_address_that_is_checked_is_the_address_that_is_used() {
    let target = Target::start(vec![Reply::ok("taken")]);
    // The name always means the scripted target, and the rules allow loopback so that this test can
    // read what arrived.
    let resolver = Scripted::meaning(target.address());
    let client = Client::resolving(policy(loopback_only()), resolver.clone());
    let port = target.address().port();
    let hook = url(&format!("http://subscriber.example.test:{port}/hook"));

    let outcome = client.attempt(&Request::post(hook)).await;
    assert!(
        matches!(outcome, Outcome::Success(_)),
        "a name that means an address these rules allow is delivered to: {outcome:?}"
    );
    assert_eq!(
        target.requests().len(),
        1,
        "and the request arrived, at the address the name answered with"
    );
    assert_eq!(
        resolver.asked(),
        vec![("subscriber.example.test".to_owned(), port)],
        "the name was resolved exactly once. A client that resolves to check and then hands the \
         name to the operating system to connect has asked twice, and the second answer is the one \
         an attacker gets to choose"
    );
}

/// A name that answers with a public address and then with a private one is never asked twice.
#[tokio::test]
async fn a_name_that_changes_its_answer_is_not_asked_twice() {
    let target = Target::start(vec![Reply::ok("taken")]);
    // The rebinding attack, written as a fixture: the first answer is the ordinary one, and the
    // second — the one an attacker waits to give, after the check has passed — points into the
    // deployment's own network.
    let resolver = Arc::new(Scripted::new(vec![
        vec![target.address()],
        vec![SocketAddr::new(at("10.0.0.5"), 80)],
    ]));
    let client = Client::resolving(policy(loopback_only()), resolver.clone());
    let hook = url(&format!(
        "http://subscriber.example.test:{}/hook",
        target.address().port()
    ));

    let outcome = client.attempt(&Request::post(hook)).await;
    assert!(
        matches!(outcome, Outcome::Success(_)),
        "the first answer is an address these rules allow, so the attempt is made: {outcome:?}"
    );
    assert_eq!(
        resolver.asked().len(),
        1,
        "there is no second question, so there is no second answer to be redirected to: the \
         address that was checked is the address the socket went to"
    );
    assert_eq!(
        target.requests().len(),
        1,
        "and it went there, not where the second answer pointed"
    );
}

/// A name that answers with somewhere we may not go is refused, and refused for good.
#[tokio::test]
async fn a_name_that_points_somewhere_we_may_not_go_is_refused() {
    // The second row is the one nobody writes by hand: a name that answers with a public address
    // *and* an internal one. A delivery that picks the address it likes is a delivery that can be
    // told to pick the internal one, so any address that fails, fails the target.
    let rows: [(&str, &[&str], &str); 2] = [
        ("a private network", &["10.0.0.5"], "private"),
        (
            "the metadata endpoint, beside a public address",
            &["93.184.216.34", "169.254.169.254"],
            "metadata",
        ),
    ];
    for (described, answer, class) in rows {
        let addresses: Vec<SocketAddr> = answer
            .iter()
            .map(|text| SocketAddr::new(at(text), 80))
            .collect();
        let resolver = Arc::new(Scripted::new(vec![addresses]));
        let client = Client::resolving(policy(Rules::public_only()), resolver.clone());
        let outcome = client
            .attempt(&Request::post(url("http://subscriber.example.test/hook")))
            .await;

        assert_eq!(
            meaning(&outcome),
            "permanent",
            "a name that means {described} is not a subscriber, and no attempt will change that: \
             {outcome:?}"
        );
        let why = refused(&outcome);
        assert!(
            answer.iter().any(|text| why.contains(text)),
            "the refusal names the address it refused ({answer:?}): {why}"
        );
        assert!(why.contains(class), "and the class (`{class}`): {why}");
        assert_eq!(
            resolver.asked().len(),
            1,
            "resolved once, judged once: {why}"
        );
    }
}

/// A redirect is checked again before it is followed, one hop at a time.
#[tokio::test]
async fn a_redirect_is_checked_again_before_it_is_followed() {
    // The first hop is on loopback and allowed. Where it points is neither.
    let rows: [(&str, &str, &str); 2] = [
        ("http://10.0.0.5/next", "10.0.0.5", "private"),
        (
            "http://169.254.169.254/latest/meta-data/iam/security-credentials/",
            "169.254.169.254",
            "metadata",
        ),
    ];
    for (location, address, class) in rows {
        let target = Target::start(vec![Reply::redirect(location)]);
        let client = Client::new(policy(loopback_only()));
        let outcome = client
            .attempt(&Request::post(url(&target.url("/hook"))))
            .await;

        assert_eq!(
            meaning(&outcome),
            "permanent",
            "a subscriber that redirects us into its own network is not going to stop doing it, and \
             following it would put the deployment's internals behind a URL a tenant controls: \
             {outcome:?}"
        );
        let why = refused(&outcome);
        assert!(
            why.contains(address) && why.contains(class),
            "the refusal names the hop it refused (`{address}`, `{class}`): {why}"
        );
        assert_eq!(
            target.requests().len(),
            1,
            "the loopback hop arrived and the second hop never did: {why}"
        );
    }
}

/// The sender this service delivers with refuses what the configuration forbids, and nothing leaves
/// the process.
#[tokio::test]
async fn the_sender_the_service_delivers_with_refuses_what_the_configuration_forbids() {
    let target = Target::start(vec![Reply::ok("taken")]);
    let hook = url(&target.url("/hook"));

    // The configuration this service is started with when nobody configures anything. The policy
    // comes from it, which is what makes the refusal a deployment decision rather than a property
    // of the client: `Policy::default()` is the library's own value, and a library does not get to
    // decide who a deployment may talk to.
    let default = Config::load(&Flags::default(), &[]).expect("the defaults load");
    let sender = HttpSender::new(Policy::of(&default));
    match sender.send(delivery(&hook)).await {
        Err(SendError::Permanent(why)) => assert!(
            why.contains("127.0.0.1") && why.contains("loopback"),
            "the attempt is refused permanently, and the reason names the address and the rule: \
             {why}"
        ),
        other => panic!("a target the configuration forbids is a permanent failure: {other:?}"),
    }
    assert_eq!(
        target.requests().len(),
        0,
        "nothing was dialled: the check is what stops a request to the deployment's own machine, so \
         a refused target has to be refused before a socket is opened"
    );

    // The same target with the setting a deployment uses when its subscribers are on a network only
    // it can reach — which is how this file, and every earlier step's fixtures, reach a scripted
    // target on the loopback interface.
    let permissive = Config::load(
        &Flags::default(),
        &[(ALLOW_PRIVATE.to_owned(), "true".to_owned())],
    )
    .expect("the setting loads");
    let sender = HttpSender::new(Policy::of(&permissive));
    let receipt = sender
        .send(delivery(&hook))
        .await
        .expect("a target the configuration allows is dialled");
    assert_eq!(receipt.status, 200, "{receipt:?}");
    assert_eq!(
        target.requests().len(),
        1,
        "and it arrived: the same code, one setting apart"
    );
}
