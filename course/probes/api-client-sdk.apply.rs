//! probe = "api/client-sdk@apply"
//! title = "One key per call, one loop, and an error type a caller can act on"
//! hint = "Draw the idempotency key once before the first attempt, follow a redirect, retry an unavailability with that same key, stop on a refusal, and give up at `MAX_ATTEMPTS` instead of spinning."
//!
//! Step `meshkv/5.11-client-sdk` is where the track's rules become a product. The leader (`5.7`),
//! the idempotency key (`5.8`), the state machine's digest (`5.10`) are all rules a *caller* has to
//! obey, and if the client is an example in a README then every caller writes its own version of
//! them, and one of them forgets the key. The SDK is where the rules live inside the API.
//!
//! Four decisions, and each one is a thing a caller stops having to remember:
//!
//! 1. **the key is drawn once per call, before the first attempt.** Every attempt is then the *same*
//!    write, which is what makes a retry safe; a key per attempt would turn the retry into a second
//!    write, and the store would apply both. The caller never sees the key, so the caller cannot
//!    forget it;
//! 2. **the error type is the contract.** `NotLeader` (retry there) and `Unavailable` (retry later)
//!    are retryable; `Refused` (this will not change) and `RetriesExhausted` (the SDK already gave
//!    up) are not. [`SdkError::is_retryable`] is that table in one place, and the loop reads the same
//!    table the caller does — a caller that cannot tell them apart cannot behave correctly;
//! 3. **the bound counts attempts, and the last allowed one is used.** A cluster that never names a
//!    leader ends at `RetriesExhausted { attempts: MAX_ATTEMPTS }`, not in a spin, and a cluster that
//!    answers on the very last attempt is a success. The number is the SDK's, so no caller chooses it;
//! 4. **a write and a read follow one policy.** Following a redirect is one loop, and both operations
//!    go through it, so a fix to the loop is a fix to both — the caller sees only the answer.
//!
//! ## What you write
//!
//! * [`SdkError::is_retryable`] — the table, from the error alone;
//! * `Sdk::exchange` — the loop: draw the key for a write, send one attempt, follow a redirect, retry
//!   an unavailability with the same key, return a refusal, and stop at `MAX_ATTEMPTS` with the
//!   SDK's own verdict.
//!
//! [`Reply`], [`Answer`], [`Request`], the [`Backend`] trait, [`Keys`] and the [`Sdk`] accessors are
//! given, and [`MAX_ATTEMPTS`] with them. The backend is a trait because the SDK's subject is the
//! *policy* — keys, retries, errors — not the transport: a probe cannot run a socket, so the tests
//! bring an implementation that answers from a script, and the SDK cannot tell it from a cluster.
//!
//! ## What the tests ask
//!
//! A write sent to a follower follows the redirect, is answered with the position it was applied at,
//! and leaves the SDK pointed at the leader for the next call. One call that needs three attempts
//! carries **one** key on all three, and the next call carries a different one. A refusal — whether
//! the store said it or the backend failed for a reason retrying cannot fix — is returned after one
//! attempt, and the answer the script held for a retry is never asked for. The bound is tested from
//! both sides: `MAX_ATTEMPTS - 1` redirects and then a leader is a success that took every attempt,
//! and one answer more than the bound allows is never asked for at all. Two unavailabilities and then
//! the leader is a success on the last attempt, with the same key on all three. A read goes through
//! the same loop without a key of its own. And the error table: two retryable answers, two that are
//! the end of the call.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no sockets and no leader election**: the backend is a script of
//! answers, so there is no HTTP status to classify, no backoff and no sleeping, and no cluster that
//! actually elects somebody. The real `conformance@2` suite runs this contract against two
//! implementations of the port — a scripted table and the in-process cluster —
//! which is `db/port-conformance@apply`'s habit; the store's side of the key is
//! `delivery/idempotency@apply`'s file, and the *wait* between two attempts is
//! `delivery/retry-budget@design`'s. What is re-checked here is the part every caller would
//! otherwise rewrite: which failures are retryable, which key a retry carries, and where the loop
//! stops.
//!
//! Missing on purpose: the key is a counter rather than a session-scoped UUID, because the tests have
//! to see that two calls draw two keys; and nothing here keeps a key across a process restart, which
//! is the session's job (`5.8`) rather than the SDK's.

use std::fmt;

/// How many attempts one call may take before the SDK stops.
pub const MAX_ATTEMPTS: usize = 3;

/// Where the SDK's idempotency keys come from: one per call, and a fresh one for the next.
#[derive(Debug, Default)]
pub struct Keys {
    next: u64,
}

impl Keys {
    /// A source that has handed nothing out yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The key for one call. The SDK calls this **once**, before the first attempt: every attempt is
    /// then the same write, which is what makes a retry safe.
    pub fn next(&mut self) -> String {
        self.next += 1;
        format!("k{}", self.next)
    }
}

/// What a caller has to be able to tell apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdkError {
    /// The node that answered is not the leader; the one it names may be.
    NotLeader {
        /// The leader the node named, when it named one.
        leader: Option<String>,
    },
    /// The node could not be reached, or answered something the SDK cannot classify.
    Unavailable {
        /// What went wrong, phrased for a caller who cannot fix it.
        detail: String,
    },
    /// The write was refused: retrying will not help.
    Refused {
        /// Why the store refused it.
        reason: String,
    },
    /// The SDK gave up after [`MAX_ATTEMPTS`] attempts.
    RetriesExhausted {
        /// How many attempts were made.
        attempts: usize,
        /// The last thing the SDK heard.
        last: String,
    },
}

impl SdkError {
    /// `true` when asking again might succeed.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        todo!()
    }
}

impl fmt::Display for SdkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLeader {
                leader: Some(leader),
            } => write!(f, "not the leader: {leader}"),
            Self::NotLeader { leader: None } => f.write_str("not the leader, and none was named"),
            Self::Unavailable { detail } => write!(f, "the cluster is unavailable: {detail}"),
            Self::Refused { reason } => write!(f, "the write was refused: {reason}"),
            Self::RetriesExhausted { attempts, last } => {
                write!(f, "gave up after {attempts} attempt(s): {last}")
            }
        }
    }
}

/// What one call to a backend answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// The call was answered: where it sits in the log, and the value a read saw.
    Applied {
        /// The position the answer is correct at.
        index: u64,
        /// The value a read saw, if the call was a read.
        value: Option<Vec<u8>>,
    },
    /// This node is not the leader; the one it names may be.
    Redirect {
        /// The leader this node knows of.
        leader: String,
    },
    /// This node knows of no leader yet: an election is probably running.
    NoLeader,
    /// The store refused the request, and asking again will not change that.
    Refused {
        /// Why.
        reason: String,
    },
}

/// What a call that finished answers: where a write sits, and what a read saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The position the answer is correct at.
    pub index: u64,
    /// The value, for a read.
    pub value: Option<Vec<u8>>,
}

/// One call the SDK drives to completion: a write, or a read of one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request<'a> {
    /// A write: the command bytes the state machine reads (`dist/deterministic-state-machine@apply`).
    Write(&'a [u8]),
    /// A read of one key.
    Read(&'a str),
}

/// One implementation of "talk to the cluster": the seam the SDK is written against.
///
/// A probe cannot run a transport, so the tests bring an implementation that answers from a script,
/// and the SDK cannot tell it from a cluster. That is the point of the port: `conformance@2` runs one
/// suite against two implementations, and a policy that passes on one and fails on the other depends
/// on an accident of the first.
///
/// # Errors
///
/// A failure to *reach* the node: [`SdkError::Unavailable`], which is retried, or an error that is
/// not retryable, which the SDK hands back untouched. What the store *said* is a [`Reply`], not an
/// error.
pub trait Backend {
    /// Send one write to `node`, with its idempotency key.
    fn write(&mut self, node: &str, key: &str, command: &[u8]) -> Result<Reply, SdkError>;

    /// Ask `node` for one key.
    fn read(&mut self, node: &str, key: &str) -> Result<Reply, SdkError>;
}

/// A client of the cluster, with the retry policy that makes it usable.
#[derive(Debug)]
pub struct Sdk<B> {
    backend: B,
    keys: Keys,
    node: String,
    attempts: usize,
}

impl<B: Backend> Sdk<B> {
    /// An SDK that draws its keys from `keys` and starts at `node`.
    pub fn new(backend: B, keys: Keys, node: impl Into<String>) -> Self {
        Self {
            backend,
            keys,
            node: node.into(),
            attempts: 0,
        }
    }

    /// The node the next call will try first.
    #[must_use]
    pub fn node(&self) -> &str {
        &self.node
    }

    /// How many attempts the last call took.
    #[must_use]
    pub fn attempts(&self) -> usize {
        self.attempts
    }

    /// The backend, for a test that wants to see the attempts from the other side of the port.
    #[must_use]
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Write `command`, following redirects and retrying with the same idempotency key.
    ///
    /// # Errors
    ///
    /// [`SdkError::Refused`] when the store refuses the write, and [`SdkError::RetriesExhausted`]
    /// when the attempts run out first.
    pub fn write(&mut self, command: &[u8]) -> Result<u64, SdkError> {
        Ok(self.exchange(Request::Write(command))?.index)
    }

    /// Read `key`, following redirects.
    ///
    /// # Errors
    ///
    /// Whatever the backend refuses without a retry, and [`SdkError::RetriesExhausted`] when the
    /// attempts run out.
    pub fn read(&mut self, key: &str) -> Result<Option<Vec<u8>>, SdkError> {
        Ok(self.exchange(Request::Read(key))?.value)
    }

    /// Drive one call to an answer: the one loop both operations go through.
    ///
    /// The key is drawn once, before the first attempt, so every attempt of one call is the same
    /// write. A redirect moves the SDK and is an attempt like any other; a refusal ends the call;
    /// a retryable failure spends an attempt, and the attempt after `MAX_ATTEMPTS` is not made.
    fn exchange(&mut self, request: Request<'_>) -> Result<Answer, SdkError> {
        todo!()
    }

    /// The SDK's own verdict, with the last thing it heard.
    fn exhausted(&self, last: &str) -> SdkError {
        SdkError::RetriesExhausted {
            attempts: self.attempts,
            last: last.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// One attempt, as the backend saw it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Call {
        node: String,
        /// The idempotency key for a write, the key being read for a read.
        key: String,
        /// `Some` for a write, `None` for a read.
        command: Option<Vec<u8>>,
    }

    /// A backend that answers from a script, and records every attempt it was handed.
    #[derive(Debug, Default)]
    struct Scripted {
        replies: VecDeque<Result<Reply, SdkError>>,
        calls: Vec<Call>,
    }

    impl Scripted {
        fn new(replies: Vec<Result<Reply, SdkError>>) -> Self {
            Self {
                replies: replies.into(),
                calls: Vec::new(),
            }
        }

        /// The attempts that were writes: what a test reads for the key and the bound.
        fn write_calls(&self) -> Vec<&Call> {
            self.calls
                .iter()
                .filter(|call| call.command.is_some())
                .collect()
        }

        /// The next answer, and a refusal once the script is spent: a call that made one attempt
        /// more than the script allows fails rather than hanging.
        fn next(&mut self) -> Result<Reply, SdkError> {
            self.replies.pop_front().unwrap_or_else(|| {
                Err(SdkError::Refused {
                    reason: "the script is spent: the SDK made an attempt nobody wrote".to_owned(),
                })
            })
        }
    }

    impl Backend for Scripted {
        fn write(&mut self, node: &str, key: &str, command: &[u8]) -> Result<Reply, SdkError> {
            self.calls.push(Call {
                node: node.to_owned(),
                key: key.to_owned(),
                command: Some(command.to_vec()),
            });
            self.next()
        }

        fn read(&mut self, node: &str, key: &str) -> Result<Reply, SdkError> {
            self.calls.push(Call {
                node: node.to_owned(),
                key: key.to_owned(),
                command: None,
            });
            self.next()
        }
    }

    /// A node that did not answer, which is the failure the SDK retries.
    fn unavailable() -> SdkError {
        SdkError::Unavailable {
            detail: "the node did not answer".to_owned(),
        }
    }

    #[test]
    fn a_write_that_starts_at_a_follower_follows_the_redirect() {
        let backend = Scripted::new(vec![
            Ok(Reply::Redirect {
                leader: "n1".to_owned(),
            }),
            Ok(Reply::Applied {
                index: 5,
                value: None,
            }),
        ]);
        let mut sdk = Sdk::new(backend, Keys::new(), "n2");

        assert_eq!(
            sdk.write(b"set k 1"),
            Ok(5),
            "the caller is answered with where the write sits, and the redirect is the SDK's \
             business rather than the caller's"
        );
        assert_eq!(sdk.attempts(), 2, "one redirect, then the leader");
        assert_eq!(
            sdk.node(),
            "n1",
            "the SDK remembers where the leader is, so the next call starts there instead of \
             asking the follower again"
        );
        assert_eq!(
            sdk.backend().calls.len(),
            2,
            "`attempts` is what the backend saw, not a number the SDK made up"
        );
    }

    #[test]
    fn one_call_carries_one_key_and_the_next_call_a_new_one() {
        let backend = Scripted::new(vec![
            Ok(Reply::NoLeader),
            Ok(Reply::Redirect {
                leader: "n1".to_owned(),
            }),
            Ok(Reply::Applied {
                index: 7,
                value: None,
            }),
            Ok(Reply::Applied {
                index: 8,
                value: None,
            }),
        ]);
        let mut sdk = Sdk::new(backend, Keys::new(), "n3");

        assert_eq!(sdk.write(b"set k 1"), Ok(7));
        let first = sdk.backend().write_calls().len();
        assert_eq!(
            first, 3,
            "an election and a redirect are one attempt each, and then the leader answers"
        );
        let key = sdk.backend().write_calls()[0].key.clone();
        assert!(
            !key.is_empty(),
            "the SDK draws the key, not the caller: a caller that had to build one could forget it"
        );
        for call in sdk.backend().write_calls() {
            assert_eq!(
                call.key, key,
                "every attempt of one call is the same write: a key drawn per attempt would make \
                 the retry a second write, and the store would apply both"
            );
        }

        assert_eq!(sdk.write(b"set k 2"), Ok(8));
        let writes = sdk.backend().write_calls();
        assert_eq!(writes.len(), 4);
        assert_ne!(
            writes[3].key, key,
            "a second call is a different write: a key reused across calls would let the store \
             answer one write with the other's position"
        );
    }

    #[test]
    fn a_refusal_is_the_end_of_the_call() {
        let backend = Scripted::new(vec![
            Ok(Reply::Refused {
                reason: "the compare-and-set does not match".to_owned(),
            }),
            Ok(Reply::Applied {
                index: 2,
                value: None,
            }),
        ]);
        let mut sdk = Sdk::new(backend, Keys::new(), "n1");
        match sdk.write(b"cas k other new") {
            Err(error @ SdkError::Refused { .. }) => assert!(
                !error.is_retryable(),
                "a refusal is not retryable, and a caller told otherwise would spend its patience \
                 on an answer that cannot change"
            ),
            other => panic!(
                "the store refused the write, and the SDK must say so rather than ask again: \
                 {other:?}"
            ),
        }
        assert_eq!(
            sdk.attempts(),
            1,
            "retrying a refusal is an attempt spent on an answer that will not change"
        );
        assert_eq!(
            sdk.backend().calls.len(),
            1,
            "the answer the script held for a retry was never asked for"
        );

        // The same rule one layer down: a backend that fails for a reason retrying cannot fix.
        let backend = Scripted::new(vec![
            Err(SdkError::Refused {
                reason: "the command is not one this store reads".to_owned(),
            }),
            Ok(Reply::Applied {
                index: 2,
                value: None,
            }),
        ]);
        let mut sdk = Sdk::new(backend, Keys::new(), "n1");
        match sdk.write(b"not a command") {
            Err(error @ SdkError::Refused { .. }) => assert!(
                !error.is_retryable(),
                "and it comes back as it was: the SDK does not relabel a failure it never retried"
            ),
            other => {
                panic!("a failure that asking again cannot fix is the caller's to see: {other:?}")
            }
        }
        assert_eq!(sdk.attempts(), 1);
    }

    #[test]
    fn errors_say_what_a_caller_can_do_about_them() {
        assert!(
            SdkError::NotLeader {
                leader: Some("n1".to_owned())
            }
            .is_retryable(),
            "a redirect is worth following"
        );
        assert!(
            SdkError::NotLeader { leader: None }.is_retryable(),
            "a node that names nobody is still not the leader: the answer is to ask again later, \
             not to give up"
        );
        assert!(
            SdkError::Unavailable {
                detail: "down".to_owned()
            }
            .is_retryable(),
            "a node that is down may come back"
        );
        assert!(
            !SdkError::Refused {
                reason: "no".to_owned()
            }
            .is_retryable(),
            "a refusal is the store's final answer"
        );
        assert!(
            !SdkError::RetriesExhausted {
                attempts: MAX_ATTEMPTS,
                last: "no leader".to_owned()
            }
            .is_retryable(),
            "the SDK has already given up: telling the caller to retry this call is the answer it \
             just decided against"
        );
    }

    #[test]
    fn the_bound_counts_attempts_and_the_last_one_is_used() {
        // Exactly the bound: a redirect from each of the attempts before the last, and then the
        // leader.
        let mut replies: Vec<Result<Reply, SdkError>> = (1..MAX_ATTEMPTS)
            .map(|step| {
                Ok(Reply::Redirect {
                    leader: format!("n{step}"),
                })
            })
            .collect();
        replies.push(Ok(Reply::Applied {
            index: 1,
            value: None,
        }));
        let mut sdk = Sdk::new(Scripted::new(replies), Keys::new(), "n0");
        assert_eq!(
            sdk.write(b"set k 1"),
            Ok(1),
            "the last attempt the bound allows is still an attempt: a policy that stopped one \
             short would give up on a cluster that was about to answer"
        );
        assert_eq!(sdk.attempts(), MAX_ATTEMPTS);

        // One answer more than the bound allows: it is never asked for.
        let replies = vec![Ok::<Reply, SdkError>(Reply::NoLeader); MAX_ATTEMPTS + 1];
        let mut sdk = Sdk::new(Scripted::new(replies), Keys::new(), "n1");
        match sdk.write(b"set k 1") {
            Err(SdkError::RetriesExhausted { attempts, .. }) => assert_eq!(
                attempts, MAX_ATTEMPTS,
                "the SDK's verdict names the number of attempts it spent, and the number is its \
                 own bound"
            ),
            other => panic!(
                "a cluster that never names a leader must not take the caller with it: {other:?}"
            ),
        }
        assert_eq!(
            sdk.backend().calls.len(),
            MAX_ATTEMPTS,
            "the extra answer in the script was never asked for: the call stopped at its own bound \
             rather than at the end of the script"
        );
    }

    #[test]
    fn an_unreachable_node_is_tried_again_with_the_same_key() {
        let backend = Scripted::new(vec![
            Err(unavailable()),
            Err(unavailable()),
            Ok(Reply::Applied {
                index: 9,
                value: None,
            }),
        ]);
        let mut sdk = Sdk::new(backend, Keys::new(), "n1");
        assert_eq!(
            sdk.write(b"set k 1"),
            Ok(9),
            "the attempt that never reached the store did not use the call up: the node came back \
             inside the bound"
        );
        assert_eq!(
            sdk.attempts(),
            MAX_ATTEMPTS,
            "and it took every attempt the bound allows"
        );
        let calls = sdk.backend().write_calls();
        assert_eq!(calls.len(), MAX_ATTEMPTS);
        assert!(
            calls.iter().all(|call| call.key == calls[0].key),
            "a failed attempt never reached the store, so retrying it under another key would be a \
             second write if the first one had arrived after all"
        );
        assert_eq!(
            sdk.node(),
            "n1",
            "an unreachable node has named no leader, so there is nowhere else to go"
        );
    }

    #[test]
    fn a_read_follows_the_same_policy_without_a_key_of_its_own() {
        let backend = Scripted::new(vec![
            Ok(Reply::Redirect {
                leader: "n1".to_owned(),
            }),
            Ok(Reply::Applied {
                index: 4,
                value: Some(b"1".to_vec()),
            }),
        ]);
        let mut sdk = Sdk::new(backend, Keys::new(), "n2");
        assert_eq!(
            sdk.read("k"),
            Ok(Some(b"1".to_vec())),
            "a read is answered by the leader, and the redirect is followed by the same loop that \
             follows it for a write"
        );
        assert_eq!(sdk.attempts(), 2);
        assert_eq!(sdk.node(), "n1");
        let calls = &sdk.backend().calls;
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].command, None, "a read carries no command");
        assert_eq!(
            calls[0].key, "k",
            "and it carries the key it reads rather than an idempotency key a read does not need"
        );
    }
}
