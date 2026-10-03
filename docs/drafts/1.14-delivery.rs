// DRAFT for step `mailer/1.14-worker` — not compiled by anything.
//
// This is the working tree's `app/mailer/src/delivery.rs` as of the eighth chunk: the bounded queue,
// its worker, the retry policy and the unit tests are written and green, and what is left is the
// wiring the handoff lists (permit into `subscribe`, the shutdown twins of `serve`/`serve_on`,
// `main.rs`'s signal handling, the step's spec, patches and body). It lives here because `app/` is not
// replicated between worktrees: copy it back to `app/mailer/src/delivery.rs` to continue.
//
// The queue's `Cargo.toml` counterpart (already applied in the tree) is the `test-util` feature on
// `tokio`, which is what makes the paused clock available to the retry tests.

//! The delivery queue: one bounded queue, one worker, and what happens when either runs out
//! (step `mailer/1.14-worker`).
//!
//! Step `mailer/1.13-email-port` gave the service a place to put a message. This step is about
//! *when*: sending inside `POST /signup` makes the caller wait for a mail server, and it makes a
//! mail server that is slow into a service that is slow. So the request hands the message to a
//! queue and answers, and one worker task does the sending.
//!
//! Four decisions, and each of them is a way the queue could have been written wrong:
//!
//! * **the queue is bounded** ([`QUEUE_CAPACITY`]). An unbounded queue turns "the mail server is
//!   away" into "this process ran out of memory", and the failure shows up somewhere else entirely
//!   — which is why the *request* is what gets refused (a `503`), not the process;
//! * **capacity is reserved before the work it is for** ([`Queue::reserve`]): the handler takes its
//!   seat *before* the insert, so a full queue refuses a signup that has not written anything yet
//!   (a refused client can try again; a subscriber that exists with nobody able to confirm it
//!   cannot be fixed by trying again). The `Permit` that comes back is also the cancel-safe way to
//!   send: a `send` cancelled inside a `select!` loses the message, a permit does not;
//! * **a failed send is retried, with jitter** ([`backoff`]): N workers whose mail server just
//!   recovered must not come back in step, which is what makes the delay random *and* increasing;
//! * **stopping means stopping taking work, then draining** ([`Queue::drain`]): the worker is told
//!   to close the queue to new messages and finish what it already has, and the caller waits for
//!   that — at most [`DRAIN_DEADLINE`], because a process that will not stop is a process an
//!   orchestrator kills, and being killed is how messages are lost.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::mpsc::{self, Permit};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::email::{EmailSender, Message, SendError};
use crate::error::Error;

/// How many messages may wait for the worker before a request is refused.
///
/// A bound is the point of a queue in a service: it is a promise about how much work this process
/// has taken on that it has not finished yet, and it is what turns backpressure into an answer a
/// caller can act on instead of memory growth nobody is watching.
pub const QUEUE_CAPACITY: usize = 64;

/// How many times one message is handed to the sender before it is given up on.
pub const MAX_ATTEMPTS: u32 = 3;

/// The wait before the second attempt: the first delay, before jitter and doubling.
const RETRY_BASE: Duration = Duration::from_millis(250);

/// The longest wait between two attempts. A cap is what keeps a retry from outliving the process's
/// own shutdown: the drain has a deadline, and a message sleeping for thirty minutes would sleep
/// through it.
const RETRY_CAP: Duration = Duration::from_secs(30);

/// How long the worker is given to finish what it already has, once the process is asked to stop.
pub const DRAIN_DEADLINE: Duration = Duration::from_secs(10);

/// A queue with one worker behind it.
///
/// Cloneable (it is a handle, like the database pool): every clone sends into the same queue and
/// they all share one worker, so a handler cannot accidentally grow a second one. Cloning does not
/// give a caller a second `drain` to race with — the first one to take the worker's handle is the
/// one that waits for it.
#[derive(Debug, Clone)]
pub struct Queue {
    /// The sending half of the channel: where a reserved seat turns into a queued message.
    jobs: mpsc::Sender<Message>,
    /// Set when the process is asked to stop: the worker stops accepting and finishes its buffer.
    stop: watch::Sender<bool>,
    /// The worker task, taken once by whoever drains the queue.
    worker: Arc<Mutex<Option<JoinHandle<usize>>>>,
}

impl Queue {
    /// Start the worker and answer a handle to the queue in front of it.
    ///
    /// The worker owns the receiving half for its whole life, so there is nothing to hand it later
    /// and no way for a second one to appear.
    #[must_use]
    pub fn start(sender: Arc<dyn EmailSender>) -> Self {
        let (jobs, mut waiting) = mpsc::channel::<Message>(QUEUE_CAPACITY);
        let (stop, mut stopping) = watch::channel(false);

        let worker = tokio::spawn(async move {
            let mut delivered = 0_usize;
            loop {
                tokio::select! {
                    // `changed()` fires once per change and then waits for the next one, so the
                    // close below happens exactly once and the loop keeps draining afterwards.
                    changed = stopping.changed() => {
                        if changed.is_err() || *stopping.borrow() {
                            // Stop taking new work, keep what is already queued: `recv` answers
                            // `None` once the buffer is empty, which is how this loop ends.
                            waiting.close();
                        }
                    }
                    message = waiting.recv() => {
                        let Some(message) = message else {
                            break;
                        };
                        match deliver(sender.as_ref(), &message).await {
                            Ok(()) => delivered += 1,
                            Err(error) => {
                                error!(%error, to = %message.to, "giving up on an email");
                            }
                        }
                    }
                }
            }
            info!(delivered, "the delivery queue drained");
            delivered
        });

        Self {
            jobs,
            stop,
            worker: Arc::new(Mutex::new(Some(worker))),
        }
    }

    /// Reserve room for one message, or refuse the caller that wanted it.
    ///
    /// The seat is taken *before* the work the message is about, which is what makes "the queue is
    /// full" an answer a caller can act on: they are told to come back, and nothing was written for
    /// them in the meantime.
    ///
    /// # Errors
    ///
    /// Reports [`Error::QueueFull`] when [`QUEUE_CAPACITY`] messages are already waiting.
    pub fn reserve(&self) -> Result<Permit<'_, Message>, Error> {
        self.jobs.try_reserve().map_err(|_| Error::QueueFull)
    }

    /// Stop taking work, finish what is queued, and answer how many messages were delivered.
    ///
    /// # Errors
    ///
    /// Reports [`Error::DrainTimeout`] when the worker is still busy after `deadline`, and
    /// [`Error::WorkerLost`] when the worker task itself is gone — neither of which is a caller's
    /// problem, and both of which mean some messages may not have been delivered.
    pub async fn drain(self, deadline: Duration) -> Result<usize, Error> {
        // The worker watches this; a failure only means it is already gone, which the join below
        // reports.
        let _ = self.stop.send(true);
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(worker) = worker else {
            return Ok(0);
        };
        match tokio::time::timeout(deadline, worker).await {
            Ok(Ok(delivered)) => Ok(delivered),
            Ok(Err(lost)) => Err(Error::WorkerLost {
                source: Box::new(lost),
            }),
            Err(_elapsed) => Err(Error::DrainTimeout),
        }
    }
}

/// Hand one message to the sender, retrying the failures that are worth retrying.
async fn deliver(sender: &dyn EmailSender, message: &Message) -> Result<(), SendError> {
    let mut attempt = 1_u32;
    loop {
        match sender.send(message.clone()).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                if attempt >= MAX_ATTEMPTS {
                    return Err(error);
                }
                let wait = backoff(attempt);
                warn!(%error, attempt, ?wait, "the email could not be sent; trying again");
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
        }
    }
}

/// The wait before the next attempt: exponential, capped, and jittered.
///
/// *Full jitter*, in the shape of the AWS backoff article: a random delay in
/// `[0, min(cap, base · 2^attempt))`, not a fixed one. The randomisation is the point — a fleet of
/// workers retrying one broken mail server must not come back in step, and the failures they are
/// retrying are usually the same failure.
///
/// The randomness comes from the same place a confirmation token's does. A failed generator falls
/// back to the ceiling: a delay that is not random is still a delay, and refusing to retry because
/// the system cannot shuffle is worse than retrying too neatly.
#[must_use]
pub fn backoff(attempt: u32) -> Duration {
    let step = RETRY_BASE.saturating_mul(1_u32 << attempt.min(16));
    let ceiling = step.min(RETRY_CAP);
    let millis = u64::try_from(ceiling.as_millis()).unwrap_or(u64::MAX);
    let mut bytes = [0_u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        return ceiling;
    }
    let roll = u128::from(u64::from_le_bytes(bytes));
    // In `u128`, because the interesting values overflow `u64` immediately: `millis * roll` for a
    // full-width roll is a number that does not fit, and a saturating multiply answers the same
    // millisecond every time — which is a jitter that is not one (the test below is what found it).
    let scaled = u128::from(millis).saturating_mul(roll) / u128::from(u64::MAX);
    Duration::from_millis(u64::try_from(scaled).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::{DRAIN_DEADLINE, MAX_ATTEMPTS, Queue, RETRY_BASE, RETRY_CAP, backoff};
    use crate::domain::Email;
    use crate::email::{EmailSender, Message, SendError};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    fn message() -> Message {
        Message {
            to: Email::parse("ada@example.com").expect("the fixture address is valid"),
            subject: "Confirm your subscription".to_owned(),
            body: "Confirm your subscription by opening the link.".to_owned(),
        }
    }

    /// A sender that fails a fixed number of times and then succeeds.
    #[derive(Debug)]
    struct Flaky {
        failures: u32,
        attempts: AtomicU32,
    }

    impl Flaky {
        fn failing(times: u32) -> Self {
            Self {
                failures: times,
                attempts: AtomicU32::new(0),
            }
        }

        fn attempts(&self) -> u32 {
            self.attempts.load(Ordering::Relaxed)
        }
    }

    impl EmailSender for Flaky {
        fn send(
            &self,
            _message: Message,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), SendError>> + Send + '_>>
        {
            Box::pin(async move {
                let attempt = self.attempts.fetch_add(1, Ordering::Relaxed) + 1;
                if attempt <= self.failures {
                    return Err(SendError::new(std::io::Error::other(
                        "the mail server is away",
                    )));
                }
                Ok(())
            })
        }
    }

    /// A sender that never answers: the mail server that accepts a connection and says nothing.
    #[derive(Debug)]
    struct Hanging;

    impl EmailSender for Hanging {
        fn send(
            &self,
            _message: Message,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), SendError>> + Send + '_>>
        {
            Box::pin(std::future::pending())
        }
    }

    #[test]
    fn the_wait_is_bounded_by_the_cap_and_grows_with_the_attempt() {
        for attempt in 0..12 {
            let wait = backoff(attempt);
            let ceiling = RETRY_BASE
                .saturating_mul(1_u32 << attempt.min(16))
                .min(RETRY_CAP);
            assert!(
                wait <= ceiling,
                "attempt {attempt} waited {wait:?}, past its ceiling {ceiling:?}"
            );
            assert!(
                wait < Duration::from_millis(0) || wait <= RETRY_CAP,
                "a wait has to fit inside the cap"
            );
        }
        assert!(
            RETRY_BASE.saturating_mul(1_u32 << 12).min(RETRY_CAP) == RETRY_CAP,
            "the fixture has to reach the cap, or this test proves nothing about it"
        );
    }

    #[test]
    fn the_wait_is_not_the_same_every_time() {
        let waits: Vec<Duration> = (0..64).map(|_| backoff(2)).collect();
        let distinct: std::collections::BTreeSet<Duration> = waits.iter().copied().collect();
        assert!(
            distinct.len() > 1,
            "jitter that always answers the same value is not jitter: {waits:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_send_is_retried_and_then_given_up_on() {
        let retried = Flaky::failing(MAX_ATTEMPTS - 1);
        let queue = Queue::start(Arc::new(retried));
        let seat = queue.reserve().expect("an empty queue has room");
        seat.send(message());
        let delivered = queue.drain(DRAIN_DEADLINE).await.expect("the queue drains");
        assert_eq!(delivered, 1, "the message arrived after its retries");

        let hopeless = Arc::new(Flaky::failing(u32::MAX));
        let queue = Queue::start(hopeless.clone());
        let seat = queue.reserve().expect("an empty queue has room");
        seat.send(message());
        let delivered = queue
            .drain(DRAIN_DEADLINE)
            .await
            .expect("a message that cannot be sent must not hold the drain open");
        assert_eq!(delivered, 0, "nothing was delivered");
        assert_eq!(
            hopeless.attempts(),
            MAX_ATTEMPTS,
            "a send that keeps failing is attempted exactly {MAX_ATTEMPTS} times, not forever"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_queue_refuses_instead_of_waiting() {
        let queue = Queue::start(Arc::new(Hanging));
        let mut seats = Vec::new();
        for _ in 0..super::QUEUE_CAPACITY {
            seats.push(queue.reserve().expect("the queue has room to fill"));
        }
        assert!(
            queue.reserve().is_err(),
            "a queue that is full has to say so, not wait for room that may never come"
        );
        // The seats are dropped here, which returns the capacity: a caller that took a seat and did
        // not send anything (a signup that lost a race, say) must not cost the queue a slot.
        drop(seats);
        assert!(queue.reserve().is_ok(), "a returned seat is room again");
    }

    #[tokio::test(start_paused = true)]
    async fn draining_gives_up_on_the_deadline_and_leaves_the_worker_behind() {
        let queue = Queue::start(Arc::new(Hanging));
        let seat = queue.reserve().expect("an empty queue has room");
        seat.send(message());
        let drained = queue.drain(Duration::from_millis(10)).await;
        assert!(
            drained.is_err(),
            "a worker stuck on a message must not drain quietly: {drained:?}"
        );
    }
}
