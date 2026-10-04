//! probe = "obs/structured-logs@design"
//! title = "One line per event, the names an operator greps for, and a secret that does not survive"
//! hint = "Encode one event as one line with the fixed names first and the custom fields sorted, and redact a secret-named field and a known secret value before the line is escaped."
//!
//! Step `jobq/3.17-error-budget-alerts` is where a log stops being for a person watching a terminal
//! and becomes a contract with the tools that read it: a collector parses every line, a dashboard
//! counts a field, an operator greps for a name. A probe cannot run a collector, so this one builds
//! the events and encodes them, and asserts the text a collector would see. Three things make the
//! text a contract:
//!
//! 1. **one event, one line.** A collector that reads lines cannot tell a message with a newline in
//!    it from two events, and the second "event" has no level and no timestamp. The encoder escapes
//!    the message rather than passing it through;
//! 2. **the names are fixed and the fields are ordered.** `timestamp`, `level`, `target` and `msg`
//!    are what a query refers to; a field an operator greps for is a field that is always spelled
//!    the same way. Custom fields follow, sorted by name, so the same event is the same bytes twice;
//! 3. **a secret must not survive the encoder.** A password in a field, and the same password inside
//!    a free-form message, are both things an event carries by accident. The encoder is told the
//!    secrets the process knows and replaces them *before* it escapes, because escaping a secret
//!    turns it into a string the scrubber no longer recognises.
//!
//! ## What you write
//!
//! * [`Encoder::line`] — one event as one line: the fixed names, the fields sorted, the values
//!   rendered;
//! * [`Encoder::redact`] — every secret this encoder was told about, replaced by [`REDACTED`];
//! * [`Encoder::render`] — one value as the text after its colon, and the one rule for a field whose
//!   *name* is a [`SECRET_KEYS`] entry.
//!
//! [`Event`], [`Event::new`], [`Event::field`], [`Level`], [`Value`], [`quote`] and [`Encoder::new`]
//! are given. Escaping is not the lesson — *one event stays one line* is, and [`quote`] is what makes
//! that possible — and building an event is not either.
//!
//! ## What the tests ask
//!
//! A full event, byte for byte, with the fields sorted rather than in the order they were added. A
//! message with a newline and a quote in it is still one line. A field named like a secret — and a
//! secret-named field logged as a *number* — loses its value while keeping its name. A secret the
//! process knows is gone from the message and from a field that is not named like one, and a secret
//! that contains a quote is still found, because redaction runs before escaping.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no collector and no `serde`**: the line is JSON-shaped text the
//! tests compare as bytes, and the fake is the events the test builds, because what is re-checked is
//! what the encoder writes and not what a parser reads back. The shape a real encoder shares with
//! this one is the contract above; the crates that spell it (`tracing`, `serde_json`) are the step's
//! business and not the probe's.
//!
//! Missing on purpose: redaction by *key* is a fixed list, not a config a deployment supplies (that
//! is `cfg/layered-config@design`), the `timestamp` is the unix seconds a query can sort on rather
//! than an RFC 3339 string (`sched/cron@apply` owns turning an instant into a calendar), and nothing
//! here decides *which* events to log — this is the encoder, and the call sites are the step.

/// The field names a value is never allowed to survive under.
///
/// A fixed list rather than a rule: these are the names a team agrees on, and an operator greps for
/// one of them to confirm the line was scrubbed. Matched case-insensitively, because `Password` and
/// `password` are the same field to a person and must be the same to the encoder.
pub const SECRET_KEYS: [&str; 5] = ["password", "secret", "token", "authorization", "api_key"];

/// What a redacted value becomes. One fixed string, so a working grep and a passing test are the
/// same thing.
pub const REDACTED: &str = "[redacted]";

/// The severity of an event, and the string it is logged as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Fine-grained detail a person turns on to debug.
    Debug,
    /// The normal case.
    Info,
    /// Something is off but the request went through.
    Warn,
    /// The request did not go through.
    Error,
}

impl Level {
    /// The lower-case name a query filters on. Fixed: a level spelled two ways is two levels.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// A field's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A string, which is escaped and redacted.
    Str(String),
    /// A whole number, logged bare.
    Int(i64),
    /// A boolean, logged bare.
    Bool(bool),
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Str(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Str(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

/// One thing that happened, as a value: the four fixed fields and whatever else the call site knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Seconds since the epoch: the `timestamp` field.
    pub timestamp: u64,
    /// The `level` field.
    pub level: Level,
    /// The `target` field: which module or component logged it.
    pub target: String,
    /// The `msg` field.
    pub message: String,
    /// Everything else, in no particular order: the encoder sorts it.
    pub fields: Vec<(String, Value)>,
}

impl Event {
    /// An event with the four fixed fields and no others.
    #[must_use]
    pub fn new(timestamp: u64, level: Level, target: &str, message: &str) -> Self {
        Self {
            timestamp,
            level,
            target: target.to_owned(),
            message: message.to_owned(),
            fields: Vec::new(),
        }
    }

    /// Add a field.
    ///
    /// The order fields are added in is not the order they are logged in — see [`Encoder::line`] —
    /// so a call site can add them as it learns them.
    #[must_use]
    pub fn field(mut self, name: &str, value: impl Into<Value>) -> Self {
        self.fields.push((name.to_owned(), value.into()));
        self
    }
}

/// `text` as a JSON string literal: the quotes around it, and every character that would end the
/// line or break the parse escaped.
///
/// Given: escaping is not the lesson, but *one event stays one line* is, and this is what makes it
/// possible — a raw newline in a message becomes the two characters `\n`, so a collector that reads
/// lines sees one event rather than two.
#[must_use]
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// The encoder: the secrets it knows about, and the one rule that keeps them out of the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoder {
    /// The literal strings that must not survive, wherever they turn up.
    secrets: Vec<String>,
}

impl Encoder {
    /// An encoder told the secrets this process knows.
    #[must_use]
    pub fn new(secrets: &[&str]) -> Self {
        Self {
            secrets: secrets.iter().map(|secret| (*secret).to_owned()).collect(),
        }
    }

    /// One event as one line.
    ///
    /// The fixed names first — `timestamp`, `level`, `target`, `msg` — then the custom fields sorted
    /// by name, so the same event is the same bytes however it was built. Every string that goes
    /// into the line passes through [`Encoder::redact`] before [`quote`] escapes it: the order
    /// matters, because escaping a secret changes it into a string the scrubber no longer knows.
    #[must_use]
    pub fn line(&self, event: &Event) -> String {
        let mut out = format!(
            "{{\"timestamp\":{},\"level\":{},\"target\":{},\"msg\":{}",
            event.timestamp,
            quote(event.level.name()),
            quote(&self.redact(&event.target)),
            quote(&self.redact(&event.message)),
        );
        // Sorted by name, so that two call sites that add the same fields in a different order
        // write the same bytes: a field order that depends on the code path is not a contract.
        let mut fields: Vec<&(String, Value)> = event.fields.iter().collect();
        fields.sort_by(|left, right| left.0.cmp(&right.0));
        for (name, value) in fields {
            out.push(',');
            out.push_str(&quote(name));
            out.push(':');
            out.push_str(&self.render(name, value));
        }
        out.push('}');
        out
    }

    /// `text` with every secret this encoder knows replaced by [`REDACTED`].
    fn redact(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for secret in &self.secrets {
            // An empty secret would match everywhere and redact the whole line.
            if secret.is_empty() {
                continue;
            }
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), REDACTED);
            }
        }
        out
    }

    /// One field's value as the text after the colon.
    ///
    /// A string is quoted, redacted and escaped; a number or a boolean is the bare literal. A field
    /// whose *name* is a [`SECRET_KEYS`] entry is [`REDACTED`] whatever its value is, so a password
    /// logged as a number is as redacted as one logged as a string.
    fn render(&self, name: &str, value: &Value) -> String {
        if SECRET_KEYS.iter().any(|key| key.eq_ignore_ascii_case(name)) {
            return quote(REDACTED);
        }
        match value {
            Value::Str(text) => quote(&self.redact(text)),
            Value::Int(number) => number.to_string(),
            Value::Bool(flag) => flag.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An event with a custom field added before another, so the sorted order is visible.
    fn event() -> Event {
        Event::new(1_760_000_000, Level::Info, "worker", "delivery attempted")
            .field("tenant", "acme")
            .field("attempt", 2_i64)
    }

    #[test]
    fn one_event_is_one_line_with_the_fixed_names_and_the_fields_sorted() {
        let encoder = Encoder::new(&[]);
        assert_eq!(
            encoder.line(&event()),
            "{\"timestamp\":1760000000,\"level\":\"info\",\"target\":\"worker\",\
             \"msg\":\"delivery attempted\",\"attempt\":2,\"tenant\":\"acme\"}",
            "the fixed names come first in one order, and the custom fields are sorted rather than \
             in the order they were added"
        );
        assert_eq!(
            encoder.line(&event()).lines().count(),
            1,
            "and the event is one line"
        );
    }

    #[test]
    fn a_message_with_a_newline_in_it_is_still_one_event() {
        let encoder = Encoder::new(&[]);
        let event = Event::new(1, Level::Warn, "worker", "first line\nsecond \"line\"");
        let line = encoder.line(&event);
        assert_eq!(
            line.lines().count(),
            1,
            "a collector reads lines: a raw newline in a message is two events, and the second one \
             has no level and no timestamp"
        );
        assert!(
            !line.contains('\n'),
            "the newline is escaped rather than passed through: {line}"
        );
        assert!(
            line.contains("first line\\nsecond \\\"line\\\""),
            "the message is a JSON string: {line}"
        );
    }

    #[test]
    fn the_same_event_is_the_same_bytes_however_it_was_built() {
        let encoder = Encoder::new(&[]);
        let forwards = Event::new(5, Level::Error, "api", "boom")
            .field("z", 1_i64)
            .field("a", "x");
        let backwards = Event::new(5, Level::Error, "api", "boom")
            .field("a", "x")
            .field("z", 1_i64);
        assert_eq!(
            encoder.line(&forwards),
            encoder.line(&backwards),
            "sorting by name is what makes the text a contract rather than a habit"
        );
    }

    #[test]
    fn a_secret_named_field_keeps_its_name_and_loses_its_value() {
        let encoder = Encoder::new(&[]);
        let event = Event::new(7, Level::Info, "signup", "user created")
            .field("password", "hunter2")
            .field("api_key", "sk-live-123");
        let line = encoder.line(&event);
        assert!(
            !line.contains("hunter2"),
            "the password is not in the line: {line}"
        );
        assert!(
            !line.contains("sk-live-123"),
            "and neither is the key: {line}"
        );
        assert!(
            line.contains("\"password\":\"[redacted]\""),
            "the name stays and the value is the marker: {line}"
        );
        assert!(
            line.contains("\"api_key\":\"[redacted]\""),
            "an operator greps for the name to see the line was scrubbed: {line}"
        );

        // A secret-named field logged as a number is redacted too: the name is the rule, not the
        // value's type.
        let numeric = Event::new(8, Level::Info, "signup", "x").field("token", 12_345_i64);
        assert!(
            !encoder.line(&numeric).contains("12345"),
            "a token logged as a number survives no better than one logged as a string"
        );
    }

    #[test]
    fn a_secret_the_encoder_was_told_about_does_not_survive_anywhere() {
        let encoder = Encoder::new(&["hunter2"]);
        let event = Event::new(9, Level::Error, "auth", "sent password hunter2 to ada")
            .field("note", "retried with hunter2")
            .field("ok", true);
        let line = encoder.line(&event);
        assert!(
            !line.contains("hunter2"),
            "the secret is gone from the message and from a field that is not named like one: {line}"
        );
        assert_eq!(
            line.matches(REDACTED).count(),
            2,
            "both places are redacted: {line}"
        );
        assert!(
            line.contains("sent password [redacted] to ada"),
            "and the rest of the message is untouched: {line}"
        );
        assert!(
            line.contains("\"ok\":true"),
            "a boolean is logged bare: {line}"
        );
    }

    #[test]
    fn redaction_runs_before_escaping_so_a_secret_with_a_quote_is_still_found() {
        let secret = "p@ss\"word";
        let encoder = Encoder::new(&[secret]);
        let event = Event::new(11, Level::Warn, "auth", &format!("tried {secret} for ada"));
        let line = encoder.line(&event);
        assert!(
            !line.contains("p@ss"),
            "escaping first would turn the quote into a backslash-quote and hide the secret from \
             the scrubber: {line}"
        );
        assert!(
            line.contains(REDACTED),
            "the secret was found first: {line}"
        );
    }

    #[test]
    fn a_batch_of_events_is_one_line_each() {
        let encoder = Encoder::new(&["hunter2"]);
        let events = [event(), Event::new(2, Level::Debug, "db", "query")];
        let text: String = events
            .iter()
            .map(|event| encoder.line(event))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            text.lines().count(),
            events.len(),
            "a batch is one line per event: {text}"
        );
    }

    #[test]
    fn levels_have_one_spelling() {
        let encoder = Encoder::new(&[]);
        let expected = [
            (Level::Debug, "debug"),
            (Level::Info, "info"),
            (Level::Warn, "warn"),
            (Level::Error, "error"),
        ];
        for (level, name) in expected {
            let line = encoder.line(&Event::new(0, level, "t", "m"));
            assert!(
                line.contains(&format!("\"level\":\"{name}\"")),
                "the level is the lower-case name a query filters on: {line}"
            );
        }
    }
}
