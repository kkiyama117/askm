//! Integration tests for `askm::link::status` — broken-link detection and
//! managed-vs-foreign classification.
//!
//! Everything runs against `tempfile::tempdir()` with `Scope::Project`, never the
//! real `Scope::User` ($HOME). Shadowing detection itself is covered by a pure
//! unit test in `src/link.rs` (it needs a `ScopeRef::User` entry, which `status`
//! only ever produces by scanning the real `$HOME` — not appropriate here).

use std::fs;
use std::path::{Path, PathBuf};

use askm::link::{enable, status, EnableRequest, EntryKind};
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

fn enable_symlink(
    plugin_root: &Path,
    skill: &str,
    target: &AgentTarget,
    scope: &Scope,
) -> askm::link::EnableOutcome {
    enable(
        &State::default(),
        EnableRequest {
            skill,
            plugin: "demo@official",
            plugin_root,
            target,
            scope,
            mode: LinkMode::Symlink,
            force: false,
            source: None,
        },
    )
    .unwrap()
}

#[test]
fn status_reports_a_managed_symlink_whose_target_was_deleted_as_broken() {
    let tmp = tempdir().unwrap();
    let plugin_root = plugin_with_skill(tmp.path(), "demo-skill");
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();
    let enabled = enable_symlink(&plugin_root, "demo-skill", &target, &scope);
    fs::remove_dir_all(plugin_root.join("skills/demo-skill")).unwrap();

    let report = status(
        &enabled.state,
        std::slice::from_ref(&target),
        std::slice::from_ref(&scope),
    )
    .unwrap();

    let entry = report
        .entries
        .iter()
        .find(|e| e.skill == "demo-skill")
        .expect("the enabled skill should be reported");
    assert!(
        entry.broken,
        "a symlink whose target was deleted must be reported broken"
    );
    assert_eq!(entry.kind, EntryKind::ManagedSymlink);
}

#[test]
fn status_does_not_flag_a_healthy_managed_symlink_as_broken() {
    let tmp = tempdir().unwrap();
    let plugin_root = plugin_with_skill(tmp.path(), "demo-skill");
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();
    let enabled = enable_symlink(&plugin_root, "demo-skill", &target, &scope);

    let report = status(
        &enabled.state,
        std::slice::from_ref(&target),
        std::slice::from_ref(&scope),
    )
    .unwrap();

    let entry = report
        .entries
        .iter()
        .find(|e| e.skill == "demo-skill")
        .unwrap();
    assert!(!entry.broken);
    assert_eq!(
        entry.points_to.as_deref(),
        Some(plugin_root.join("skills/demo-skill").as_path())
    );
}

#[test]
fn status_reports_a_foreign_directory_as_unmanaged_and_not_broken() {
    let tmp = tempdir().unwrap();
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();
    let skills_dir = target.skills_dir(&scope).unwrap();
    fs::create_dir_all(skills_dir.join("agmsg")).unwrap();

    let report = status(
        &State::default(),
        std::slice::from_ref(&target),
        std::slice::from_ref(&scope),
    )
    .unwrap();

    let entry = report.entries.iter().find(|e| e.skill == "agmsg").unwrap();
    assert_eq!(entry.kind, EntryKind::ForeignDirectory);
    assert!(!entry.broken);
}

#[test]
fn status_reports_a_dangling_foreign_symlink_as_broken() {
    let tmp = tempdir().unwrap();
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();
    let skills_dir = target.skills_dir(&scope).unwrap();
    fs::create_dir_all(&skills_dir).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(tmp.path().join("gone"), skills_dir.join("quint-lang")).unwrap();

    let report = status(
        &State::default(),
        std::slice::from_ref(&target),
        std::slice::from_ref(&scope),
    )
    .unwrap();

    let entry = report
        .entries
        .iter()
        .find(|e| e.skill == "quint-lang")
        .unwrap();
    assert_eq!(entry.kind, EntryKind::ForeignSymlink);
    assert!(entry.broken);
}

#[test]
fn status_returns_no_entries_for_a_target_scope_with_no_skills_dir() {
    let tmp = tempdir().unwrap();
    let scope = Scope::Project(tmp.path().join("project"));
    let target = agents_target();

    let report = status(
        &State::default(),
        std::slice::from_ref(&target),
        std::slice::from_ref(&scope),
    )
    .unwrap();

    assert!(report.entries.is_empty());
    assert!(report.shadows.is_empty());
}
