//! Spec for `rpc/8.5-multiplexing` — one connection, many streams, no head-of-line blocking.

use rpc::multiplex::{Connection, pump};
use rpc::stream::Stream;

fn connection(streams: Vec<Stream>) -> Connection {
    Connection::new(streams)
}

#[test]
fn a_blocked_stream_does_not_stop_the_others() {
    let mut connection = connection(vec![
        Stream::new(&["a", "b"], 0),
        Stream::new(&["x", "y"], 2),
    ]);
    let sent = pump(&mut connection);
    assert_eq!(sent, [(1, "x".to_owned()), (1, "y".to_owned()),]);
}

#[test]
fn a_round_is_one_item_per_stream() {
    let mut connection = connection(vec![
        Stream::new(&["a", "b"], 1),
        Stream::new(&["x", "y"], 1),
    ]);
    let sent = pump(&mut connection);
    assert_eq!(
        sent,
        [(0, "a".to_owned()), (1, "x".to_owned())],
        "one item each before blocking"
    );
}

#[test]
fn a_bigger_window_sends_more_rounds() {
    let mut connection = connection(vec![
        Stream::new(&["a", "b"], 2),
        Stream::new(&["x", "y"], 2),
    ]);
    let sent = pump(&mut connection);
    assert_eq!(
        sent,
        [
            (0, "a".to_owned()),
            (1, "x".to_owned()),
            (0, "b".to_owned()),
            (1, "y".to_owned()),
        ]
    );
}

#[test]
fn nothing_to_send_is_an_empty_pass() {
    let mut connection = connection(vec![Stream::new(&[], 2)]);
    assert!(pump(&mut connection).is_empty());
}

#[test]
fn everyone_blocked_is_an_empty_pass() {
    let mut connection = connection(vec![Stream::new(&["a"], 0), Stream::new(&["x"], 0)]);
    assert!(pump(&mut connection).is_empty());
}

#[test]
fn an_acknowledgement_lets_the_next_pass_continue() {
    let mut connection = connection(vec![
        Stream::new(&["a", "b"], 1),
        Stream::new(&["x", "y"], 1),
    ]);
    assert_eq!(pump(&mut connection).len(), 2);
    for stream in connection.streams_mut() {
        stream.ack(1);
    }
    let sent = pump(&mut connection);
    assert_eq!(sent, [(0, "b".to_owned()), (1, "y".to_owned())]);
}
