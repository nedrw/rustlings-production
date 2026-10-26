//! Spec for `lakelet/4.1-ingest-protocol`: framing, the edges of framing, and the store behind it.
//!
//! `kiln run` copies this file into the project as `tests/spec_ingest_protocol.rs`; it is the
//! course's file, not the learner's (plan §3 D2). Twelve cases, and the shape of them is the point:
//!
//! * **a stream is not a sequence of messages** — every strict prefix of a frame has to come back as
//!   `Incomplete` *with the number of bytes it is still missing*, because that number is what stops a
//!   reader from asking for one byte at a time;
//! * **the peer is hostile until proven otherwise** — a length above the limit and a version this
//!   build does not speak are refused from the bytes that carry them, before any payload is buffered;
//!   both cases are checked with a buffer that holds *only* those bytes, so an implementation that
//!   waits for the whole frame cannot pass them;
//! * **a checksum covers what it says it covers** — a flipped payload byte is caught by the checksum,
//!   and a flipped version byte by the version check, which are two different sentences;
//! * **the daemon is judged as a process** — three frames sent over a socket become three records the
//!   store can read back, in order; a peer that stops in the middle of a frame is not a record and
//!   does not stop the daemon; a refused frame is counted and the *next* one is still stored;
//! * **the boundary the track is built on** — `src/engine/**` may not name `crate::service`, read out
//!   of the source because a single package cannot enforce it (the same rule, and the same reason, as
//!   `jobq/3.1-engine-boundary`).
//!
//! Nothing here sleeps to synchronise: the daemon is waited for through its own log lines and its own
//! port, and every wait has a deadline.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use support::Var;
use support::service::Service;

use lakelet::engine::Spool;
use lakelet::protocol::batch::{self, Record};
use lakelet::protocol::frame::{Decoded, Frame, MAX_PAYLOAD, OVERHEAD, VERSION};

/// A payload that is not round-numbered, so a length/payload mix-up cannot pass by accident.
fn payload(marker: u8) -> Vec<u8> {
    (0..37u8).map(|index| marker.wrapping_add(index)).collect()
}

/// Three records to send.
fn records() -> Vec<Record> {
    vec![
        Record {
            series: "cpu,host=h0".to_owned(),
            ts_millis: 1_700_000_000_000,
            value: 12.5,
        },
        Record {
            series: "cpu,host=h1".to_owned(),
            ts_millis: 1_700_000_001_000,
            value: -3.25,
        },
        Record {
            series: "mem,host=h0".to_owned(),
            ts_millis: 1_700_000_002_000,
            value: 0.0,
        },
    ]
}

/// The frames a client would send for `records`, batched one record per frame.
fn frames() -> Vec<Vec<u8>> {
    records()
        .iter()
        .map(|record| Frame::new(batch::encode(std::slice::from_ref(record))).encode())
        .collect()
}

/// The bytes a client sends, concatenated, for the fixture's `send`.
fn wire(frames: &[Vec<u8>]) -> Vec<u8> {
    frames.iter().flatten().copied().collect()
}

/// A record's bytes, as the store holds them: the batch the client sent.
fn batch_of(record: &Record) -> Vec<u8> {
    batch::encode(std::slice::from_ref(record))
}

#[test]
fn a_frame_round_trips() {
    let frame = Frame::new(payload(1));
    let bytes = frame.encode();
    assert_eq!(
        bytes.len(),
        OVERHEAD + frame.payload.len(),
        "a frame is its payload plus the nine bytes of framing"
    );
    match lakelet::protocol::frame::decode(&bytes) {
        Decoded::Frame(decoded, used) => {
            assert_eq!(
                decoded, frame,
                "the payload and the version come back unchanged"
            );
            assert_eq!(used, bytes.len(), "a whole buffer is consumed by one frame");
        }
        other => panic!("a frame this crate just encoded was not read back: {other:?}"),
    }
}

#[test]
fn every_prefix_of_a_frame_asks_for_exactly_what_is_missing() {
    let bytes = Frame::new(payload(2)).encode();
    for length in 0..bytes.len() {
        let prefix = bytes
            .get(..length)
            .expect("the prefix is a slice of the frame");
        match lakelet::protocol::frame::decode(prefix) {
            Decoded::Incomplete { needed } => {
                let missing = bytes.len() - length;
                // The first four bytes announce the length and the fifth announces the version, so an
                // implementation may ask for those before it can compute the whole frame — it may not
                // ask for more than the frame is missing, and it may not ask for nothing.
                let allowed = if missing >= OVERHEAD {
                    [4 - length.min(4), 1, missing]
                } else {
                    [missing, 4 - length.min(4), 1]
                };
                assert!(
                    allowed.contains(&needed),
                    "a prefix of {length} byte(s) asked for {needed} more, and it is missing {missing}"
                );
                assert!(needed > 0, "a strict prefix is never a whole frame");
            }
            other => panic!("a prefix of {length} byte(s) answered {other:?}"),
        }
    }
}

#[test]
fn a_length_above_the_limit_is_refused_before_the_payload_exists() {
    for length in [MAX_PAYLOAD as u32 + 1, 4_000_000, u32::MAX] {
        let header = length.to_le_bytes();
        match lakelet::protocol::frame::decode(&header) {
            Decoded::Invalid { reason } => {
                assert!(
                    reason.contains(&length.to_string()),
                    "the refusal names the length it saw: {reason}"
                );
                assert!(
                    reason.contains(&MAX_PAYLOAD.to_string()),
                    "the refusal names the limit: {reason}"
                );
            }
            other => panic!("a declared payload of {length} byte(s) answered {other:?}"),
        }
    }
}

#[test]
fn a_version_this_build_does_not_speak_is_refused_from_the_header_alone() {
    let mut header = 1_048_576u32.to_le_bytes().to_vec();
    header.push(VERSION + 9);
    match lakelet::protocol::frame::decode(&header) {
        Decoded::Invalid { reason } => {
            assert!(
                reason.contains(&(VERSION + 9).to_string()),
                "the refusal names the version it saw: {reason}"
            );
            assert!(
                reason.contains(&VERSION.to_string()),
                "the refusal names the version this build speaks: {reason}"
            );
        }
        other => panic!("five bytes of a frame from another version answered {other:?}"),
    }
}

#[test]
fn a_corrupted_payload_fails_its_checksum_and_a_corrupted_version_its_version() {
    let bytes = Frame::new(payload(3)).encode();

    let mut flipped = bytes.clone();
    let index = OVERHEAD;
    let victim = flipped
        .get_mut(index)
        .expect("the payload is in the buffer");
    *victim = victim.wrapping_add(1);
    match lakelet::protocol::frame::decode(&flipped) {
        Decoded::Invalid { reason } => assert!(
            reason.contains("checksum"),
            "a payload byte that changed is a checksum failure: {reason}"
        ),
        other => panic!("a corrupted payload answered {other:?}"),
    }

    let mut version = bytes;
    let slot = version
        .get_mut(4)
        .expect("the version byte is in the buffer");
    *slot = VERSION.wrapping_add(1);
    match lakelet::protocol::frame::decode(&version) {
        Decoded::Invalid { reason } => assert!(
            reason.contains("version"),
            "a changed version is a version failure: {reason}"
        ),
        other => panic!("a corrupted version answered {other:?}"),
    }
}

#[test]
fn a_batch_round_trips_and_refuses_what_its_count_promises() {
    let records = records();
    let bytes = batch::encode(&records);
    let decoded = batch::decode(&bytes).expect("a batch this crate encoded is a batch it can read");
    assert_eq!(decoded.len(), records.len());
    for (before, after) in records.iter().zip(&decoded) {
        assert_eq!(before.series, after.series);
        assert_eq!(before.ts_millis, after.ts_millis);
        assert_eq!(before.value.to_bits(), after.value.to_bits(), "bit for bit");
    }

    // A record that ends in the middle of its value.
    let short = bytes
        .get(..bytes.len() - 3)
        .expect("the batch is longer than three bytes");
    assert!(
        batch::decode(short).is_err(),
        "a torn record is not a batch"
    );

    // A count that promises more records than the bytes carry.
    let mut greedy = bytes.clone();
    let count = (records.len() as u32 + 4).to_le_bytes();
    greedy.splice(..4, count);
    assert!(
        batch::decode(&greedy).is_err(),
        "a count above the bytes is refused"
    );

    // Trailing bytes after the last record: a batch is all of it or none of it.
    let mut trailing = bytes.clone();
    trailing.push(0);
    let error = batch::decode(&trailing).expect_err("trailing bytes are not a batch");
    assert!(
        error.to_string().contains('1'),
        "the refusal says how much was left over: {error}"
    );

    // A record whose series name is empty, and one whose name is not UTF-8.
    let empty = batch::encode(&[Record {
        series: String::new(),
        ts_millis: 1,
        value: 1.0,
    }]);
    assert!(
        batch::decode(&empty).is_err(),
        "an empty series name is refused"
    );
    let mut not_utf8 = batch::encode(&[Record {
        series: "a".to_owned(),
        ts_millis: 1,
        value: 1.0,
    }]);
    let name_at = 4 + 2;
    not_utf8.splice(name_at..name_at + 1, [0xff]);
    assert!(
        batch::decode(&not_utf8).is_err(),
        "a series name that is not UTF-8 is refused"
    );
}

#[test]
fn the_daemon_stores_what_a_client_sent() {
    let service = Service::start("stores", &[]);
    service.send(&wire(&frames()));
    let line = service.wait_for_line("accepted=3");
    assert!(
        line.contains("connection closed"),
        "the daemon says what it accepted when the client goes away: {line}"
    );
    assert!(
        line.contains(&format!("bytes={}", three_payload_bytes())),
        "and how many payload bytes those were: {line}"
    );

    let spool = Spool::open(service.store().path()).expect("the store the daemon wrote");
    let stored = spool.records().expect("records the store can read back");
    let expected: Vec<Vec<u8>> = records().iter().map(batch_of).collect();
    assert_eq!(
        stored, expected,
        "three frames in, three records out, in the order they were accepted"
    );
}

#[test]
fn a_peer_that_stops_mid_frame_leaves_no_record_and_does_not_stop_the_daemon() {
    let mut service = Service::start("half-frame", &[]);
    let whole = &frames()[0];
    let half = whole
        .get(..whole.len() / 2)
        .expect("a frame is more than one byte");
    service.send(half);

    // The daemon is still serving, and the *next* client's frame is stored: a peer that went away
    // mid-frame is not an error this process can act on, and it must not be a record either.
    assert!(service.is_running(), "a half frame is not a reason to stop");
    service.send(&wire(&frames()[1..]));
    service.wait_for_line("accepted=2");

    let spool = Spool::open(service.store().path()).expect("the store the daemon wrote");
    let stored = spool.records().expect("records the store can read back");
    let expected: Vec<Vec<u8>> = records()[1..].iter().map(batch_of).collect();
    assert_eq!(
        stored, expected,
        "the half frame left nothing behind, and the complete ones did"
    );
}

#[test]
fn a_frame_that_is_not_one_is_refused_and_counted_and_the_next_one_is_stored() {
    let service = Service::start("rejected", &[]);
    let mut garbage = (MAX_PAYLOAD as u32 + 1).to_le_bytes().to_vec();
    garbage.extend_from_slice(&frames()[0]);
    service.send(&garbage);
    service.send(&wire(&frames()[1..2]));

    let refused = service.wait_for_line("frame rejected");
    assert!(
        refused.contains("above this protocol's limit"),
        "the log line carries the reason the frame was refused: {refused}"
    );
    let closed = service.wait_for_line("rejected=1");
    assert!(
        closed.contains("accepted=0"),
        "the connection that sent nothing but a refusal accepted nothing: {closed}"
    );
    // The two frames travel on two connections, and the refusal's close line says nothing about the
    // other one: the record this test asserts on exists once *its* connection has closed with it,
    // so that is the line to wait for. (Measured: waiting only on the refusal raced the daemon's
    // read of the second socket and read the store before the frame was in it.)
    let stored_line = service.wait_for_line("accepted=1");
    assert!(
        stored_line.contains("rejected=0"),
        "the second connection carried the good frame and refused nothing: {stored_line}"
    );
    let spool = Spool::open(service.store().path()).expect("the store the daemon wrote");
    let stored = spool.records().expect("records the store can read back");
    assert_eq!(
        stored,
        vec![batch_of(&records()[1])],
        "the refused frame is not a record and the frame after it is"
    );
}

#[test]
fn the_daemon_stops_on_sigterm_with_a_zero_exit_code() {
    let mut service = Service::start("sigterm", &[]);
    service.send(&wire(&frames()));
    service.wait_for_line("accepted=3");
    service.stop();
    let code = service.wait_for_exit();
    assert_eq!(code, 0, "a requested stop is not a failure");
    let draining = service.wait_for_line("draining");
    assert!(
        draining.contains("draining"),
        "the stop is announced, because a deployment watches for it: {draining}"
    );
}

#[test]
fn check_config_prints_the_settings_and_the_layers_still_win_in_order() {
    let store = support::StoreDir::new("check-config");
    let file = store.path().join("lakelet.toml");
    std::fs::write(
        &file,
        "[listen]\naddr = \"127.0.0.1:7100\"\n[store]\ndir = \"from-file\"\n",
    )
    .expect("the fixture writes its own configuration file");

    // The file first: every key is reported, and the value comes from the file.
    let from_file = run_daemon(
        &["--check-config", "--config", &file.display().to_string()],
        &[],
    );
    for key in ["listen.addr", "store.dir", "log.level", "log.format"] {
        assert!(
            from_file.contains(key),
            "`{key}` is part of the report:\n{from_file}"
        );
    }
    assert!(
        from_file.contains("127.0.0.1:7100"),
        "the file set the address:\n{from_file}"
    );
    assert!(
        from_file.contains("from-file"),
        "the file set the store directory:\n{from_file}"
    );

    // The environment beats the file, and a flag beats the environment.
    let from_environment = run_daemon(
        &["--check-config", "--config", &file.display().to_string()],
        &[Var::new("LAKELET_STORE_DIR", "from-environment")],
    );
    assert!(
        from_environment.contains("from-environment"),
        "the environment wins over the file:\n{from_environment}"
    );
    let from_flag = run_daemon(
        &[
            "--check-config",
            "--config",
            &file.display().to_string(),
            "--store-dir",
            "from-flag",
        ],
        &[Var::new("LAKELET_STORE_DIR", "from-environment")],
    );
    assert!(
        from_flag.contains("from-flag"),
        "the flag wins over the environment:\n{from_flag}"
    );
    assert!(
        !from_flag.contains("from-environment") && !from_flag.contains("from-file"),
        "and the layers it beat are not in the report:\n{from_flag}"
    );
}

#[test]
fn the_engine_does_not_reach_into_the_service() {
    let engine = PathBuf::from("src/engine");
    let mut files = Vec::new();
    collect_rust(&engine, &mut files).expect("the engine directory is part of the project");
    assert!(
        !files.is_empty(),
        "{} holds no Rust file, so this case would pass without checking anything",
        engine.display()
    );
    for file in &files {
        let source = std::fs::read_to_string(file).expect("a source file the project keeps");
        for forbidden in ["crate::service", "crate::cli"] {
            assert!(
                !source.contains(forbidden),
                "{} names `{forbidden}` — the daemon uses the engine, never the other way",
                file.display()
            );
        }
    }
}

/// The payload bytes of the three records the fixture sends, as the daemon counts them.
fn three_payload_bytes() -> usize {
    records().iter().map(|record| batch_of(record).len()).sum()
}

/// Run `lakeletd` to completion with `arguments` and `environment`, and answer its stdout.
fn run_daemon(arguments: &[&str], environment: &[Var]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lakeletd"));
    command.args(arguments).env_clear();
    for var in environment {
        command.env(&var.name, &var.value);
    }
    let output = command
        .output()
        .expect("the lakeletd binary is built by cargo test");
    assert!(
        output.status.success(),
        "`lakeletd {}` exited {}: {}",
        arguments.join(" "),
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Every `.rs` file below `directory`, recursively.
fn collect_rust(directory: &Path, found: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_rust(&path, found)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
    Ok(())
}
