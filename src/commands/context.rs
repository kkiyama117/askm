//! Shared command context: resolved store layout, loaded config and state,
//! plus the `--target`/`--user`/`--project` resolution nearly every command needs.

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};
use askm::config::{Config, DefaultScope};
use askm::paths::{default_targets, find_target, AgentTarget, Layout, Scope};
use askm::state::State;

/// Bundles what nearly every command needs, loaded once at startup.
pub struct Context {
    pub layout: Layout,
    pub config: Config,
    pub state: State,
    pub json: bool,
}

impl Context {
    pub fn load(store_root: Option<&Path>, json: bool) -> Result<Self> {
        let layout = match store_root {
            Some(root) => Layout::rooted_at(root),
            None => Layout::from_env().context("resolving askm's store location")?,
        };
        let config = Config::load(&layout.config_file())?;
        let state = State::load(&layout.state_file())?;
        Ok(Self {
            layout,
            config,
            state,
            json,
        })
    }

    /// Persist `state` to disk and adopt it as the context's current state, so
    /// later steps within the same command see the update immediately.
    pub fn persist_state(&mut self, state: State) -> Result<()> {
        state.save(&self.layout.state_file())?;
        self.state = state;
        Ok(())
    }

    pub fn persist_config(&mut self, config: Config) -> Result<()> {
        config.save(&self.layout.config_file())?;
        self.config = config;
        Ok(())
    }
}

/// Resolve `--target`'s comma-separated ids against the known target set,
/// falling back to the config's default list when none were given.
pub fn resolve_targets(config: &Config, requested: Option<&[String]>) -> Result<Vec<AgentTarget>> {
    let all = default_targets();
    let ids: Vec<String> = match requested {
        Some(ids) => ids.to_vec(),
        None => config.default_targets.clone(),
    };
    if ids.is_empty() {
        bail!("no agent targets to act on; pass --target or set default_targets in the config");
    }
    ids.iter().map(|id| lookup_target(&all, id)).collect()
}

fn lookup_target(all: &[AgentTarget], id: &str) -> Result<AgentTarget> {
    find_target(all, id).with_context(|| {
        let valid: Vec<&str> = all.iter().map(|t| t.id.as_str()).collect();
        format!(
            "unknown agent target {id:?}; valid targets are: {}",
            valid.join(", ")
        )
    })
}

/// Resolve `--user`/`--project` (mutually exclusive, enforced by clap) into a
/// `Scope`. With neither flag, falls back to the config's default scope
/// (`user` by default). `--project` walks up from the current directory to
/// the nearest ancestor containing `.git`, or uses the current directory
/// itself if none is found.
pub fn resolve_scope(user: bool, project: bool, config: &Config) -> Result<Scope> {
    let use_project = project || (!user && config.default_scope == DefaultScope::Project);
    if !use_project {
        return Ok(Scope::User);
    }
    let cwd = env::current_dir().context("resolving the current directory")?;
    Ok(Scope::Project(project_root(&cwd)))
}

/// Both scopes at once, for commands (`status`, `doctor`) that report across
/// the whole store rather than acting in a single resolved scope.
pub fn all_scopes() -> Result<Vec<Scope>> {
    let cwd = env::current_dir().context("resolving the current directory")?;
    Ok(vec![Scope::User, Scope::Project(project_root(&cwd))])
}

fn project_root(start: &Path) -> PathBuf {
    let mut dir = start;
    loop {
        if dir.join(".git").is_dir() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return start.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn project_root_walks_up_to_the_nearest_git_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let nested = repo.join("a/b/c");
        fs::create_dir_all(&nested).unwrap();

        assert_eq!(project_root(&nested), repo);
    }

    #[test]
    fn project_root_falls_back_to_the_start_dir_when_no_git_directory_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let leaf = tmp.path().join("no/git/here");
        fs::create_dir_all(&leaf).unwrap();

        assert_eq!(project_root(&leaf), leaf);
    }

    #[test]
    fn resolve_targets_falls_back_to_config_default_when_none_requested() {
        let config = Config::default();

        let targets = resolve_targets(&config, None).unwrap();

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, "agents");
    }

    #[test]
    fn resolve_targets_rejects_an_unknown_id_with_a_helpful_message() {
        let config = Config::default();

        let err = resolve_targets(&config, Some(&["nonexistent".to_string()])).unwrap_err();

        assert!(err.to_string().contains("unknown agent target"), "{err}");
        assert!(err.to_string().contains("agents"), "{err}");
    }

    #[test]
    fn resolve_scope_defaults_to_user_with_neither_flag() {
        let config = Config::default();

        let scope = resolve_scope(false, false, &config).unwrap();

        assert_eq!(scope, Scope::User);
    }
}
