//! probe = "obs/structured-logs@apply"
//! title = "A log line is fields, not a sentence"
//! hint = "One event is one line of key=value pairs, the run id is always there, and a value is quoted when it could be mistaken for structure."
//!
//! Step `mailer/1.6-logging` writes one structured event per event — `event=listening run.id=…
//! address=127.0.0.1:39121` — because the reader is as often a query as a person. The shape is
//! the contract: the event's name first, the correlation id second, then the event's own fields,
//! and a value that contains a space, a quote or `=` quoted so it cannot be read as another field.
//!
//! ## What you write
//!
//! [`line`] — one line for `event`:
//!
//! * `event=<name> run.id=<id>`, then each field as `<key>=<value>` in the order given, separated
//!   by one space;
//! * every value goes through [`render_value`] (given): bare when it is safe, otherwise quoted
//!   with `"` and with `"` and a newline escaped — so one event is always exactly one line.
//!
//! ## What the tests ask
//!
//! An event with no fields (the id is still there); fields in order; a value with a space; a value
//! with a quote; an empty value; a value with `=` in it; and a value with a newline, which must not
//! break the line.

/// One event: its name and its own fields, in the order the caller gives them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event<'a> {
    /// The event's name (`starting`, `listening`, `delivered`).
    pub name: &'a str,
    /// The event's own fields.
    pub fields: Vec<(&'a str, String)>,
}

/// Render one event as exactly one line.
#[must_use]
pub fn line(run_id: &str, event: &Event) -> String {
    let mut out = format!(
        "event={} run.id={}",
        render_value(event.name),
        render_value(run_id)
    );
    for (key, value) in &event.fields {
        out.push(' ');
        out.push_str(key);
        out.push('=');
        out.push_str(&render_value(value));
    }
    out
}

/// Render one value: bare when it cannot be mistaken for structure, quoted otherwise.
fn render_value(value: &str) -> String {
    let needs_quotes = value.is_empty()
        || value
            .chars()
            .any(|character| character.is_whitespace() || character == '"' || character == '=');
    if !needs_quotes {
        return value.to_owned();
    }
    let mut out = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_with_no_fields_still_carries_the_run_id() {
        let event = Event {
            name: "starting",
            fields: Vec::new(),
        };
        assert_eq!(line("run-1", &event), "event=starting run.id=run-1");
    }

    #[test]
    fn fields_follow_the_run_id_in_the_order_given() {
        let event = Event {
            name: "listening",
            fields: vec![
                ("address", "127.0.0.1:39121".to_owned()),
                ("status", "ok".to_owned()),
            ],
        };
        assert_eq!(
            line("run-1", &event),
            "event=listening run.id=run-1 address=127.0.0.1:39121 status=ok"
        );
    }

    #[test]
    fn a_value_with_a_space_is_quoted() {
        let event = Event {
            name: "failed",
            fields: vec![("reason", "connection refused".to_owned())],
        };
        assert_eq!(
            line("run-1", &event),
            "event=failed run.id=run-1 reason=\"connection refused\""
        );
    }

    #[test]
    fn a_value_with_a_quote_is_escaped() {
        let event = Event {
            name: "failed",
            fields: vec![("reason", "the field says \"no\"".to_owned())],
        };
        assert_eq!(
            line("run-1", &event),
            "event=failed run.id=run-1 reason=\"the field says \\\"no\\\"\""
        );
    }

    #[test]
    fn an_empty_value_is_quoted_not_dropped() {
        let event = Event {
            name: "delivered",
            fields: vec![("message_id", String::new())],
        };
        assert_eq!(
            line("run-1", &event),
            "event=delivered run.id=run-1 message_id=\"\""
        );
    }

    #[test]
    fn a_value_that_looks_like_a_field_is_quoted() {
        let event = Event {
            name: "failed",
            fields: vec![("reason", "status=500".to_owned())],
        };
        assert_eq!(
            line("run-1", &event),
            "event=failed run.id=run-1 reason=\"status=500\""
        );
    }

    #[test]
    fn a_newline_in_a_value_does_not_break_the_line() {
        let event = Event {
            name: "failed",
            fields: vec![("reason", "first\nsecond".to_owned())],
        };
        let rendered = line("run-1", &event);
        assert!(
            !rendered.contains('\n'),
            "two lines for one event: {rendered}"
        );
        assert_eq!(
            rendered,
            "event=failed run.id=run-1 reason=\"first\\nsecond\""
        );
    }
}
