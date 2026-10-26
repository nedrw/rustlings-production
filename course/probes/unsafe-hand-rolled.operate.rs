//! probe = "unsafe/hand-rolled@operate"
//! title = "The ring's indices are the argument, and safe code can still check every clause"
//! hint = "Refuse a capacity no mask can describe, answer full and empty from the two indices, take the slot with the mask, and let a raced pair of loads give zero rather than an underflow."
//!
//! Step `meshkv/5.19-unsafe-deep-dive` is the one step in this track allowed to write `unsafe`, and
//! its subject is a single-producer single-consumer ring whose whole job is to avoid a lock. This
//! file is the `operate` rung of that capability, and it deliberately contains **no `unsafe`**: what
//! can be re-drilled without aliasing is the *index protocol* the `// SAFETY:` notes stand on, and
//! it is the half that is usually wrong even in code that compiles. Five rules, each one a bug
//! somebody shipped:
//!
//! 1. **full and empty are different facts even when they share a slot.** Empty is `head == tail`;
//!    full is `tail − head == capacity`. With a mask, a full ring of two has `head` and `tail`
//!    naming the same slot, so a ring that tests slot equality cannot tell the two states apart and
//!    overwrites the value it was supposed to refuse.
//! 2. **the capacity is a mask, so the constructor refuses a shape no mask describes.** `seq &
//!    (capacity − 1)` visits every slot exactly once only when the capacity is a power of two; a ring
//!    of three with `mask = 2` aliases slot 2 and loses a slot's worth of values. The refusal is made
//!    once, where the capacity is chosen, instead of silently in every push.
//! 3. **`len` is a snapshot, not a moment.** It reads two atomics; between the loads another thread
//!    can move `head` past the `tail` the caller read, so the difference is `saturating_sub`. A
//!    healthy ring under load is exactly when the plain subtraction panics.
//! 4. **a full ring refuses and hands the value back.** It does not overwrite, block or drop: the
//!    write happens only while `tail − head < capacity`, because the producer is the only writer of
//!    `tail` and the consumer is the only reader of the slot it has not reached.
//! 5. **the mask wraps at exactly capacity.** A sequence `tail = head + capacity` names the same slot
//!    as `head` only after the consumer has moved — the same arithmetic as the full check, read from
//!    the other side. Off by one here either refuses a push with a free slot or reuses a live one.
//!
//! ## What you write
//!
//! * [`Ring::new`] — a ring whose capacity can be a mask, or the reason it cannot;
//! * [`Ring::push`] — one value at the producer's slot, or the value back when the ring is full;
//! * [`Ring::pop`] — the oldest value, or `None` when the two indices agree;
//! * [`Ring::len`] — the snapshot difference, which must not underflow.
//!
//! [`Ring`]'s fields, [`Ring::capacity`], [`Ring::is_empty`] and [`Flaw`] are given. The slots are
//! `Mutex<Option<T>>` rather than `UnsafeCell<MaybeUninit<T>>`: locking one slot is what safe code
//! can do instead of aliasing it, and it keeps the *index arithmetic* — which slot, whether the ring
//! is full, whether the value is there — as the thing under test. The test-only `snapshot` fixture
//! is given too: it builds a ring with the two indices a raced `len` would observe, so the
//! underflow is checked deterministically instead of hoped for.
//!
//! ## What the tests ask
//!
//! Four slots are accepted and one and two are the smallest masks; a capacity of zero, three or six
//! is refused with its number. Values come back first in, first out, and an empty ring answers
//! `None`. A ring of two takes exactly two values and refuses the third *with the value itself*,
//! then accepts one more after one pop. Five rounds of push-two/pop-two reuse the same two slots and
//! stay FIFO, which is the wrap at exactly capacity. A full ring and an empty one are told apart
//! when their indices name the same slot. The snapshot case reads `head = 6, tail = 5` as zero
//! rather than an underflow, and `head = 2, tail = 5` as three. And a two-thousand-step
//! push/pop walk — both interleaved and drained — keeps `pushed − popped == len`, never exceeds the
//! capacity, never repeats a value, and never loses one.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no `unsafe`**. The aliasing, the `UnsafeCell`, the `MaybeUninit`
//! initialisation argument and the `acquire`/`release` pair that makes the slot read sound are not
//! in this file — they are `meshkv/5.19`'s ring, its loom models and its `// SAFETY:` notes. The
//! per-slot mutex is a lock the real structure exists to avoid, and because it synchronises the slot
//! on its own, a wrong atomic ordering here would not be caught by these tests; the ordering is a
//! label this file does not get to verify. The `Drop` obligation is absent for a second reason:
//! `Option<T>` drops itself, so dropping exactly once cannot be exercised through this shape.
//!
//! Also missing: no two threads, no loom, no batch move, no backpressure policy. What *is* here is
//! the part of the argument that survives without aliasing, and it is worth re-drilling precisely
//! because a lock-free ring is only as sound as its indices: the full/empty distinction, the mask
//! and its power-of-two precondition, the snapshot subtraction, and a refusal that hands the value
//! back are the four clauses a `// SAFETY:` note would otherwise be standing on alone. The note
//! itself is `unsafe/budget@apply`'s habit (`jobq/3.24`), the bounded refusal is
//! `io/batching@design`'s, and first-in-first-out is `delivery/ordering@apply`'s.

use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A bounded ring whose slot is chosen by masking the sequence.
///
/// One producer and one consumer may use it; more than that is misuse the type does not detect,
/// which is the same sentence `meshkv/5.19`'s page says about its ring.
#[derive(Debug)]
pub struct Ring<T> {
    /// One slot per capacity, locked for as long as a value is being placed or taken.
    slots: Vec<Mutex<Option<T>>>,
    /// `capacity − 1`, the mask that turns a sequence number into a slot.
    mask: usize,
    /// The next sequence to read.
    head: AtomicUsize,
    /// The next sequence to write.
    tail: AtomicUsize,
}

/// Why a capacity cannot be a ring's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flaw {
    /// The capacity is zero or not a power of two.
    NotAMask {
        /// The capacity that was asked for.
        capacity: usize,
    },
}

impl fmt::Display for Flaw {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAMask { capacity } => write!(
                formatter,
                "a capacity of {capacity} cannot be a mask: a slot is `sequence & (capacity - 1)`, \
                 which names every slot only when the capacity is a power of two"
            ),
        }
    }
}

impl<T> Ring<T> {
    /// A ring of `capacity` slots.
    ///
    /// # Errors
    ///
    /// [`Flaw::NotAMask`] when the capacity is zero or not a power of two: the mask would alias
    /// slots, and a ring that silently drops a value is worse than one that refused the shape.
    pub fn new(capacity: usize) -> Result<Self, Flaw> {
        todo!()
    }

    /// How many slots the ring has.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// `true` when the snapshot says it holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many values it holds, as a snapshot of the two indices.
    ///
    /// The tail is loaded first and the head second, as `meshkv/5.19`'s `len` does; a concurrent
    /// pop can move `head` past the `tail` this call read, so the difference saturates at zero
    /// rather than underflowing.
    #[must_use]
    pub fn len(&self) -> usize {
        todo!()
    }

    /// Push `value`, or refuse it when the ring is full.
    ///
    /// # Errors
    ///
    /// The value itself, when the difference of the two indices is the capacity: a full ring hands
    /// the value back rather than overwriting a slot or dropping what the caller gave it.
    pub fn push(&self, value: T) -> Result<(), T> {
        todo!()
    }

    /// Pop the oldest value, if there is one.
    ///
    /// The empty test is `head == tail`, not a slot comparison: after a wrap a full ring and an
    /// empty one can name the same slot, and only the sequence numbers tell them apart.
    pub fn pop(&self) -> Option<T> {
        todo!()
    }

    /// A ring whose two indices are `head` and `tail`, whatever they are.
    ///
    /// A fixture for the snapshot case: the pair is exactly what one `len` call can observe when the
    /// consumer moves between its two loads. The slots are empty, because the values are not what
    /// the snapshot test is about.
    #[cfg(test)]
    fn snapshot(capacity: usize, head: usize, tail: usize) -> Self {
        Self {
            slots: (0..capacity).map(|_| Mutex::new(None)).collect(),
            mask: capacity - 1,
            head: AtomicUsize::new(head),
            tail: AtomicUsize::new(tail),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capacity_is_a_mask_and_a_shape_no_mask_describes_is_refused() {
        let ring = Ring::<u8>::new(4).expect("four slots");
        assert_eq!(
            ring.capacity(),
            4,
            "the ring has the capacity it was built with"
        );
        assert!(ring.is_empty(), "and it starts empty");
        assert_eq!(ring.len(), 0, "with no values in it");

        assert_eq!(
            Ring::<u8>::new(1).map(|ring| ring.capacity()),
            Ok(1),
            "one slot is the smallest capacity a mask can name"
        );
        assert_eq!(
            Ring::<u8>::new(2).map(|ring| ring.capacity()),
            Ok(2),
            "and two is the smallest ring that can hold a value while one is taken"
        );
        assert_eq!(
            Ring::<u8>::new(0).err(),
            Some(Flaw::NotAMask { capacity: 0 }),
            "no slot can never hold anything"
        );
        assert_eq!(
            Ring::<u8>::new(3).err(),
            Some(Flaw::NotAMask { capacity: 3 }),
            "three slots with a mask of two would alias the last slot, so the shape is refused"
        );
        assert!(
            Ring::<u8>::new(6).is_err(),
            "six is not a power of two either, and a mask cannot describe it"
        );
    }

    #[test]
    fn values_come_back_first_in_first_out() {
        let ring = Ring::new(4).expect("a ring");
        assert_eq!(ring.push(1u8), Ok(()), "the first value has room");
        assert_eq!(ring.push(2u8), Ok(()), "and so does the second");
        assert_eq!(ring.push(3u8), Ok(()), "and the third");
        assert_eq!(ring.len(), 3, "three values are in the ring");
        assert!(!ring.is_empty(), "which is not empty");

        assert_eq!(ring.pop(), Some(1), "the oldest value comes back first");
        assert_eq!(ring.pop(), Some(2), "then the next oldest");
        assert_eq!(ring.pop(), Some(3), "then the last");
        assert_eq!(ring.pop(), None, "and an empty ring has nothing to give");
        assert!(ring.is_empty(), "it says so");
        assert_eq!(ring.len(), 0, "and the count agrees");
    }

    #[test]
    fn a_full_ring_refuses_and_hands_the_value_back() {
        let ring = Ring::new(2).expect("a ring");
        assert_eq!(ring.push(String::from("a")), Ok(()));
        assert_eq!(ring.push(String::from("b")), Ok(()));
        assert_eq!(
            ring.push(String::from("c")),
            Err(String::from("c")),
            "the value at exactly the capacity is refused, and the caller still owns it"
        );

        assert_eq!(
            ring.pop().as_deref(),
            Some("a"),
            "a pop frees exactly one slot"
        );
        assert_eq!(
            ring.push(String::from("d")),
            Ok(()),
            "one free slot is enough for one value"
        );
        assert_eq!(ring.pop().as_deref(), Some("b"), "then the next oldest");
        assert_eq!(
            ring.pop().as_deref(),
            Some("d"),
            "then the value that reused the slot"
        );
    }

    #[test]
    fn the_mask_wraps_at_exactly_capacity_and_stays_fifo() {
        let ring = Ring::new(2).expect("a ring");
        for round in 0..5u32 {
            assert_eq!(ring.push(round * 2), Ok(()), "round {round}'s first value");
            assert_eq!(
                ring.push(round * 2 + 1),
                Ok(()),
                "round {round}'s second value"
            );
            assert_eq!(
                ring.len(),
                2,
                "the ring stops at its capacity and does not count the wrap as a third slot"
            );
            assert_eq!(
                ring.pop(),
                Some(round * 2),
                "the oldest value of round {round}"
            );
            assert_eq!(ring.pop(), Some(round * 2 + 1), "and the one after it");
            assert!(ring.is_empty(), "the two slots are free again");
        }
    }

    #[test]
    fn full_and_empty_are_not_the_same_slot_question() {
        let ring = Ring::new(2).expect("a ring");
        ring.push(1u8).expect("room");
        ring.push(2u8).expect("room");
        assert!(
            !ring.is_empty(),
            "a full ring is not empty even though head and tail name the same slot after the wrap"
        );
        assert_eq!(
            ring.push(3u8),
            Err(3),
            "the full check is the difference of the sequences, not the equality of the slots"
        );
        assert_eq!(ring.pop(), Some(1));
        assert_eq!(ring.pop(), Some(2));
        assert!(
            ring.is_empty(),
            "and the empty check is the sequences being equal"
        );
        assert_eq!(ring.push(3u8), Ok(()), "so the slot can be written again");
        assert_eq!(ring.pop(), Some(3));
    }

    #[test]
    fn the_length_is_a_snapshot_and_a_raced_pair_of_loads_is_zero_not_a_panic() {
        let inverted = Ring::<u8>::snapshot(4, 6, 5);
        assert_eq!(
            inverted.len(),
            0,
            "a consumer that moved past the tail this call read leaves an empty snapshot, not an \
             underflow"
        );
        assert!(
            inverted.is_empty(),
            "and the ring is empty by the same snapshot"
        );

        let normal = Ring::<u8>::snapshot(4, 2, 5);
        assert_eq!(
            normal.len(),
            3,
            "the same two loads in their legal order are the length"
        );
        assert!(!normal.is_empty(), "so the ring is not empty");
    }

    #[test]
    fn the_counts_are_conserved_over_every_interleaving() {
        let ring = Ring::new(4).expect("a ring");
        let mut pushed = 0u64;
        let mut popped = 0u64;
        let mut next = 0u64;
        let mut state = 0x2545_f491_4f6c_dd1du64;

        for _ in 0..2_000 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            if state >> 63 == 0 {
                if ring.push(next).is_ok() {
                    pushed += 1;
                    next += 1;
                }
            } else if let Some(value) = ring.pop() {
                assert_eq!(
                    value, popped,
                    "a pop returns the oldest value: a later one, or a repeat, means the indices \
                     wrapped over a live slot"
                );
                popped += 1;
            }
            assert_eq!(
                pushed - popped,
                ring.len() as u64,
                "every value is either in the ring or was popped: a full ring refuses, it does not \
                 lose"
            );
            assert!(
                ring.len() <= ring.capacity(),
                "the ring never holds more than its capacity"
            );
        }

        while let Some(value) = ring.pop() {
            assert_eq!(
                value, popped,
                "draining returns the values in the order they were pushed"
            );
            popped += 1;
        }
        assert_eq!(
            popped, pushed,
            "a drained ring has handed back every value it accepted"
        );
    }
}
