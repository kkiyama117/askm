//! `askm`'s own configuration: registered marketplaces, the default agent
//! target list, and the default scope.
//!
//! Stored as JSON at [`crate::paths::Layout::config_file`] rather than TOML —
//! no TOML crate is in this workspace's dependency set, and `serde_json`
//! already is, so JSON needs no new code and no new dependency. A missing
//! config file is not an error: [`Config::load`] returns [`Config::default`],
//! which matches [`crate::paths::default_targets`] and defaults to a single
//! `agents` target and the user scope.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::EntrySource;

/// Which scope a command targets when neither `--user` nor `--project` is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DefaultScope {
    #[default]
    User,
    Project,
}

fn default_target_list() -> Vec<String> {
    vec!["agents".to_string()]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Registered marketplaces, keyed by the name they were added under. The
    /// source is kept here (not just the synced cache) so `marketplace update`
    /// knows where to re-sync from.
    #[serde(default)]
    pub marketplaces: BTreeMap<String, EntrySource>,
    /// Agent target ids used when `--target` is not given on the command line.
    #[serde(default = "default_target_list")]
    pub default_targets: Vec<String>,
    #[serde(default)]
    pub default_scope: DefaultScope,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            marketplaces: BTreeMap::new(),
            default_targets: default_target_list(),
            default_scope: DefaultScope::default(),
        }
    }
}

impl Config {
    /// Read config from disk. A missing file yields [`Config::default`], not
    /// an error — nothing has been configured yet is a normal starting state.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        serde_json::from_str(&raw)
            .with_context(|| format!("parsing {} — refusing to overwrite it", path.display()))
    }

    /// Write config via a temporary file and a rename, so an interrupted
    /// write cannot leave a half-written config file behind.
    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .with_context(|| format!("{} has no parent directory", path.display()))?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

        let tmp = path.with_extension("json.tmp");
        let encoded = serde_json::to_string_pretty(self).context("encoding config")?;
        fs::write(&tmp, encoded).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, path)
            .with_context(|| format!("replacing {} with {}", path.display(), tmp.display()))?;
        Ok(())
    }

    pub fn with_marketplace(&self, name: impl Into<String>, source: EntrySource) -> Self {
        let mut next = self.clone();
        next.marketplaces.insert(name.into(), source);
        next
    }

    pub fn without_marketplace(&self, name: &str) -> Self {
        let mut next = self.clone();
        next.marketplaces.remove(name);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_returns_sensible_defaults_when_the_file_is_absent() {
        let dir = tempfile::tempdir().unwrap();

        let config = Config::load(&dir.path().join("config.json")).unwrap();

        assert!(config.marketplaces.is_empty());
        assert_eq!(config.default_targets, vec!["agents".to_string()]);
        assert_eq!(config.default_scope, DefaultScope::User);
    }

    #[test]
    fn config_survives_a_save_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");
        let original = Config::default().with_marketplace(
            "official",
            EntrySource::Git {
                url: "https://github.com/obra/superpowers-marketplace.git".to_string(),
                reference: None,
                sha: None,
                subpath: None,
            },
        );

        original.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();

        assert_eq!(original, loaded);
    }

    #[test]
    fn with_marketplace_and_without_marketplace_round_trip() {
        let config = Config::default().with_marketplace(
            "local-dev",
            EntrySource::Local {
                path: "/tmp/plugins".to_string(),
            },
        );
        assert!(config.marketplaces.contains_key("local-dev"));

        let removed = config.without_marketplace("local-dev");
        assert!(!removed.marketplaces.contains_key("local-dev"));
    }

    #[test]
    fn load_refuses_to_silently_reset_a_corrupt_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "{not json").unwrap();

        assert!(Config::load(&path).is_err());
    }

    #[test]
    fn save_leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");

        Config::default().save(&path).unwrap();

        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|name| name != "config.json")
            .collect();
        assert!(leftovers.is_empty(), "unexpected files: {leftovers:?}");
    }

    #[test]
    fn missing_fields_in_a_hand_edited_config_fall_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"marketplaces": {}}"#).unwrap();

        let config = Config::load(&path).unwrap();

        assert_eq!(config.default_targets, vec!["agents".to_string()]);
        assert_eq!(config.default_scope, DefaultScope::User);
    }
}
