//! probe = "rpc/status-codes@apply"
//! title = "The code is the contract, and retry follows from it"
//! hint = "Map each failure to its canonical code, then answer retryable from the code: transient is retryable, the caller's mistake and the server's bug are not."
//!
//! Step `rpc/8.3-status-codes` is the small closed set a client branches on. This probe keeps the
//! same judgement with the service's failures spelled differently — the point is the mapping, not
//! the spelling.
//!
//! ## What you write
//!
//! * [`code_for`] — the canonical code for each failure;
//! * [`retryable`] — true for `Unavailable` and `ResourceExhausted`, false for the rest.
//!
//! ## What the tests ask
//!
//! Every mapping; the retryable pair; a caller's mistake; the server's bug; and that a failure
//! mapped to a non-retryable code stays that way.

/// How the service failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The caller's input is unusable.
    Invalid,
    /// The row is not there.
    Missing,
    /// The row is already there.
    Duplicate,
    /// The server is out of room.
    Busy,
    /// A bug.
    Bug,
    /// A dependency is down.
    Downstream,
}

/// The canonical code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// Unusable input.
    InvalidArgument,
    /// Absent.
    NotFound,
    /// Already present.
    AlreadyExists,
    /// Out of room.
    ResourceExhausted,
    /// A dependency is down.
    Unavailable,
    /// A server bug.
    Internal,
}

/// The canonical code for `failure`.
#[must_use]
pub fn code_for(failure: Failure) -> Code {
    todo!("one canonical code per failure")
}

/// Whether a client may retry this code.
#[must_use]
pub fn retryable(code: Code) -> bool {
    todo!("transient codes only")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_has_its_code() {
        let table = [
            (Failure::Invalid, Code::InvalidArgument),
            (Failure::Missing, Code::NotFound),
            (Failure::Duplicate, Code::AlreadyExists),
            (Failure::Busy, Code::ResourceExhausted),
            (Failure::Bug, Code::Internal),
            (Failure::Downstream, Code::Unavailable),
        ];
        for (failure, code) in table {
            assert_eq!(code_for(failure), code, "{failure:?}");
        }
    }

    #[test]
    fn transient_codes_are_retryable() {
        assert!(retryable(Code::Unavailable));
        assert!(retryable(Code::ResourceExhausted));
    }

    #[test]
    fn a_callers_mistake_is_not_retryable() {
        assert!(!retryable(Code::InvalidArgument));
        assert!(!retryable(Code::NotFound));
        assert!(!retryable(Code::AlreadyExists));
    }

    #[test]
    fn a_bug_is_not_retryable() {
        assert!(!retryable(Code::Internal));
    }

    #[test]
    fn a_busy_server_is_retryable_and_a_bug_is_not() {
        assert!(retryable(code_for(Failure::Busy)));
        assert!(retryable(code_for(Failure::Downstream)));
        assert!(!retryable(code_for(Failure::Bug)));
        assert!(!retryable(code_for(Failure::Invalid)));
    }
}
