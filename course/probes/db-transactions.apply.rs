//! probe = "db/transactions@apply"
//! title = "All of it, or none of it"
//! hint = "Decide and write inside one critical section: a debit that lands and a credit that is refused is money that left one account and arrived nowhere."
//!
//! Step `mailer/1.17-publish-newsletter` writes two things that must agree — the newsletter row
//! and the fan-out work it creates — inside one transaction, so a failure halfway leaves neither.
//! This probe is the arithmetic of that sentence at the smallest scale that still has a failure
//! in it.
//!
//! The ledger here is a `BTreeMap` behind `&mut self`, because a probe runs offline and in
//! seconds. `&mut self` is the transaction: nobody can look between the decision and the write,
//! which is what the database gives the real statement for free. What is left to get wrong is
//! the order inside the call:
//!
//! * every refusal happens **before** the first write (check the source, then the target, then
//!   the money);
//! * a transfer that fails writes nothing at all — not "debit, then fail to credit";
//! * a transfer that succeeds writes both sides, so the total never moves.
//!
//! ## What you write
//!
//! [`Ledger::transfer`] — move `amount` from one account to another, all or nothing. The
//! refusals are [`TransferError::UnknownSource`], [`TransferError::UnknownTarget`] (an account
//! that does not exist or is closed) and [`TransferError::InsufficientFunds`], checked in that
//! order so a call with two faults reports the first one. `0` is a legal amount and moves
//! nothing; a transfer to itself is allowed and moves nothing either.
//!
//! ## What the tests ask
//!
//! A successful move, every refusal leaving the ledger exactly as it was, the closed-target case
//! (the partial-write trap: the debit must not stand), and 200 deterministic transfers with
//! failures mixed in, where the sum of all balances has to stay put.

use std::collections::BTreeMap;

/// Why a transfer was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferError {
    /// The source account does not exist.
    UnknownSource,
    /// The target account does not exist, or is closed.
    UnknownTarget,
    /// The source does not hold enough.
    InsufficientFunds,
}

/// One account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Account {
    /// What the account holds, in whole units.
    pub balance: u64,
    /// A closed account can still pay out of what it holds; nothing can be paid into it.
    pub open: bool,
}

/// The ledger. `&mut self` is one transaction at a time.
#[derive(Debug, Default)]
pub struct Ledger {
    accounts: BTreeMap<String, Account>,
}

impl Ledger {
    /// An empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an account, or reset the balance of an existing one and reopen it.
    pub fn open(&mut self, id: &str, balance: u64) {
        self.accounts.insert(
            id.to_owned(),
            Account {
                balance,
                open: true,
            },
        );
    }

    /// Close an account: it can still pay, but nothing can be paid into it.
    pub fn close(&mut self, id: &str) {
        if let Some(account) = self.accounts.get_mut(id) {
            account.open = false;
        }
    }

    /// What an account holds, or `None` when it does not exist.
    #[must_use]
    pub fn balance(&self, id: &str) -> Option<u64> {
        self.accounts.get(id).map(|account| account.balance)
    }

    /// Everything the ledger holds.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.accounts.values().map(|account| account.balance).sum()
    }

    /// Move `amount` from `from` to `to`, all or nothing.
    ///
    /// # Errors
    ///
    /// See [`TransferError`]. The checks run source, target, funds, and a refused transfer writes
    /// nothing.
    pub fn transfer(&mut self, from: &str, to: &str, amount: u64) -> Result<(), TransferError> {
        todo!("decide everything first, then write both sides")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> Ledger {
        let mut ledger = Ledger::new();
        ledger.open("alice", 100);
        ledger.open("bob", 0);
        ledger
    }

    #[test]
    fn a_transfer_moves_the_amount_and_leaves_the_total_alone() {
        let mut ledger = ledger();
        assert_eq!(ledger.transfer("alice", "bob", 30), Ok(()));
        assert_eq!(ledger.balance("alice"), Some(70));
        assert_eq!(ledger.balance("bob"), Some(30));
        assert_eq!(ledger.total(), 100);
    }

    #[test]
    fn every_refusal_leaves_the_ledger_exactly_as_it_was() {
        let mut ledger = ledger();
        ledger.open("carol", 0);
        ledger.close("carol");
        let before = (ledger.balance("alice"), ledger.balance("bob"), ledger.balance("carol"), ledger.total());

        assert_eq!(
            ledger.transfer("nobody", "bob", 10),
            Err(TransferError::UnknownSource)
        );
        assert_eq!(
            ledger.transfer("alice", "nobody", 10),
            Err(TransferError::UnknownTarget)
        );
        assert_eq!(
            ledger.transfer("alice", "carol", 10),
            Err(TransferError::UnknownTarget)
        );
        assert_eq!(
            ledger.transfer("alice", "bob", 101),
            Err(TransferError::InsufficientFunds)
        );

        let after = (ledger.balance("alice"), ledger.balance("bob"), ledger.balance("carol"), ledger.total());
        assert_eq!(before, after, "a refused transfer wrote something");
    }

    #[test]
    fn a_closed_target_does_not_let_the_debit_stand() {
        let mut ledger = ledger();
        ledger.open("carol", 0);
        ledger.close("carol");

        assert_eq!(
            ledger.transfer("alice", "carol", 10),
            Err(TransferError::UnknownTarget)
        );
        // The trap: debit first, discover the closed account second, and 10 units are gone.
        assert_eq!(ledger.balance("alice"), Some(100), "the debit stood");
        assert_eq!(ledger.balance("carol"), Some(0));
        assert_eq!(ledger.total(), 100);
    }

    #[test]
    fn two_faults_report_the_first_one() {
        let mut ledger = ledger();
        assert_eq!(
            ledger.transfer("nobody", "alsonobody", 10),
            Err(TransferError::UnknownSource)
        );
        // The target is checked before the money: an unknown target with an amount the source
        // cannot cover is still `UnknownTarget`.
        assert_eq!(
            ledger.transfer("alice", "nobody", 10_000),
            Err(TransferError::UnknownTarget)
        );
    }

    #[test]
    fn zero_and_self_transfers_move_nothing_and_are_not_failures() {
        let mut ledger = ledger();
        assert_eq!(ledger.transfer("alice", "bob", 0), Ok(()));
        assert_eq!(ledger.transfer("alice", "alice", 25), Ok(()));
        assert_eq!(ledger.balance("alice"), Some(100));
        assert_eq!(ledger.balance("bob"), Some(0));
        assert_eq!(ledger.total(), 100);
    }

    #[test]
    fn two_hundred_transfers_keep_the_sum_and_never_overdraw() {
        let mut ledger = Ledger::new();
        let accounts = ["a", "b", "c", "d"];
        for account in accounts {
            ledger.open(account, 1_000);
        }
        let total = ledger.total();
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;

        for _ in 0..200 {
            // A tiny LCG: deterministic, and its seed lives in this test rather than in a clock.
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let from = accounts[((state >> 33) % accounts.len() as u64) as usize];
            let to = accounts[((state >> 21) % accounts.len() as u64) as usize];
            let amount = (state >> 11) % 400;
            let _ = ledger.transfer(from, to, amount);

            assert_eq!(ledger.total(), total, "{from} -> {to} {amount} moved the total");
            for account in accounts {
                let balance = ledger.balance(account).expect("every account exists");
                assert!(balance <= total, "{account} holds more than the ledger");
            }
        }
        assert_eq!(ledger.total(), total);
    }
}
