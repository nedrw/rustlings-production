//! probe = "io/batching@design"
//! title = "One call for the batch, and a counter for every one it saves"
//! hint = "Seal before the record that would not fit, charge the counters where the call is made, and let an empty seal cost nothing."
//!
//! Step `lakelet/4.5-batch-writes` is about what an append *costs*. A device flush is milliseconds;
//! an open and a write are microseconds — and all three are paid per **call** and not per byte, so
//! the same records cost a different number of calls depending only on how many of them are handed
//! over at once. Sixty-four records in one piece are one open and one write; one at a time they are
//! a hundred and twenty-eight calls, for the same bytes in the same order. That ratio is **write
//! amplification** plus the calls per record, and the step's whole point is that the write path
//! *counts* it instead of arguing about it.
//!
//! Four rules are the ones somebody paid for once:
//!
//! * **a batch changes the calls and nothing else.** The frame one seal writes is the bytes N
//!   appends write, record for record, in the order they arrived: the record format belongs to
//!   `lakelet/4.4-wal-recovery`, and a batch that moved a byte of it — sorted the records, merged
//!   two of them, dropped a duplicate — would hand every reader a different series;
//! * **a counter is charged where the call is made.** `records_in` moves in [`Batcher::push`],
//!   `seals` and `bytes_written` in [`Batcher::seal`], `flushes` where the device is asked. A
//!   counter the caller increments reports what the caller believes it did, which is how a write
//!   path stays expensive while a budget passes;
//! * **an empty seal is not a seal.** No call, no frame, no counter: in `4.6` a turn that carried
//!   nothing is the common case, and a write path that touched the file for it spends two calls on
//!   a turn in which nothing arrived;
//! * **the bound is a capacity, and a record is never split.** The record that would take a batch
//!   over its bound seals what is pending *first* and starts the next batch; a record larger than
//!   the whole bound is a batch of one. Splitting a record to fill the gap writes bytes no append
//!   would have written, and dropping it loses a sample.
//!
//! ## What you write
//!
//! * [`Bound::new`] — a batch's capacity, and the refusal of a bound that can never be satisfied;
//! * [`Batcher::push`] — one record in, sealing what has to be sealed to make room for it;
//! * [`Batcher::seal`] — what is waiting, as **one** frame in **one** call.
//!
//! [`Batcher::new`], [`Batcher::len`], [`Batcher::stats`], the [`Stats`] counters and the ways the
//! write path charges them, [`Sink`] and [`Log`] are given. The counters are the contract the step's
//! budget lane reads; the sink is the seam a probe can put a counting stand-in behind, and it is
//! where a frame's shape is written down.
//!
//! ## What the tests ask
//!
//! The boundaries first: a seal with nothing waiting charges nothing, `records_in == frames_out +
//! pending` holds after every push and every seal, and the record that filled a batch of two is in
//! the sealed batch and not left pending. Then the two ways a bound is reached: a count bound seals
//! exactly at the count, and a size bound seals the batch *before* the record that would not fit —
//! where a record larger than the whole bound goes out alone and comes back whole. The same records
//! batched and written one at a time are the same bytes and one call against eight, which is the
//! coefficient this step exists to bring down. The device call is one per batch under `per-batch`
//! and none under `never`; a store that refuses the call is a refusal that charges nothing and
//! loses no record, and the retry is the same batcher and the same pending batch. One case pins a
//! number the prose promises — a frame is a four-byte length in front of every record — because a
//! counter nobody can predict is a counter nobody can gate on.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no store**: [`Log`] is a `Vec` of frames and not a file, so what is
//! re-checked is the accounting and not the bytes a device took. The frame carries the *shape* of
//! `4.4`'s record — a length and then the payload — while the per-record seal, the log's `LKLT`
//! header and the sweep over every cut of a batch-written log are that step's own subject; a probe
//! that hand-rolled them would re-teach `4.4` instead of re-checking this step's habit.
//!
//! Missing on purpose: what a *device* refusal does to the acknowledgement (here [`Sink::sync`]
//! always succeeds, and the real answer is `4.3`'s policy plus `4.1`'s rule that a written record is
//! readable and an unflushed one is not promised), the accept loop that decides *where* a batch
//! ends — `4.5` writes one turn's frames as one batch, and a probe has no turn — and every
//! time-based measurement, because the step is judged by counts and a probe with a clock in it would
//! answer differently on a loaded machine.

/// One payload: the bytes of one record's content, before the write path frames it.
pub type Payload = Vec<u8>;

/// One batch's capacity: at most `records` records, and at most `bytes` payload bytes, whichever
/// comes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bound {
    /// The most records one batch holds.
    records: usize,
    /// The most payload bytes one batch holds.
    bytes: usize,
}

impl Bound {
    /// A bound of `records` records and `bytes` payload bytes.
    ///
    /// A bound of zero on either axis is refused: a batch that can never hold a record is not a
    /// batch, and a write path that sealed every record on its own would report the very counts
    /// this step exists to bring down. `lakelet/4.6-ingest-backpressure` refuses a queue of zero
    /// frames the same way, and for the same reason — the refusal names the key a deployment wrote
    /// rather than quietly doing something else.
    ///
    /// # Errors
    ///
    /// [`Refused::UnusableBound`], carrying both numbers it was given.
    pub fn new(records: usize, bytes: usize) -> Result<Self, Refused> {
        todo!()
    }

    /// The most records one batch holds.
    #[must_use]
    pub fn records(&self) -> usize {
        self.records
    }

    /// The most payload bytes one batch holds.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

/// What a seal asks of the device, once the bytes are written.
///
/// `4.3-write-path-adr` owns this decision — whether an append waits for the device is a property
/// of what the store *is*, decided once and shipped in the build — and this file takes it as given,
/// because a probe has no store and no clock. The two answers that matter to the accounting are
/// "the device is asked once per batch" and "the bytes are written and not waited for".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    /// One device call after every seal — the policy the step calls `per-record`, asked once per
    /// batch instead of once per record.
    PerBatch,
    /// Write the bytes and do not wait for the device.
    Never,
}

/// Why the write path refused to do something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// A bound that can never be satisfied: one of the two numbers is zero.
    UnusableBound {
        /// The record bound it was given.
        records: usize,
        /// The byte bound it was given.
        bytes: usize,
    },
    /// The store refused the call, after `frames` frames: a store that filled up. Nothing a failed
    /// call would have charged is charged, and the records behind it are still the batch's.
    Store {
        /// Frames the sink had taken before it refused this one.
        frames: usize,
    },
}

/// The five counters `lakelet/4.5-batch-writes` froze, under this file's names.
///
/// The step's `Spool::stats` calls them `records`, `bytes`, `calls`, `syncs` and `batches`; here a
/// frame is one call and one batch, so `frames_out` is the records that have left, `seals` is the
/// batches and `flushes` is the device calls. The counting *is* the interface — the budget lane and
/// the daemon's stop line both read it — so the counters and the ways the write path charges them
/// are given, and charging them where the calls happen is the exercise.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Records the batcher took in.
    records_in: u64,
    /// Of those, the ones that have left in a sealed frame.
    frames_out: u64,
    /// Bytes written to the sink, framing included.
    bytes_written: u64,
    /// Frames sealed: the write calls the write path made.
    seals: u64,
    /// Device calls asked for.
    flushes: u64,
}

impl Stats {
    /// Records the batcher has taken in.
    #[must_use]
    pub fn records_in(&self) -> u64 {
        self.records_in
    }

    /// Of those, the ones that have left in a sealed frame.
    #[must_use]
    pub fn frames_out(&self) -> u64 {
        self.frames_out
    }

    /// Bytes written to the sink, framing included.
    #[must_use]
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Frames sealed: the write calls the write path made.
    #[must_use]
    pub fn seals(&self) -> u64 {
        self.seals
    }

    /// Device calls asked for.
    #[must_use]
    pub fn flushes(&self) -> u64 {
        self.flushes
    }

    /// Charge one record the batcher took in.
    pub fn charge_in(&mut self, records: u64) {
        self.records_in += records;
    }

    /// Charge one seal: `records` records and `bytes` on the wire, in the one call it took.
    pub fn charge_frame(&mut self, records: u64, bytes: u64) {
        self.frames_out += records;
        self.bytes_written += bytes;
        self.seals += 1;
    }

    /// Charge one device call, where it was asked for.
    pub fn charge_flush(&mut self) {
        self.flushes += 1;
    }
}

/// Where a sealed frame goes: the store, in one call.
///
/// Given rather than written, because it is the *seam*: the counters exist to say how many times the
/// write path asked the kernel to do something, so the thing it asks is the one thing a probe can
/// put a counting stand-in behind.
pub trait Sink {
    /// Write one frame: one record per payload, each framed as `[length: u32 LE][payload]`.
    ///
    /// # Errors
    ///
    /// [`Refused::Store`] when the store refused the call.
    fn write(&mut self, frame: &[u8]) -> Result<(), Refused>;

    /// Ask the device to make the bytes written so far durable.
    ///
    /// # Errors
    ///
    /// [`Refused::Store`] when the device refused.
    fn sync(&mut self) -> Result<(), Refused>;
}

/// A sink that keeps every frame it was handed: the tests' store.
///
/// Given because a frame's bytes are the contract every counter in this file is stated in — the
/// reader below is what a frame *is*, so a batcher that framed its records some other way fails the
/// cases that read them back rather than the cases that count bytes.
#[derive(Debug, Default)]
pub struct Log {
    /// Every frame, in the order it was written.
    frames: Vec<Payload>,
    /// How many times the device was asked.
    syncs: usize,
    /// How many frames this store takes before it refuses every later one.
    refuses_after: Option<usize>,
}

impl Log {
    /// An empty log that refuses nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A log that takes `after` frames and refuses every one after them: a store that filled up.
    #[must_use]
    pub fn refusing_after(after: usize) -> Self {
        Self {
            refuses_after: Some(after),
            ..Self::default()
        }
    }

    /// Every frame, in the order it was written.
    #[must_use]
    pub fn frames(&self) -> &[Payload] {
        &self.frames
    }

    /// How many times the device was asked.
    #[must_use]
    pub fn syncs(&self) -> usize {
        self.syncs
    }

    /// The records a reader that never went through a batcher reads out of the frames.
    #[must_use]
    pub fn records(&self) -> Vec<Payload> {
        let mut records = Vec::new();
        for frame in &self.frames {
            let mut at = 0;
            while at + 4 <= frame.len() {
                let length =
                    u32::from_le_bytes(frame[at..at + 4].try_into().expect("four length bytes"))
                        as usize;
                at += 4;
                let Some(record) = frame.get(at..at + length) else {
                    break;
                };
                records.push(record.to_vec());
                at += length;
            }
        }
        records
    }
}

impl Sink for Log {
    fn write(&mut self, frame: &[u8]) -> Result<(), Refused> {
        if self
            .refuses_after
            .is_some_and(|after| self.frames.len() >= after)
        {
            return Err(Refused::Store {
                frames: self.frames.len(),
            });
        }
        self.frames.push(frame.to_vec());
        Ok(())
    }

    fn sync(&mut self) -> Result<(), Refused> {
        self.syncs += 1;
        Ok(())
    }
}

/// The batcher: what has arrived and not been sealed, and what the write path has cost.
///
/// The fields are given, because the bound and the counters are the contract other steps read; what
/// is not given is the decision, which is [`Batcher::push`] and [`Batcher::seal`].
#[derive(Debug)]
pub struct Batcher {
    /// The capacity one batch is sealed at.
    bound: Bound,
    /// What a seal asks of the device.
    device: Device,
    /// The records that have arrived and not been sealed, in the order they arrived.
    pending: Vec<Payload>,
    /// The payload bytes those records carry.
    pending_bytes: usize,
    /// What this write path has cost.
    stats: Stats,
}

impl Batcher {
    /// A batcher that seals at `bound` and asks the device as `device` says.
    #[must_use]
    pub fn new(bound: Bound, device: Device) -> Self {
        Self {
            bound,
            device,
            pending: Vec::new(),
            pending_bytes: 0,
            stats: Stats::default(),
        }
    }

    /// Records held right now: the third term of `records_in == frames_out + pending`.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Whether nothing is waiting to be sealed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Payload bytes held right now.
    #[must_use]
    pub fn pending_bytes(&self) -> usize {
        self.pending_bytes
    }

    /// What this write path has cost since it was built.
    #[must_use]
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Take one record in, sealing what has to be sealed to make room for it.
    ///
    /// The record is never split and never dropped: what is pending is sealed *before* a record
    /// that would take the batch over its bound, so the boundary moves to before that record
    /// instead of through it, and a record larger than the whole bound is a batch of one. A batch
    /// that is full is sealed as soon as it is full — a batcher that waited for the next record, or
    /// for a timer, would be holding records that a caller has already been answered for
    /// (`4.6-ingest-backpressure` is where a bound in front of the write path belongs).
    ///
    /// `records_in` is charged here, where the record is taken: a record the batcher did not take
    /// because it could not seal the batch in front of it is the caller's, and is not counted.
    ///
    /// # Errors
    ///
    /// Whatever the seal it had to make answered: [`Refused::Store`] leaves the pending batch and
    /// its records where they were, so the same batcher can be retried against a sink that works.
    pub fn push(&mut self, payload: &[u8], sink: &mut impl Sink) -> Result<(), Refused> {
        todo!()
    }

    /// Seal what is waiting as one frame, in one call.
    ///
    /// One call to [`Sink::write`] carrying every pending record in the order it arrived, framed as
    /// [`Sink::write`] documents, then — under [`Device::PerBatch`] — one call to [`Sink::sync`].
    /// Nothing waiting is not a write: no call, no frame, and no counter moves. `seals`,
    /// `bytes_written` and `frames_out` are charged where the call is made, and `flushes` where the
    /// device is asked, because a counter the caller charges reports what the caller believes it
    /// did.
    ///
    /// # Errors
    ///
    /// [`Refused::Store`] from the call that failed: nothing about a frame that was not written is
    /// charged, and the records are still the batch's, because a frame that is not on the store is
    /// not acknowledged.
    pub fn seal(&mut self, sink: &mut impl Sink) -> Result<(), Refused> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The payload every case writes: sixty-four bytes, so two of them are over a 70-byte bound.
    const PAYLOAD: usize = 64;

    /// `count` payloads of [`PAYLOAD`] bytes, each one its own bytes.
    ///
    /// Derived from the index rather than zero-filled: a batcher that reordered two records,
    /// duplicated one or dropped one cannot pass by agreeing with a pattern.
    fn payloads(count: usize) -> Vec<Payload> {
        (0..count)
            .map(|index| {
                (0..PAYLOAD)
                    .map(|offset| (index as u8).wrapping_mul(31).wrapping_add(offset as u8))
                    .collect()
            })
            .collect()
    }

    /// A batcher of `records` records and `bytes` bytes of room, asking the device as `device` says.
    fn new_batcher(records: usize, bytes: usize, device: Device) -> Batcher {
        let bound = Bound::new(records, bytes).unwrap_or_else(|refused| {
            panic!("a bound of {records} record(s) and {bytes} byte(s) is a bound: {refused:?}")
        });
        Batcher::new(bound, device)
    }

    /// What one frame of `count` [`PAYLOAD`]-byte records costs on the wire: a four-byte length in
    /// front of every record, which is the number the counter has to agree with.
    fn framed_len(count: usize) -> usize {
        count * (4 + PAYLOAD)
    }

    #[test]
    fn an_empty_seal_writes_nothing_and_costs_nothing() {
        let mut sink = Log::new();
        let mut batcher = new_batcher(64, 4096, Device::PerBatch);

        batcher
            .seal(&mut sink)
            .expect("a seal with nothing waiting is not a failure");
        assert!(batcher.is_empty(), "and it leaves nothing behind");
        assert_eq!(
            batcher.stats(),
            Stats::default(),
            "a seal with nothing to write is not a write: no call, no frame, and no counter moved"
        );
        assert!(sink.frames().is_empty(), "nothing reached the store");
        assert_eq!(
            sink.syncs(),
            0,
            "and the device was not asked to make nothing durable"
        );

        // Sealing twice with nothing in between is the same nothing, twice.
        batcher
            .seal(&mut sink)
            .expect("a second empty seal is not a failure either");
        assert_eq!(batcher.stats(), Stats::default());
        assert!(sink.frames().is_empty());
    }

    #[test]
    fn a_batch_is_one_write_call_whatever_the_batch_size() {
        let payloads = payloads(64);
        let mut sink = Log::new();
        let mut batcher = new_batcher(64, 4096, Device::PerBatch);
        for payload in &payloads {
            batcher
                .push(payload, &mut sink)
                .expect("a batch of sixty-four records");
        }

        assert_eq!(
            sink.frames().len(),
            1,
            "sixty-four records in one batch are one write call: a write path that opened the file \
             once per record would have made sixty-four"
        );
        assert_eq!(
            sink.syncs(),
            1,
            "and the device was asked once, because the policy is asked once per call"
        );
        let stats = batcher.stats();
        assert_eq!(
            (stats.records_in(), stats.frames_out(), stats.seals()),
            (64, 64, 1),
            "sixty-four records in, sixty-four out, in one call: the calls per record is what a \
             batch is for"
        );
        assert_eq!(batcher.len(), 0, "a sealed batch is not pending");
        assert_eq!(
            sink.records(),
            payloads,
            "and the frame the call wrote holds every record, in the order it arrived"
        );
    }

    #[test]
    fn the_batch_changes_the_calls_and_not_the_bytes() {
        let payloads = payloads(8);

        let mut batched = Log::new();
        let mut together = new_batcher(8, 4096, Device::Never);
        for payload in &payloads {
            together
                .push(payload, &mut batched)
                .expect("one batch of eight");
        }

        let mut one_at_a_time = Log::new();
        let mut separately = new_batcher(1, 4096, Device::Never);
        for payload in &payloads {
            separately
                .push(payload, &mut one_at_a_time)
                .expect("eight batches of one");
        }

        assert_eq!(
            batched.records(),
            one_at_a_time.records(),
            "a batch is the bytes N appends write, down to the last one: the format may not notice \
             that the records arrived together"
        );
        assert_eq!(
            together.stats().bytes_written(),
            separately.stats().bytes_written(),
            "and the bytes did not move: the same four-byte length in front of every record, one \
             frame or eight of them"
        );
        assert_eq!(
            together.stats().bytes_written(),
            framed_len(8) as u64,
            "eight 64-byte records are 544 bytes with their framing, which is the number the \
             page's bytes per payload is taken from"
        );
        assert_eq!(together.stats().seals(), 1);
        assert_eq!(
            separately.stats().seals(),
            8,
            "eight records written one at a time are eight calls, which is the coefficient this \
             step exists to bring down"
        );
        assert_eq!(
            together.stats().records_in(),
            separately.stats().records_in()
        );
    }

    #[test]
    fn the_count_bound_seals_the_batch_at_the_bound_and_not_the_record() {
        let payloads = payloads(5);
        let mut sink = Log::new();
        let mut batcher = new_batcher(2, 4096, Device::Never);

        batcher
            .push(&payloads[0], &mut sink)
            .expect("the first record");
        assert_eq!(batcher.len(), 1, "one record is not a full batch of two");
        assert!(
            sink.frames().is_empty(),
            "and nothing has been written yet: a batch of one is what a bound of one would have \
             written, and this bound is two"
        );
        batcher
            .push(&payloads[1], &mut sink)
            .expect("the record that fills it");
        assert_eq!(
            batcher.len(),
            0,
            "the record that completed the batch is in the sealed batch, not left pending for \
             the next call: a batch of three records out of a bound of two is one call too few"
        );
        assert_eq!(sink.frames().len(), 1);

        batcher
            .push(&payloads[2], &mut sink)
            .expect("the third record");
        batcher
            .push(&payloads[3], &mut sink)
            .expect("the fourth record");
        assert_eq!(
            sink.frames().len(),
            2,
            "four records and a bound of two are two batches"
        );

        // The fifth is held, and the invariant says so from both directions.
        batcher
            .push(&payloads[4], &mut sink)
            .expect("the fifth record");
        let stats = batcher.stats();
        assert_eq!(stats.records_in(), 5, "five records arrived");
        assert_eq!(stats.frames_out(), 4, "four of them left in sealed frames");
        assert_eq!(batcher.len(), 1, "and one is still here");
        assert_eq!(
            stats.records_in(),
            stats.frames_out() + batcher.len() as u64,
            "`records_in == frames_out + pending`: every record that came in has left or is still \
             here, and no record is counted twice"
        );
        assert_eq!(
            sink.records(),
            payloads[..4].to_vec(),
            "and the sealed frames hold the first four, in the order they arrived"
        );
    }

    #[test]
    fn the_size_bound_seals_before_the_record_that_would_not_fit() {
        let payloads = payloads(2);
        let mut sink = Log::new();
        // Seventy bytes of room and sixty-four-byte records: two records do not fit, so the first
        // one goes out on its own and the second starts a batch of its own.
        let mut batcher = new_batcher(1024, 70, Device::Never);

        batcher
            .push(&payloads[0], &mut sink)
            .expect("the first record");
        assert!(
            sink.frames().is_empty(),
            "one 64-byte record fits in 70 bytes of room, so it waits for a batch mate"
        );
        batcher
            .push(&payloads[1], &mut sink)
            .expect("the record that does not fit");
        assert_eq!(
            sink.frames().len(),
            1,
            "the record that would have taken the batch over its bound sealed what was pending \
             first, and is pending itself"
        );
        assert_eq!(
            sink.records(),
            payloads[..1].to_vec(),
            "and what went out is the first record alone: the arriving one was not split to fill \
             the gap, because a split record is bytes no append would have written"
        );
        assert_eq!(
            batcher.len(),
            1,
            "the arriving record is waiting for the next batch"
        );
        let stats = batcher.stats();
        assert_eq!(
            (stats.records_in(), stats.frames_out()),
            (2, 1),
            "two records in, one of them sealed"
        );
        assert_eq!(
            stats.bytes_written(),
            framed_len(1) as u64,
            "and the call carried one record: the boundary moved to before the record that did \
             not fit, not through it"
        );

        // And the batch that was held is a batch: one more seal and both are on the store.
        batcher.seal(&mut sink).expect("the batch that was waiting");
        assert_eq!(sink.records(), payloads);
        assert_eq!(batcher.stats().frames_out(), 2);
    }

    #[test]
    fn a_record_larger_than_the_whole_bound_is_a_batch_of_one() {
        let payloads = payloads(1);
        let mut sink = Log::new();
        let mut batcher = new_batcher(1024, 8, Device::Never);

        batcher
            .push(&payloads[0], &mut sink)
            .expect("a record with no room to wait for a mate");
        assert_eq!(
            sink.frames().len(),
            1,
            "a 64-byte record in 8 bytes of room is a batch of one: the bound is a batch's \
             capacity, not a limit on a record"
        );
        assert_eq!(
            sink.records(),
            payloads,
            "and the record is whole — nothing was truncated to fit the bound, and nothing was \
             refused for being too big"
        );
        assert_eq!(batcher.stats().frames_out(), 1);
        assert!(
            batcher.is_empty(),
            "and it did not wait for a mate that could never fit"
        );
    }

    #[test]
    fn a_refused_call_charges_nothing_and_keeps_the_records() {
        let payloads = payloads(2);

        // The store refuses the very first frame: the record that filled the batch is still here,
        // and nothing was charged for a call that did not happen.
        let mut refusing = Log::refusing_after(0);
        let mut batcher = new_batcher(1, 4096, Device::Never);
        assert_eq!(
            batcher.push(&payloads[0], &mut refusing),
            Err(Refused::Store { frames: 0 }),
            "the write path answers the store's refusal, because a frame that was not written is \
             not one to acknowledge"
        );
        assert_eq!(batcher.len(), 1, "the record is still the batch's");
        let stats = batcher.stats();
        assert_eq!(
            (
                stats.records_in(),
                stats.frames_out(),
                stats.seals(),
                stats.bytes_written()
            ),
            (1, 0, 0, 0),
            "nothing about the failed call is charged: a counter that moved here would report a \
             frame that is not on the store"
        );
        assert!(refusing.frames().is_empty(), "and the store holds nothing");

        // The retry is the same batcher and the same pending batch, against a store that works.
        let mut working = Log::new();
        batcher
            .seal(&mut working)
            .expect("the retry of the batch the refusal left behind");
        assert_eq!(working.records(), payloads[..1].to_vec());
        assert_eq!(
            batcher.stats().records_in(),
            batcher.stats().frames_out(),
            "and now every record that came in has left"
        );

        // A record the batcher did not take is the caller's: a seal that failed before the record
        // went in leaves `records_in` where it was.
        let mut refusing = Log::refusing_after(0);
        let mut bounded = new_batcher(1024, 70, Device::Never);
        bounded
            .push(&payloads[0], &mut refusing)
            .expect("nothing pending always has room");
        assert_eq!(
            bounded.push(&payloads[1], &mut refusing),
            Err(Refused::Store { frames: 0 }),
            "the second record does not fit, so the batch in front of it had to be sealed"
        );
        assert_eq!(
            bounded.stats().records_in(),
            1,
            "the record that did not fit was not taken in, so it is not counted: `records_in` is \
             what the batcher holds"
        );
        assert_eq!(
            bounded.len(),
            1,
            "and what is pending is the batch that could not be written"
        );
    }

    #[test]
    fn the_device_call_is_the_policys_and_a_batch_pays_for_one() {
        let payloads = payloads(8);

        let mut eager = Log::new();
        let mut batcher = new_batcher(8, 4096, Device::PerBatch);
        for payload in &payloads {
            batcher.push(payload, &mut eager).expect("a batch of eight");
        }
        assert_eq!(
            eager.syncs(),
            1,
            "one device call for a batch of eight, and not eight: the policy is asked once per \
             call, which is the difference between a store that keeps up and one that cannot"
        );
        assert_eq!(
            batcher.stats().flushes(),
            1,
            "and the counter is charged there"
        );

        let mut lazy = Log::new();
        let mut batcher = new_batcher(8, 4096, Device::Never);
        for payload in &payloads {
            batcher.push(payload, &mut lazy).expect("a batch of eight");
        }
        assert_eq!(
            lazy.syncs(),
            0,
            "a policy that never waits asks for no device call"
        );
        assert_eq!(batcher.stats().flushes(), 0);
        assert_eq!(
            lazy.records(),
            eager.records(),
            "and the bytes are written either way: written is readable, durable is what a policy \
             asks for"
        );
        assert_eq!(
            batcher.stats().bytes_written(),
            framed_len(8) as u64,
            "the device call did not change what was written"
        );
    }

    #[test]
    fn the_counters_are_what_the_sink_saw() {
        let payloads = payloads(5);
        let mut sink = Log::new();
        let mut batcher = new_batcher(3, 4096, Device::PerBatch);
        for payload in &payloads {
            batcher.push(payload, &mut sink).expect("a batch of three");
        }
        batcher.seal(&mut sink).expect("the two that were left");

        let stats = batcher.stats();
        assert_eq!(
            stats.bytes_written(),
            sink.frames().iter().map(Vec::len).sum::<usize>() as u64,
            "the counter is the bytes that reached the store, not the payload bytes the caller \
             handed over"
        );
        assert_eq!(
            stats.bytes_written(),
            (framed_len(3) + framed_len(2)) as u64,
            "a batch of three and a batch of two: {}+{} bytes for five records, which is the \
             framing the page's bytes per payload includes",
            framed_len(3),
            framed_len(2)
        );
        assert_eq!(
            stats.seals(),
            sink.frames().len() as u64,
            "every seal is a frame, and every frame is a seal"
        );
        assert_eq!(
            stats.frames_out(),
            sink.records().len() as u64,
            "and the records that left are the records a reader finds in the frames"
        );
        assert_eq!(
            stats.records_in(),
            stats.frames_out() + batcher.len() as u64,
            "five in, five out, nothing pending"
        );
        assert_eq!(sink.records(), payloads, "in the order they arrived");
    }

    #[test]
    fn a_bound_nobody_can_seal_against_is_refused() {
        assert_eq!(
            Bound::new(0, 4096),
            Err(Refused::UnusableBound {
                records: 0,
                bytes: 4096
            }),
            "a batch that can never hold a record is not a batch: the refusal names both numbers, \
             because a deployment that wrote one of them has to see which"
        );
        assert_eq!(
            Bound::new(64, 0),
            Err(Refused::UnusableBound {
                records: 64,
                bytes: 0
            }),
            "and a bound with no room for a byte is the same mistake on the other axis"
        );
        let bound =
            Bound::new(64, 4096).expect("one record of room and one byte of room is enough");
        assert_eq!((bound.records(), bound.bytes()), (64, 4096));
    }
}
