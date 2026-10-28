//! Spec for `rpc/8.4-server-streaming` — a window, not a buffer.

use rpc::stream::Stream;

fn stream() -> Stream {
    Stream::new(&["a", "b", "c", "d"], 2)
}

#[test]
fn the_window_bounds_what_is_in_flight() {
    let mut stream = stream();
    assert_eq!(stream.send().as_deref(), Some("a"));
    assert_eq!(stream.send().as_deref(), Some("b"));
    assert_eq!(stream.send(), None, "the window was exceeded");
    assert_eq!(stream.in_flight(), 2);
}

#[test]
fn an_acknowledgement_reopens_the_window() {
    let mut stream = stream();
    stream.send();
    stream.send();
    stream.ack(1);
    assert_eq!(stream.in_flight(), 1);
    assert_eq!(stream.send().as_deref(), Some("c"));
}

#[test]
fn a_zero_window_never_sends() {
    let mut stream = Stream::new(&["a"], 0);
    assert_eq!(stream.send(), None);
}

#[test]
fn the_stream_ends_when_the_items_do() {
    let mut stream = stream();
    stream.send();
    stream.send();
    stream.ack(10);
    assert_eq!(stream.send().as_deref(), Some("c"));
    assert_eq!(stream.send().as_deref(), Some("d"));
    assert_eq!(stream.send(), None);
}

#[test]
fn an_ack_past_what_was_sent_is_clamped() {
    let mut stream = stream();
    stream.send();
    stream.ack(5);
    assert_eq!(stream.in_flight(), 0);
    assert_eq!(stream.send().as_deref(), Some("b"));
}
