//! Spec for `relay/2.4-cursor-pagination`: walking a stream that grows while the caller reads it.
//!
//! `kiln run` copies this file into the project as `tests/spec_pagination.rs`; it is the course's
//! file, not the learner's (plan §3 D2). The stream is judged from outside the service (HTTP) and
//! from inside the table (`psql`), and the property that matters is one sentence:
//!
//! > a caller that follows the cursors from the first page to the last sees **every** event exactly
//! > once, newest first.
//!
//! The shapes that break it are the shapes this file writes, and they are written on purpose:
//!
//! * **several events share one instant.** `received_at` is not unique — a batch arrives in one
//!   transaction, two machines write in the same millisecond — and a position that is only an
//!   instant steps over every row that has it. This is the step's TRAP, and the test for it is
//!   named after the symptom: a second page that comes back empty;
//! * **the page size does not divide the stream.** A page of three over eight events, a page of one
//!   over four: the last page is the one that has no `next`, and a page that stops early for any
//!   other reason skips what it stopped before;
//! * **the filter and the cursor are the same stream.** `kind=` narrows the stream and the cursor
//!   continues *inside* it: a page of two filtered events is full even when unfiltered rows sit
//!   between them.
//!
//! `received_at` is written by `psql` rather than through the API in most of these tests: the point
//! is what happens when events *share* an instant, and requests arriving one after another through
//! HTTP would only tie by luck.
//!
//! The stream shapes come from a small deterministic generator rather than from `proptest`: the
//! course introduces property-based testing with `proptest` in T4·4.13 (`test/model-based@apply`),
//! and this file needs a *reproducible* set of shapes, not a framework.

mod support;

use std::collections::BTreeSet;

use serde_json::{Value, json};
use support::app::{self, App};
use support::client::{Request, Response};
use support::postgres::Postgres;
use support::var;

/// The tenant these tests seed, and the key it authenticates with.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// The page size a caller gets when it asks for nothing: the scaffold's `DEFAULT_LIMIT`.
const DEFAULT_LIMIT: usize = 25;
/// The largest page this service serves: the scaffold's `MAX_LIMIT`.
const MAX_LIMIT: i64 = 100;
/// Two instants, one second apart. Everything written at one of them shares it with everything else
/// written at that one, which is the case a cursor made of an instant alone gets wrong.
const NEWER: &str = "2026-01-01 00:00:02+00";
const OLDER: &str = "2026-01-01 00:00:01+00";

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
        ]));
        // The fixture's readiness check is the *port*, and the kernel answers a connect before the
        // service has finished starting: connecting to the database, migrating it and seeding the
        // configured tenants all happen first. Most of this file reads the tables with `psql`
        // rather than through the service, so it waits for an answer only the running service can
        // give — no sleeping, and a failure here says what was waited for.
        let serving = app.client().get("/healthz");
        assert_eq!(
            serving.status, 200,
            "the service has to be serving before this file asks its database anything: {}",
            serving.body
        );
        Self { app, postgres }
    }

    /// Write `count` events of one kind at one instant, straight into the table.
    fn write_at(&self, at: &str, kind: &str, count: usize) {
        if count == 0 {
            return;
        }
        let tenant = self
            .postgres
            .query("select id from tenants where name = 'acme'");
        self.postgres.execute(&format!(
            "insert into events (id, tenant_id, idempotency_key, kind, payload, received_at) \
             select gen_random_uuid(), '{tenant}'::uuid, gen_random_uuid()::text, '{kind}', \
                    '{{\"n\": 1}}'::jsonb, '{at}'::timestamptz \
             from generate_series(1, {count})",
        ));
    }

    /// The ids of the events a caller paging from the start has to see, in order.
    fn expected(&self, kind: Option<&str>) -> Vec<String> {
        let filter = match kind {
            Some(kind) => format!("where kind = '{kind}'"),
            None => String::new(),
        };
        self.postgres
            .query(&format!(
                "select id from events {filter} order by received_at desc, id desc"
            ))
            .lines()
            .map(|line| line.trim().to_owned())
            .collect()
    }

    /// One request for a page, as a caller writes it: `limit=2&kind=…`, or nothing at all.
    fn ask(&self, query: &str) -> Response {
        let path = if query.is_empty() {
            "/events".to_owned()
        } else {
            format!("/events?{query}")
        };
        self.app.client().get_with_key(&path, ACME.1)
    }
}

/// One answer to a page request: the events it holds, and where it continues from.
struct Page {
    ids: Vec<String>,
    next: Option<String>,
}

impl Page {
    /// Read an answer as a page, failing with the body when it is not one.
    fn of(answer: &Response) -> Self {
        assert_eq!(
            answer.status, 200,
            "a page request this API can serve answers with a page: {}",
            answer.body
        );
        let body = answer.json();
        let events = body
            .get("events")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("a page carries its events in an `events` array: {body}"));
        let ids = events
            .iter()
            .map(|event| {
                event
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("every event in a page carries its `id`: {event}"))
                    .to_owned()
            })
            .collect();
        let next = match body.get("next") {
            Some(Value::String(cursor)) => Some(cursor.clone()),
            Some(Value::Null) => None,
            other => panic!(
                "a page answers with `next`: the cursor that continues it, or `null` when the \
                 stream ends here (got {other:?}): {body}"
            ),
        };
        Self { ids, next }
    }
}

/// Follow the cursors from the first page to the last, and answer the ids in the order they came
/// back.
fn walk(fixture: &Fixture, query: &str) -> Vec<String> {
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(
            pages <= 64,
            "paging `{query}` did not end after {pages} pages: `next` either never becomes `null`, \
             or it keeps pointing at a row that is already behind the cursor"
        );
        let next_query = match &cursor {
            Some(cursor) if query.is_empty() => format!("cursor={cursor}"),
            Some(cursor) => format!("{query}&cursor={cursor}"),
            None => query.to_owned(),
        };
        let page = Page::of(&fixture.ask(&next_query));
        seen.extend(page.ids);
        match page.next {
            Some(next) => cursor = Some(next),
            None => return seen,
        }
    }
}

/// What a walked stream has to look like: every event once, in the order the stream is in.
fn assert_walked(what: &str, seen: &[String], expected: &[String]) {
    let unique: BTreeSet<&String> = seen.iter().collect();
    assert_eq!(
        unique.len(),
        seen.len(),
        "{what}: an event came back more than once, so two pages overlap: {seen:?}"
    );
    assert_eq!(
        seen.len(),
        expected.len(),
        "{what}: {} of {} events came back. A page that stops before the cursor is usually a \
         position that is only an instant: every row that shares that instant is skipped at \
         once: {seen:?}",
        seen.len(),
        expected.len()
    );
    assert_eq!(
        seen, expected,
        "{what}: the pages do not continue where the last one stopped (the order is newest \
         first): {seen:?}"
    );
}

/// A deterministic source of stream shapes and page sizes.
///
/// The same seed gives the same shapes on every machine, so a failure can be reproduced from the
/// seed alone.
struct Generator(u64);

impl Generator {
    /// The next number, xorshift64*: small, determined, and wide enough for page sizes.
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// One value in `0..bound`.
    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

/// Three events share one instant, and a page of one still walks all three.
#[test]
fn a_continued_page_does_not_step_over_a_row_that_shares_the_instant() {
    let fixture = Fixture::start();
    fixture.write_at(NEWER, "invoice.paid", 3);

    let first = Page::of(&fixture.ask("limit=1"));
    assert_eq!(
        first.ids.len(),
        1,
        "a page holds what was asked for: {:?}",
        first.ids
    );
    let cursor = first
        .next
        .expect("three events and a page of one: there is a page behind this one");

    let second = Page::of(&fixture.ask(&format!("limit=1&cursor={cursor}")));
    assert_eq!(
        second.ids.len(),
        1,
        "the second page is empty. Three events share one instant, and a position that carries \
         only the instant puts the cursor *after* all of them at once: the position is the \
         instant **and** the identity of the row"
    );
    assert_ne!(
        second.ids, first.ids,
        "the second page has to continue, not repeat: {:?}",
        second.ids
    );

    assert_walked(
        "three events at one instant, a page of one",
        &walk(&fixture, "limit=1"),
        &fixture.expected(None),
    );
}

/// Every stream shape a few generated rounds produce is walked exactly once.
#[test]
fn paging_through_a_stream_visits_every_event_exactly_once() {
    let fixture = Fixture::start();
    let mut generator = Generator(0x5eed_1234_9abc_def0);

    for round in 0..10 {
        fixture.postgres.execute("delete from events");
        let tied = generator.below(5) as usize;
        let older = generator.below(5) as usize;
        fixture.write_at(NEWER, "invoice.paid", tied);
        fixture.write_at(OLDER, "invoice.paid", older);
        let what = format!("round {round}: {tied} events at one instant and {older} at another");
        let expected = fixture.expected(None);

        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= 32,
                "{what}: paging did not end after {pages} pages"
            );
            let limit = 1 + generator.below(3) as i64;
            let query = match &cursor {
                Some(cursor) => format!("limit={limit}&cursor={cursor}"),
                None => format!("limit={limit}"),
            };
            let page = Page::of(&fixture.ask(&query));
            assert!(
                page.ids.len() as i64 <= limit,
                "{what}: a page holds at most what was asked for (`limit={limit}`): {:?}",
                page.ids
            );
            assert!(
                page.next.is_none() || page.ids.len() as i64 == limit,
                "{what}: a page with a `next` is as full as it can be, or the caller pays a round \
                 trip for rows the database already had (`limit={limit}`): {:?}",
                page.ids
            );
            seen.extend(page.ids);
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_walked(&what, &seen, &expected);
    }
}

/// The default is a page, the maximum is a maximum, and everything else is a `400`.
#[test]
fn the_page_size_is_bounded_and_the_default_is_a_page() {
    let fixture = Fixture::start();
    fixture.write_at(NEWER, "invoice.paid", 30);

    let default = Page::of(&fixture.ask(""));
    assert_eq!(
        default.ids.len(),
        DEFAULT_LIMIT,
        "a caller that asks for no size gets a page of {DEFAULT_LIMIT}, not the tenant's whole \
         table: {:?}",
        default.ids
    );
    assert!(
        default.next.is_some(),
        "thirty events and a page of {DEFAULT_LIMIT}: there is a page behind this one"
    );

    let largest = Page::of(&fixture.ask(&format!("limit={MAX_LIMIT}")));
    assert_eq!(
        largest.ids.len(),
        30,
        "a page is bounded by the stream too: {:?}",
        largest.ids
    );
    assert!(
        largest.next.is_none(),
        "thirty events behind a page of {MAX_LIMIT}: there is no next page"
    );

    for bad in ["0", "-1", "101", "abc", ""] {
        let answer = fixture.ask(&format!("limit={bad}"));
        assert_eq!(
            answer.status, 400,
            "`limit={bad}` is not a page size this API serves (1 to {MAX_LIMIT}), and answering \
             `200` with a different size than the one asked for is a lie a client pages on: {}",
            answer.body
        );
        assert!(
            answer.body.to_lowercase().contains("limit"),
            "the refusal names the parameter, so the caller can fix it: {}",
            answer.body
        );
    }

    let one = fixture.ask("limit=1");
    assert_eq!(
        one.status, 200,
        "the range is inclusive at both ends: {}",
        one.body
    );
}

/// A cursor this service did not issue is refused rather than guessed at.
#[test]
fn a_cursor_this_service_did_not_issue_is_refused() {
    let fixture = Fixture::start();
    fixture.write_at(NEWER, "invoice.paid", 2);

    let issued = Page::of(&fixture.ask("limit=1"))
        .next
        .expect("one event is behind the cursor");
    let accepted = fixture.ask(&format!("limit=1&cursor={issued}"));
    assert_eq!(
        accepted.status, 200,
        "a cursor this service issued is a cursor this service accepts: {}",
        accepted.body
    );

    let truncated = issued[..issued.len() - 1].to_owned();
    let inserted = format!("0{issued}");
    let impossible = [
        "not-a-cursor".to_owned(),
        String::new(),
        "!".repeat(issued.len()),
        truncated,
        inserted,
    ];
    for bad in impossible {
        let answer = fixture.ask(&format!("cursor={bad}"));
        assert_eq!(
            answer.status, 400,
            "`{bad}` is not a position this service ever wrote, so it cannot be a position it \
             continues from: {}",
            answer.body
        );
        assert!(
            answer.body.to_lowercase().contains("cursor"),
            "the refusal names the parameter, so the caller can fix it: {}",
            answer.body
        );
    }
}

/// The filter narrows the stream, and the cursor continues inside what is left of it.
#[test]
fn the_kind_filter_and_the_cursor_are_the_same_stream() {
    let fixture = Fixture::start();
    // Kinds alternate, one event per instant: a page of two `invoice.paid` events has an
    // `invoice.voided` row *between* the two it holds.
    for (index, kind) in ["invoice.paid", "invoice.voided"]
        .into_iter()
        .cycle()
        .take(6)
        .enumerate()
    {
        fixture.write_at(&format!("2026-01-01 00:00:0{}+00", 6 - index), kind, 1);
    }

    let body = fixture.ask("kind=invoice.paid&limit=2").json();
    let events = body["events"]
        .as_array()
        .unwrap_or_else(|| panic!("a page carries an `events` array: {body}"));
    for event in events {
        assert_eq!(
            event["kind"],
            json!("invoice.paid"),
            "a filtered page holds only what was filtered for: {body}"
        );
    }
    assert_eq!(
        events.len(),
        2,
        "the filter narrows the stream and the page fills up from it: three `invoice.paid` events \
         are in this table, and a page that ends at the first row of another kind is not a page \
         of two: {body}"
    );

    assert_walked(
        "kind=invoice.paid",
        &walk(&fixture, "kind=invoice.paid"),
        &fixture.expected(Some("invoice.paid")),
    );
    assert_eq!(
        fixture.expected(Some("invoice.paid")).len(),
        3,
        "the fixture writes three events of this kind"
    );
}

/// An empty stream is an empty page, and what the API accepted is what the listing hands back.
#[test]
fn an_empty_stream_is_an_empty_page_and_the_listing_is_the_same_stream() {
    let fixture = Fixture::start();

    let empty = Page::of(&fixture.ask(""));
    assert!(
        empty.ids.is_empty(),
        "a tenant with no events gets a page with no events, not an error: {:?}",
        empty.ids
    );
    assert!(
        empty.next.is_none(),
        "and no next page: there is nothing behind nothing"
    );

    let accepted = fixture.app.client().send(
        Request::post("/events", Vec::new())
            .bearer(ACME.1)
            .header("idempotency-key", "key-1")
            .json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })),
    );
    assert_eq!(accepted.status, 201, "{}", accepted.body);

    let page = Page::of(&fixture.ask(""));
    assert_eq!(
        page.ids,
        vec![accepted.field("id").as_str().unwrap().to_owned()],
        "the event `POST /events` accepted is the event the listing hands back: one stream, two \
         doors"
    );
    assert!(page.next.is_none(), "one event, one page");
}

/// The order the pages walk in is the order an index can serve.
#[test]
fn the_page_query_has_an_index_to_walk() {
    let fixture = Fixture::start();

    let indexes = fixture
        .postgres
        .query("select indexdef from pg_indexes where tablename = 'events'");
    let walkable = indexes
        .lines()
        .map(|line| line.replace(" DESC", "").replace(" ASC", ""))
        .any(|line| line.contains("(tenant_id, received_at, id)"));
    assert!(
        walkable,
        "a page reads one tenant's events in `(received_at, id)` order, so the index has to be in \
         that order too: without it the database sorts the tenant's whole table for every page, and \
         the cost of a page grows with the tenant. Indexes on `events`: {indexes}"
    );
}
