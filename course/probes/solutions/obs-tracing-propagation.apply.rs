//! probe = "obs/tracing-propagation@apply"
//! title = "The trace comes in, a span of your own goes out"
//! hint = "Read the four fields, keep the trace id and the flags, mint a span id per hop, and drop the vendor state with a header you could not read."
//!
//! Step `relay/2.19-trace-propagation` carries one trace id from a caller's request to a
//! subscriber, across a queue, through a worker that never saw the request. The header that carries
//! it is the W3C [`traceparent`](https://www.w3.org/TR/trace-context/):
//! `<version>-<trace-id>-<span-id>-<flags>`, 32 and 16 lower-case hex characters, exactly four
//! fields. The discipline around it is three sentences.
//!
//! 1. **the trace id is the caller's and the span id is not.** A span is one hop. Reusing the
//!    caller's span makes a four-second delivery look like a four-second request and the queue
//!    between them disappears from the waterfall; and two attempts at one delivery are two spans,
//!    or the retry that is worth looking at is invisible;
//! 2. **the header is parsed and regenerated, never forwarded.** What a subscriber receives is a
//!    header this service built from the trace id it stored, so a caller with broken bytes cannot
//!    reproduce their brokenness in somebody else's collector. A header this service cannot read is
//!    not an error either: the trace restarts, the event is delivered, and the vendor state that
//!    arrived beside the unreadable header goes with it — a `tracestate` means something only next
//!    to the trace it came with;
//! 3. **the sampling decision is propagated rather than made.** A hop with no sampler has no
//!    business overriding the sampler that started the trace, so the flags travel whole.
//!
//! ## What you write
//!
//! * [`TraceId::parse`] and [`SpanId::parse`] — lower-case hex of the right length, and never the
//!   all-zero id the format reserves for "there is no trace";
//! * [`Traceparent::parse`] — the four fields of the header, and [`Traceparent::sampled`];
//! * [`Context::from_headers`] — what a request's two headers mean, which is the trace, the flags
//!   and the vendor state, or `None` for a header this service cannot read;
//! * [`Context::start`] — the trace to use when there was no readable parent;
//! * [`Context::header`] — the header one hop sends, built from the stored trace and a span id
//!   minted for that hop.
//!
//! [`TraceId::from_bytes`], [`TraceId::to_hex`], [`SpanId`]'s two, [`Context`]'s fields and the
//! lower-case hex reader are given: reading and writing hex is not the lesson, the lengths, the
//! zeroes and the hop are. [`IdSource`] is the injection point for a new trace — see below.
//!
//! ## What the tests ask
//!
//! A readable header keeps the trace id, the span id and the sampling bit, and the vendor state
//! travels with it byte for byte. A hop's header carries the caller's trace and a span id that is
//! neither the caller's nor the previous hop's. Ten unreadable headers — wrong lengths, upper case,
//! non-hex, all zeroes, a version this service does not speak, five fields — are `None` rather than
//! an error, and starting a trace from there yields a well-formed header with no vendor state. And
//! a header this service sent is one it can read back.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, and the clock, the socket and the subscriber are all absent: this is
//! the header's grammar and the one rule about which ids belong to whom. Two things are injected
//! rather than real, and both are stated in the open rather than hidden. **The ids of a new trace
//! come from [`IdSource`]** — the service fills a buffer from `rand`, and a probe that reads the
//! system's entropy cannot be run twice on the same numbers; what is re-checked here is that a
//! started trace *has* an id of its own and that a stored one is preserved. **The version this
//! service speaks is `00` alone**: the format's forward-compatible rule would have a reader keep
//! the four fields of a `01` header and ignore the extra ones, and a service that regenerates the
//! header is better off restarting the trace than forwarding semantics it has not read.

use std::fmt;

/// The version this service speaks, and the only one it reads.
pub const VERSION: &str = "00";

/// The flag bit that says the sampler that started this trace kept it.
pub const SAMPLED: u8 = 0x01;

/// One lower-case hex digit, or `None`.
fn digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// The bytes a run of lower-case hex spells.
///
/// `None` for anything else: upper case is a different string, not the same id, and a stray
/// character or an odd length is not hex at all. The checks that belong to *this* format — 32
/// characters for a trace, 16 for a span, never all zeroes — are the ones you write.
#[must_use]
fn hex(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || text.len() % 2 != 0 {
        return None;
    }
    let digits = text.as_bytes();
    let mut bytes = Vec::with_capacity(digits.len() / 2);
    for pair in digits.chunks(2) {
        let (hi, lo) = (digit(pair[0])?, digit(pair[1])?);
        bytes.push((hi << 4) | lo);
    }
    Some(bytes)
}

/// A 128-bit trace id: 32 lower-case hex characters, and never the all-zero one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TraceId([u8; 16]);

impl TraceId {
    /// The id these bytes spell, or `None` for the all-zero id the format reserves for "there is
    /// no trace": accepting it would put every caller who sends it into one trace together, and
    /// that trace is nobody's.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 16]) -> Option<Self> {
        if bytes == [0; 16] {
            None
        } else {
            Some(Self(bytes))
        }
    }

    /// Parse the 32 lower-case hex characters of a trace id.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        // The length is the format's, so it is checked before the bytes are: `hex` is happy with
        // any run of hex, and a 30-character one is not a trace id that happens to be short.
        if text.len() != 32 {
            return None;
        }
        let bytes: [u8; 16] = hex(text)?.try_into().ok()?;
        Self::from_bytes(bytes)
    }

    /// The id as a header spells it: 32 characters, lower case.
    #[must_use]
    pub fn to_hex(self) -> String {
        format!("{:032x}", u128::from_be_bytes(self.0))
    }
}

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// A 64-bit span id: 16 lower-case hex characters, and never the all-zero one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpanId([u8; 8]);

impl SpanId {
    /// The id these bytes spell, or `None` for the all-zero id.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 8]) -> Option<Self> {
        if bytes == [0; 8] {
            None
        } else {
            Some(Self(bytes))
        }
    }

    /// Parse the 16 lower-case hex characters of a span id.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() != 16 {
            return None;
        }
        let bytes: [u8; 8] = hex(text)?.try_into().ok()?;
        Self::from_bytes(bytes)
    }

    /// The id as a header spells it: 16 characters, lower case.
    #[must_use]
    pub fn to_hex(self) -> String {
        format!("{:016x}", u64::from_be_bytes(self.0))
    }
}

impl fmt::Display for SpanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// One `traceparent` header, read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Traceparent {
    /// The trace this hop belongs to: the caller's, kept.
    pub trace: TraceId,
    /// The span the caller's hop ran in. This service does not send it on: it is the caller's hop.
    pub span: SpanId,
    /// The caller's flags, whole. Bit [`SAMPLED`] is the sampling decision and the rest is not this
    /// service's to interpret.
    pub flags: u8,
}

impl Traceparent {
    /// Parse `00-<trace>-<span>-<flags>`.
    ///
    /// Four fields exactly, this service's version, lower-case hex of 32, 16 and 2 characters, and
    /// neither id all zeroes. Anything else is `None`: this header goes straight into a trace, and a
    /// field this service guessed at is a link in somebody else's waterfall that means something
    /// different than it says.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        // Four fields, exactly: `split` plus one `next` for the fifth is how the format's "and
        // there is nothing after it" is checked without counting separators by hand.
        let mut fields = text.split('-');
        let version = fields.next()?;
        let trace = fields.next()?;
        let span = fields.next()?;
        let flags = fields.next()?;
        if version != VERSION || fields.next().is_some() {
            return None;
        }
        // `hex` is happy with any even run; the flags are one byte, so the length is checked here.
        let flags: [u8; 1] = hex(flags)?.try_into().ok()?;
        Some(Self {
            trace: TraceId::parse(trace)?,
            span: SpanId::parse(span)?,
            flags: flags[0],
        })
    }

    /// Is the sampler that started this trace keeping it?
    #[must_use]
    pub fn sampled(self) -> bool {
        self.flags & SAMPLED == SAMPLED
    }
}

/// Where a trace this service starts gets its ids.
///
/// The real service fills a buffer from `rand`. It is a parameter here — and only here — so that a
/// trace started inside a test is the same trace every time, which is what lets the tests below say
/// "an id of its own" instead of "an id".
pub trait IdSource {
    /// An id for a trace this service starts.
    fn trace_id(&mut self) -> TraceId;

    /// An id for one hop of a trace that is already running.
    fn span_id(&mut self) -> SpanId;
}

/// What crosses the boundary: the trace, the sampling decision, and the caller's vendor state.
///
/// No span id, deliberately: a span is one hop's, and the delivery is a different hop from the
/// request. Two attempts at the same delivery are two hops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    /// The trace this service keeps for the event and every delivery it fans out to.
    pub trace: TraceId,
    /// The caller's flags, propagated rather than decided.
    pub flags: u8,
    /// The caller's `tracestate`, kept only while the trace it arrived with is kept. An empty
    /// header is not vendor state.
    pub tracestate: Option<String>,
}

impl Context {
    /// What a request's two headers mean to this service, or `None` when the `traceparent` is not
    /// one this service can read — in which case the caller's `tracestate` is dropped too, because
    /// vendor state means something only beside the trace it arrived with.
    ///
    /// `None` is not an error the caller hears about: the layer starts a trace instead
    /// ([`Context::start`]) and the event is accepted, because a lost parent link is a worse trace
    /// and a refused event is a worse service.
    #[must_use]
    pub fn from_headers(traceparent: Option<&str>, tracestate: Option<&str>) -> Option<Self> {
        // The header is the whole gate: without a readable one there is no trace to keep the vendor
        // state beside, so both answers come from here rather than from the caller's second header.
        let parent = Traceparent::parse(traceparent?)?;
        Some(Self {
            trace: parent.trace,
            flags: parent.flags,
            tracestate: tracestate
                .filter(|state| !state.is_empty())
                .map(str::to_owned),
        })
    }

    /// A trace with no parent: an id from this service's source, and nothing else.
    ///
    /// The flags are `0`: this service has no sampler, so a trace it starts is one nobody has
    /// decided to keep. Stamping `SAMPLED` here would make the decision on behalf of the collector
    /// that will read it.
    #[must_use]
    pub fn start<I: IdSource>(ids: &mut I) -> Self {
        Self {
            trace: ids.trace_id(),
            flags: 0,
            tracestate: None,
        }
    }

    /// The header this hop sends: `00-<trace>-<span>-<flags>`, from the stored trace and a span id
    /// minted for *this* hop.
    #[must_use]
    pub fn header(&self, span: SpanId) -> String {
        // Built, never forwarded: the trace id is this service's stored one and the span id is the
        // hop's own, which is what makes the header one this service can read back.
        format!(
            "{VERSION}-{}-{}-{:02x}",
            self.trace.to_hex(),
            span.to_hex(),
            self.flags
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The caller in the format's own example, and the span of their hop.
    const CALLER: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    /// The vendor state that arrived with it.
    const VENDOR: &str = "congo=t61rcWkgMzE,rojo=00f067aa0ba902b7";

    /// An [`IdSource`] that counts, so "a new id" is a number a test can name.
    struct Sequence {
        next: u64,
    }

    impl Sequence {
        fn new() -> Self {
            Self { next: 0 }
        }

        fn bytes<const N: usize>(&mut self, fill: u8) -> [u8; N] {
            let mut bytes = [fill; N];
            let counter = self.next.to_be_bytes();
            self.next += 1;
            bytes[N - 2] = counter[6];
            bytes[N - 1] = counter[7];
            bytes
        }
    }

    impl IdSource for Sequence {
        fn trace_id(&mut self) -> TraceId {
            TraceId::from_bytes(self.bytes::<16>(0x11)).expect("a non-zero id")
        }

        fn span_id(&mut self) -> SpanId {
            SpanId::from_bytes(self.bytes::<8>(0x22)).expect("a non-zero id")
        }
    }

    #[test]
    fn a_readable_header_keeps_the_trace_the_span_and_the_sampling_decision() {
        let context = Context::from_headers(Some(CALLER), Some(VENDOR)).expect("a readable header");

        assert_eq!(
            context.trace.to_hex(),
            "4bf92f3577b34da6a3ce929d0e0e4736",
            "the trace id is the caller's, character for character"
        );
        assert_eq!(
            Traceparent::parse(CALLER).expect("readable").span.to_hex(),
            "00f067aa0ba902b7",
            "the caller's span is read, and then belongs to the caller's hop"
        );
        assert_eq!(context.flags, 0x01);
        assert!(
            Traceparent::parse(CALLER).expect("readable").sampled(),
            "the sampler that started this trace kept it, and this hop answers the same"
        );
        assert!(
            !Traceparent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00")
                .expect("readable")
                .sampled(),
            "a trace nobody sampled is not kept by a hop that has no sampler"
        );
    }

    #[test]
    fn a_hop_takes_a_span_of_its_own_and_the_trace_is_still_the_callers() {
        let context = Context::from_headers(Some(CALLER), None).expect("a readable header");
        let mut ids = Sequence::new();

        let first = context.header(ids.span_id());
        let second = context.header(ids.span_id());
        assert_ne!(
            first, second,
            "two attempts at one delivery are two hops, so they are two headers"
        );

        // The fixture has to be readable, or the two assertions below check the caller's own ids
        // against two copies of themselves.
        let caller = Traceparent::parse(CALLER).expect("a readable fixture");
        for header in [&first, &second] {
            let hop = Traceparent::parse(header).expect("this service sends readable headers");
            assert_eq!(
                hop.trace, caller.trace,
                "the trace id is the caller's: this is one trace, not three"
            );
            assert_ne!(
                hop.span, caller.span,
                "a span is one hop: reusing the caller's makes the queue between them vanish"
            );
            assert_eq!(
                hop.flags, caller.flags,
                "the sampling decision is propagated, not made here"
            );
        }
    }

    #[test]
    fn a_header_this_service_cannot_read_starts_a_trace_and_drops_the_vendor_state() {
        let unreadable = [
            "",
            "4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-",
            "FF-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
            "00-4bf92f3577b34da6a3ce929d0e0e473-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-0x",
            "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        ];
        let mut ids = Sequence::new();

        for header in unreadable {
            assert_eq!(
                Context::from_headers(Some(header), Some(VENDOR)),
                None,
                "`{header}` is not a header this service can read"
            );
            let fresh = Context::from_headers(Some(header), Some(VENDOR))
                .unwrap_or_else(|| Context::start(&mut ids));
            assert!(
                fresh.tracestate.is_none(),
                "a `tracestate` kept beside a trace that restarted is somebody else's baggage \
                 under this service's trace id"
            );
            assert!(
                Traceparent::parse(&fresh.header(ids.span_id())).is_some(),
                "the event still gets a trace, and that trace is one this service can read"
            );
        }

        assert_eq!(
            Context::from_headers(None, Some(VENDOR)),
            None,
            "vendor state with no trace beside it is not a context either"
        );
    }

    #[test]
    fn the_vendor_state_travels_unchanged_when_the_trace_does() {
        let context = Context::from_headers(Some(CALLER), Some(VENDOR)).expect("a readable header");
        assert_eq!(
            context.tracestate.as_deref(),
            Some(VENDOR),
            "the vendor state is passed on as the caller wrote it, not re-encoded through this \
             service's own idea of it"
        );

        assert_eq!(
            Context::from_headers(Some(CALLER), Some(""))
                .expect("readable")
                .tracestate,
            None,
            "an empty header is not vendor state"
        );
        assert_eq!(
            Context::from_headers(Some(CALLER), None)
                .expect("readable")
                .tracestate,
            None
        );
    }

    #[test]
    fn an_id_of_the_wrong_length_or_the_wrong_case_is_not_an_id() {
        let trace = "4bf92f3577b34da6a3ce929d0e0e4736";
        assert!(
            TraceId::parse(trace).is_some(),
            "the fixture is the id the rest of this test is about"
        );
        for text in [
            &trace[..31],
            &format!("{trace}0"),
            "4BF92F3577B34DA6A3CE929D0E0E4736",
            "4bf92f3577b34da6a3ce929d0e0e473g",
            "00000000000000000000000000000000",
            "",
        ] {
            assert_eq!(TraceId::parse(text), None, "`{text}` is not a trace id");
        }

        let span = "00f067aa0ba902b7";
        assert!(SpanId::parse(span).is_some());
        for text in [
            &span[..15],
            "00f067aa0ba902b7a",
            "00F067AA0BA902B7",
            "0000000000000000",
            "",
        ] {
            assert_eq!(SpanId::parse(text), None, "`{text}` is not a span id");
        }
    }

    #[test]
    fn a_trace_starts_where_nothing_was_readable_and_with_a_trace_of_its_own() {
        let mut ids = Sequence::new();
        let first = Context::start(&mut ids);
        let second = Context::start(&mut ids);

        assert_ne!(first.trace, second.trace, "two events are two traces");
        assert_eq!(
            first.flags, 0,
            "this service has no sampler to make the decision"
        );
        assert_eq!(
            first.tracestate, None,
            "a new trace has no vendor state yet"
        );
        assert!(
            Traceparent::parse(&first.header(ids.span_id())).is_some(),
            "a header this service builds is one it can read back"
        );
    }

    #[test]
    fn the_header_this_service_sends_is_one_it_can_read_back() {
        let context = Context::from_headers(Some(CALLER), Some(VENDOR)).expect("readable");
        let mut ids = Sequence::new();
        let span = ids.span_id();
        let header = context.header(span);

        assert_eq!(
            header.matches('-').count(),
            3,
            "four fields, and the header is the only place the separator appears"
        );
        assert_eq!(
            header,
            header.to_lowercase(),
            "upper-case hex is the same id to a human and a different one to every index"
        );
        assert_eq!(
            header,
            format!("00-{}-{}-01", context.trace, span),
            "the field order is the format's: version, trace, span, flags"
        );

        let read_back = Traceparent::parse(&header).expect("its own header");
        assert_eq!(read_back.trace, context.trace);
        assert_eq!(read_back.span, span);
        assert_eq!(read_back.flags, context.flags);
    }
}
