//! Persistent record of what `askm` installed and what it linked.
//!
//! This file is the reason `disable` is safe: it is the authority on which entries
//! in an agent's skills directory `askm` created. Anything not recorded here — a
//! hand-made symlink, a real directory a user dropped in — is left alone.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::EntrySource;
use crate::paths::Scope;

const STATE_VERSION: u32 = 1;

/// How an enabled skill is materialized in a target directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkMode {
    Symlink,
    Copy,
}

/// Which scope a link belongs to, in a form that survives serialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ScopeRef {
    User,
    Project { root: PathBuf },
}

impl From<&Scope> for ScopeRef {
    fn from(scope: &Scope) -> Self {
        match scope {
            Scope::User => ScopeRef::User,
            Scope::Project(root) => ScopeRef::Project { root: root.clone() },
        }
    }
}

/// A plugin's identity. Rendered as `<plugin>@<marketplace>`, matching how users
/// refer to plugins on the command line.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PluginId {
    pub plugin: String,
    pub marketplace: String,
}

impl PluginId {
    pub fn new(plugin: impl Into<String>, marketplace: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            marketplace: marketplace.into(),
        }
    }

    /// Parse `<plugin>@<marketplace>`. The marketplace half is required because a
    /// bare plugin name is ambiguous once more than one marketplace is registered.
    pub fn parse(raw: &str) -> Result<Self> {
        let (plugin, marketplace) = raw
            .split_once('@')
            .with_context(|| format!("expected <plugin>@<marketplace>, got {raw:?}"))?;
        if plugin.is_empty() || marketplace.is_empty() {
            anyhow::bail!("expected <plugin>@<marketplace>, got {raw:?}");
        }
        Ok(Self::new(plugin, marketplace))
    }
}

impl std::fmt::Display for PluginId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.plugin, self.marketplace)
    }
}

/// One installed plugin version in the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub plugin: String,
    pub marketplace: String,
    pub version: String,
    pub source: EntrySource,
    pub installed_at: u64,
    /// Skill directory names discovered under the plugin's `skills/`.
    #[serde(default)]
    pub skills: Vec<String>,
}

/// One skill projected into one agent's skills directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkRecord {
    pub skill: String,
    /// `<plugin>@<marketplace>` this skill came from.
    pub plugin: String,
    /// Agent target id, e.g. `agents` or `claude`.
    pub target: String,
    pub scope: ScopeRef,
    pub link_path: PathBuf,
    pub points_to: PathBuf,
    pub mode: LinkMode,
    pub created_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    /// Keyed by `<plugin>@<marketplace>`.
    #[serde(default)]
    pub installed: BTreeMap<String, InstalledPlugin>,
    #[serde(default)]
    pub links: Vec<LinkRecord>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            installed: BTreeMap::new(),
            links: Vec::new(),
        }
    }
}

impl State {
    /// Read state from disk. A missing file is not an error — it just means
    /// nothing has been installed yet.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };

        let state: State = serde_json::from_str(&raw)
            .with_context(|| format!("parsing {} — refusing to overwrite it", path.display()))?;

        if state.version > STATE_VERSION {
            anyhow::bail!(
                "{} was written by a newer askm (state version {}, this build understands {})",
                path.display(),
                state.version,
                STATE_VERSION
            );
        }
        Ok(state)
    }

    /// Write state via a temporary file and a rename, so an interrupted write
    /// cannot leave a half-written state file behind.
    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .with_context(|| format!("{} has no parent directory", path.display()))?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

        let tmp = path.with_extension("json.tmp");
        let encoded = serde_json::to_string_pretty(self).context("encoding state")?;
        fs::write(&tmp, encoded).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, path)
            .with_context(|| format!("replacing {} with {}", path.display(), tmp.display()))?;
        Ok(())
    }

    pub fn record_install(&self, installed: InstalledPlugin) -> Self {
        let key = PluginId::new(&installed.plugin, &installed.marketplace).to_string();
        let mut next = self.clone();
        next.installed.insert(key, installed);
        next
    }

    pub fn forget_install(&self, id: &PluginId) -> Self {
        let key = id.to_string();
        let mut next = self.clone();
        next.installed.remove(&key);
        next.links.retain(|link| link.plugin != key);
        next
    }

    pub fn installed(&self, id: &PluginId) -> Option<&InstalledPlugin> {
        self.installed.get(&id.to_string())
    }

    /// Add or replace the record for one link path.
    pub fn record_link(&self, record: LinkRecord) -> Self {
        let mut next = self.clone();
        next.links.retain(|link| link.link_path != record.link_path);
        next.links.push(record);
        next
    }

    pub fn forget_link(&self, link_path: &Path) -> Self {
        let mut next = self.clone();
        next.links.retain(|link| link.link_path != link_path);
        next
    }

    /// The record for a link path, if `askm` created it.
    pub fn link_at(&self, link_path: &Path) -> Option<&LinkRecord> {
        self.links.iter().find(|link| link.link_path == link_path)
    }

    pub fn links_for_skill<'a>(&'a self, skill: &'a str) -> impl Iterator<Item = &'a LinkRecord> {
        self.links.iter().filter(move |link| link.skill == skill)
    }
}

/// Seconds since the Unix epoch, saturating at 0 for clocks set before 1970.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_link(skill: &str, link_path: &str) -> LinkRecord {
        LinkRecord {
            skill: skill.to_string(),
            plugin: "superpowers@official".to_string(),
            target: "agents".to_string(),
            scope: ScopeRef::User,
            link_path: PathBuf::from(link_path),
            points_to: PathBuf::from("/store/plugins/official/superpowers/6.2.0/skills/x"),
            mode: LinkMode::Symlink,
            created_at: 0,
        }
    }

    fn sample_install() -> InstalledPlugin {
        InstalledPlugin {
            plugin: "superpowers".to_string(),
            marketplace: "official".to_string(),
            version: "6.2.0".to_string(),
            source: EntrySource::Git {
                url: "https://github.com/obra/superpowers.git".to_string(),
                reference: None,
                sha: Some("abc123".to_string()),
                subpath: None,
            },
            installed_at: 0,
            skills: vec!["systematic-debugging".to_string()],
        }
    }

    #[test]
    fn load_returns_empty_state_when_file_is_absent() {
        let dir = tempfile::tempdir().unwrap();

        let state = State::load(&dir.path().join("state.json")).unwrap();

        assert_eq!(state, State::default());
    }

    #[test]
    fn state_survives_a_save_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("state.json");
        let original = State::default()
            .record_install(sample_install())
            .record_link(sample_link(
                "systematic-debugging",
                "/home/u/.agents/skills/sd",
            ));

        original.save(&path).unwrap();
        let loaded = State::load(&path).unwrap();

        assert_eq!(original, loaded);
    }

    #[test]
    fn save_leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");

        State::default().save(&path).unwrap();

        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|name| name != "state.json")
            .collect();
        assert!(leftovers.is_empty(), "unexpected files: {leftovers:?}");
    }

    #[test]
    fn load_rejects_state_written_by_a_newer_askm() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        fs::write(&path, r#"{"version":99,"installed":{},"links":[]}"#).unwrap();

        let err = State::load(&path).unwrap_err().to_string();

        assert!(err.contains("newer askm"), "unexpected error: {err}");
    }

    #[test]
    fn load_refuses_to_silently_reset_a_corrupt_state_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        fs::write(&path, "{not json").unwrap();

        assert!(State::load(&path).is_err());
    }

    #[test]
    fn recording_a_link_twice_for_one_path_replaces_it() {
        let first = sample_link("a", "/home/u/.agents/skills/a");
        let mut second = sample_link("b", "/home/u/.agents/skills/a");
        second.created_at = 42;

        let state = State::default().record_link(first).record_link(second);

        assert_eq!(state.links.len(), 1);
        assert_eq!(state.links[0].skill, "b");
    }

    #[test]
    fn forgetting_an_install_also_forgets_its_links() {
        let state = State::default()
            .record_install(sample_install())
            .record_link(sample_link("a", "/home/u/.agents/skills/a"));

        let next = state.forget_install(&PluginId::new("superpowers", "official"));

        assert!(next.installed.is_empty());
        assert!(next.links.is_empty());
    }

    #[test]
    fn link_at_only_reports_paths_askm_created() {
        let state = State::default().record_link(sample_link("a", "/home/u/.agents/skills/a"));

        assert!(state
            .link_at(Path::new("/home/u/.agents/skills/a"))
            .is_some());
        assert!(state
            .link_at(Path::new("/home/u/.agents/skills/agmsg"))
            .is_none());
    }

    #[test]
    fn plugin_id_parses_and_renders_the_at_form() {
        let id = PluginId::parse("superpowers@official").unwrap();

        assert_eq!(id.plugin, "superpowers");
        assert_eq!(id.marketplace, "official");
        assert_eq!(id.to_string(), "superpowers@official");
    }

    #[test]
    fn plugin_id_rejects_input_without_a_marketplace() {
        for raw in ["superpowers", "superpowers@", "@official", ""] {
            assert!(PluginId::parse(raw).is_err(), "{raw:?} should not parse");
        }
    }
}
