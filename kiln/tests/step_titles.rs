//! A step's name lives in its body; `track.toml` repeats it as metadata.
//!
//! The repetition is deliberate — `kiln verify --json` and the spec listing must not change with
//! the reader's locale — but it drifted for seven steps before `kiln list` started reading the
//! body: the metadata said "a library the service depends on" where the page said "the daemon".
//! Nothing compared the two, so a reader saw two names for one step depending on the command.
//! This test is that comparison.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};

use kiln::info::Course;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("kiln/ has a parent")
        .to_path_buf()
}

#[test]
fn every_step_title_is_the_title_of_its_source_body() {
    let root = repository_root();
    let course = Course::load(&root).expect("the repository holds a course");
    let mut wrong: Vec<String> = Vec::new();
    for step in course.steps() {
        let path = course.dir().join(&step.entry);
        let body = kiln::i18n::read_step_body(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let Some(title) = body.front_matter.title else {
            wrong.push(format!("{}: the body has no title", step.qualified_id()));
            continue;
        };
        if title != step.title {
            wrong.push(format!(
                "{}:\n    track.toml: {}\n    body:       {}",
                step.qualified_id(),
                step.title,
                title
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} step(s) have two names; the body is the source (authoring.md §2):\n  {}",
        wrong.len(),
        wrong.join("\n  ")
    );
}
