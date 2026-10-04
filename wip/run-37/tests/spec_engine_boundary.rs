//! Spec for `jobq/3.1-engine-boundary`: the engine, judged from outside it.
//!
//! `kiln run` copies this file into the project as `tests/spec_engine_boundary.rs`; it is the
//! course's file, not the learner's (plan §3 D2). Six claims, and every one of them is a behaviour:
//!
//! * **a configuration can be inspected without starting anything** — `--check-config` prints the
//!   effective settings, in the shape this file pins, and never the password inside the connection
//!   string;
//! * **the engine runs a queue with no server in the process** — submit, claim, complete, and the
//!   states a job passes through on the way;
//! * **an attempt that fails is counted** — the budget is what stops one broken payload from
//!   occupying a worker forever;
//! * **two workers never hold the same job** — the reason this is a port and not a helper;
//! * **the daemon says it is draining and stops on `SIGTERM`** — the service half of the step;
//! * **the engine does not depend on the service** — the arrow only points one way, and this is the
//!   one edge the compiler cannot check inside a single package.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use jobq::engine::{Engine, JobState, MemoryStore, Queue, SubmitRequest, WorkerId};
use support::service::Service;
use support::var;

/// The queue every case in this file uses; the name is part of what the settings print.
const QUEUE: &str = "emails";

/// A connection string whose password must never reach the report.
const SECRET_URL: &str = "postgres://jobq:hunter2@127.0.0.1:5432/jobq";

/// The queue name, parsed, with the failure a learner can act on.
fn queue() -> Queue {
    Queue::parse(QUEUE).expect("`emails` is the name this file uses everywhere")
}

/// A payload of the shape a producer would send.
fn payload(body: &str) -> serde_json::Value {
    serde_json::json!({ "to": body })
}

/// A directory of one test's own, so two runs never share a file.
fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("jobq-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory is writable");
    dir
}

#[test]
fn the_effective_configuration_is_reported_without_starting_the_service() {
    // The layers a learner met in `mailer/1.4-config`: defaults, then the file, then the
    // environment, then the flags. The file is written here so the case exercises all four.
    let dir = scratch("check-config");
    let file = dir.join("jobqd.toml");
    std::fs::write(
        &file,
        format!("[store]\nurl = \"{SECRET_URL}\"\n\n[worker]\nqueues = [\"{QUEUE}\"]\nbatch = 4\n"),
    )
    .expect("the configuration file is writable");

    let output = Command::new(env!("CARGO_BIN_EXE_jobqd"))
        .args([
            "--check-config",
            "--config",
            file.to_str().expect("a utf-8 path"),
            "--worker-name",
            "drainer-1",
        ])
        .env_clear()
        // The environment is the layer above the file, so `batch` must come out as 16, not 4.
        .env("JOBQ__WORKER__BATCH", "16")
        .env("JOBQ__WORKER__QUEUES", "emails,exports")
        .env("JOBQ__LOG__FORMAT", "json")
        .output()
        .unwrap_or_else(|error| panic!("jobqd --check-config could not be started: {error}"));
    assert!(
        output.status.success(),
        "`jobqd --check-config` must answer with the effective settings, but it exited with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8_lossy(&output.stdout).into_owned();

    for expected in [
        "store.url",
        "worker.queues",
        "emails",
        "exports",
        "worker.batch",
        "16",
        "worker.name",
        "drainer-1",
        "log.format",
        "json",
    ] {
        assert!(
            report.contains(expected),
            "the report must name {expected:?}: a setting nobody can see is a setting that will be \
             discovered in production, and the environment must win over the file: {report}"
        );
    }
    assert!(
        !report.contains("hunter2"),
        "the report must not contain the password: it ends up in a ticket, a paste and a log line \
         (`mailer/1.4-config` made the same promise): {report}"
    );
}

#[tokio::test]
async fn a_job_runs_from_submitted_to_succeeded_with_no_service_in_the_process() {
    let engine = Engine::new(MemoryStore::new());
    let queue = queue();
    let worker = WorkerId::new("test-worker");

    let id = engine
        .submit(SubmitRequest::new(
            queue.clone(),
            payload("alice@example.com"),
        ))
        .await
        .expect("submitting to an empty memory store cannot fail");

    let submitted = engine.inspect(&id).await.expect("the job was just written");
    assert_eq!(submitted.state(), JobState::Queued);
    assert_eq!(submitted.attempts(), 0, "nothing has tried yet");
    assert_eq!(
        submitted.max_attempts(),
        3,
        "the default budget is three attempts"
    );
    assert_eq!(submitted.payload(), &payload("alice@example.com"));
    assert!(!submitted.state().is_terminal());

    let claimed = engine
        .claim(&worker, &queue, 10)
        .await
        .expect("claiming from a memory store cannot fail");
    assert_eq!(claimed.len(), 1, "the job was due, so it must be claimed");
    assert_eq!(claimed[0].id(), id);
    assert_eq!(claimed[0].state(), JobState::Running);

    let running = engine.inspect(&id).await.expect("the job is still there");
    assert_eq!(
        running.attempts(),
        1,
        "a claim is the start of an attempt: if the count stayed at 0, a worker that dies looks \
         like a worker that never started, and the attempt budget never runs out"
    );

    engine
        .complete(&worker, &id)
        .await
        .expect("completing a claimed job cannot fail");
    let done = engine.inspect(&id).await.expect("the job is still there");
    assert_eq!(done.state(), JobState::Succeeded);
    assert!(
        done.state().is_terminal(),
        "a succeeded job must never be claimed again"
    );
}

#[tokio::test]
async fn a_failing_job_is_requeued_until_its_budget_is_spent() {
    let engine = Engine::new(MemoryStore::new());
    let queue = queue();
    let worker = WorkerId::new("test-worker");

    let id = engine
        .submit(SubmitRequest::new(queue.clone(), payload("bob@example.com")).with_max_attempts(2))
        .await
        .expect("submitting cannot fail");

    engine
        .claim(&worker, &queue, 1)
        .await
        .expect("claim cannot fail");
    let state = engine
        .fail(&worker, &id, "the smtp server said 421")
        .await
        .expect("recording a failure cannot fail");
    assert_eq!(
        state,
        JobState::Queued,
        "one attempt of two is spent, so the job goes back to the queue"
    );

    let retried = engine.inspect(&id).await.expect("the job is still there");
    assert_eq!(retried.attempts(), 1);
    assert_eq!(
        retried.last_error(),
        Some("the smtp server said 421"),
        "the operator needs the reason on the row, not only in a log line that has rotated away"
    );

    engine
        .claim(&worker, &queue, 1)
        .await
        .expect("claim cannot fail");
    let state = engine
        .fail(&worker, &id, "the smtp server said 421")
        .await
        .expect("recording a failure cannot fail");
    assert_eq!(
        state,
        JobState::Failed,
        "the second attempt spends the budget, so the job is terminal"
    );

    let claimed = engine
        .claim(&worker, &queue, 10)
        .await
        .expect("claim cannot fail");
    assert!(
        claimed.is_empty(),
        "a job whose budget is spent must not be handed out again: that is how one broken payload \
         would otherwise occupy every worker forever"
    );
}

#[tokio::test]
async fn two_workers_never_hold_the_same_job_and_never_lose_one() {
    let engine = Engine::new(MemoryStore::new());
    let queue = queue();
    let first = WorkerId::new("worker-a");
    let second = WorkerId::new("worker-b");

    for index in 0..3 {
        engine
            .submit(SubmitRequest::new(
                queue.clone(),
                payload(&format!("user{index}")),
            ))
            .await
            .expect("submitting cannot fail");
    }

    let claimed_by_first = engine
        .claim(&first, &queue, 2)
        .await
        .expect("claim cannot fail");
    let claimed_by_second = engine
        .claim(&second, &queue, 2)
        .await
        .expect("claim cannot fail");
    assert_eq!(claimed_by_first.len(), 2);
    assert_eq!(
        claimed_by_second.len(),
        1,
        "three jobs and a claim of two each: the second worker gets what is left, not a copy of \
         what the first already holds"
    );

    let mut ids: Vec<_> = claimed_by_first
        .iter()
        .chain(claimed_by_second.iter())
        .map(|job| job.id())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 3, "every job is claimed exactly once");
}

#[test]
fn the_daemon_announces_its_drain_and_stops_on_a_signal() {
    // A process, not a library call: an orchestrator sends `SIGTERM` and reads the exit code, and
    // both are invisible from inside the crate. The harness waits for the daemon's own readiness
    // line instead of sleeping (`tests/support/service.rs`).
    let mut service = Service::start(&[var("JOBQ__STORE__URL", "memory")]);

    assert!(
        service
            .log_lines()
            .iter()
            .any(|line| line.contains("draining")),
        "the daemon must say it is draining before it is treated as ready"
    );
    assert!(
        service.is_running(),
        "a daemon that exits on its own is not a daemon"
    );

    service.stop();
    let code = service.wait_for_exit();
    assert_eq!(
        code, 0,
        "a signal is not a failure: an orchestrator that sees a non-zero code assumes a crash \
         (`mailer/1.14-worker` and `relay/2.15-shutdown-leases` both made this promise)"
    );
}

#[test]
fn the_engine_does_not_depend_on_the_service() {
    let engine_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/engine");
    let mut checked = 0;
    for path in rust_files(&engine_dir) {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()));
        for forbidden in ["crate::service", "crate::cli"] {
            assert!(
                !text.contains(forbidden),
                "{} uses {forbidden}: the engine is the thing the daemon depends on, and an engine \
                 that reaches back into it can only be reused by copying the daemon too. The arrow \
                 only points one way — here it is a source check rather than a crate boundary, \
                 because the whole project is one package (authoring.md §6.6)",
                path.display()
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 4,
        "expected the engine's modules under {}, found {checked}: the rule above is only worth \
         anything if it actually looked at the files",
        engine_dir.display()
    );
}

/// Every `.rs` file below `root`, in path order, so a failure names the same file twice running.
fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("{} could not be listed: {error}", directory.display()));
        for entry in entries {
            let path = entry.expect("the directory is readable").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}
