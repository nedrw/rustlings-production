//! probe = "test/fake-external@apply"
//! title = "The fake goes behind the same port as the real thing"
//! hint = "The use case talks to the port; the test hands it a fake and asserts on what the fake was asked to send — including that a failure records nothing."
//!
//! Step `mailer/1.13-email-port` puts an interface in front of the mail server and a fake behind
//! it. The point is not the fake: it is that the confirmation mail is built by the *use case*, not
//! by the test, so the test can assert on a message the binary would really have sent. A test that
//! reaches for a real SMTP server is slow and flaky; a test that asserts "no panic" proves nothing.
//!
//! ## What you write
//!
//! * [`Mailer`] implementations: [`FakeMailer`] records every accepted message and can be told to
//!   fail — and a failed send records nothing;
//! * [`send_confirmation`] — build the message (subject `Confirm your subscription`, body
//!   `Confirm: https://example.test/confirm?token=<token>`) and hand it to whatever `Mailer` it was
//!   given. It takes the *port*, never the fake, so production passes the real one.
//!
//! ## What the tests ask
//!
//! One message with the right fields; a programmed failure propagating and leaving the outbox
//! empty; two confirmations in order; and a second implementation of the port (a counter) driving
//! the same use case — which is what "the test does not know the fake" means.

/// One message to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Recipient address.
    pub to: String,
    /// Subject line.
    pub subject: String,
    /// Body, plain text.
    pub body: String,
}

/// The port: whatever sends mail implements this.
pub trait Mailer {
    /// Send one message.
    ///
    /// # Errors
    ///
    /// Whatever the transport says went wrong, as a sentence.
    fn send(&mut self, message: Message) -> Result<(), String>;
}

/// The test double: records what it was asked to send, and can be told to fail.
#[derive(Debug, Default)]
pub struct FakeMailer {
    sent: Vec<Message>,
    failure: Option<String>,
}

impl FakeMailer {
    /// A fake that accepts everything.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A fake whose next send fails with `reason`.
    #[must_use]
    pub fn failing(reason: &str) -> Self {
        Self {
            sent: Vec::new(),
            failure: Some(reason.to_owned()),
        }
    }

    /// What the fake was asked to send, in order.
    #[must_use]
    pub fn sent(&self) -> &[Message] {
        &self.sent
    }
}

impl Mailer for FakeMailer {
    fn send(&mut self, message: Message) -> Result<(), String> {
        todo!("programmed failure first; record the message only on success")
    }
}

/// Send the confirmation mail for `token` to `to`, through whatever mailer is given.
///
/// # Errors
///
/// Whatever the mailer reports.
pub fn send_confirmation(mailer: &mut dyn Mailer, to: &str, token: &str) -> Result<(), String> {
    todo!("build the message here, then hand it to the port")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_use_case_builds_the_message_the_fake_records() {
        let mut mailer = FakeMailer::new();
        assert_eq!(
            send_confirmation(&mut mailer, "a@example.test", "tok-1"),
            Ok(())
        );
        assert_eq!(
            mailer.sent(),
            [Message {
                to: "a@example.test".to_owned(),
                subject: "Confirm your subscription".to_owned(),
                body: "Confirm: https://example.test/confirm?token=tok-1".to_owned(),
            }]
        );
    }

    #[test]
    fn a_failed_send_records_nothing_and_says_why() {
        let mut mailer = FakeMailer::failing("smtp is down");
        assert_eq!(
            send_confirmation(&mut mailer, "a@example.test", "tok-1"),
            Err("smtp is down".to_owned())
        );
        assert!(
            mailer.sent().is_empty(),
            "a failed send was recorded as sent"
        );
    }

    #[test]
    fn two_confirmations_are_two_messages_in_order() {
        let mut mailer = FakeMailer::new();
        assert_eq!(
            send_confirmation(&mut mailer, "a@example.test", "tok-1"),
            Ok(())
        );
        assert_eq!(
            send_confirmation(&mut mailer, "b@example.test", "tok-2"),
            Ok(())
        );
        let subjects: Vec<&str> = mailer.sent().iter().map(|m| m.to.as_str()).collect();
        assert_eq!(subjects, ["a@example.test", "b@example.test"]);
        assert!(mailer.sent()[1].body.contains("tok-2"));
    }

    #[test]
    fn the_token_is_inserted_verbatim() {
        let mut mailer = FakeMailer::new();
        assert_eq!(
            send_confirmation(&mut mailer, "a@example.test", "tok.with-dash_1"),
            Ok(())
        );
        assert!(
            mailer.sent()[0].body.ends_with("token=tok.with-dash_1"),
            "{:?}",
            mailer.sent()
        );
    }

    #[test]
    fn a_failing_fake_refuses_every_send() {
        let mut mailer = FakeMailer::failing("smtp is down");
        assert!(send_confirmation(&mut mailer, "a@example.test", "tok-1").is_err());
        assert!(send_confirmation(&mut mailer, "b@example.test", "tok-2").is_err());
        assert!(mailer.sent().is_empty());
    }

    #[test]
    fn the_use_case_does_not_know_the_fake() {
        #[derive(Default)]
        struct CountingMailer {
            calls: usize,
        }
        impl Mailer for CountingMailer {
            fn send(&mut self, _message: Message) -> Result<(), String> {
                self.calls += 1;
                Ok(())
            }
        }

        let mut mailer = CountingMailer::default();
        assert_eq!(
            send_confirmation(&mut mailer, "a@example.test", "tok-1"),
            Ok(())
        );
        assert_eq!(mailer.calls, 1);
    }
}
