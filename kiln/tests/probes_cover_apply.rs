//! Every `apply` slot has a probe.
//!
//! M6.0 closed this gap (docs/m6-plan.md): before it, 24 of the 79 apply slots had no exercise, so
//! `kiln plan --fast` could not skip the steps that teach them and `kiln review --due` would never
//! re-check them. `kiln probe --check` now fails on a missing one; this test states the same rule
//! where a unit-test run sees it, without starting cargo.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};

use kiln::caps::Level;
use kiln::info::Course;
use kiln::probe;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("kiln/ has a parent")
        .to_path_buf()
}

#[test]
fn every_apply_slot_has_an_exercise() {
    let root = repository_root();
    let course = Course::load(&root).expect("the repository holds a course");
    let probes = probe::load(&course.dir()).expect("the probe catalogue loads");
    let mut missing: Vec<String> = Vec::new();
    for (capability, level, _) in course.ledger().slots() {
        if level != Level::Apply {
            continue;
        }
        let covered = probes.iter().any(|probe| {
            probe.requirement().level == Level::Apply && probe.requirement().id == *capability
        });
        if !covered {
            missing.push(format!("{capability}@apply"));
        }
    }
    assert!(
        missing.is_empty(),
        "{} apply slot(s) have no probe — `plan --fast` cannot skip them and `review --due` will \
         never re-check them:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}
