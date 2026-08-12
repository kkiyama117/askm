//! `disable` must refuse to touch a foreign entry it did not create, and say
//! why in a message a human can act on — the same safety rule `src/link.rs`
//! enforces at the library level, exercised here through the actual binary.

mod common;

use std::fs;

use common::{run, Fixture};

#[test]
fn disable_on_a_hand_made_directory_refuses_with_a_nonzero_exit_and_an_explanation() {
    let fx = Fixture::new();
    let skills_dir = fx.project_dir.path().join(".agents/skills");
    fs::create_dir_all(&skills_dir).unwrap();
    let hand_made = skills_dir.join("my-own-skill");
    fs::create_dir_all(&hand_made).unwrap();
    fs::write(hand_made.join("notes.md"), "hand written, do not touch").unwrap();

    let mut disable_cmd = fx.cmd(&["disable", "my-own-skill", "--project"]);
    let outcome = run(&mut disable_cmd);

    assert!(
        !outcome.status.success(),
        "disable of a foreign directory must exit non-zero"
    );
    assert!(
        outcome.stderr.contains("askm did not create")
            && outcome.stderr.contains("your files are safe"),
        "stderr: {}",
        outcome.stderr
    );
    assert!(
        hand_made.join("notes.md").exists(),
        "hand-made file must survive"
    );
    assert!(hand_made.is_dir(), "hand-made directory must survive");
}

#[test]
fn disable_on_a_foreign_symlink_refuses_and_leaves_it_pointing_where_it_did() {
    let fx = Fixture::new();
    let skills_dir = fx.project_dir.path().join(".agents/skills");
    fs::create_dir_all(&skills_dir).unwrap();
    let elsewhere = fx.project_dir.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let link_path = skills_dir.join("quint-lang");
    std::os::unix::fs::symlink(&elsewhere, &link_path).unwrap();

    let mut disable_cmd = fx.cmd(&["disable", "quint-lang", "--project"]);
    let outcome = run(&mut disable_cmd);

    assert!(
        !outcome.status.success(),
        "disable of a foreign symlink must exit non-zero"
    );
    assert!(
        outcome.stderr.contains("askm did not create")
            && outcome.stderr.contains("your files are safe"),
        "stderr: {}",
        outcome.stderr
    );
    assert_eq!(fs::read_link(&link_path).unwrap(), elsewhere);
}

#[test]
fn disable_of_a_skill_that_was_never_enabled_succeeds_as_a_no_op() {
    let fx = Fixture::new();

    let mut disable_cmd = fx.cmd(&["disable", "never-enabled", "--project"]);
    let outcome = run(&mut disable_cmd);

    assert!(outcome.status.success(), "stderr: {}", outcome.stderr);
}
