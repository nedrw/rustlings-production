//! probe = "broker/lag@apply"
//! title = "End minus commit, and never below zero"
//! hint = "Every partition of the log appears; a missing commit is a full partition of lag, and a commit past the end saturates at zero."
//!
//! Step `broker/7.4-lag` is the number that says who is behind. This probe passes the end offsets
//! in directly instead of a log — the arithmetic is the same, and the two boundary cases are the
//! point.
//!
//! ## What you write
//!
//! [`lag`] — `end.saturating_sub(committed)` for every partition of `end`, with a missing commit
//! counting as `0`.
//!
//! ## What the tests ask
//!
//! The plain difference; a partition with no commit is fully behind; a commit past the end is zero,
//! not a panic or a wrap; every partition appears, including the zero ones; and the total adds up.

use std::collections::BTreeMap;

/// Lag per partition, saturating at zero.
#[must_use]
pub fn lag(end: &BTreeMap<usize, u64>, committed: &BTreeMap<usize, u64>) -> BTreeMap<usize, u64> {
    todo!("end minus commit, saturating, one entry per partition")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end() -> BTreeMap<usize, u64> {
        BTreeMap::from([(0, 3), (1, 0), (2, 5)])
    }

    #[test]
    fn lag_is_end_minus_commit() {
        let committed = BTreeMap::from([(0, 1), (1, 0), (2, 5)]);
        let lag = lag(&end(), &committed);
        assert_eq!(lag[&0], 2);
        assert_eq!(lag[&1], 0);
        assert_eq!(lag[&2], 0);
    }

    #[test]
    fn a_partition_with_no_commit_is_fully_behind() {
        let lag = lag(&end(), &BTreeMap::new());
        assert_eq!(lag[&0], 3);
        assert_eq!(lag[&2], 5);
    }

    #[test]
    fn a_commit_past_the_end_is_zero() {
        let committed = BTreeMap::from([(0, 99)]);
        let lag = lag(&end(), &committed);
        assert_eq!(lag[&0], 0, "a commit past the end went below zero");
    }

    #[test]
    fn every_partition_appears() {
        let lag = lag(&end(), &BTreeMap::from([(0, 1)]));
        assert_eq!(lag.len(), 3);
    }

    #[test]
    fn the_total_is_the_sum_of_the_parts() {
        let committed = BTreeMap::from([(0, 1)]);
        let total: u64 = lag(&end(), &committed).values().sum();
        assert_eq!(total, 7);
    }
}
