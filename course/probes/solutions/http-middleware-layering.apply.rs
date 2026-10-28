//! probe = "http/middleware-layering@apply"
//! title = "The cheapest rejection comes first"
//! hint = "Logging is outermost so it sees the rejections; rate limiting runs before auth so a flood does not pay for password hashing; auth runs before the handler."
//!
//! Step `relay/2.1-project-skeleton` rebuilds the service with its layers in one order, and the
//! order is a decision, not a preference: logging outside everything (a rejected request is the
//! one you most want in the log), rate limiting before authentication (a cheap check in front of an
//! expensive one), authentication before the handler (nothing runs for a caller who is nobody).
//!
//! The stack here is a list of names, outermost first, because a probe runs offline and cannot
//! start a server. What is re-checked is the order a reviewer checks by reading the builder.
//!
//! ## What you write
//!
//! [`audit`] — every finding, in this order:
//!
//! * [`Finding::LoggingNotOutermost`] when `logging` is present and is not the first layer;
//! * [`Finding::RateLimitAfterAuth`] when both are present and rate limiting is inside auth;
//! * [`Finding::AuthAfterHandler`] when both are present and auth is inside the handler.
//!
//! A layer that is not in the list is not audited: a service without rate limiting has no ordering
//! mistake to fix.
//!
//! ## What the tests ask
//!
//! The course's order; logging in the middle; rate limiting inside auth; auth inside the handler;
//! a list missing the layers entirely; and two mistakes at once, in the documented order.

/// Something the middleware order gets wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Finding {
    /// Logging must see every rejection, so it goes outside everything.
    LoggingNotOutermost,
    /// A cheap rejection has to come before the expensive one.
    RateLimitAfterAuth,
    /// Nothing runs for a caller who is nobody.
    AuthAfterHandler,
}

/// Every finding about `layers` (outermost first).
#[must_use]
pub fn audit(layers: &[&str]) -> Vec<Finding> {
    let position = |name: &str| layers.iter().position(|layer| *layer == name);
    let mut findings = Vec::new();

    if let Some(logging) = position("logging")
        && logging != 0
    {
        findings.push(Finding::LoggingNotOutermost);
    }
    // Both checks are about one position being inside another; a layer that is absent has no
    // position, and a service without it has no ordering mistake to fix.
    if let (Some(rate_limit), Some(auth)) = (position("rate_limit"), position("auth"))
        && rate_limit > auth
    {
        findings.push(Finding::RateLimitAfterAuth);
    }
    if let (Some(auth), Some(handler)) = (position("auth"), position("handler"))
        && auth > handler
    {
        findings.push(Finding::AuthAfterHandler);
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(layers: &[&str], name: &str) -> Option<usize> {
        layers.iter().position(|layer| *layer == name)
    }

    #[test]
    fn the_courses_order_has_nothing_to_report() {
        let layers = ["logging", "rate_limit", "auth", "handler"];
        assert_eq!(audit(&layers), []);
    }

    #[test]
    fn logging_in_the_middle_sees_only_what_got_through() {
        let layers = ["rate_limit", "logging", "auth", "handler"];
        assert_eq!(audit(&layers), [Finding::LoggingNotOutermost]);
    }

    #[test]
    fn rate_limiting_inside_auth_pays_for_every_flood() {
        let layers = ["logging", "auth", "rate_limit", "handler"];
        assert_eq!(audit(&layers), [Finding::RateLimitAfterAuth]);
    }

    #[test]
    fn auth_inside_the_handler_is_too_late() {
        let layers = ["logging", "rate_limit", "handler", "auth"];
        assert_eq!(audit(&layers), [Finding::AuthAfterHandler]);
    }

    #[test]
    fn a_service_without_the_layers_has_no_ordering_to_get_wrong() {
        assert_eq!(audit(&["handler"]), []);
        assert_eq!(audit(&["logging", "handler"]), []);
    }

    #[test]
    fn two_mistakes_arrive_in_the_documented_order() {
        let layers = ["auth", "logging", "rate_limit", "handler"];
        assert_eq!(
            audit(&layers),
            [Finding::LoggingNotOutermost, Finding::RateLimitAfterAuth]
        );
        assert_eq!(position(&layers, "auth"), Some(0));
    }
}
