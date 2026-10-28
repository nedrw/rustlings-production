//! probe = "http/service-skeleton@apply"
//! title = "404 means nobody has this route, 405 means this route has other methods"
//! hint = "A fallback that answers 404 for every miss swallows the method too: the router knows a path's methods, and 405 has to come from the router."
//!
//! Step `mailer/1.7-http` makes the difference between three answers explicit: `200` for the
//! route, `404` for a path nobody registered, and `405` for a path that exists with a different
//! method — with `Allow` naming the methods it does have. The third answer is the one a catch-all
//! fallback destroys: it never asks the router, so it cannot know.
//!
//! The router here is a table, because a probe runs offline and cannot open a socket. What is
//! re-checked is the decision a request gets, and `HEAD` is part of it: `HEAD` is what `GET` means
//! without the body, so a route that answers `GET` answers `HEAD` too.
//!
//! ## What you write
//!
//! [`Router::handle`] — the answer for one request:
//!
//! * the path is registered and the method is allowed → `200` with `{"status":"ok"}`;
//! * the path is registered, the method is `HEAD`, and `GET` is allowed → `200` with an empty body;
//! * the path is registered and nothing allows the method → `405`, body `{"error":"method not
//!   allowed"}`, and `allow` is the route's methods in the order it was registered;
//! * the path is not registered → `404`, body `{"error":"not found"}`, and no `allow` at all.
//!
//! ## What the tests ask
//!
//! `GET` then `HEAD` on a route; an unknown path; `POST` on a route that only has `GET`; a
//! `POST`-only route asked with `GET` (the allow list is that route's, not a global one); and the
//! fallback never claiming a path it does not have.

/// A request method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Read a representation.
    Get,
    /// Read the headers only.
    Head,
    /// Change something.
    Post,
}

/// What the service answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The status code.
    pub status: u16,
    /// The body, which is empty for `HEAD`.
    pub body: &'static str,
    /// Methods allowed on this path, in registration order; empty when the path is unknown.
    pub allow: Vec<Method>,
}

/// A table of paths and the methods each one answers.
#[derive(Debug, Default)]
pub struct Router {
    routes: Vec<(&'static str, Vec<Method>)>,
}

impl Router {
    /// An empty router.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a path and the methods it answers.
    #[must_use]
    pub fn route(mut self, path: &'static str, methods: &[Method]) -> Self {
        self.routes.push((path, methods.to_vec()));
        self
    }

    /// The answer for one request.
    #[must_use]
    pub fn handle(&self, method: Method, path: &str) -> Response {
        let Some((_, methods)) = self
            .routes
            .iter()
            .find(|(registered, _)| *registered == path)
        else {
            return Response {
                status: 404,
                body: "{\"error\":\"not found\"}",
                allow: Vec::new(),
            };
        };
        // `HEAD` is what `GET` means without the body: the route that answers one answers both.
        let allowed =
            methods.contains(&method) || (method == Method::Head && methods.contains(&Method::Get));
        if allowed {
            let body = if method == Method::Head {
                ""
            } else {
                "{\"status\":\"ok\"}"
            };
            return Response {
                status: 200,
                body,
                allow: Vec::new(),
            };
        }
        Response {
            status: 405,
            body: "{\"error\":\"method not allowed\"}",
            allow: methods.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn router() -> Router {
        Router::new()
            .route("/healthz", &[Method::Get, Method::Head])
            .route("/subscribe", &[Method::Post])
    }

    #[test]
    fn a_registered_route_answers_with_its_body() {
        let response = router().handle(Method::Get, "/healthz");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "{\"status\":\"ok\"}");
        assert!(response.allow.is_empty());
    }

    #[test]
    fn head_is_what_get_means_without_the_body() {
        let response = router().handle(Method::Head, "/healthz");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "");
    }

    #[test]
    fn an_unknown_path_is_404_and_names_nothing() {
        let response = router().handle(Method::Get, "/nope");
        assert_eq!(response.status, 404);
        assert_eq!(response.body, "{\"error\":\"not found\"}");
        assert!(
            response.allow.is_empty(),
            "a path nobody has cannot say what it allows"
        );
    }

    #[test]
    fn the_wrong_method_on_a_known_path_is_405_with_allow() {
        let response = router().handle(Method::Post, "/healthz");
        assert_eq!(response.status, 405);
        assert_eq!(response.body, "{\"error\":\"method not allowed\"}");
        assert_eq!(response.allow, [Method::Get, Method::Head]);
    }

    #[test]
    fn the_allow_list_belongs_to_the_path_not_to_the_router() {
        let response = router().handle(Method::Get, "/subscribe");
        assert_eq!(response.status, 405);
        assert_eq!(response.allow, [Method::Post]);
    }

    #[test]
    fn head_on_a_post_only_route_is_not_answered_as_get() {
        let response = router().handle(Method::Head, "/subscribe");
        assert_eq!(response.status, 405);
        assert_eq!(response.allow, [Method::Post]);
    }

    #[test]
    fn a_path_registered_without_methods_answers_405_to_everything() {
        let router = Router::new().route("/empty", &[]);
        let response = router.handle(Method::Get, "/empty");
        assert_eq!(response.status, 405);
        assert!(response.allow.is_empty());
    }
}
