//! Integration tests for `askm::link::enable`.
//!
//! Everything runs against `tempfile::tempdir()` with `Scope::Project`, never the
//! real `Scope::User` ($HOME).

use std::fs;
use std::path::{Path, PathBuf};

use askm::link::{enable, EnableError, EnableRequest};
use askm::paths::{AgentTarget, Scope};
use askm::state::{LinkMode, State};
use tempfile::tempdir;

fn agents_target() -> AgentTarget {
    AgentTarget::new("agents", ".agents")
}

/// Build a fake installed plugin under `root` with one skill directory, and
/// return its `PLUGIN_ROOT`.
fn plugin_with_skill(root: &Path, version: &str, skill: &str) -> PathBuf {
    let plugin_root = root.join("store/plugins/official/demo").join(version);
    let skill_dir = plugin_root.join("skills").join(skill);
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "placeholder").unwrap();
    plugin_root
}

fn request<'a>(
    skill: &'a str,
    plugin_root: &'a Path,
    target: &'a AgentTarget,
    scope: &'a Scope,
    mode: LinkMode,
    force: bool,
) -> EnableRequest<'a> {
    EnableRequest {
        skill,
        plugin: "demo@official",
        plugin_root,
        target,
        scope,
        mode,
        force,
    }
}

#[test]
fn enable_creates_a_symlink_pointing_at_the_skill_in_the_plugin_root() {
    let tmp = tempdir().unwrap();
    let plugin_root = plugin_with_skill(tmp.path(), "1.0.0", "demo-skill");
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();

    let outcome = enable(
        &State::default(),
        request(
            "demo-skill",
            &plugin_root,
            &target,
            &scope,
            LinkMode::Symlink,
            false,
        ),
    )
    .unwrap();

    assert!(outcome.created);
    assert_eq!(
        outcome.link_path,
        tmp.path().join("project/.agents/skills/demo-skill")
    );
    assert_eq!(outcome.points_to, plugin_root.join("skills/demo-skill"));
    let on_disk_target = fs::read_link(&outcome.link_path).unwrap();
    assert_eq!(on_disk_target, outcome.points_to);
}

#[test]
fn enable_twice_is_idempotent_and_leaves_exactly_one_link() {
    let tmp = tempdir().unwrap();
    let plugin_root = plugin_with_skill(tmp.path(), "1.0.0", "demo-skill");
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();

    let first = enable(
        &State::default(),
        request(
            "demo-skill",
            &plugin_root,
            &target,
            &scope,
            LinkMode::Symlink,
            false,
        ),
    )
    .unwrap();
    let second = enable(
        &first.state,
        request(
            "demo-skill",
            &plugin_root,
            &target,
            &scope,
            LinkMode::Symlink,
            false,
        ),
    )
    .unwrap();

    assert!(first.created);
    assert!(!second.created, "the second call should be a no-op");
    assert_eq!(second.state.links.len(), 1);
    let siblings: Vec<_> = fs::read_dir(first.link_path.parent().unwrap())
        .unwrap()
        .collect();
    assert_eq!(
        siblings.len(),
        1,
        "exactly one entry should exist in the skills dir"
    );
}

#[test]
fn enable_refuses_when_a_foreign_directory_occupies_the_path() {
    let tmp = tempdir().unwrap();
    let plugin_root = plugin_with_skill(tmp.path(), "1.0.0", "demo-skill");
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();
    let link_path = target.skills_dir(&scope).unwrap().join("demo-skill");
    fs::create_dir_all(&link_path).unwrap();
    fs::write(link_path.join("real-file.txt"), "not askm's").unwrap();

    let err = enable(
        &State::default(),
        request(
            "demo-skill",
            &plugin_root,
            &target,
            &scope,
            LinkMode::Symlink,
            false,
        ),
    )
    .unwrap_err();

    assert!(
        err.downcast_ref::<EnableError>().is_some(),
        "collision must surface the typed EnableError, got: {err}"
    );
    assert!(
        link_path.join("real-file.txt").exists(),
        "foreign data must survive a refused enable"
    );
}

#[test]
fn enable_refuses_when_a_foreign_file_occupies_the_path() {
    let tmp = tempdir().unwrap();
    let plugin_root = plugin_with_skill(tmp.path(), "1.0.0", "demo-skill");
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();
    let link_path = target.skills_dir(&scope).unwrap().join("demo-skill");
    fs::create_dir_all(link_path.parent().unwrap()).unwrap();
    fs::write(&link_path, "just a stray file").unwrap();

    let err = enable(
        &State::default(),
        request(
            "demo-skill",
            &plugin_root,
            &target,
            &scope,
            LinkMode::Symlink,
            false,
        ),
    )
    .unwrap_err();

    assert!(err.downcast_ref::<EnableError>().is_some());
    assert_eq!(fs::read_to_string(&link_path).unwrap(), "just a stray file");
}

#[test]
fn enable_with_force_replaces_a_link_askm_previously_managed() {
    let tmp = tempdir().unwrap();
    let plugin_root_v1 = plugin_with_skill(tmp.path(), "1.0.0", "demo-skill");
    let plugin_root_v2 = plugin_with_skill(tmp.path(), "2.0.0", "demo-skill");
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();

    let first = enable(
        &State::default(),
        request(
            "demo-skill",
            &plugin_root_v1,
            &target,
            &scope,
            LinkMode::Symlink,
            false,
        ),
    )
    .unwrap();

    // Pointing the same skill at a different version, without force, is a
    // collision (askm made the existing link, but not at this target) — not a
    // silent switch.
    let refused = enable(
        &first.state,
        request(
            "demo-skill",
            &plugin_root_v2,
            &target,
            &scope,
            LinkMode::Symlink,
            false,
        ),
    );
    assert!(refused.is_err());
    assert_eq!(
        fs::read_link(&first.link_path).unwrap(),
        plugin_root_v1.join("skills/demo-skill")
    );

    let forced = enable(
        &first.state,
        request(
            "demo-skill",
            &plugin_root_v2,
            &target,
            &scope,
            LinkMode::Symlink,
            true,
        ),
    )
    .unwrap();

    assert!(forced.created);
    assert_eq!(forced.points_to, plugin_root_v2.join("skills/demo-skill"));
    assert_eq!(
        fs::read_link(&forced.link_path).unwrap(),
        plugin_root_v2.join("skills/demo-skill")
    );
    assert_eq!(
        forced.state.links.len(),
        1,
        "the stale record must be replaced, not duplicated"
    );
}

/// The safety rule extends into `enable`: `force` may swap a symlink (askm's own
/// or not — symlinks are cheap to repoint) but must never delete a real
/// directory askm has no proof it created, exactly like `disable`.
#[test]
fn enable_with_force_still_refuses_a_foreign_directory() {
    let tmp = tempdir().unwrap();
    let plugin_root = plugin_with_skill(tmp.path(), "1.0.0", "demo-skill");
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();
    let link_path = target.skills_dir(&scope).unwrap().join("demo-skill");
    fs::create_dir_all(&link_path).unwrap();
    fs::write(link_path.join("real-file.txt"), "not askm's, do not delete").unwrap();

    let result = enable(
        &State::default(),
        request(
            "demo-skill",
            &plugin_root,
            &target,
            &scope,
            LinkMode::Symlink,
            true,
        ),
    );

    assert!(
        result.is_err(),
        "force must not override the safety check for a foreign directory"
    );
    assert!(
        link_path.is_dir(),
        "the foreign directory must survive a forced enable"
    );
    assert_eq!(
        fs::read_to_string(link_path.join("real-file.txt")).unwrap(),
        "not askm's, do not delete"
    );
}

#[test]
fn enable_in_copy_mode_materializes_a_real_directory_with_the_skill_contents() {
    let tmp = tempdir().unwrap();
    let plugin_root = plugin_with_skill(tmp.path(), "1.0.0", "demo-skill");
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();

    let outcome = enable(
        &State::default(),
        request(
            "demo-skill",
            &plugin_root,
            &target,
            &scope,
            LinkMode::Copy,
            false,
        ),
    )
    .unwrap();

    let meta = fs::symlink_metadata(&outcome.link_path).unwrap();
    assert!(meta.is_dir(), "copy mode must materialize a real directory");
    assert!(!meta.file_type().is_symlink());
    assert_eq!(
        fs::read_to_string(outcome.link_path.join("SKILL.md")).unwrap(),
        "placeholder"
    );
    let record = outcome.state.link_at(&outcome.link_path).unwrap();
    assert_eq!(record.mode, LinkMode::Copy);
}

#[test]
fn enable_in_copy_mode_is_also_idempotent() {
    let tmp = tempdir().unwrap();
    let plugin_root = plugin_with_skill(tmp.path(), "1.0.0", "demo-skill");
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();

    let first = enable(
        &State::default(),
        request(
            "demo-skill",
            &plugin_root,
            &target,
            &scope,
            LinkMode::Copy,
            false,
        ),
    )
    .unwrap();
    let second = enable(
        &first.state,
        request(
            "demo-skill",
            &plugin_root,
            &target,
            &scope,
            LinkMode::Copy,
            false,
        ),
    )
    .unwrap();

    assert!(!second.created);
    assert_eq!(second.state.links.len(), 1);
}
