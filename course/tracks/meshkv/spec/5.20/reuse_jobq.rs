//! What `meshkv/5.20-reuse-jobq` has to be true about.
//!
//! The step's subject is the dependency itself: it is a `path` dependency on the learner's own T3
//! project, the engine's types are reachable through it, and this project's adapter turns one of
//! them into a key without asking `jobq` to change. The cases below check the three, and the third
//! is the one the whole `path`-dependency rule exists for.

use meshkv::kv::Command;
use meshkv::reuse::{JobId, job_key, record_job};

/// This project's manifest, as it ships.
fn manifest() -> String {
    std::fs::read_to_string("Cargo.toml").expect("the project's own manifest")
}

#[test]
fn jobq_is_a_path_dependency_on_the_sibling_project() {
    let manifest = manifest();
    let line = manifest
        .lines()
        .find(|line| line.trim_start().starts_with("jobq ="))
        .expect("the manifest depends on jobq");
    assert!(
        line.contains("path = \"../jobq\""),
        "`jobq` has to be a path dependency on the learner's own T3 project, not a copy: {line}"
    );
}

#[test]
fn the_engines_types_are_reachable_through_the_dependency() {
    // A `JobId` is the engine's own identity type (`jobq/3.1` made it a type so a queue can talk
    // about a job without a string). Reaching it through `meshkv::reuse` is what proves the
    // dependency is a dependency and not a re-export of something this project wrote.
    let id = JobId::new();
    let text = id.to_string();
    assert!(!text.is_empty(), "a job id renders");
    assert!(
        std::any::type_name::<JobId>().contains("jobq::engine"),
        "the type comes from `jobq::engine`, not from this project: {}",
        std::any::type_name::<JobId>()
    );
}

#[test]
fn a_job_id_becomes_a_key_in_this_store() {
    let id = JobId::new();
    let key = job_key(&id);
    assert_eq!(
        key,
        format!("job:{id}"),
        "the adapter namespaces the engine's id"
    );
    assert!(key.starts_with("job:"), "and says whose id it is");
}

#[test]
fn a_job_state_is_a_command_the_state_machine_understands() {
    let id = JobId::new();
    let payload = record_job(&id, b"queued");
    let command = Command::parse(&payload).expect("the adapter writes a command this store reads");
    match command {
        Command::Set { key, value } => {
            assert_eq!(key, job_key(&id));
            assert_eq!(value, b"queued");
        }
        other => panic!("recording a job is a set, not {other:?}"),
    }
}

#[test]
fn two_jobs_do_not_collide() {
    let first = JobId::new();
    let second = JobId::new();
    assert_ne!(first, second);
    assert_ne!(job_key(&first), job_key(&second));
}
