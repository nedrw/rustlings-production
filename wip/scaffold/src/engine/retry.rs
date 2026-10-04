//! Retrying what the database refused, with a bound.
//!
//! Two writers touching the same rows do not both need to be told no: one of them is told *again*.
//! PostgreSQL says it in a class of its own — `40001` (a serialisation failure under an isolation
//! level that holds a snapshot) and `40P01` (a deadlock) — and that class is a failure another
//! attempt can change, which is exactly what [`Error::is_retryable`] answers. Step
//! `jobq/3.6-transaction-isolation` is where the engine gains both the class
//! (`Error::Conflict`, raised by the durable backend) and this: the loop that acts on it.
//!
//! Why a helper rather than a `loop` at each call site:
//!
//! * **the bound is the point.** Retrying without a limit turns one contended row into a process
//!   that never finishes; retrying zero times turns a transient refusal into a failed job. The
//!   number is a parameter, the default is [`DEFAULT_ATTEMPTS`], and the *closure* is what gets
//!   repeated — so the caller writes the transaction once and the retry is not a second copy of it;
//! * **the caller says what is retried, the engine says when.** The predicate is the engine's own
//!   classification ([`Error::is_retryable`]), so a caller cannot accidentally retry a rejected
//!   payload, and a failure the engine has not classified as retryable stops at one attempt —
//!   which is why a *closure* is the right shape: an operation that can fail twice for two
//!   different reasons is retried only for the reason that is worth it;
//! * **the answer says how many attempts it took.** A settle that converged on the second try is a
//!   different story from one that never lost a race, and it is the number a log line or a counter
//!   needs; [`Retried::attempts`] is where it is kept.
//!
//! What this deliberately is *not*: a backoff. How long to wait between attempts — if at all —
//! is a policy that needs a clock the engine does not own yet (`jobq/3.8-clock-injection` puts one
//! behind an interface), and a `sleep` written here would make every test that exercises this file
//! slower than the thing it tests. The bound is the whole of the policy for now.

use std::future::Future;

use crate::engine::error::Error;

/// How many attempts a retryable operation gets when the caller does not say.
///
/// Three, which is the same shape the engine gives a job: enough to absorb a deadlock with a second
/// writer, few enough that a queue which has stopped making progress fails loudly instead of
/// spinning.
pub const DEFAULT_ATTEMPTS: u32 = 3;

/// What a retried operation ended with: its answer, and how many attempts it took.
///
/// Both halves are the caller's business: the value is why the operation was run, and the count is
/// what an operator reads when a queue starts retrying more than it used to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retried<T> {
    /// What the operation answered.
    value: T,
    /// How many times the closure was called: `1` when it succeeded first time.
    attempts: u32,
}

impl<T> Retried<T> {
    /// The answer.
    #[must_use]
    pub fn value(self) -> T {
        self.value
    }

    /// How many attempts it took, counting the one that worked.
    #[must_use]
    pub fn attempts(&self) -> u32 {
        self.attempts
    }
}

/// Run `operation`, and run it again while it fails with an error the engine calls retryable.
///
/// `attempts` is the number of tries, and it is read as *at least one*: an operation nobody runs has
/// no answer to give, and `retry(0, …)` silently returning the closure's own future unpolled would
/// be a much worse reading of the same sentence.
///
/// The closure is `FnMut` and is called once per attempt — a future that has resolved cannot be
/// polled again, so "retrying the future" is not a thing, and "calling the closure again" is what
/// a retry actually is. Whatever the closure does per attempt (opening a transaction, taking a
/// connection) therefore happens again on the retry, which is the point.
///
/// # Errors
///
/// The last failure, unchanged. A caller that needs to know how many attempts were spent before
/// giving up counts its own calls (the closure is called once per attempt); replacing the failure
/// with "I tried three times" would throw away the only sentence in the error that says what went
/// wrong.
pub async fn retry<T, F, Fut>(attempts: u32, mut operation: F) -> Result<Retried<T>, Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{
    let _ = attempts;
    let _ = operation;
    todo!(
        "jobq/3.6-transaction-isolation: call the closure, and call it again while the failure is one another attempt could change"
    )
}
