//! Filesystem layout.
//!
//! `askm` keeps everything it owns inside XDG directories and never writes its own
//! bookkeeping into the shared `.agents/` namespace. The only thing it puts under an
//! agent's directory is the skill symlinks themselves, which is what makes it possible
//! to project one store into several agents' skill directories at once.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use etcetera::base_strategy::{BaseStrategy, Xdg};

/// Directory name used under each XDG base directory.
const APP: &str = "askm";

/// Subdirectory every agent uses to hold skills, relative to its dot-directory.
const SKILLS_SUBDIR: &str = "skills";

/// The store `askm` owns. All four roots are independent so a user can, for example,
/// blow away the cache without losing which skills they had enabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    data: PathBuf,
    cache: PathBuf,
    state: PathBuf,
    config: PathBuf,
}

impl Layout {
    /// Resolve the layout from the environment's XDG base directories.
    pub fn from_env() -> Result<Self> {
        let xdg = Xdg::new().context("could not resolve XDG base directories")?;
        Ok(Self {
            data: xdg.data_dir().join(APP),
            cache: xdg.cache_dir().join(APP),
            state: xdg.state_dir().unwrap_or_else(|| xdg.data_dir()).join(APP),
            config: xdg.config_dir().join(APP),
        })
    }

    /// Build a layout rooted at a single directory. Used by tests and by
    /// `ASKM_ROOT` for throwaway or vendored stores.
    pub fn rooted_at(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self {
            data: root.join("data"),
            cache: root.join("cache"),
            state: root.join("state"),
            config: root.join("config"),
        }
    }

    /// Installed plugin packages, laid out per Agent Plugins 1.0.
    pub fn plugins_dir(&self) -> PathBuf {
        self.data.join("plugins")
    }

    /// `PLUGIN_ROOT` for one installed version of a plugin.
    ///
    /// Versions are kept side by side so an upgrade can be rolled back and so a
    /// project can pin a different version than the user default.
    pub fn plugin_root(&self, marketplace: &str, plugin: &str, version: &str) -> PathBuf {
        self.plugins_dir()
            .join(marketplace)
            .join(plugin)
            .join(version)
    }

    /// `PLUGIN_DATA` for a plugin. Deliberately *not* version-scoped: the spec
    /// requires this to survive plugin updates.
    pub fn plugin_data(&self, marketplace: &str, plugin: &str) -> PathBuf {
        self.data.join("plugin-data").join(marketplace).join(plugin)
    }

    /// Cloned marketplace repositories.
    pub fn marketplace_cache(&self, name: &str) -> PathBuf {
        self.cache.join("marketplaces").join(name)
    }

    pub fn marketplaces_dir(&self) -> PathBuf {
        self.cache.join("marketplaces")
    }

    /// Which skills are enabled where.
    pub fn state_file(&self) -> PathBuf {
        self.state.join("state.json")
    }

    /// Config is JSON, not TOML: no TOML crate is in this workspace's dependency
    /// set, and `serde_json` already is. See `crate::config`.
    pub fn config_file(&self) -> PathBuf {
        self.config.join("config.json")
    }

    pub fn data_dir(&self) -> &Path {
        &self.data
    }
}

/// Where an enabled skill gets projected.
///
/// Project scope carries its own root because it is resolved from the working
/// directory, and the spec's precedence rule (project overrides user) depends on
/// knowing both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    User,
    Project(PathBuf),
}

impl Scope {
    /// The directory that agent dot-directories hang off of.
    pub fn base(&self) -> Result<PathBuf> {
        match self {
            Scope::User => etcetera::home_dir().context("could not resolve home directory"),
            Scope::Project(root) => Ok(root.clone()),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Project(_) => "project",
        }
    }
}

/// An agent whose skills directory `askm` can write symlinks into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTarget {
    /// Short name used on the command line, e.g. `agents`, `claude`.
    pub id: String,
    /// Dot-directory relative to the scope base, e.g. `.agents`.
    pub dir: String,
}

impl AgentTarget {
    pub fn new(id: impl Into<String>, dir: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            dir: dir.into(),
        }
    }

    /// Absolute skills directory for this agent within the given scope.
    pub fn skills_dir(&self, scope: &Scope) -> Result<PathBuf> {
        Ok(scope.base()?.join(&self.dir).join(SKILLS_SUBDIR))
    }
}

/// Targets known out of the box.
///
/// `agents` is first and is the default: it is the cross-client convention, so a
/// skill enabled there is visible to every compliant agent at once. The rest exist
/// for agents that only scan their own directory.
pub fn default_targets() -> Vec<AgentTarget> {
    [
        ("agents", ".agents"),
        ("claude", ".claude"),
        ("codex", ".codex"),
        ("gemini", ".gemini"),
        ("cursor", ".cursor"),
        ("opencode", ".opencode"),
    ]
    .iter()
    .map(|(id, dir)| AgentTarget::new(*id, *dir))
    .collect()
}

/// Look up a target by its command-line name.
pub fn find_target(targets: &[AgentTarget], id: &str) -> Option<AgentTarget> {
    targets.iter().find(|t| t.id == id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_data_is_shared_across_versions() {
        let layout = Layout::rooted_at("/store");

        let v1 = layout.plugin_data("official", "superpowers");
        let v2 = layout.plugin_data("official", "superpowers");

        assert_eq!(v1, v2);
        assert!(!v1.to_string_lossy().contains("6.2.0"));
    }

    #[test]
    fn plugin_roots_are_version_scoped() {
        let layout = Layout::rooted_at("/store");

        let old = layout.plugin_root("official", "superpowers", "6.1.0");
        let new = layout.plugin_root("official", "superpowers", "6.2.0");

        assert_ne!(old, new);
    }

    #[test]
    fn store_never_lands_inside_the_agents_namespace() {
        let layout = Layout::rooted_at("/store");

        for path in [
            layout.plugins_dir(),
            layout.marketplaces_dir(),
            layout.state_file(),
            layout.config_file(),
        ] {
            assert!(
                !path.to_string_lossy().contains(".agents"),
                "{path:?} must not live under the shared .agents namespace"
            );
        }
    }

    #[test]
    fn project_scope_resolves_skills_dir_under_project_root() {
        let scope = Scope::Project(PathBuf::from("/work/repo"));
        let target = AgentTarget::new("agents", ".agents");

        let dir = target.skills_dir(&scope).unwrap();

        assert_eq!(dir, PathBuf::from("/work/repo/.agents/skills"));
    }

    #[test]
    fn default_targets_lead_with_the_cross_client_convention() {
        let targets = default_targets();

        assert_eq!(targets[0].id, "agents");
        assert_eq!(targets[0].dir, ".agents");
    }

    #[test]
    fn find_target_returns_none_for_unknown_agent() {
        let targets = default_targets();

        assert!(find_target(&targets, "claude").is_some());
        assert!(find_target(&targets, "nonexistent").is_none());
    }
}
