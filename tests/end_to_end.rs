//! Cross-module flows: install (Phase 3) feeding enable/disable (Phase 4).
//!
//! Each phase was built behind its own boundary, so the seam between them — what
//! path `install` hands to `enable` — is only exercised here. The `git-subdir`
//! case is the one that catches a mismatch: for those plugins the skills live
//! under a subdirectory of the checkout, not at the plugin root.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use askm::link::{self, DisableAction, EnableRequest};
use askm::model::{EntrySource, MarketplaceEntry};
use askm::paths::{AgentTarget, Layout, Scope};
use askm::state::{LinkMode, State};
use askm::store;

fn write_plugin(root: &Path, name: &str, skills: &[&str]) {
    fs::create_dir_all(root).unwrap();
    fs::write(
        root.join("plugin.json"),
        format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
    )
    .unwrap();

    for skill in skills {
        let dir = root.join("skills").join(skill);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {skill}\ndescription: A test skill named {skill}.\n---\n\nBody.\n"),
        )
        .unwrap();
    }
}

fn local_entry(name: &str, path: &str) -> MarketplaceEntry {
    MarketplaceEntry {
        name: name.to_string(),
        version: Some("1.0.0".to_string()),
        description: None,
        source: EntrySource::Local {
            path: path.to_string(),
        },
        category: None,
        keywords: Vec::new(),
        homepage: None,
        author: None,
        explicit_skills: Vec::new(),
    }
}

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Fixture {
    _tmp: tempfile::TempDir,
    layout: Layout,
    project: PathBuf,
    target: AgentTarget,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::rooted_at(tmp.path().join("store"));
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        Self {
            _tmp: tmp,
            layout,
            project,
            target: AgentTarget::new("agents", ".agents"),
        }
    }

    fn scope(&self) -> Scope {
        Scope::Project(self.project.clone())
    }

    fn skills_dir(&self) -> PathBuf {
        self.target.skills_dir(&self.scope()).unwrap()
    }
}

#[test]
fn a_local_plugins_skill_can_be_enabled_and_then_disabled() {
    let fx = Fixture::new();
    let repo = fx._tmp.path().join("marketplace");
    write_plugin(&repo.join("demo-plugin"), "demo-plugin", &["debugging"]);

    let outcome = store::install(
        &fx.layout,
        &State::default(),
        "mk",
        &repo,
        &local_entry("demo-plugin", "./demo-plugin"),
    )
    .unwrap();
    assert_eq!(outcome.installed.skills, vec!["debugging".to_string()]);

    let enabled = link::enable(
        &outcome.state,
        EnableRequest {
            skill: "debugging",
            plugin: "demo-plugin@mk",
            plugin_root: &outcome.effective_root,
            target: &fx.target,
            scope: &fx.scope(),
            mode: LinkMode::Symlink,
            force: false,
            source: None,
        },
    )
    .unwrap();

    let link_path = fx.skills_dir().join("debugging");
    assert!(enabled.created);
    assert!(link_path.join("SKILL.md").is_file(), "link must resolve");

    let disabled = link::disable(
        &enabled.state,
        "debugging",
        &fx.target,
        &fx.scope(),
        fx.layout.data_dir(),
    )
    .unwrap();

    assert_eq!(disabled.action, DisableAction::RemovedSymlink);
    assert!(!link_path.exists());
}

#[test]
fn a_git_subdir_plugins_skills_resolve_through_the_effective_root() {
    let fx = Fixture::new();
    let upstream = fx._tmp.path().join("upstream");
    write_plugin(
        &upstream.join("plugins").join("nested"),
        "nested",
        &["analysis"],
    );
    git(fx._tmp.path(), &["init", "-q", "upstream"]);
    git(&upstream, &["add", "-A"]);
    git(&upstream, &["commit", "-qm", "init"]);

    let entry = MarketplaceEntry {
        source: EntrySource::Git {
            url: upstream.to_string_lossy().to_string(),
            reference: None,
            sha: None,
            subpath: Some("plugins/nested".to_string()),
        },
        ..local_entry("nested", "unused")
    };

    let outcome = store::install(&fx.layout, &State::default(), "mk", &upstream, &entry).unwrap();
    assert_eq!(outcome.installed.skills, vec!["analysis".to_string()]);

    // The seam: the plugin root is the checkout, but the skills live one
    // subdirectory down. Enabling against the checkout root would dangle.
    assert!(
        !fx.layout
            .plugin_root("mk", "nested", "1.0.0")
            .join("skills")
            .exists(),
        "fixture must actually exercise the subdir case"
    );

    let enabled = link::enable(
        &outcome.state,
        EnableRequest {
            skill: "analysis",
            plugin: "nested@mk",
            plugin_root: &outcome.effective_root,
            target: &fx.target,
            scope: &fx.scope(),
            mode: LinkMode::Symlink,
            force: false,
            source: None,
        },
    )
    .unwrap();

    assert!(
        enabled.link_path.join("SKILL.md").is_file(),
        "enabled skill must resolve to a real SKILL.md"
    );
}

#[test]
fn disable_leaves_hand_made_entries_in_a_populated_skills_directory_alone() {
    let fx = Fixture::new();
    let skills = fx.skills_dir();
    fs::create_dir_all(&skills).unwrap();

    // Mirrors a real ~/.agents/skills: a directory the user maintains by hand,
    // and a symlink they made themselves pointing outside any askm store.
    let real_dir = skills.join("agmsg");
    fs::create_dir_all(real_dir.join("scripts")).unwrap();
    fs::write(real_dir.join("SKILL.md"), "---\nname: agmsg\n---\n").unwrap();

    let external = fx._tmp.path().join("elsewhere").join("quint-lang");
    fs::create_dir_all(&external).unwrap();
    fs::write(external.join("SKILL.md"), "---\nname: quint-lang\n---\n").unwrap();
    let foreign_link = skills.join("quint-lang");
    std::os::unix::fs::symlink(&external, &foreign_link).unwrap();

    let state = State::default();
    for skill in ["agmsg", "quint-lang"] {
        let outcome =
            link::disable(&state, skill, &fx.target, &fx.scope(), fx.layout.data_dir()).unwrap();
        assert!(
            matches!(outcome.action, DisableAction::Refused(_)),
            "{skill}: expected a refusal, got {:?}",
            outcome.action
        );
    }

    assert!(real_dir.join("scripts").is_dir(), "real directory survived");
    assert!(fs::symlink_metadata(&foreign_link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(external.join("SKILL.md").is_file(), "link target untouched");
}
