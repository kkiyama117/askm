//! Integration tests for `askm::link::disable` — the safety rule in particular:
//! only ever remove what `askm` can prove it created. See `src/link.rs`'s module
//! doc comment for the rule this file exists to pin down.
//!
//! Everything runs against `tempfile::tempdir()` with `Scope::Project`, never the
//! real `Scope::User` ($HOME).

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use askm::link::{disable, enable, DisableAction, EnableRequest, Occupant};
use askm::paths::{AgentTarget, Scope};
use askm::state::{LinkMode, State};
use tempfile::tempdir;

fn agents_target() -> AgentTarget {
    AgentTarget::new("agents", ".agents")
}

fn plugin_with_skill(root: &Path, skill: &str) -> PathBuf {
    let plugin_root = root.join("store/plugins/official/demo/1.0.0");
    let skill_dir = plugin_root.join("skills").join(skill);
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "placeholder").unwrap();
    plugin_root
}

fn enable_skill(
    state: &State,
    skill: &str,
    plugin_root: &Path,
    target: &AgentTarget,
    scope: &Scope,
    mode: LinkMode,
) -> askm::link::EnableOutcome {
    enable(
        state,
        EnableRequest {
            skill,
            plugin: "demo@official",
            plugin_root,
            target,
            scope,
            mode,
            force: false,
            source: None,
        },
    )
    .unwrap()
}

/// This test is the single most important one in the whole crate: it proves
/// `disable` will never touch a real directory a user made by hand — the exact
/// shape of `~/.agents/skills/agmsg` in the real store this tool manages.
#[test]
fn disable_does_not_remove_a_real_directory_at_the_skill_path() {
    let tmp = tempdir().unwrap();
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();
    let skills_dir = target.skills_dir(&scope).unwrap();
    let real_dir = skills_dir.join("agmsg");
    fs::create_dir_all(&real_dir).unwrap();
    fs::write(real_dir.join("keep-me.txt"), "important user data").unwrap();
    let store_root = tmp.path().join("store");

    let outcome = disable(&State::default(), "agmsg", &target, &scope, &store_root).unwrap();

    assert_eq!(outcome.action, DisableAction::Refused(Occupant::Directory));
    assert!(real_dir.exists(), "the real directory must survive disable");
    assert!(
        real_dir.join("keep-me.txt").exists(),
        "its contents must survive too"
    );
}

/// The second most important test: a hand-made symlink pointing entirely outside
/// askm's store — the exact shape of `~/.agents/skills/quint-lang` pointing at
/// `/data/softwares/quint-llm/...` — must never be removed, even though it *is*
/// a symlink and would otherwise look removable.
#[test]
fn disable_does_not_remove_a_foreign_symlink_pointing_outside_the_store() {
    let tmp = tempdir().unwrap();
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();
    let skills_dir = target.skills_dir(&scope).unwrap();
    fs::create_dir_all(&skills_dir).unwrap();
    let elsewhere = tmp.path().join("elsewhere/quint-llm");
    fs::create_dir_all(&elsewhere).unwrap();
    let link_path = skills_dir.join("quint-lang");
    symlink(&elsewhere, &link_path).unwrap();
    let store_root = tmp.path().join("store");
    fs::create_dir_all(&store_root).unwrap();

    let outcome = disable(
        &State::default(),
        "quint-lang",
        &target,
        &scope,
        &store_root,
    )
    .unwrap();

    assert_eq!(
        outcome.action,
        DisableAction::Refused(Occupant::ForeignSymlink)
    );
    assert!(
        link_path.exists(),
        "the foreign symlink must survive disable"
    );
    assert_eq!(fs::read_link(&link_path).unwrap(), elsewhere);
}

#[test]
fn disable_removes_an_askm_created_symlink_and_clears_its_state_record() {
    let tmp = tempdir().unwrap();
    let plugin_root = plugin_with_skill(&tmp.path().join("store"), "demo-skill");
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();
    let enabled = enable_skill(
        &State::default(),
        "demo-skill",
        &plugin_root,
        &target,
        &scope,
        LinkMode::Symlink,
    );
    let store_root = tmp.path().join("store");

    let outcome = disable(&enabled.state, "demo-skill", &target, &scope, &store_root).unwrap();

    assert_eq!(outcome.action, DisableAction::RemovedSymlink);
    assert!(!enabled.link_path.exists());
    assert!(outcome.state.link_at(&enabled.link_path).is_none());
}

#[test]
fn disable_of_an_absent_link_is_a_no_op_success() {
    let tmp = tempdir().unwrap();
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();
    let store_root = tmp.path().join("store");

    let outcome = disable(
        &State::default(),
        "never-enabled",
        &target,
        &scope,
        &store_root,
    )
    .unwrap();

    assert_eq!(outcome.action, DisableAction::Absent);
}

#[test]
fn disable_removes_a_copy_mode_directory_only_because_state_records_it() {
    let tmp = tempdir().unwrap();
    let plugin_root = plugin_with_skill(&tmp.path().join("store"), "demo-skill");
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();
    let enabled = enable_skill(
        &State::default(),
        "demo-skill",
        &plugin_root,
        &target,
        &scope,
        LinkMode::Copy,
    );
    assert!(fs::symlink_metadata(&enabled.link_path).unwrap().is_dir());
    let store_root = tmp.path().join("store");

    let outcome = disable(&enabled.state, "demo-skill", &target, &scope, &store_root).unwrap();

    assert_eq!(outcome.action, DisableAction::RemovedCopy);
    assert!(!enabled.link_path.exists());
    assert!(outcome.state.link_at(&enabled.link_path).is_none());
}

/// Without a state record, a real directory at the link path is refused even in
/// copy mode's own shape — proving state is what unlocks removal, not merely
/// "this looks like something askm might have copied."
#[test]
fn disable_refuses_a_copy_shaped_directory_that_state_does_not_record() {
    let tmp = tempdir().unwrap();
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();
    let skills_dir = target.skills_dir(&scope).unwrap();
    let dir_path = skills_dir.join("orchestrate-agents");
    fs::create_dir_all(&dir_path).unwrap();
    fs::write(dir_path.join("notes.md"), "hand-written").unwrap();
    let store_root = tmp.path().join("store");

    let outcome = disable(
        &State::default(),
        "orchestrate-agents",
        &target,
        &scope,
        &store_root,
    )
    .unwrap();

    assert_eq!(outcome.action, DisableAction::Refused(Occupant::Directory));
    assert!(dir_path.join("notes.md").exists());
}

/// A recovery path: a symlink askm made lost its state record (state.json edited
/// or wiped by hand), but its target still resolves inside askm's own store, so
/// it is safe to clean up.
#[test]
fn disable_recovers_an_unrecorded_symlink_whose_target_resolves_inside_the_store() {
    let tmp = tempdir().unwrap();
    let store_root = tmp.path().join("store");
    let inside_store = store_root.join("plugins/official/demo/1.0.0/skills/demo-skill");
    fs::create_dir_all(&inside_store).unwrap();
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();
    let skills_dir = target.skills_dir(&scope).unwrap();
    fs::create_dir_all(&skills_dir).unwrap();
    let link_path = skills_dir.join("demo-skill");
    symlink(&inside_store, &link_path).unwrap();

    let outcome = disable(
        &State::default(),
        "demo-skill",
        &target,
        &scope,
        &store_root,
    )
    .unwrap();

    assert_eq!(outcome.action, DisableAction::RemovedSymlink);
    assert!(!link_path.exists());
}

/// A dangling symlink with no state record can't be proven to be askm's, so it
/// is refused, not guessed at.
#[test]
fn disable_refuses_an_unrecorded_dangling_symlink() {
    let tmp = tempdir().unwrap();
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();
    let skills_dir = target.skills_dir(&scope).unwrap();
    fs::create_dir_all(&skills_dir).unwrap();
    let link_path = skills_dir.join("dangling");
    symlink(tmp.path().join("does/not/exist"), &link_path).unwrap();
    let store_root = tmp.path().join("store");

    let outcome = disable(&State::default(), "dangling", &target, &scope, &store_root).unwrap();

    assert_eq!(
        outcome.action,
        DisableAction::Refused(Occupant::ForeignSymlink)
    );
    assert!(link_path.is_symlink());
}
