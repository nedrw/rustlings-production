//! probe = "test/app-factory@apply"
//! title = "The test builds the app the binary runs"
//! hint = "One factory builds the application; the configured port is a hint the factory ignores, because a test that binds it collides with the next test."
//!
//! Step `mailer/1.8-test-harness` has one rule underneath its convenience: the harness and the
//! binary build the *same* application. A test that assembles its own router proves things about a
//! service nobody deploys; a test that inherits a process-wide app lets one test's state become
//! the next one's starting point. And the port comes from the operating system, not from the
//! configuration — two tests run at once, and a fixed port is a collision by schedule.
//!
//! ## What you write
//!
//! [`start`] — build one application:
//!
//! * the parts are `router` always, then `database` and `mail` when the configuration enables
//!   them, in that order (the list is what a test compares against production wiring);
//! * the port comes from [`Ports::next`], **never** from `config.port`: `0` means "the operating
//!   system picks", and a harness that honours a fixed port hands two concurrent tests the same
//!   one.
//!
//! ## What the tests ask
//!
//! The configured port ignored (including a fixed one); two applications getting different ports;
//! the parts following the configuration; and a default configuration wiring only the router.

/// What the operator wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// The port the operator asked for; `0` means "whatever is free".
    pub port: u16,
    /// Whether the service needs a database.
    pub database: bool,
    /// Whether the service sends mail.
    pub mail: bool,
}

/// What one test (or one process) got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    /// The port it is actually bound to.
    pub port: u16,
    /// The parts it was built with, in wiring order.
    pub components: Vec<&'static str>,
}

/// A model of the operating system's port allocator: every call is a port nobody else got.
#[derive(Debug, Default)]
pub struct Ports {
    next: u16,
}

impl Ports {
    /// An allocator whose first port is `first`.
    #[must_use]
    pub fn new(first: u16) -> Self {
        Self { next: first }
    }

    /// Hand out a port nobody else has been given.
    pub fn next(&mut self) -> u16 {
        let port = self.next;
        self.next = self.next.wrapping_add(1);
        port
    }
}

/// Build one application from `config`, taking its port from `ports`.
#[must_use]
pub fn start(config: &Config, ports: &mut Ports) -> App {
    todo!("wire the parts the configuration asks for; ignore config.port")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(port: u16, database: bool, mail: bool) -> Config {
        Config {
            port,
            database,
            mail,
        }
    }

    #[test]
    fn a_fixed_configured_port_is_ignored() {
        let mut ports = Ports::new(8001);
        let app = start(&config(9999, false, false), &mut ports);
        assert_eq!(app.port, 8001, "the harness honoured a fixed port");
    }

    #[test]
    fn port_zero_means_the_allocator_picks() {
        let mut ports = Ports::new(8001);
        let app = start(&config(0, false, false), &mut ports);
        assert_eq!(app.port, 8001);
    }

    #[test]
    fn two_applications_never_share_a_port() {
        let mut ports = Ports::new(8001);
        let first = start(&config(0, false, false), &mut ports);
        let second = start(&config(0, false, false), &mut ports);
        assert_ne!(first.port, second.port);
    }

    #[test]
    fn the_parts_follow_the_configuration_in_wiring_order() {
        let mut ports = Ports::new(8001);
        let app = start(&config(0, true, true), &mut ports);
        assert_eq!(app.components, ["router", "database", "mail"]);
        assert_eq!(app.port, 8001);
    }

    #[test]
    fn a_default_configuration_wires_only_the_router() {
        let mut ports = Ports::new(8001);
        let app = start(&config(0, false, false), &mut ports);
        assert_eq!(app.components, ["router"]);
    }

    #[test]
    fn both_optional_parts_together_keep_their_order() {
        let mut ports = Ports::new(8001);
        let first = start(&config(0, false, true), &mut ports);
        assert_eq!(first.components, ["router", "mail"]);
        let second = start(&config(0, true, false), &mut ports);
        assert_eq!(second.components, ["router", "database"]);
    }
}
