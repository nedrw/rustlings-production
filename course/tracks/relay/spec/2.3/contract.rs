//! Spec for `relay/2.3-contract-first`: the document the service publishes *is* the API.
//!
//! `kiln run` copies this file into the project as `tests/spec_contract.rs`; it is the course's
//! file, not the learner's (plan §3 D2). The document is fetched over HTTP (`GET /openapi.json`),
//! not called as a function: what a client reads is what this file judges, and a contract the
//! service does not serve is a file in a repository.
//!
//! A document is easy to write and easy to let rot, so every claim it makes is checked against the
//! running service:
//!
//! * **every route it declares exists.** The spec sends each `path` and `method` the document
//!   lists, and an answer other than 404 or 405 is the proof that the router serves it. A document
//!   that invents a route is worse than no document at all: somebody writes a client against it;
//! * **every answer it gives is described.** The status has to appear in the route's `responses`,
//!   and the body has to match the schema declared for it — which is what `additionalProperties:
//!   false` is for. A field the service starts sending without documenting it is a contract break,
//!   and this is the only place that notices;
//! * **`required` is a claim about the code, not a wish.** The spec builds a request *out of the
//!   document's own `required` list*, watches the service accept it, then drops those fields one at
//!   a time and watches each one be refused;
//! * **the previous release keeps working.** The body step `relay/2.2-idempotent-ingest` accepted
//!   still has to be accepted, and the document may not require a field that body never carried —
//!   backwards compatibility is the half of contract-first that is easy to promise and hard to
//!   keep;
//! * **the document is reproducible.** Two fetches, byte for byte: a snapshot of it in a pull
//!   request has to show a changed contract, not a changed iteration order or a timestamp.

mod support;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{Map, Value, json};
use support::app::{self, App};
use support::client::{Client, Request, Response};
use support::postgres::Postgres;
use support::var;

/// The tenant these tests seed, and the key it authenticates with.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// Where the service publishes its contract.
const DOCUMENT: &str = "/openapi.json";
/// The routes this step's contract is about: the ones the service already served before it had a
/// document. A client that reads the contract has to find them there.
///
/// The list is deliberately a *minimum*: later steps add routes, and this file is judged on the
/// final tree too (`kiln verify`), so "exactly these" would be a promise this step cannot keep.
const PINNED: [(&str, &str); 6] = [
    ("get", "/healthz"),
    ("get", "/readyz"),
    ("get", "/endpoints"),
    ("post", "/endpoints"),
    ("post", "/events"),
    // The document itself: a contract nobody can fetch is a file, not a contract.
    ("get", "/openapi.json"),
];
/// HTTP methods, as OpenAPI spells them in a path item.
const METHODS: [&str; 7] = ["get", "post", "put", "patch", "delete", "head", "options"];

/// A running service, with the database it was started against.
///
/// The container is handed back rather than hidden: the app answers requests for exactly as long as
/// its database is there, and a fixture that dropped it early would turn a passing test into
/// error lines on stderr.
fn service(tenants: &[(&str, &str)]) -> (App, Postgres) {
    let postgres = Postgres::start();
    let seeded = tenants
        .iter()
        .map(|(name, key)| format!("{name}={key}"))
        .collect::<Vec<_>>()
        .join(",");
    let app = App::start(app::config(&[
        var(app::DATABASE_URL, postgres.url()),
        var(app::BOOTSTRAP_TENANTS, seeded),
    ]));
    (app, postgres)
}

/// The contract the running service publishes.
fn published(client: &Client) -> Value {
    let answer = client.get(DOCUMENT);
    assert_eq!(
        answer.status, 200,
        "the service publishes its contract at `{DOCUMENT}`, so a client can read what it promises \
         without reading its source: {}",
        answer.body
    );
    answer.json()
}

/// The `paths` object of a document.
fn paths(document: &Value) -> Map<String, Value> {
    document
        .get("paths")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| panic!("the document declares no `paths` at all: {document}"))
}

/// The schemas a document declares by name.
fn schemas(document: &Value) -> Map<String, Value> {
    document
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| panic!("the document declares no `components.schemas`: {document}"))
}

/// One path item, as the document declares it.
fn path_item(document: &Value, path: &str) -> Value {
    paths(document)
        .get(path)
        .cloned()
        .unwrap_or_else(|| panic!("the document declares no `{path}`: {:?}", paths(document)))
}

/// One route: the path item's entry for one method.
fn route(document: &Value, method: &str, path: &str) -> Value {
    path_item(document, path)
        .get(method)
        .cloned()
        .unwrap_or_else(|| panic!("the document declares `{path}` without `{method}`"))
}

/// `POST /events` with a fresh idempotency key, so every call is a new event unless a test says
/// otherwise.
fn post(client: &Client, body: &Value) -> Response {
    static KEYS: AtomicU32 = AtomicU32::new(0);
    let key = format!("key-{}", KEYS.fetch_add(1, Ordering::Relaxed));
    client.send(
        Request::post("/events", Vec::new())
            .bearer(ACME.1)
            .header("idempotency-key", &key)
            .json(body),
    )
}

/// The body a client of the previous release sends.
///
/// Step `relay/2.2-idempotent-ingest` accepted this shape before there was a document, and the
/// document may not require anything it does not carry.
fn old_client_body() -> Value {
    json!({ "kind": "invoice.paid", "payload": { "id": "inv_1", "amount": 4200 } })
}

/// A value of the type a schema declares, for building a request out of the document itself.
fn example(schema: &Value) -> Value {
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => json!({}),
        Some("string") => json!("x"),
        Some("boolean") => json!(true),
        Some("integer") | Some("number") => json!(1),
        Some("array") => json!([]),
        other => panic!("this test cannot build a value for the declared type {other:?}: {schema}"),
    }
}

/// `true` when a value is the JSON type a schema names.
fn is_a(value: &Value, kind: &str) -> bool {
    match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

/// The schema a `$ref` points at.
fn resolve<'a>(document: &'a Value, reference: &str) -> Result<&'a Value, String> {
    let name = reference
        .strip_prefix("#/components/schemas/")
        .ok_or_else(|| format!("`{reference}` is not a reference into this document's schemas"))?;
    document
        .pointer(&format!("/components/schemas/{name}"))
        .ok_or_else(|| format!("`{reference}` points at a schema that is not declared"))
}

/// A schema with its `$ref` followed: what the document really says about a value.
fn resolved(document: &Value, schema: &Value) -> Value {
    match schema.get("$ref").and_then(Value::as_str) {
        Some(reference) => resolve(document, reference)
            .unwrap_or_else(|why| panic!("`{reference}`: {why}"))
            .clone(),
        None => schema.clone(),
    }
}

/// Every `$ref` a document contains.
fn references(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (name, entry) in fields {
                if name == "$ref" {
                    if let Some(reference) = entry.as_str() {
                        found.push(reference.to_owned());
                    }
                } else {
                    references(entry, found);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                references(item, found);
            }
        }
        _ => {}
    }
}

/// Judge a value against a schema of the document.
///
/// A JSON Schema implementation is not what this step is about; agreeing *mechanically* on the
/// shapes this API produces is. The subset here is the subset the API's own shapes use — `type`,
/// `required`, `properties`, `additionalProperties`, `items` and `$ref` — and a schema outside it
/// is reported rather than skipped, so a document this file cannot read is visible in its output.
fn check(value: &Value, schema: &Value, document: &Value, what: &str) -> Result<(), String> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let target = resolve(document, reference)?;
        return check(value, target, document, what);
    }
    let types: Vec<&str> = match schema.get("type") {
        Some(Value::String(one)) => vec![one.as_str()],
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
        other => {
            return Err(format!(
                "{what}: the schema declares no usable `type` ({other:?})"
            ));
        }
    };
    if !types.iter().any(|kind| is_a(value, kind)) {
        return Err(format!(
            "{what}: the answer is `{value}`, which is none of the declared types {types:?}"
        ));
    }
    if let Some(required) = schema.get("required") {
        for field in required
            .as_array()
            .unwrap_or_else(|| panic!("{what}: `required` is not an array"))
        {
            let field = field.as_str().unwrap_or_else(|| {
                panic!("{what}: `required` names something that is not a field")
            });
            if value.get(field).is_none() {
                return Err(format!(
                    "{what}: the document says `{field}` is required and the answer has no such \
                     field: {value}"
                ));
            }
        }
    }
    if let (Some(properties), Some(object)) = (
        schema.get("properties").and_then(Value::as_object),
        value.as_object(),
    ) {
        for (field, subschema) in properties {
            if let Some(field_value) = object.get(field) {
                check(field_value, subschema, document, &format!("{what}.{field}"))?;
            }
        }
        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            for field in object.keys() {
                if !properties.contains_key(field) {
                    return Err(format!(
                        "{what}: the answer carries `{field}`, which the document does not \
                         declare — and it closes the object (`additionalProperties: false`)"
                    ));
                }
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (index, item) in array.iter().enumerate() {
            check(item, items, document, &format!("{what}[{index}]"))?;
        }
    }
    Ok(())
}

/// The JSON schema a route declares for one answer, if it declares JSON for it.
fn declared_schema(route: &Value, status: u16) -> Result<Option<Value>, String> {
    let responses = route
        .get("responses")
        .ok_or_else(|| "it declares no `responses` at all".to_owned())?;
    let declared = responses.get(status.to_string()).ok_or_else(|| {
        let described = responses
            .as_object()
            .map(|statuses| statuses.keys().cloned().collect::<Vec<_>>().join(", "))
            .unwrap_or_else(|| "nothing".to_owned());
        format!("the document does not describe a `{status}` for it (it describes: {described})")
    })?;
    Ok(declared
        .get("content")
        .and_then(|content| content.get("application/json"))
        .and_then(|media| media.get("schema"))
        .cloned())
}

/// The service publishes a document a client can act on.
#[test]
fn the_service_publishes_an_openapi_document() {
    let (app, _database) = service(&[ACME]);
    let document = published(&app.client());

    let version = document
        .get("openapi")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the document declares no `openapi` version: {document}"));
    assert!(
        version.starts_with("3."),
        "`openapi` names the version of the format this document is written in, and a client \
         parses it accordingly: {version}"
    );
    let title = document
        .pointer("/info/title")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the document declares no `info.title`: {document}"));
    assert!(
        !title.is_empty(),
        "`info.title` is what a generated client is called: {document}"
    );
    assert!(
        !paths(&document).is_empty(),
        "a document with no paths promises a client nothing: {document}"
    );
    assert!(
        !schemas(&document).is_empty(),
        "the paths have to be built from named schemas, or nothing is shared between them: {document}"
    );
}

/// Every schema is typed, closed where it describes a record, and every reference lands somewhere.
#[test]
fn every_schema_is_typed_and_every_reference_resolves() {
    let (app, _database) = service(&[ACME]);
    let document = published(&app.client());
    let declared = schemas(&document);

    for (name, schema) in &declared {
        assert!(
            schema.get("type").is_some(),
            "`components.schemas.{name}` declares no `type`, so nothing reading this document can \
             tell what it is: {schema}"
        );
        let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
            // A schema with no fields is a free-form map: an event payload, or the document itself.
            continue;
        };
        assert_eq!(
            schema.get("additionalProperties"),
            Some(&Value::Bool(false)),
            "`{name}` describes a record with fields, so it has to close the object: without \
             `additionalProperties: false` the service can start sending fields nobody agreed to \
             and no test notices"
        );
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("`{name}` declares no `required` list: {schema}"));
        for field in required {
            let field = field
                .as_str()
                .unwrap_or_else(|| panic!("`{name}`: {field}"));
            assert!(
                properties.contains_key(field),
                "`{name}` requires `{field}`, which it never declares: {schema}"
            );
        }
    }

    let mut found = Vec::new();
    references(&document, &mut found);
    for reference in found {
        resolve(&document, &reference).unwrap_or_else(|why| panic!("`{reference}`: {why}"));
    }
}

/// Two fetches of the document are the same bytes, so a snapshot of it means something.
#[test]
fn the_document_is_the_same_one_every_time_it_is_generated() {
    let (app, _database) = service(&[ACME]);
    let client = app.client();

    let first = client.get(DOCUMENT);
    let second = client.get(DOCUMENT);
    assert_eq!(first.status, 200, "{}", first.body);
    assert_eq!(second.status, 200, "{}", second.body);
    assert_eq!(
        first.body, second.body,
        "the document is generated, not remembered: a field order that comes out of a hash map, \
         or a version that carries the time, makes every snapshot diff noise"
    );
}

/// Every route the document declares is a route the service serves, and every answer is described.
#[test]
fn every_route_the_document_declares_is_a_route_the_service_serves() {
    let (app, _database) = service(&[ACME]);
    let client = app.client();
    let document = published(&client);

    for (method, path) in PINNED {
        assert!(
            path_item(&document, path).get(method).is_some(),
            "the document does not declare `{method} {path}`, which this service serves: {:?}",
            paths(&document)
        );
    }

    for (path, item) in paths(&document) {
        // A path with a parameter needs a real id to point at, and this file has one event at most:
        // the shape of such a path is judged by the schema checks above, not by a request.
        if path.contains('{') {
            continue;
        }
        for method in METHODS {
            let Some(route) = item.get(method) else {
                continue;
            };
            let answer = probe(&client, method, &path);
            assert!(
                answer.status != 404 && answer.status != 405,
                "the document declares `{method} {path}` and the router does not serve it ({}): {}",
                answer.status,
                answer.body
            );
            match declared_schema(route, answer.status) {
                Err(why) => panic!(
                    "the service answered `{method} {path}` with {} and {why}. Every answer a \
                     caller can get has to be in the contract:\n{}",
                    answer.status, answer.body
                ),
                Ok(None) => {}
                Ok(Some(schema)) => {
                    let body = answer.json();
                    check(
                        &body,
                        &schema,
                        &document,
                        &format!("{} {method} {path}", answer.status),
                    )
                    .unwrap_or_else(|why| panic!("{why}\n{}", answer.body));
                }
            }
        }
    }
}

/// One request to a route the document declares.
///
/// Nothing goes in the body on purpose: this file is asking whether the route *exists* and whether
/// the document describes every answer it can give, not whether the request is a good one. A 400
/// for a missing body is an answer like any other, and it has to be in the document too.
fn probe(client: &Client, method: &str, path: &str) -> Response {
    let request = Request::new(&method.to_uppercase(), path);
    let request = if method == "get" {
        request
    } else {
        request.header("content-type", "application/json")
    };
    client.send(request.bearer(ACME.1))
}

/// The document's `required` list is what the route really requires, no more and no less.
#[test]
fn the_request_schema_is_the_body_the_route_accepts() {
    let (app, _database) = service(&[ACME]);
    let client = app.client();
    let document = published(&client);
    let raw = route(&document, "post", "/events")
        .pointer("/requestBody/content/application~1json/schema")
        .cloned()
        .unwrap_or_else(|| panic!("the document does not describe the body `POST /events` takes"));
    let schema = resolved(&document, &raw);

    let required: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the request schema declares no `required`: {schema}"))
        .iter()
        .map(|field| field.as_str().unwrap().to_owned())
        .collect();
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("the request schema declares no `properties`: {schema}"));

    // The body is not the whole request. The header that makes a retry safe is a required part of
    // it, and a client that is not told so will find out by getting a 400 in production.
    let parameters = route(&document, "post", "/events")
        .get("parameters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        parameters.iter().any(|parameter| {
            parameter.get("name").and_then(Value::as_str) == Some("Idempotency-Key")
                && parameter.get("in").and_then(Value::as_str) == Some("header")
                && parameter.get("required") == Some(&Value::Bool(true))
        }),
        "`POST /events` is unusable without the `Idempotency-Key` header, so the document has to \
         declare it as a required header parameter: {parameters:?}"
    );

    let mut body = Map::new();
    for field in &required {
        let property = properties
            .get(field)
            .unwrap_or_else(|| panic!("`{field}` is required and undeclared: {schema}"));
        body.insert(field.clone(), example(property));
    }
    let accepted = post(&client, &Value::Object(body.clone()));
    assert_eq!(
        accepted.status, 201,
        "a body built out of the document's own `required` fields is what the route accepts: {}",
        accepted.body
    );

    for field in &required {
        let mut without = body.clone();
        without.remove(field);
        let refused = post(&client, &Value::Object(without));
        assert_eq!(
            refused.status, 400,
            "the document calls `{field}` required, so a body without it has to be refused — one \
             of the two is wrong: {}",
            refused.body
        );
    }

    assert_eq!(
        schema.get("additionalProperties"),
        Some(&Value::Bool(false)),
        "the request schema has to close the object: an undeclared field is a field the handler \
         silently ignores: {schema}"
    );
    let mut extra = body.clone();
    extra.insert("not_in_the_document".to_owned(), json!(1));
    let refused = post(&client, &Value::Object(extra));
    assert_eq!(
        refused.status, 400,
        "the document says the body is closed, so an undeclared field is a 400 rather than a \
         typo the caller never hears about: {}",
        refused.body
    );

    let old = old_client_body();
    for field in &required {
        assert!(
            old.get(field).is_some(),
            "the document requires `{field}`, which a client of the previous release never sent: \
             {old}"
        );
    }
    let still_accepted = post(&client, &old);
    assert_eq!(
        still_accepted.status, 201,
        "the previous release's body is still a body this service accepts: {}",
        still_accepted.body
    );
}

/// What the service answers is the shape the document describes for that answer.
#[test]
fn the_answer_is_the_shape_the_document_describes() {
    let (app, _database) = service(&[ACME]);
    let client = app.client();
    let document = published(&client);
    let route = route(&document, "post", "/events");

    let key = "key-documented";
    let body = old_client_body();
    let send = |key: &str| {
        client.send(
            Request::post("/events", Vec::new())
                .bearer(ACME.1)
                .header("idempotency-key", key)
                .json(&body),
        )
    };

    let accepted = send(key);
    assert_eq!(accepted.status, 201, "{}", accepted.body);
    judge(&document, &route, &accepted);

    let replayed = send(key);
    assert_eq!(
        replayed.status, 200,
        "the second POST with one key is the answer the document has to describe for a replay: {}",
        replayed.body
    );
    judge(&document, &route, &replayed);
}

/// Judge one answer against the schema the document gives that status.
fn judge(document: &Value, route: &Value, answer: &Response) {
    let schema = declared_schema(route, answer.status)
        .unwrap_or_else(|why| panic!("the service answered {} and {why}", answer.status))
        .unwrap_or_else(|| {
            panic!(
                "the document describes `{}` without a JSON schema, so nothing checks what comes \
                 back: {}",
                answer.status, answer.body
            )
        });
    let schema = resolved(document, &schema);
    let body = answer.json();
    check(
        &body,
        &schema,
        document,
        &format!("{} POST /events", answer.status),
    )
    .unwrap_or_else(|why| panic!("{why}\n{}", answer.body));
    let described: BTreeSet<&String> = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().collect())
        .unwrap_or_default();
    let answered: BTreeSet<&String> = body
        .as_object()
        .map(|body| body.keys().collect())
        .unwrap_or_default();
    assert_eq!(
        answered, described,
        "the document's schema and the answer it describes have to be the same set of fields, or \
         the snapshot a reviewer reads is not the API a client gets"
    );
}
