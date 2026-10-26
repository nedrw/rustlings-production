//! What `meshkv/5.19-unsafe-deep-dive` has to be true about.
//!
//! The step's subject is a structure whose safety rests on an argument, and the cases below are the
//! parts of the argument a test can hold: the queue is FIFO, a full queue refuses and gives the value
//! back, dropping the queue drops exactly the values in it, and the three unsafe obligations are
//! exercised — initialisation on push, a read only of a written slot on pop, and one drop per value.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use meshkv::ring::Ring;

#[test]
fn a_ring_has_the_capacity_it_was_built_with() {
    let ring: Ring<u8> = Ring::new(4).expect("a ring");
    assert_eq!(ring.capacity(), 4);
    assert!(ring.is_empty());
    assert_eq!(ring.len(), 0);
    assert!(
        Ring::<u8>::new(0).is_err(),
        "a ring with no slots holds nothing"
    );
}

#[test]
fn a_ring_is_first_in_first_out() {
    let ring = Ring::new(3).expect("a ring");
    ring.push(1u8).expect("room");
    ring.push(2).expect("room");
    ring.push(3).expect("room");
    assert_eq!(ring.len(), 3);
    assert_eq!(ring.pop(), Some(1));
    assert_eq!(ring.pop(), Some(2));
    assert_eq!(ring.pop(), Some(3));
    assert_eq!(ring.pop(), None, "an empty ring has nothing to give");
    assert!(ring.is_empty());
}

#[test]
fn a_full_ring_refuses_and_gives_the_value_back() {
    let ring = Ring::new(2).expect("a ring");
    ring.push(String::from("a")).expect("room");
    ring.push(String::from("b")).expect("room");
    let refused = ring.push(String::from("c")).expect_err("the ring is full");
    assert_eq!(refused, "c", "the refusal hands the value back");
    assert_eq!(ring.pop().as_deref(), Some("a"));
    ring.push(String::from("d")).expect("room again");
    assert_eq!(ring.pop().as_deref(), Some("b"));
    assert_eq!(ring.pop().as_deref(), Some("d"));
}

#[test]
fn the_ring_wraps_around_and_stays_fifo() {
    let ring = Ring::new(2).expect("a ring");
    for round in 0..5u32 {
        ring.push(round * 2).expect("room");
        ring.push(round * 2 + 1).expect("room");
        assert_eq!(ring.pop(), Some(round * 2));
        assert_eq!(ring.pop(), Some(round * 2 + 1));
    }
}

#[test]
fn dropping_the_ring_drops_each_value_exactly_once() {
    // A counter in the value's `Drop` is the only way to see this: a queue that leaked would leave
    // the count below three, and one that dropped twice would take it above.
    static DROPS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Debug)]
    struct Counted;
    impl Drop for Counted {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    {
        let ring = Ring::new(4).expect("a ring");
        ring.push(Counted).expect("room");
        ring.push(Counted).expect("room");
        ring.push(Counted).expect("room");
        assert_eq!(DROPS.load(Ordering::SeqCst), 0, "nothing dropped yet");
    }
    assert_eq!(
        DROPS.load(Ordering::SeqCst),
        3,
        "the three values in the ring were dropped exactly once"
    );

    // And a value that is popped is not dropped by the ring.
    let ring = Ring::new(2).expect("a ring");
    ring.push(Counted).expect("room");
    let taken = ring.pop().expect("one value");
    assert_eq!(
        DROPS.load(Ordering::SeqCst),
        3,
        "the popped value is still alive"
    );
    drop(taken);
    assert_eq!(DROPS.load(Ordering::SeqCst), 4, "and dropped by its owner");
}

#[test]
fn a_producer_and_a_consumer_on_two_threads_move_every_value() {
    // The concurrency case the structure exists for: one producer, one consumer, no lock. If the
    // orderings were wrong the test would lose or duplicate values — it is the case loom checks
    // exhaustively, and this is the case that runs on every machine.
    const VALUES: usize = 10_000;
    let ring = Arc::new(Ring::new(8).expect("a ring"));

    let producer = {
        let ring = Arc::clone(&ring);
        std::thread::spawn(move || {
            let mut next = 0usize;
            while next < VALUES {
                if ring.push(next).is_ok() {
                    next += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
        })
    };
    let consumer = {
        let ring = Arc::clone(&ring);
        std::thread::spawn(move || {
            let mut seen = Vec::with_capacity(VALUES);
            while seen.len() < VALUES {
                if let Some(value) = ring.pop() {
                    seen.push(value);
                } else {
                    std::hint::spin_loop();
                }
            }
            seen
        })
    };

    producer.join().expect("the producer ran");
    let seen = consumer.join().expect("the consumer ran");
    assert_eq!(seen.len(), VALUES, "nothing was lost or duplicated");
    assert!(
        seen.windows(2).all(|pair| pair[0] < pair[1]),
        "and the values arrived in the order they were pushed"
    );
}
