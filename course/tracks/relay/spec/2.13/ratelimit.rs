//! Spec for `relay/2.13-rate-limit`: one bucket per caller, and a refusal that says when to return.
//!
//! `kiln run` copies this file into the project as `tests/spec_ratelimit.rs`; it is the course's
//! file, not the learner's (plan §3 D2). The limiter is judged from two sides, because the two
//! answer different questions:
//!
//! * **the arithmetic, against a clock the test owns.** A bucket is a function of elapsed time, and
//!   a test that waited for the refill would take minutes and be flaky at the edge of every second.
//!   `Clock` is injected, so the tests below spend a burst, move the clock, and read the answer —
//!   including the two properties only a clock that does *not* move can judge: what a bucket holds
//!   when nothing happened, and what concurrent callers get;
//! * **the wire, against a running service.** A quota is only a quota if the *service* enforces it
//!   where callers arrive, keys it by the caller, keeps one tenant out of another's bucket, and
//!   turns a refusal into a `429` with a `Retry-After` a client can act on. The first two tests do
//!   that against the real routing — including the ordering that makes an anonymous flood cheap:
//!   a request over quota is refused *before* the lookup that would have answered `401`.
//!
//! The two Postgres-backed tests are the only ones that need a database: a caller's identity is
//! seeded configuration, and a quota has to be enforced on the routes that do the work.

mod support;

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use relay::config::{Config, Flags};
use relay::ratelimit::{self, Clock, Limiter, Quota, Verdict};
use serde_json::json;
use support::app::{self, App};
use support::client::{Client, Response};
use support::postgres::Postgres;
use support::var;

/// The tenants these tests seed, and the keys they authenticate with.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
const GLOBEX: (&str, &str) = ("globex", "key-globex-91b7");

/// A clock a test moves by hand: the reason `Limiter` takes one at all.
#[derive(Debug)]
struct Manual {
    /// How far past the origin this clock has been moved.
    now: Mutex<Duration>,
}

impl Manual {
    /// A clock at its origin.
    fn started() -> Self {
        Self {
            now: Mutex::new(Duration::ZERO),
        }
    }

    /// Move it forward by `by`.
    fn advance(&self, by: Duration) {
        let mut now = self
            .now
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *now += by;
    }
}

impl Clock for Manual {
    fn now(&self) -> Duration {
        *self
            .now
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A limiter with this quota, over a clock the test owns.
fn limiter(burst: u32, per_second: u32) -> (Limiter, Arc<Manual>) {
    let clock = Arc::new(Manual::started());
    let quota = Quota::new(burst, per_second)
        .unwrap_or_else(|error| panic!("`burst = {burst}`, `per_second = {per_second}`: {error}"));
    (Limiter::new(quota, clock.clone()), clock)
}

/// The duration one refusal asks the caller to wait.
fn wait(verdict: Verdict) -> Duration {
    match verdict {
        Verdict::Refused { retry_after } => retry_after,
        Verdict::Allowed => panic!("this caller had nothing left to spend and was allowed anyway"),
    }
}

/// Settings, as the environment variable list `Config::load` takes.
fn env(settings: &[(&str, &str)]) -> Vec<(String, String)> {
    settings
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

/// Accept one event for `key`, under an idempotency key nothing else uses.
fn accept(client: &Client, key: &str, idempotency: &str) -> Response {
    client.send(
        support::client::Request::post("/events", Vec::new())
            .bearer(key)
            .header("idempotency-key", idempotency)
            .json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })),
    )
}

/// A service over a database of its own, with two tenants and a quota of the test's choosing.
fn service(burst: &str, per_second: &str) -> (App, Postgres) {
    let postgres = Postgres::start();
    let app = App::start(app::config(&[
        var(app::DATABASE_URL, postgres.url()),
        var(
            app::BOOTSTRAP_TENANTS,
            format!("{}={},{}={}", ACME.0, ACME.1, GLOBEX.0, GLOBEX.1),
        ),
        var("RELAY__RATE_LIMIT__BURST", burst),
        var("RELAY__RATE_LIMIT__PER_SECOND", per_second),
    ]));
    let ready = app.client().get("/readyz");
    assert_eq!(
        ready.status, 200,
        "the service has to be serving before this file asks it anything: {}",
        ready.body
    );
    (app, postgres)
}

/// One tenant's burst is nobody else's problem, and a refusal says when to come back.
#[test]
fn a_tenant_that_floods_does_not_spend_another_tenants_quota() {
    let (app, _database) = service("3", "1");
    let client = app.client();

    let mut refusals = 0;
    let mut first_refusal = None;
    for index in 0..8 {
        let answer = accept(&client, ACME.1, &format!("acme-{index}"));
        assert!(
            answer.status == 201 || answer.status == 429,
            "an event is either accepted or refused for being over quota: {} {}",
            answer.status,
            answer.body
        );
        if answer.status == 429 {
            refusals += 1;
            first_refusal.get_or_insert(answer);
        }
    }
    let refusal = first_refusal.unwrap_or_else(|| {
        panic!("a burst of 8 against a bucket of 3 was never refused: nothing is limiting here")
    });

    let after = refusal
        .header("retry-after")
        .unwrap_or_else(|| panic!("a 429 has to say when to come back: {}", refusal.body));
    let seconds: u64 = after.parse().unwrap_or_else(|_| {
        panic!("`Retry-After` is a number of seconds, and this one is `{after}`")
    });
    assert!(
        seconds >= 1,
        "and a positive one, so a client can wait exactly that long: `{after}`"
    );
    let body = refusal.json();
    assert_eq!(
        body.get("error"),
        Some(&json!("rate_limited")),
        "the refusal is the ordinary error shape, with a token a caller switches on: {body}"
    );
    assert_eq!(
        body.get("retryable"),
        Some(&json!(true)),
        "and it says the same request will succeed later: {body}"
    );

    // The other tenant has spent nothing. A shared bucket would have refused this request, and the
    // answer to "why am I being refused" would have been "somebody else was busy".
    let theirs = accept(&client, GLOBEX.1, "globex-0");
    assert_eq!(
        theirs.status, 201,
        "{refusals} of acme's requests were refused over quota and globex's first was not: {}",
        theirs.body
    );
}

/// A request with no credential has a bucket of its own, and it is spent before the lookup.
#[test]
fn a_request_with_no_key_has_a_bucket_of_its_own() {
    let (app, _database) = service("2", "1");
    let client = app.client();

    // No key at all: the shared bucket is spent and the request is refused for what it is.
    for attempt in 1..=2 {
        let answer = client.get("/endpoints");
        assert_eq!(
            answer.status, 401,
            "attempt {attempt} carries no key and is refused for that: {}",
            answer.body
        );
    }
    // The third one finds that bucket empty, and is refused *before* any lookup could have answered
    // 401. That ordering is the point: the refusal that costs nothing is what a flood gets.
    let over = client.get("/endpoints");
    assert_eq!(
        over.status, 429,
        "anonymous callers share one bucket, and it is empty: {}",
        over.body
    );

    // A tenant's credential is a different bucket, so the anonymous flood did not touch it.
    let theirs = client.get_with_key("/endpoints", ACME.1);
    assert_eq!(
        theirs.status, 200,
        "a tenant's bucket is not the anonymous one: {}",
        theirs.body
    );
}

/// A bucket refills with time, and never past its burst.
#[test]
fn the_bucket_refills_with_time_and_never_past_its_burst() {
    let (limiter, clock) = limiter(2, 1);

    assert_eq!(limiter.check("acme"), Verdict::Allowed);
    assert_eq!(limiter.check("acme"), Verdict::Allowed);
    let wait = wait(limiter.check("acme"));
    assert!(
        wait > Duration::from_millis(900) && wait <= Duration::from_secs(1),
        "an empty bucket at one request a second has one request again in a second, and the refusal \
         says how long that is: {wait:?}"
    );

    clock.advance(Duration::from_millis(500));
    assert_eq!(
        limiter.check("acme"),
        Verdict::Refused {
            retry_after: Duration::from_millis(500)
        },
        "half a second of refill is half a request, and half a request is not a request: rounding it \
         up is a limiter that goes faster than its own setting"
    );

    clock.advance(Duration::from_millis(500));
    assert_eq!(
        limiter.check("acme"),
        Verdict::Allowed,
        "the other half of the second arrived, and with it one request"
    );

    // An hour of idleness is one burst, not an hour of requests: a caller who was quiet does not come
    // back holding the right to everybody else's minute.
    clock.advance(Duration::from_secs(3600));
    assert_eq!(limiter.check("acme"), Verdict::Allowed);
    assert_eq!(limiter.check("acme"), Verdict::Allowed);
    assert!(
        matches!(limiter.check("acme"), Verdict::Refused { .. }),
        "the burst is the cap, however long the bucket sat idle"
    );
}

/// Which bucket a caller belongs to is decided by what they presented, and nothing shares it.
#[test]
fn one_caller_is_one_bucket() {
    let (limiter, _clock) = limiter(1, 1);
    let acme = ratelimit::bucket_of(Some(ACME.1));
    let globex = ratelimit::bucket_of(Some(GLOBEX.1));
    let nobody = ratelimit::bucket_of(None);

    assert_eq!(limiter.check(&acme), Verdict::Allowed);
    assert!(
        matches!(limiter.check(&acme), Verdict::Refused { .. }),
        "the same credential is the same bucket"
    );
    assert_eq!(
        limiter.check(&globex),
        Verdict::Allowed,
        "a different credential is a different bucket"
    );
    assert_eq!(
        limiter.check(&nobody),
        Verdict::Allowed,
        "and anonymous callers share one of their own"
    );
    assert_eq!(nobody, ratelimit::ANONYMOUS);

    // The bucket key is not the credential: this string lives in a map for the length of a run.
    assert_ne!(acme, ACME.1, "a key is hashed on the way in");
    assert!(
        acme.len() >= 32 && acme.chars().all(|character| character.is_ascii_hexdigit()),
        "and the hash is what the map holds, not the key: {acme}"
    );
}

/// Concurrent callers cannot talk a bucket into more requests than it holds.
#[test]
fn a_bucket_never_admits_more_than_it_holds() {
    let (limiter, _clock) = limiter(5, 1);
    let limiter = Arc::new(limiter);
    let allowed = Arc::new(AtomicUsize::new(0));

    // Eight threads, four attempts each, against a bucket of five and a clock that does not move:
    // the answer is five. A limiter whose check and whose decrement are not one step answers more.
    let mut threads = Vec::new();
    for _ in 0..8 {
        let limiter = Arc::clone(&limiter);
        let allowed = Arc::clone(&allowed);
        threads.push(std::thread::spawn(move || {
            for _ in 0..4 {
                if limiter.check("acme") == Verdict::Allowed {
                    allowed.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
    }
    for thread in threads {
        thread.join().unwrap_or_else(|_| panic!("no thread panics"));
    }
    assert_eq!(
        allowed.load(Ordering::SeqCst),
        5,
        "a burst of five is five requests, counted from eight threads at once"
    );
}

/// A bucket nobody is spending from is forgotten, so the limiter does not grow with invented keys.
#[test]
fn a_bucket_that_is_full_again_is_forgotten() {
    let (limiter, clock) = limiter(4, 1);
    for index in 0..200 {
        limiter.check(&format!("credential-{index}"));
    }
    assert_eq!(
        limiter.known(),
        200,
        "200 credentials have been seen, and each one has a bucket"
    );

    // Every one of those buckets is full again, which is the same state as no bucket at all.
    clock.advance(Duration::from_secs(60));
    limiter.check("credential-0");
    assert_eq!(
        limiter.known(),
        1,
        "the buckets that were full again are gone: one bucket per credential ever seen is a \
         limiter that grows with an attacker's imagination"
    );
}

/// The quota is configuration, and a quota that refuses everything for ever is refused first.
#[test]
fn the_quota_comes_from_the_configuration_and_zero_is_refused() {
    for (burst, per_second) in [(0, 1), (1, 0), (0, 0)] {
        assert!(
            Quota::new(burst, per_second).is_err(),
            "`burst = {burst}`, `per_second = {per_second}` is a bucket that refuses every request \
             for ever, and nobody who wrote it meant that"
        );
    }

    let configured = Config::load(
        &Flags::default(),
        &env(&[
            ("RELAY__RATE_LIMIT__BURST", "7"),
            ("RELAY__RATE_LIMIT__PER_SECOND", "3"),
        ]),
    )
    .expect("the settings load");
    let limiter = Limiter::new(configured.rate_limit, Arc::new(Manual::started()));
    assert_eq!(
        (limiter.quota().burst(), limiter.quota().per_second()),
        (7, 3),
        "the bucket is the one the configuration asked for"
    );

    assert!(
        Config::load(
            &Flags::default(),
            &env(&[("RELAY__RATE_LIMIT__BURST", "0")])
        )
        .is_err(),
        "a zero burst is refused where it is read, not quietly turned into one"
    );
    assert!(
        Config::load(
            &Flags::default(),
            &env(&[("RELAY__RATE_LIMIT__BURST", "lots")])
        )
        .is_err(),
        "and so is a number that is not a number"
    );

    // `--check-config` says what the bucket is, like every other setting: a deployment asks before it
    // starts the service, not after.
    let ran = Command::new(env!("CARGO_BIN_EXE_relay"))
        .arg("--check-config")
        .env_clear()
        .env("RELAY__RATE_LIMIT__BURST", "7")
        .output()
        .expect("the relay binary runs");
    let stdout = String::from_utf8_lossy(&ran.stdout).into_owned();
    assert!(
        ran.status.success(),
        "`--check-config` succeeds without a database: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(
        stdout.contains("rate_limit.burst = 7"),
        "the report names the quota, or a deployment cannot check it without starting anything: \
         {stdout}"
    );
}
