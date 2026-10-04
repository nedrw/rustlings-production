//! probe = "domain/state-machine@apply"
//! title = "Illegal moves are values, not silence"
//! hint = "One total function on (state, event): the four states, the four events, and a named error for every move that is not allowed — a terminal job answers every event with the same refusal."
//!
//! Step `jobq/3.14-job-state-machine` writes down what may happen to a job. The rule that makes it
//! worth writing down: **the row's `state` column is an encoding of the model, never the model
//! itself.** A string in a database column can hold anything — including the name of a state this
//! build has never heard of — so the model is an enum, the column is a spelling of it, and a
//! spelling nobody recognises is a defect the operator has to see rather than a value to default.
//!
//! The other half is [`apply`]: one total function on `(state, event)`. Total means every pair has
//! an answer, and the answer for a move that is not allowed is a **named refusal** rather than a
//! panic, a silent no-op or an `unwrap` — because the caller that reaches it is a worker with a
//! message from the queue, and "the job is already finished" is a thing an operator has to be able
//! to read.
//!
//! The four states are the engine's own ([`State`]), and the transitions are the engine's policy:
//!
//! | from | event | to |
//! | --- | --- | --- |
//! | `Queued` | `Claim` | `Running` |
//! | `Running` | `Complete` | `Succeeded` |
//! | `Running` | `Fail { retryable: true }` | `Queued` — the attempt failed and there are attempts left |
//! | `Running` | `Fail { retryable: false }` | `Failed` |
//! | `Running` | `LeaseExpire` | `Queued` — the worker is gone, the job is owed again |
//! | `Succeeded`, `Failed` | anything | refused: nothing happens after the end |
//!
//! Everything else is refused, and the refusal says which of the three reasons it is: the job is
//! finished, the event needed it to be running, or the event needed it to be waiting.
//!
//! ## What you write
//!
//! * [`apply`] — the transition function: total, with a named [`Illegal`] for the rest;
//! * [`State::is_terminal`] — `Succeeded` and `Failed`, the two states a job never leaves;
//! * [`State::as_str`] and [`State::parse`] — the encoding, and its refusal.
//!
//! [`State`], [`Event`], [`Illegal`], [`UnknownState`] and the two enumerations [`STATES`] and
//! [`EVENTS`] are given: the vocabulary is what the rest of the engine matches on, and a probe that
//! let you invent it would not be checking the machine everybody else already speaks.
//!
//! ## What the tests ask
//!
//! Every state against every event, asserting the machine is total: no pair panics, no pair is
//! silently ignored, and the refusals are the ones the table implies rather than a general "not
//! allowed". The life of a job is walked claim, complete, and its terminal state answers every event
//! the same way. A retryable failure puts the job back where a worker can claim it; a failure out of
//! budget ends it. An expired lease is the job coming back rather than the worker losing it. And the
//! encoding round-trips every state, with a name nobody knows refused *by the text that was stored*
//! — which is the string an operator has in their hand.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies and **no database**: the row is [`State::as_str`] and nothing else.
//! The real step has a `state` column with a constraint, a store that writes it, and a spec that
//! drives both through a live queue; what a probe can re-check is the *model* — which moves exist,
//! what a refusal says, and the fact that nothing outside the enum is a state.
//!
//! Missing on purpose: nothing here counts attempts or decides `retryable` (the store does, from the
//! attempt budget), nothing transitions on a *timeout* (`db/atomic-claim@design` owns leases), and
//! there is no transition log — an audit trail is a table, and a probe has no table.

use std::fmt;

/// Where a job is. The engine's four, in the order a job moves through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    /// Waiting for a worker.
    Queued,
    /// A worker has it. It comes back to [`State::Queued`] if that worker's lease expires.
    Running,
    /// Done, and no worker will ever run it again.
    Succeeded,
    /// Out of attempts. Terminal: a human decides what happens next.
    Failed,
}

/// Everything that may happen to a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Event {
    /// A worker took it.
    Claim,
    /// The handler finished it.
    Complete,
    /// The handler failed. `retryable` is the attempt budget's answer, not the state machine's.
    Fail {
        /// Whether another attempt is allowed.
        retryable: bool,
    },
    /// The worker's lease ran out: it is gone, and the job is owed again.
    LeaseExpire,
}

/// Why a move is not allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Illegal {
    /// The job is finished. Nothing happens after the end, whatever the event is.
    Terminal {
        /// The state it ended in.
        state: State,
    },
    /// The event is about a running job, and this one is not running.
    NotRunning {
        /// The state it is in instead.
        state: State,
    },
    /// The event is about a waiting job, and this one has been claimed already.
    NotQueued {
        /// The state it is in instead.
        state: State,
    },
}

/// Why a stored state name could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownState {
    /// Exactly what the row held, because that is the string an operator has to grep for.
    pub text: String,
}

/// Every state, for the callers that have to be exhaustive rather than optimistic.
pub const STATES: [State; 4] = [
    State::Queued,
    State::Running,
    State::Succeeded,
    State::Failed,
];

/// Every event, both spellings of [`Event::Fail`] included.
pub const EVENTS: [Event; 5] = [
    Event::Claim,
    Event::Complete,
    Event::Fail { retryable: true },
    Event::Fail { retryable: false },
    Event::LeaseExpire,
];

impl State {
    /// The name stored in the `state` column and printed by the CLI.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        todo!()
    }

    /// Read a name out of the row.
    ///
    /// # Errors
    ///
    /// [`UnknownState`] for anything but the four names. Refused rather than defaulted: a value the
    /// model does not know is a bug or a version skew, and rounding it down to `queued` is how a
    /// job that finished is run a second time.
    pub fn parse(text: &str) -> Result<Self, UnknownState> {
        todo!()
    }

    /// `true` when no worker will ever run this job again.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        todo!()
    }
}

impl fmt::Display for State {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What happens to a job when `event` occurs while it is in `state`.
///
/// Total on purpose: every state and every event has an answer, and the answer for a move the
/// machine does not allow is an [`Illegal`] naming the reason. A caller that has to handle three
/// distinct refusals cannot accidentally treat "already finished" as "somebody else has it".
///
/// # Errors
///
/// [`Illegal`], as the table in this file's header says.
pub fn apply(state: State, event: Event) -> Result<State, Illegal> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_and_every_event_has_an_answer() {
        for state in STATES {
            for event in EVENTS {
                // The call itself is the assertion: a `todo!()`, an `unreachable!()` or a panic on
                // a pair nobody thought about is exactly what totality is for.
                let answer = apply(state, event);
                if let Ok(next) = answer {
                    assert!(
                        STATES.contains(&next),
                        "{state} + {event:?} answered with a state that is not one of the four"
                    );
                }
            }
        }
    }

    #[test]
    fn a_terminal_job_refuses_everything_the_same_way() {
        for state in [State::Succeeded, State::Failed] {
            assert!(state.is_terminal());
            for event in EVENTS {
                assert_eq!(
                    apply(state, event),
                    Err(Illegal::Terminal { state }),
                    "{state} is the end of the job: every event after it is the same refusal, not a \
                     different one"
                );
            }
        }
        for state in [State::Queued, State::Running] {
            assert!(!state.is_terminal(), "{state} is not the end of anything");
        }
    }

    #[test]
    fn the_only_move_out_of_the_queue_is_a_claim() {
        for event in EVENTS {
            let answer = apply(State::Queued, event);
            if event == Event::Claim {
                assert_eq!(answer, Ok(State::Running));
            } else {
                assert_eq!(
                    answer,
                    Err(Illegal::NotRunning {
                        state: State::Queued
                    }),
                    "a job that has not been claimed cannot be completed, failed or have a lease \
                     expire: {event:?}"
                );
            }
        }
    }

    #[test]
    fn a_claimed_job_can_be_finished_but_not_claimed_again() {
        for event in EVENTS {
            let answer = apply(State::Running, event);
            match event {
                Event::Claim => assert_eq!(
                    answer,
                    Err(Illegal::NotQueued {
                        state: State::Running
                    }),
                    "a second claim on a running job is not a transition: it is two workers holding \
                     one job, and the store is what refuses it"
                ),
                _ => assert!(answer.is_ok(), "{event:?} is a move a running job can make"),
            }
        }
    }

    #[test]
    fn the_life_of_a_job_is_claim_then_finish() {
        let mut state = State::Queued;
        state = apply(state, Event::Claim).expect("a queued job can be claimed");
        assert_eq!(state, State::Running);
        state = apply(state, Event::Complete).expect("a running job can be completed");
        assert_eq!(state, State::Succeeded);
        assert!(state.is_terminal());
        assert_eq!(
            apply(state, Event::Complete),
            Err(Illegal::Terminal { state }),
            "completing a completed job is not idempotence, it is a caller that did not look"
        );
    }

    #[test]
    fn a_failure_goes_back_to_the_queue_or_out_of_it() {
        // Attempts left: the job is owed again, and the very next move may be a claim.
        let queued = apply(State::Running, Event::Fail { retryable: true }).expect("a running job");
        assert_eq!(queued, State::Queued);
        assert_eq!(apply(queued, Event::Claim), Ok(State::Running));

        // Out of budget: terminal, and nothing gets it back. Replay is a different capability with
        // a different answer (`ops/dlq-tooling@apply`), not a fourth state.
        let failed =
            apply(State::Running, Event::Fail { retryable: false }).expect("a running job");
        assert_eq!(failed, State::Failed);
        assert_eq!(
            apply(failed, Event::Claim),
            Err(Illegal::Terminal {
                state: State::Failed
            })
        );
    }

    #[test]
    fn an_expired_lease_is_the_job_coming_back() {
        let running = apply(State::Queued, Event::Claim).expect("a queued job");
        assert_eq!(apply(running, Event::LeaseExpire), Ok(State::Queued));
        assert_eq!(
            apply(State::Queued, Event::LeaseExpire),
            Err(Illegal::NotRunning {
                state: State::Queued
            }),
            "a job with no worker cannot lose one"
        );

        // The job that comes back is the *same* job: identity lives outside the enum, which is why
        // nothing in this model carries an id.
        let again = apply(running, Event::LeaseExpire).expect("a running job");
        assert_eq!(again, State::Queued);
        assert_eq!(apply(again, Event::Claim), Ok(State::Running));
    }

    #[test]
    fn the_row_is_an_encoding_of_the_model_and_an_unknown_name_is_refused() {
        for state in STATES {
            assert_eq!(
                State::parse(state.as_str()),
                Ok(state),
                "every state the model has, the column can hold"
            );
        }

        let names: Vec<&str> = STATES.iter().map(|state| state.as_str()).collect();
        assert_eq!(
            names,
            vec!["queued", "running", "succeeded", "failed"],
            "the names are an interface: the CLI prints them, the column stores them and a \
             dashboard groups by them, so a rename is a migration rather than a refactor"
        );

        // A name nobody knows is refused *by the text*, because that text is what the operator has.
        assert_eq!(
            State::parse("dead"),
            Err(UnknownState {
                text: "dead".to_owned()
            }),
            "a state this build does not know is a defect to see, not a value to round down to \
             `queued`"
        );
        assert_eq!(
            State::parse("Queued"),
            Err(UnknownState {
                text: "Queued".to_owned()
            }),
            "the encoding is lower case, and case is part of the interface"
        );
        assert!(State::parse("").is_err());
    }
}
