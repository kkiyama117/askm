//! Marketplace and plugin content sources.
//!
//! A [`Source`] populates a destination directory with content — either by
//! cloning/fetching from git ([`git::GitSource`]), or by registering a local
//! path via a symlink, never copying ([`local::LocalSource`]). [`catalog`]
//! parses catalog documents that list plugins by an [`crate::model::EntrySource`]-
//! shaped source. [`MarketplaceRegistry`] ties `Source` syncing to
//! [`Layout::marketplace_cache`] for the add/list/remove/update lifecycle of
//! registered marketplaces.

pub mod catalog;
pub mod git;
pub mod local;

use std::fs;
use std::io;
use std::path::Path;

use anyhow::{Context, Result};

use crate::manifest::marketplace;
use crate::model::{Marketplace, Warning};
use crate::paths::Layout;

pub use git::GitSource;
pub use local::LocalSource;

/// The result of one [`Source::sync`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    /// The resolved revision, when the source has one (a git commit sha).
    pub revision: Option<String>,
    /// Whether `dest`'s content changed as a result of this call.
    pub changed: bool,
}

/// Populates a destination directory with a source's content.
pub trait Source {
    fn sync(&self, dest: &Path) -> Result<SyncOutcome>;
}

/// Marketplace registry operations backed by [`Layout::marketplace_cache`].
pub struct MarketplaceRegistry<'a> {
    layout: &'a Layout,
}

impl<'a> MarketplaceRegistry<'a> {
    pub fn new(layout: &'a Layout) -> Self {
        Self { layout }
    }

    /// Sync `source` into the cache under `name` and parse its `marketplace.json`.
    pub fn add(
        &self,
        name: &str,
        source: &dyn Source,
    ) -> Result<(Marketplace, Vec<Warning>, SyncOutcome)> {
        let dest = self.layout.marketplace_cache(name);
        let outcome = source
            .sync(&dest)
            .with_context(|| format!("syncing marketplace {name:?}"))?;
        let (parsed, warnings) = marketplace::load_from_repo(&dest)
            .with_context(|| format!("loading marketplace.json for {name:?} after sync"))?;
        Ok((parsed, warnings, outcome))
    }

    /// Re-sync an already-registered marketplace. Identical to [`Self::add`]:
    /// every `Source` impl treats a re-sync of an existing destination as an
    /// update rather than a fresh fetch.
    pub fn update(
        &self,
        name: &str,
        source: &dyn Source,
    ) -> Result<(Marketplace, Vec<Warning>, SyncOutcome)> {
        self.add(name, source)
    }

    /// Names of marketplaces currently in the cache.
    pub fn list(&self) -> Result<Vec<String>> {
        let dir = self.layout.marketplaces_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };

        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("reading entry in {}", dir.display()))?;
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    /// Delete a marketplace's cache entry. A no-op if it is not present.
    pub fn remove(&self, name: &str) -> Result<()> {
        remove_path(&self.layout.marketplace_cache(name))
    }
}

/// Remove a path that may be a plain directory or a symlink (e.g. one
/// [`LocalSource::sync`] created). Symlinks are unlinked, never followed for
/// recursive removal — following one would delete the real target's
/// content, which for a `Local` source is a directory the user owns.
/// A missing path is treated as already removed.
pub(crate) fn remove_path(path: &Path) -> Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("inspecting {}", path.display())),
    };

    if meta.file_type().is_symlink() || meta.is_file() {
        fs::remove_file(path).with_context(|| format!("removing {}", path.display()))
    } else {
        fs::remove_dir_all(path).with_context(|| format!("removing {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    struct RecordingSource {
        marketplace_json: &'static str,
    }

    impl Source for RecordingSource {
        fn sync(&self, dest: &Path) -> Result<SyncOutcome> {
            let plugins_dir = dest.join(".agents").join("plugins");
            fs::create_dir_all(&plugins_dir)?;
            fs::write(plugins_dir.join("marketplace.json"), self.marketplace_json)?;
            Ok(SyncOutcome {
                revision: Some("v1".to_string()),
                changed: true,
            })
        }
    }

    const DEMO_MARKETPLACE: &str = r#"{
        "name": "demo",
        "plugins": [{"name": "demo-plugin", "source": "./"}]
    }"#;

    #[test]
    fn add_syncs_and_parses_the_resulting_marketplace_json() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::rooted_at(temp.path());
        let registry = MarketplaceRegistry::new(&layout);
        let source = RecordingSource {
            marketplace_json: DEMO_MARKETPLACE,
        };

        let (marketplace, warnings, outcome) = registry.add("demo", &source).unwrap();

        assert_eq!(marketplace.name, "demo");
        assert!(warnings.is_empty());
        assert_eq!(outcome.revision.as_deref(), Some("v1"));
    }

    #[test]
    fn list_enumerates_cached_marketplace_names() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::rooted_at(temp.path());
        let registry = MarketplaceRegistry::new(&layout);
        registry
            .add(
                "demo",
                &RecordingSource {
                    marketplace_json: DEMO_MARKETPLACE,
                },
            )
            .unwrap();

        let names = registry.list().unwrap();

        assert_eq!(names, vec!["demo".to_string()]);
    }

    #[test]
    fn list_returns_empty_when_no_marketplaces_are_cached() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::rooted_at(temp.path());
        let registry = MarketplaceRegistry::new(&layout);

        assert!(registry.list().unwrap().is_empty());
    }

    #[test]
    fn remove_deletes_the_cache_directory() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::rooted_at(temp.path());
        let registry = MarketplaceRegistry::new(&layout);
        registry
            .add(
                "demo",
                &RecordingSource {
                    marketplace_json: DEMO_MARKETPLACE,
                },
            )
            .unwrap();

        registry.remove("demo").unwrap();

        assert!(registry.list().unwrap().is_empty());
        assert!(!layout.marketplace_cache("demo").exists());
    }

    #[test]
    fn remove_is_a_no_op_for_a_marketplace_that_was_never_added() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::rooted_at(temp.path());
        let registry = MarketplaceRegistry::new(&layout);

        assert!(registry.remove("never-added").is_ok());
    }

    #[test]
    fn remove_unlinks_a_symlinked_cache_entry_without_touching_its_target() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::rooted_at(temp.path());
        let target = temp.path().join("real-marketplace");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("marker.txt"), "keep me").unwrap();
        let cache_entry = layout.marketplace_cache("linked");
        fs::create_dir_all(cache_entry.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &cache_entry).unwrap();

        remove_path(&cache_entry).unwrap();

        assert!(!cache_entry.exists());
        assert!(target.join("marker.txt").exists(), "target must survive");
    }
}
