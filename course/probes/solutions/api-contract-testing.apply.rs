//! probe = "api/contract-testing@apply"
//! title = "Additive is compatible; subtraction is a breaking change"
//! hint = "Compare the snapshot you reviewed with the one you are about to publish: a removed endpoint, or a removed field on either side of it, breaks every client that already exists."
//!
//! Step `relay/2.3-api-contract` keeps a snapshot of the API and fails a change that breaks it.
//! The rule is asymmetric: adding an endpoint or a field is compatible (existing clients did not
//! know about it), and removing one is not — the client that sends the field you no longer accept
//! gets a 422 nobody warned them about.
//!
//! The API here is a list of endpoints, because a probe runs offline and cannot generate a
//! document. What is re-checked is the comparison the snapshot test makes.
//!
//! ## What you write
//!
//! [`breaking`] — every incompatible difference between `before` and `after`, sorted:
//!
//! * [`Breaking::Removed`] for an endpoint (method and path) that existed and is gone;
//! * [`Breaking::FieldRemoved`] for a request field the API no longer accepts, or a response field
//!   it no longer returns — a client sends the first and reads the second;
//! * nothing for an added endpoint or an added field.
//!
//! ## What the tests ask
//!
//! An unchanged API; a removed endpoint; a removed request field; a removed response field; an
//! added field and an added endpoint (both compatible); and several breaks arriving sorted.

/// One endpoint of the API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// `GET`, `POST`, …
    pub method: String,
    /// The path.
    pub path: String,
    /// Fields a client may send.
    pub request: Vec<String>,
    /// Fields the response carries.
    pub response: Vec<String>,
}

/// A change that breaks clients written against the old snapshot.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Breaking {
    /// An endpoint is gone.
    Removed {
        /// `POST /events`
        endpoint: String,
    },
    /// A field is gone from one side of an endpoint.
    FieldRemoved {
        /// `POST /events`
        endpoint: String,
        /// The field.
        field: String,
        /// `request` or `response`.
        side: &'static str,
    },
}

/// Every breaking difference, sorted.
#[must_use]
pub fn breaking(before: &[Endpoint], after: &[Endpoint]) -> Vec<Breaking> {
    let identifier = |endpoint: &Endpoint| format!("{} {}", endpoint.method, endpoint.path);
    let mut breaks = Vec::new();

    for old in before {
        let name = identifier(old);
        let Some(new) = after.iter().find(|candidate| identifier(candidate) == name) else {
            breaks.push(Breaking::Removed { endpoint: name });
            continue;
        };
        for (side, old_fields, new_fields) in [
            ("request", &old.request, &new.request),
            ("response", &old.response, &new.response),
        ] {
            for field in old_fields {
                if !new_fields.contains(field) {
                    breaks.push(Breaking::FieldRemoved {
                        endpoint: name.clone(),
                        field: field.clone(),
                        side,
                    });
                }
            }
        }
    }

    breaks.sort();
    breaks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(method: &str, path: &str, request: &[&str], response: &[&str]) -> Endpoint {
        Endpoint {
            method: method.to_owned(),
            path: path.to_owned(),
            request: request.iter().map(|f| (*f).to_owned()).collect(),
            response: response.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    fn one() -> Endpoint {
        endpoint("POST", "/events", &["kind", "payload"], &["id", "status"])
    }

    #[test]
    fn an_unchanged_api_breaks_nothing() {
        assert_eq!(breaking(&[one()], &[one()]), []);
    }

    #[test]
    fn a_removed_endpoint_breaks_every_client_of_it() {
        assert_eq!(
            breaking(&[one()], &[]),
            [Breaking::Removed {
                endpoint: "POST /events".to_owned(),
            }]
        );
    }

    #[test]
    fn a_removed_request_field_is_a_break() {
        let after = [endpoint("POST", "/events", &["kind"], &["id", "status"])];
        assert_eq!(
            breaking(&[one()], &after),
            [Breaking::FieldRemoved {
                endpoint: "POST /events".to_owned(),
                field: "payload".to_owned(),
                side: "request",
            }]
        );
    }

    #[test]
    fn a_removed_response_field_is_a_break() {
        let after = [endpoint("POST", "/events", &["kind", "payload"], &["id"])];
        assert_eq!(
            breaking(&[one()], &after),
            [Breaking::FieldRemoved {
                endpoint: "POST /events".to_owned(),
                field: "status".to_owned(),
                side: "response",
            }]
        );
    }

    #[test]
    fn adding_a_field_or_an_endpoint_is_compatible() {
        let after = [
            endpoint(
                "POST",
                "/events",
                &["kind", "payload", "trace"],
                &["id", "status"],
            ),
            endpoint("GET", "/events/{id}", &[], &["id"]),
        ];
        assert_eq!(breaking(&[one()], &after), []);
    }

    #[test]
    fn several_breaks_arrive_sorted() {
        let after = [endpoint("POST", "/events", &["kind"], &["id"])];
        let other = endpoint("GET", "/events", &[], &["id"]);
        let breaks = breaking(&[one(), other], &after);
        assert_eq!(breaks.len(), 3, "{breaks:?}");
        assert!(matches!(breaks[0], Breaking::Removed { .. }), "{breaks:?}");
        assert!(
            breaks.windows(2).all(|pair| pair[0] <= pair[1]),
            "not sorted: {breaks:?}"
        );
    }
}
