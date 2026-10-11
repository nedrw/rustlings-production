//! probe = "async/backpressure@design"
//! title = "Overload is a decision, and every one of them is counted"
//! hint = "Give each policy its one rule, count every offer exactly once, and keep `offered == accepted + dropped` true — an eviction takes a frame back out of `accepted`."
//!
//! Step `lakelet/4.6-ingest-backpressure` puts a bound in front of the write path and gives what
//! happens at the bound a name. `4.5` left one sentence in its own FURTHER — what the write path
//! writes in one call is whatever arrived in one turn, and *nothing bounds how much that is* — and
//! the answer is not a bigger batch: it is a queue with a capacity, a policy, and a counter for
//! every frame the policy cost.
//!
//! Four rules are the ones somebody paid for once:
//!
//! * **the bound is the queue's own length.** Not a counter beside it, not a pool of permits that
//!   is not the slots: a second number that tracks the same thing is a second number that can
//!   disagree with what the queue holds, and the case that catches it is a producer that never
//!   stops. The bound is a deployment's number, and a capacity of zero frames is a refusal naming
//!   its key rather than a daemon whose only behaviour is to drop everything;
//! * **each policy has one rule, and the rule says *which* frame is lost.** `Block` waits and loses
//!   nothing; `DropNewest` refuses the arriving frame and hands it back so the caller can say which
//!   one it lost; `DropOldest` takes every arrival in and pays with the frame that waited longest;
//!   `Sample` keeps one arrival in [`SAMPLE_KEEP`] and refuses the rest — a *rate* and not a bound,
//!   so it sheds load while the queue is not full;
//! * **overload is arithmetic, not a feeling.** `offered == accepted + dropped`, and
//!   `accepted == written + queued`. The second equation is what "nothing dropped is ever
//!   acknowledged" means as numbers: a frame an eviction takes back leaves `accepted` with it, and
//!   the window between `take` and `wrote` is where the equation does *not* hold, because those
//!   frames are in the writer's hands and on nothing else;
//! * **taking is not writing.** A frame is written when the writer says so, after the bytes are on
//!   the store; until then `accepted` is a promise the daemon has not kept.
//!
//! ## What you write
//!
//! * [`Queue::new`] — the bounded queue, and the refusal of a capacity of zero frames;
//! * [`Queue::offer`] — one arrival, and the one decision the four policies disagree about;
//! * [`Queue::retry`] — the frame a waiting producer still holds, offered again by the same call;
//! * [`Queue::take`] — the writer's side: everything waiting, oldest first;
//! * [`Queue::wrote`] — the moment a frame stops being the writer's, and the refusal of a write
//!   reported for frames nobody took.
//!
//! [`Payload`], [`Policy`], [`Outcome`], [`Stats`] with its four counters, the fields of [`Queue`],
//! [`Queue::depth`], [`Queue::close`] and [`Queue::stats`] are given: the decisions are the four
//! names a deployment writes, the counters are the four a collector reads off the daemon's
//! `queue drained` line, and [`Outcome`] is the one enum the accept loop branches on.
//!
//! ## What the tests ask
//!
//! The two equations first, per policy: `offered == accepted + dropped` with a frame a waiting
//! producer still holds counted as neither, and `accepted == written + queued` at rest. Then one
//! case per rule, each of them asking *which* frame survived and not only how many: the two that
//! fitted are the first two that arrived, an eviction is the frame that waited longest, `sample`
//! keeps the fourth, the eighth and the twelfth and makes room the way `drop-oldest` does, and
//! `block` at its bound is a frame that is still the caller's with `dropped` still at zero. A
//! thousand arrivals against a bound of three do not move the depth. A closed queue refuses the
//! arrival instead of waiting for room that will never come, and what it already queued is still
//! written. A write reported for frames nobody took is refused, one case pins the window in which
//! taking is not yet writing, and one pins the difference between an arrival and the same offer
//! continuing.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no threads and no runtime**: `4.6`'s queue is `async` because a
//! producer that has to wait must not hold what the writer needs, and a probe has nothing to park.
//! So `Block` at its bound answers [`Outcome::Waiting`], the frame comes back to the caller, and
//! the caller asks again with [`Queue::retry`] — the same offer *continuing*, which is what `4.6`'s
//! own case states as `offered == accepted + dropped + 1` for the one frame a producer is still
//! waiting to hand over. Waiting is not dropping, and it is not a second arrival either; that is
//! the half of the policy a single thread can check. The other half is that the producer *wakes*
//! when room appears, and `slow:sim@3` is where that is judged, because who got dropped is a
//! question about the schedule.
//!
//! Missing on purpose: one frame per offer rather than a turn's frames at once (what a turn costs
//! the write path is `4.5`'s), the per-caller `Tally` that tells one connection what its own frames
//! came to (a probe has one caller), the four configuration layers behind `queue.capacity` and
//! `queue.policy` — `4.1`'s ladder, whose "name the key you refused" rule this file keeps only the
//! wording of — and the daemon's `frame dropped by the queue` and `queue drained` lines.

use std::collections::VecDeque;

/// One frame's payload: the bytes a client sent, which the write path writes as one record.
pub type Payload = Vec<u8>;

/// How many arrivals [`Policy::Sample`] keeps: one in four, counting arrivals from the first.
pub const SAMPLE_KEEP: u64 = 4;

/// What the queue does when a frame arrives and there is no room for it.
///
/// Four answers to one question, and none of them is "whatever the allocator decides": this enum
/// is the whole of what a deployment chooses with `queue.policy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Wait for room. Nothing is dropped, and the producer is what feels the overload.
    Block,
    /// Refuse the arriving frame: the frame that pays is the one that just arrived.
    DropNewest,
    /// Take the arrival in and drop the frame that has been waiting longest.
    DropOldest,
    /// Keep one arrival in [`SAMPLE_KEEP`] and refuse the rest — a rate rather than a bound, so it
    /// drops frames while the queue is not full. A kept arrival that finds no room makes room the
    /// way [`Policy::DropOldest`] does.
    Sample,
}

impl Policy {
    /// The name the configuration key and the daemon's log lines spell this decision with.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::DropNewest => "drop-newest",
            Self::DropOldest => "drop-oldest",
            Self::Sample => "sample",
        }
    }
}

/// What the queue did with one offered frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The queue took the frame in: it is the queue's, and the writer owes it a write.
    Accepted,
    /// The arriving frame was refused, and it comes back so the caller can say *which* one it lost.
    Refused(Payload),
    /// The arrival was taken in and the frame that had waited longest came back in its place.
    Evicted(Payload),
    /// Nothing had room and this policy waits rather than dropping: the frame is still the
    /// caller's, and neither column of the queue's books moved for it. `4.6`'s queue answers this
    /// by leaving the producer's future pending; a probe has nothing to park, so the frame comes
    /// back and the caller offers it again once the writer has taken something.
    Waiting(Payload),
}

/// Why a queue refused to do something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// A capacity of zero frames. The refusal names the key the layers of `4.1`'s ladder spell,
    /// because a queue that can never take a frame in is a deployment's mistake and not a queue:
    /// a daemon that starts with one has exactly one behaviour, which is to drop everything.
    ZeroCapacity {
        /// The configuration key the capacity came from.
        key: &'static str,
    },
    /// The writer reported more frames written than it had taken. `take` is what hands frames over,
    /// and a write counted for frames nobody took would move the counters without the store.
    NotTaken {
        /// What the writer said it wrote.
        asked: usize,
        /// The frames it had taken and not reported yet.
        taken: usize,
    },
}

/// The queue's four counters, as the daemon's `queue drained` line prints them.
///
/// The names are given — a collector reads that line, and the step's page states the two equations
/// in this vocabulary — while *which* counter moves, and for which of the four decisions, is the
/// exercise. That is why the fields are here and not only accessors: an eviction has to take a
/// frame back out of `accepted`, and an implementation that lets it stay counted is an
/// implementation whose two equations disagree by exactly the number of evictions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Frames offered to the queue, whatever it did with them. A frame a `Block` producer is still
    /// waiting to hand over is offered and not yet decided.
    pub offered: u64,
    /// Frames the queue took in and has not dropped again. A frame an eviction takes back leaves
    /// this count with it.
    pub accepted: u64,
    /// Frames dropped: refused at the door, or taken back out of the queue afterwards.
    pub dropped: u64,
    /// Frames the writer reported written. [`Queue::take`] is not a write.
    pub written: u64,
}

impl Stats {
    /// Frames offered.
    #[must_use]
    pub fn offered(&self) -> u64 {
        self.offered
    }

    /// Frames the queue took in and has not dropped again.
    #[must_use]
    pub fn accepted(&self) -> u64 {
        self.accepted
    }

    /// Frames dropped.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Frames written.
    #[must_use]
    pub fn written(&self) -> u64 {
        self.written
    }
}

/// The bounded queue between accepting a frame and writing it: the daemon's whole memory budget
/// between the socket and the log.
///
/// The bound is the length of the deque the frames are waiting in, and there is no second number
/// beside it that could disagree with it. The fields are given, because where the bound lives is a
/// decision this step has already made; what is yours is the four answers: [`Queue::new`],
/// [`Queue::offer`], [`Queue::take`] and [`Queue::wrote`].
#[derive(Debug)]
pub struct Queue {
    /// How many frames may be waiting at once: the bound, and a deployment's number.
    capacity: usize,
    /// What this queue does when a frame arrives and nothing has room.
    policy: Policy,
    /// The frames waiting for the writer, oldest first.
    waiting: VecDeque<Payload>,
    /// Frames the writer has taken and not reported written: in the writer's hands and on nothing
    /// else, which is the window the second equation does not hold in.
    in_flight: usize,
    /// Whether the queue has stopped taking frames in. What is queued is still written.
    closed: bool,
    /// The counters.
    stats: Stats,
}

impl Queue {
    /// A queue of `capacity` frames that answers an arrival as `policy` says.
    ///
    /// The capacity is a deployment's number, carried by `4.1`'s four layers as `queue.capacity`,
    /// and a capacity of zero frames is a refusal naming that key: a queue that can never take a
    /// frame in is worse than one that does not start, because a deployment only finds out in
    /// production.
    ///
    /// # Errors
    ///
    /// [`Refused::ZeroCapacity`].
    pub fn new(capacity: usize, policy: Policy) -> Result<Self, Refused> {
        todo!()
    }

    /// How many frames may be waiting at once.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// What this queue does when a frame arrives and nothing has room.
    #[must_use]
    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// How many frames are waiting for the writer right now.
    ///
    /// The frames a writer has taken are not waiting: they are in its hands, and that is the whole
    /// difference between [`Queue::take`] and [`Queue::wrote`].
    #[must_use]
    pub fn depth(&self) -> usize {
        self.waiting.len()
    }

    /// Whether the queue has stopped taking frames in.
    #[must_use]
    pub fn closed(&self) -> bool {
        self.closed
    }

    /// The counters.
    #[must_use]
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Offer one frame to the queue, and answer the one decision it got.
    ///
    /// Every offer is an arrival and is counted as one, whatever the policy does with it, and the
    /// arrival number [`Policy::Sample`] is asked about is a count of arrivals: the frame this call
    /// carries is arrival `offered + 1`, counting from one, which is why the fourth, the eighth and
    /// the twelfth are the ones it keeps.
    ///
    /// One frame per call: a turn of the accept loop is many of these, and what a turn costs the
    /// write path is `4.5`'s subject, not this queue's. A frame that comes back from
    /// [`Outcome::Waiting`] is offered again with [`Queue::retry`] and not with this call: a second
    /// `offer` is a second arrival.
    #[must_use]
    pub fn offer(&mut self, frame: Payload) -> Outcome {
        todo!()
    }

    /// The frame a waiting producer still holds, offered again: the same offer continuing.
    ///
    /// `4.6`'s producer waits *inside* one `offer` — its future is not ready until there is room —
    /// so the frame it is waiting to hand over is offered once and decided once, and the page
    /// states the equation for exactly that: `offered == accepted + dropped` plus the one frame a
    /// producer is still holding. This is the call that asks again, so it counts no arrival:
    /// passing it a frame the queue never handed back under-counts `offered` against every frame
    /// that came in.
    #[must_use]
    pub fn retry(&mut self, frame: Payload) -> Outcome {
        todo!()
    }

    /// The writer's side: take everything waiting, oldest first.
    ///
    /// Answers an empty batch when nothing is waiting and the queue is still open — a writer with
    /// nothing to do is not the end of a writer — and `None` once the queue is closed *and* empty,
    /// which is what ends a writer's loop. Closing is not abandoning: what a closed queue already
    /// holds is still taken, and still written.
    ///
    /// Taking is not writing: nothing is counted here, because nothing is on the store yet.
    #[must_use]
    pub fn take(&mut self) -> Option<Vec<Payload>> {
        todo!()
    }

    /// The writer's side, after the write: `count` frames taken by [`Queue::take`] are on the file.
    ///
    /// This is where `written` moves and where a frame stops being the writer's. A write reported
    /// before the bytes are on the store would acknowledge frames that could still fail, which is
    /// the promise `4.1` made and this step keeps through a queue.
    ///
    /// # Errors
    ///
    /// [`Refused::NotTaken`] when the writer reports more frames than it took.
    pub fn wrote(&mut self, count: usize) -> Result<(), Refused> {
        todo!()
    }

    /// Stop taking frames in. What is already queued is still written.
    pub fn close(&mut self) {
        self.closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long one payload is. Not a round number, so a length and a payload cannot be confused.
    const PAYLOAD: usize = 41;

    /// One payload, named by its marker so a case can say which one survived.
    fn payload(marker: u8) -> Payload {
        (0..PAYLOAD)
            .map(|offset| marker.wrapping_mul(31).wrapping_add(offset as u8))
            .collect()
    }

    /// A queue of `capacity` frames under `policy`.
    fn queue(capacity: usize, policy: Policy) -> Queue {
        Queue::new(capacity, policy).unwrap_or_else(|refused| {
            panic!(
                "a queue of {capacity} frame(s) under `{}` is a queue: {refused:?}",
                policy.as_str()
            )
        })
    }

    /// The four policies, so a case that is about all of them says so once.
    const POLICIES: [Policy; 4] = [
        Policy::Block,
        Policy::DropNewest,
        Policy::DropOldest,
        Policy::Sample,
    ];

    /// Offer one frame, count the arrival, and keep a frame the policy would not take yet.
    ///
    /// The count is kept here rather than read off the queue so that a case can say what the
    /// queue's `offered` *should* be: the queue counts arrivals, and the frames that come back are
    /// offered again with [`Queue::retry`].
    fn offer_counting(
        queue: &mut Queue,
        frame: Payload,
        offered: &mut u64,
        held: &mut Vec<Payload>,
    ) {
        *offered += 1;
        if let Outcome::Waiting(frame) = queue.offer(frame) {
            held.push(frame);
        }
    }

    #[test]
    fn the_conservation_law_holds_for_every_policy() {
        for policy in POLICIES {
            let mut queue = queue(3, policy);
            // The frames a `block` producer is still holding: offered, and in neither column.
            let mut held: Vec<Payload> = Vec::new();
            let mut offered = 0u64;

            for round in 0..20u8 {
                offer_counting(&mut queue, payload(round), &mut offered, &mut held);

                let stats = queue.stats();
                assert_eq!(
                    stats.offered(),
                    offered,
                    "`{}`: every frame that was offered is counted, and counted once",
                    policy.as_str()
                );
                assert_eq!(
                    stats.offered(),
                    stats.accepted() + stats.dropped() + held.len() as u64,
                    "`{}`: offered == accepted + dropped, with the frame a producer is still \
                     waiting to hand over counted as neither — it is in the caller's hands and in \
                     none of the queue's columns",
                    policy.as_str()
                );
                assert!(
                    queue.depth() <= 3,
                    "`{}` let the queue hold {} frame(s) against a bound of three",
                    policy.as_str(),
                    queue.depth()
                );

                // Every fifth round a writer drains what is waiting, reports the write, and the
                // frames a `block` producer was holding go in through the room that made.
                if round % 5 == 4 {
                    let batch = queue.take().expect("the queue is open");
                    queue
                        .wrote(batch.len())
                        .expect("the writer reports exactly the frames it took");
                    let stats = queue.stats();
                    assert_eq!(
                        stats.accepted(),
                        stats.written() + queue.depth() as u64,
                        "`{}`: accepted == written + queued — a queue at rest owes a write for \
                         exactly what it holds",
                        policy.as_str()
                    );
                    for frame in std::mem::take(&mut held) {
                        if let Outcome::Waiting(frame) = queue.retry(frame) {
                            held.push(frame);
                        }
                    }
                }
            }

            let stats = queue.stats();
            assert_eq!(
                stats.offered(),
                stats.accepted() + stats.dropped() + held.len() as u64,
                "`{}`: and the law still holds at the end",
                policy.as_str()
            );
            assert_eq!(
                stats.accepted(),
                stats.written() + queue.depth() as u64,
                "`{}`: the queue owes a write for what it holds, and for nothing else",
                policy.as_str()
            );
        }
    }

    #[test]
    fn a_producer_that_never_stops_cannot_make_the_queue_grow() {
        // No writer at all: the queue has to hold its bound, keep its books and stay alive.
        // `block` is not in this list because it waits for room rather than answering.
        for policy in [Policy::DropNewest, Policy::DropOldest, Policy::Sample] {
            let mut queue = queue(3, policy);
            for round in 0..1_000u64 {
                let _ = queue.offer(payload((round % 251) as u8));
                assert!(
                    queue.depth() <= 3,
                    "`{}` let the queue hold {} frame(s) against a bound of three",
                    policy.as_str(),
                    queue.depth()
                );
            }

            let stats = queue.stats();
            assert_eq!(
                stats.offered(),
                1_000,
                "`{}`: every frame was offered and counted",
                policy.as_str()
            );
            assert_eq!(
                stats.offered(),
                stats.accepted() + stats.dropped(),
                "`{}`: every frame is either the queue's or dropped, and the two add up",
                policy.as_str()
            );
            assert_eq!(
                stats.accepted(),
                3,
                "`{}`: the bound is what the queue can hold, and it holds exactly that",
                policy.as_str()
            );
            assert_eq!(
                stats.dropped(),
                997,
                "`{}`: the other 997 arrived while nothing had room and are not the queue's",
                policy.as_str()
            );
            assert_eq!(
                stats.written(),
                0,
                "`{}`: nothing was written, because no writer ever ran",
                policy.as_str()
            );
        }
    }

    #[test]
    fn block_waits_for_room_instead_of_dropping_the_frame() {
        let mut queue = queue(1, Policy::Block);
        let first = payload(1);
        let second = payload(2);

        assert_eq!(
            queue.offer(first.clone()),
            Outcome::Accepted,
            "the first frame has room"
        );
        assert_eq!(
            queue.offer(second.clone()),
            Outcome::Waiting(second.clone()),
            "a `block` queue with no room waits for room instead of refusing the frame, and the \
             frame comes back so the caller still has it"
        );
        let stats = queue.stats();
        assert_eq!(stats.dropped(), 0, "waiting is not dropping");
        assert_eq!(
            queue.depth(),
            1,
            "and the frame that is waiting is not in the queue yet"
        );
        assert_eq!(
            stats.offered(),
            stats.accepted() + stats.dropped() + 1,
            "the frame the producer is still holding is offered and not decided: it is in the \
             caller's hands and in none of the queue's two columns"
        );

        // The room the writer frees is the event the producer was waiting for.
        assert_eq!(
            queue.take().expect("the queue holds the first frame"),
            vec![first],
            "the writer takes what was queued, in the order it arrived"
        );
        queue.wrote(1).expect("one frame taken, one reported");
        assert_eq!(
            queue.retry(second.clone()),
            Outcome::Accepted,
            "the frame the writer made room for is taken in, not refused"
        );

        let stats = queue.stats();
        assert_eq!(
            stats.offered(),
            2,
            "two arrivals: the frame that waited was the same offer continuing, so `retry` counted \
             no second arrival for it"
        );
        assert_eq!(stats.accepted(), 2, "both were taken in, in the end");
        assert_eq!(
            stats.dropped(),
            0,
            "`block` loses nothing, which is the whole of what it promises"
        );
        assert_eq!(stats.written(), 1);
        assert_eq!(
            stats.accepted(),
            stats.written() + queue.depth() as u64,
            "one frame written and one still queued"
        );
    }

    #[test]
    fn drop_newest_refuses_the_arriving_frame() {
        let mut queue = queue(2, Policy::DropNewest);
        let frames: Vec<Payload> = (0..5u8).map(payload).collect();

        let outcomes: Vec<Outcome> = frames
            .iter()
            .map(|frame| queue.offer(frame.clone()))
            .collect();
        assert_eq!(
            outcomes,
            vec![
                Outcome::Accepted,
                Outcome::Accepted,
                Outcome::Refused(frames[2].clone()),
                Outcome::Refused(frames[3].clone()),
                Outcome::Refused(frames[4].clone()),
            ],
            "the frame that is refused is the one that arrived, and it comes back so the caller \
             can say which one it lost"
        );

        let stats = queue.stats();
        assert_eq!(stats.offered(), 5);
        assert_eq!(stats.accepted(), 2);
        assert_eq!(stats.dropped(), 3);
        assert_eq!(queue.depth(), 2, "the two that fitted are still waiting");
        assert_eq!(
            queue.take().expect("the queue holds two frames"),
            frames[..2].to_vec(),
            "and they are the first two that arrived, not the last two: the loss landed on the \
             arrivals, not on what was already queued"
        );
        queue.wrote(2).expect("two frames taken");
        assert_eq!(queue.stats().written(), 2);
        assert_eq!(
            queue.stats().accepted(),
            queue.stats().written() + queue.depth() as u64
        );
    }

    #[test]
    fn drop_oldest_evicts_the_frame_that_waited_longest() {
        let mut queue = queue(2, Policy::DropOldest);
        let frames: Vec<Payload> = (0..4u8).map(payload).collect();

        let outcomes: Vec<Outcome> = frames
            .iter()
            .map(|frame| queue.offer(frame.clone()))
            .collect();
        assert_eq!(
            outcomes,
            vec![
                Outcome::Accepted,
                Outcome::Accepted,
                Outcome::Evicted(frames[0].clone()),
                Outcome::Evicted(frames[1].clone()),
            ],
            "`drop-oldest` takes every arrival in and pays with the frame that has been waiting \
             longest"
        );
        assert_eq!(
            queue.depth(),
            2,
            "and the bound does not move while it does"
        );

        let stats = queue.stats();
        assert_eq!(stats.offered(), 4);
        assert_eq!(
            stats.accepted(),
            2,
            "two frames are the queue's: the last two that arrived"
        );
        assert_eq!(
            stats.dropped(),
            2,
            "and the two it took back are dropped, not accepted: a frame an eviction takes back \
             leaves `accepted` with it, or the two equations disagree by the number of evictions"
        );
        assert_eq!(
            stats.accepted(),
            stats.written() + queue.depth() as u64,
            "the queue owes a write for exactly the two it holds"
        );
        assert_eq!(
            queue.take().expect("the queue holds two frames"),
            vec![frames[2].clone(), frames[3].clone()],
            "what is left is the newest two: the loss landed on what arrived earlier"
        );
        queue.wrote(2).expect("two frames taken");
        let stats = queue.stats();
        assert_eq!(stats.written(), 2);
        assert_eq!(stats.accepted(), stats.written() + queue.depth() as u64);
    }

    #[test]
    fn sample_keeps_one_arrival_in_four_and_makes_room_like_drop_oldest() {
        // One slot and three rounds of the fraction: the kept arrivals are the fourth, the eighth
        // and the twelfth, and the two that find the queue full make room the way `drop-oldest`
        // does.
        let mut queue = queue(1, Policy::Sample);
        let frames: Vec<Payload> = (0..SAMPLE_KEEP as u8 * 3).map(payload).collect();

        let outcomes: Vec<Outcome> = frames
            .iter()
            .map(|frame| queue.offer(frame.clone()))
            .collect();
        let kept: Vec<usize> = outcomes
            .iter()
            .enumerate()
            .filter(|(_, outcome)| !matches!(outcome, Outcome::Refused(_)))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(
            kept,
            vec![3, 7, 11],
            "one arrival in {SAMPLE_KEEP} is taken in: the fourth, the eighth and the twelfth, \
             whatever the queue's depth was when they arrived"
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, Outcome::Evicted(_)))
                .count(),
            2,
            "and the two kept arrivals that found the queue full took the oldest queued frame \
             back: `sample` is a rate, and the room it needs it makes like `drop-oldest`"
        );

        let stats = queue.stats();
        assert_eq!(stats.offered(), 12);
        assert_eq!(
            stats.accepted(),
            1,
            "one frame is the queue's at the end: the last arrival it kept"
        );
        assert_eq!(
            stats.dropped(),
            11,
            "nine arrivals were refused and two were taken back"
        );
        assert_eq!(stats.offered(), stats.accepted() + stats.dropped());
        assert_eq!(queue.depth(), 1);
        assert_eq!(
            queue.take().expect("the queue holds one frame"),
            vec![frames[11].clone()]
        );
        queue.wrote(1).expect("one frame taken");
        assert_eq!(queue.stats().written(), 1);
    }

    #[test]
    fn a_closed_queue_refuses_the_arrival_and_still_writes_what_it_holds() {
        let mut queue = queue(1, Policy::Block);
        let queued = payload(1);
        assert_eq!(queue.offer(queued.clone()), Outcome::Accepted);
        queue.close();
        assert!(queue.closed(), "the queue stopped taking frames in");

        assert_eq!(
            queue.offer(payload(2)),
            Outcome::Refused(payload(2)),
            "a closed queue refuses the arrival rather than making a producer wait for room that \
             will never come"
        );
        let stats = queue.stats();
        assert_eq!(stats.offered(), 2);
        assert_eq!(
            stats.dropped(),
            1,
            "and the refused arrival is counted, once"
        );

        // Closing is not abandoning: what was accepted before the close is still written.
        assert_eq!(
            queue
                .take()
                .expect("what was queued before the close is still there"),
            vec![queued],
            "a closed queue still hands the writer what it holds"
        );
        queue.wrote(1).expect("one frame taken");
        assert_eq!(queue.stats().written(), 1);
        assert!(
            queue.take().is_none(),
            "and once it is closed and empty, the writer's loop is over"
        );
    }

    #[test]
    fn taking_is_not_writing_and_wrote_is_what_acknowledges() {
        let mut queue = queue(2, Policy::DropNewest);
        let frames: Vec<Payload> = (0..2u8).map(payload).collect();
        for frame in &frames {
            assert_eq!(queue.offer(frame.clone()), Outcome::Accepted);
        }

        let batch = queue.take().expect("the queue holds the two frames");
        assert_eq!(batch, frames, "the writer gets what was waiting, in order");
        assert_eq!(
            queue.stats().written(),
            0,
            "handing frames to the writer is not writing them, and it is not an acknowledgement \
             either"
        );
        assert_eq!(queue.depth(), 0, "they are not waiting any more");
        assert_ne!(
            queue.stats().accepted(),
            queue.stats().written() + queue.depth() as u64,
            "`accepted == written + queued` is a statement about a queue at rest: in the window \
             between `take` and `wrote` the frames are in the writer's hands and on nothing else, \
             and this is what that looks like from the counters"
        );

        queue.wrote(batch.len()).expect("the write is reported");
        assert_eq!(queue.stats().written(), 2);
        assert_eq!(
            queue.stats().accepted(),
            queue.stats().written() + queue.depth() as u64,
            "and the equation holds again the moment the write is reported"
        );
    }

    #[test]
    fn a_write_reported_for_frames_nobody_took_is_refused() {
        let mut queue = queue(1, Policy::DropNewest);
        assert_eq!(
            queue.wrote(1),
            Err(Refused::NotTaken { asked: 1, taken: 0 }),
            "`take` is what hands frames over, so a writer that reports a write before taking \
             anything is refused rather than counted: the counters are the lesson, and a write \
             counted for frames nobody took would move them without the store"
        );

        assert_eq!(queue.offer(payload(1)), Outcome::Accepted);
        let batch = queue.take().expect("the one frame that was queued");
        assert_eq!(batch.len(), 1);
        assert_eq!(
            queue.wrote(2),
            Err(Refused::NotTaken { asked: 2, taken: 1 }),
            "a writer may report the frames it took and not one more"
        );
        queue.wrote(1).expect("the one frame it took");
        let stats = queue.stats();
        assert_eq!(stats.written(), 1);
        assert_eq!(stats.accepted(), stats.written() + queue.depth() as u64);
    }

    #[test]
    fn an_offer_is_counted_once_whatever_the_policy_does_with_it() {
        for policy in POLICIES {
            let mut queue = queue(1, policy);
            let outcome = queue.offer(payload(1));
            // The frame a `block` producer still holds is offered and in neither column.
            let held = u64::from(matches!(outcome, Outcome::Waiting(_)));
            let stats = queue.stats();
            assert_eq!(
                stats.offered(),
                1,
                "`{}`: one offer is one arrival, whatever the policy did with it",
                policy.as_str()
            );
            assert_eq!(
                stats.accepted() + stats.dropped() + held,
                1,
                "`{}`: after one offer the frame is in exactly one place, and no policy charged \
                 its answer twice",
                policy.as_str()
            );
        }

        // The frame a full `block` queue hands back is offered once: `retry` is the same offer
        // continuing, and the arrival was already counted.
        let mut blocking = queue(1, Policy::Block);
        let waiting = match blocking.offer(payload(1)) {
            Outcome::Accepted => match blocking.offer(payload(2)) {
                Outcome::Waiting(frame) => frame,
                other => panic!("a full `block` queue hands the arrival back, not {other:?}"),
            },
            other => panic!("the first frame has room, and was {other:?}"),
        };
        assert_eq!(
            (blocking.stats().offered(), blocking.stats().accepted()),
            (2, 1),
            "two arrivals, one of them the queue's, and one in the caller's hands"
        );
        let _ = blocking
            .take()
            .expect("the queue is open and holds one frame");
        blocking.wrote(1).expect("one frame taken");
        assert_eq!(blocking.retry(waiting), Outcome::Accepted);
        let stats = blocking.stats();
        assert_eq!(
            (stats.offered(), stats.accepted(), stats.dropped()),
            (2, 2, 0),
            "the retry added an arrival to no column but `accepted`: the frame was offered once, \
             and `block` loses nothing"
        );

        // A frame the queue *decided* about is another matter: a caller that offers a refused frame
        // again is making a new arrival, which is why a retry loop against `drop-newest` grows
        // `dropped` and not `accepted` — and why the default a deployment gets is the policy that
        // waits.
        let mut dropping = queue(1, Policy::DropNewest);
        let _ = dropping.offer(payload(1));
        let refused = match dropping.offer(payload(2)) {
            Outcome::Refused(frame) => frame,
            other => panic!("a full `drop-newest` queue refuses the arrival, not {other:?}"),
        };
        let _ = dropping.offer(refused);
        let stats = dropping.stats();
        assert_eq!(
            (stats.offered(), stats.dropped()),
            (3, 2),
            "three arrivals and two of them refused: a refusal is counted once per arrival, and \
             offering the refused frame again is a third arrival"
        );
        assert_eq!(
            stats.accepted(),
            1,
            "and the queue is behind exactly one frame"
        );
    }
}
