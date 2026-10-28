//! probe = "test/chaos-injection@apply"
//! title = "The failure is an input, not an accident"
//! hint = "A scripted dependency hands out delay, reset and status in a fixed order: the client's deadline, retry and recording are then asserted deterministically."
//!
//! Step `relay/2.17-chaos-injection` makes the failures testable by scripting them: a proxy (or a
//! fake) that delays, resets or answers 5xx on request. The alternative is hoping the real network
//! misbehaves on the day the test runs — which is how a retry policy ships untested.
//!
//! The script here is a `VecDeque` of faults, because a probe runs offline and in seconds. The
//! delays are **ticks**, not seconds: a probe that waits on the clock flakes, and the decision
//! ("this delay is over budget") is the part worth re-checking.
//!
//! ## What you write
//!
//! [`call`] — one call against the scripted dependency, with a budget in ticks:
//!
//! * the next fault in the script is consumed; a call with nothing scripted is
//!   [`Outcome::Ok`];
//! * `Delay(ticks)` within the budget → `Ok`; over the budget → [`Outcome::TimedOut`] (the client's
//!   deadline fired);
//! * `Reset` and `Status(code)` → [`Outcome::Failed`] with a sentence naming it (`connection reset`,
//!   `status 500`).
//!
//! ## What the tests ask
//!
//! Nothing scripted; a delay inside and outside the budget; a reset; a 500; and a script of three
//! faults consumed in order.

use std::collections::VecDeque;

/// What the dependency does on the next call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Nothing: the call succeeds.
    None,
    /// Takes this many ticks before answering.
    Delay(u32),
    /// Drops the connection.
    Reset,
    /// Answers with this status code.
    Status(u16),
}

/// What one call produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The call succeeded within the budget.
    Ok,
    /// The delay was over the budget; the client gave up.
    TimedOut,
    /// The dependency failed, with the reason.
    Failed(String),
}

/// The scripted dependency.
#[derive(Debug, Default)]
pub struct Dependency {
    scripted: VecDeque<Fault>,
}

impl Dependency {
    /// A dependency that succeeds every time.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Script the next calls, in order.
    #[must_use]
    pub fn script(mut self, faults: &[Fault]) -> Self {
        self.scripted.extend(faults.iter().copied());
        self
    }
}

/// Make one call with a budget in ticks.
pub fn call(dependency: &mut Dependency, budget: u32) -> Outcome {
    let fault = dependency.scripted.pop_front().unwrap_or(Fault::None);
    match fault {
        Fault::None => Outcome::Ok,
        // The budget is ticks, not seconds: what is re-checked is the decision, not the clock.
        Fault::Delay(ticks) if ticks <= budget => Outcome::Ok,
        Fault::Delay(_) => Outcome::TimedOut,
        Fault::Reset => Outcome::Failed("connection reset".to_owned()),
        Fault::Status(code) => Outcome::Failed(format!("status {code}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_scripted_is_a_success() {
        let mut dependency = Dependency::new();
        assert_eq!(call(&mut dependency, 10), Outcome::Ok);
    }

    #[test]
    fn a_delay_inside_the_budget_succeeds() {
        let mut dependency = Dependency::new().script(&[Fault::Delay(5)]);
        assert_eq!(call(&mut dependency, 10), Outcome::Ok);
    }

    #[test]
    fn a_delay_over_the_budget_times_out() {
        let mut dependency = Dependency::new().script(&[Fault::Delay(11)]);
        assert_eq!(call(&mut dependency, 10), Outcome::TimedOut);
    }

    #[test]
    fn a_reset_is_a_failure_with_a_reason() {
        let mut dependency = Dependency::new().script(&[Fault::Reset]);
        assert_eq!(
            call(&mut dependency, 10),
            Outcome::Failed("connection reset".to_owned())
        );
    }

    #[test]
    fn a_status_is_a_failure_with_the_code() {
        let mut dependency = Dependency::new().script(&[Fault::Status(500)]);
        assert_eq!(
            call(&mut dependency, 10),
            Outcome::Failed("status 500".to_owned())
        );
    }

    #[test]
    fn the_script_is_consumed_in_order() {
        let mut dependency =
            Dependency::new().script(&[Fault::Reset, Fault::None, Fault::Status(503)]);
        assert!(matches!(call(&mut dependency, 10), Outcome::Failed(_)));
        assert_eq!(call(&mut dependency, 10), Outcome::Ok);
        assert_eq!(
            call(&mut dependency, 10),
            Outcome::Failed("status 503".to_owned())
        );
    }
}
