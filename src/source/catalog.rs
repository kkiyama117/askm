//! Catalog documents: JSON listings of plugins keyed by id, each carrying a
//! [`MarketplaceEntry`]-shaped source.
//!
//! There is no HTTP client in this crate's dependency set, so catalogs are
//! loaded from a local file or from bytes already in hand (`Catalog`'s
//! [`FromStr`] impl, and [`Catalog::from_reader`]). Both funnel through the
//! same [`parse`] path, so a network fetcher can be layered on top later —
//! fetch the response body, then hand it to `Catalog::from_reader` — without
//! touching this module.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::model::{Author, EntrySource, MarketplaceEntry};

/// A parsed catalog: plugins keyed by their catalog id (or, for a list-shaped
/// catalog, by the entry's own `name`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub plugins: BTreeMap<String, MarketplaceEntry>,
}

/// `Catalog::from_str` is implemented via the standard `FromStr` trait
/// (rather than an inherent method of the same name) so it also gives callers
/// `content.parse::<Catalog>()` for free.
impl FromStr for Catalog {
    type Err = anyhow::Error;

    fn from_str(content: &str) -> Result<Self> {
        parse(content)
    }
}

impl Catalog {
    pub fn from_reader<R: Read>(mut reader: R) -> Result<Self> {
        let mut buf = String::new();
        reader
            .read_to_string(&mut buf)
            .context("reading catalog input")?;
        parse(&buf)
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        parse(&content)
    }
}

fn parse(content: &str) -> Result<Catalog> {
    let doc: CatalogDocument =
        serde_json::from_str(content).context("failed to parse catalog JSON")?;
    let plugins_value = doc
        .catalog
        .and_then(|c| c.plugins)
        .or(doc.plugins)
        .context("catalog JSON has no \"plugins\" field (checked top level and catalog.plugins)")?;
    Ok(Catalog {
        plugins: collect_entries(plugins_value),
    })
}

fn collect_entries(value: PluginsValue) -> BTreeMap<String, MarketplaceEntry> {
    match value {
        PluginsValue::Map(map) => map
            .into_iter()
            .map(|(id, shape)| (id, unwrap_shape(shape).into()))
            .collect(),
        PluginsValue::List(list) => list
            .into_iter()
            .map(|shape| {
                let entry: MarketplaceEntry = unwrap_shape(shape).into();
                (entry.name.clone(), entry)
            })
            .collect(),
    }
}

fn unwrap_shape(shape: CatalogEntryShape) -> CatalogEntry {
    match shape {
        CatalogEntryShape::Wrapped(wrapped) => wrapped.marketplace_entry,
        CatalogEntryShape::Bare(entry) => entry,
    }
}

// --- wire shapes -------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CatalogDocument {
    #[serde(default)]
    catalog: Option<NestedCatalog>,
    #[serde(default)]
    plugins: Option<PluginsValue>,
}

#[derive(Debug, Deserialize)]
struct NestedCatalog {
    #[serde(default)]
    plugins: Option<PluginsValue>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PluginsValue {
    List(Vec<CatalogEntryShape>),
    Map(BTreeMap<String, CatalogEntryShape>),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CatalogEntryShape {
    /// Anthropic's shape: `catalog.plugins.<id>.marketplace_entry`.
    Wrapped(WrappedCatalogEntry),
    /// A bare entry, for producers that skip the `marketplace_entry` nesting.
    Bare(CatalogEntry),
}

#[derive(Debug, Deserialize)]
struct WrappedCatalogEntry {
    marketplace_entry: CatalogEntry,
}

/// Mirrors [`MarketplaceEntry`]'s fields. Kept as a separate raw struct
/// because `MarketplaceEntry` only derives `Serialize` — parsing always goes
/// through a producer-specific raw shape and a `From` impl, the same pattern
/// `manifest::marketplace` uses for its two dialects.
#[derive(Debug, Deserialize)]
struct CatalogEntry {
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    source: EntrySource,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    author: Option<Author>,
    #[serde(default)]
    skills: Vec<String>,
}

impl From<CatalogEntry> for MarketplaceEntry {
    fn from(e: CatalogEntry) -> Self {
        MarketplaceEntry {
            name: e.name,
            version: e.version,
            description: e.description,
            source: e.source,
            category: e.category,
            keywords: e.keywords,
            homepage: e.homepage,
            author: e.author,
            explicit_skills: e.skills,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANTHROPIC_SHAPED: &str = r#"{
        "catalog": {
            "plugins": {
                "superpowers": {
                    "marketplace_entry": {
                        "name": "superpowers",
                        "version": "6.2.0",
                        "source": {"source": "url", "url": "https://github.com/obra/superpowers.git", "sha": "abc123"}
                    }
                }
            }
        }
    }"#;

    #[test]
    fn parses_the_nested_catalog_plugins_id_marketplace_entry_shape() {
        let catalog = Catalog::from_str(ANTHROPIC_SHAPED).unwrap();

        let entry = catalog.plugins.get("superpowers").expect("plugin present");
        assert_eq!(entry.name, "superpowers");
        assert_eq!(entry.version.as_deref(), Some("6.2.0"));
        assert_eq!(
            entry.source,
            EntrySource::Git {
                url: "https://github.com/obra/superpowers.git".to_string(),
                reference: None,
                sha: Some("abc123".to_string()),
                subpath: None,
            }
        );
    }

    #[test]
    fn parses_a_flat_plugins_map_without_the_catalog_wrapper() {
        let content = r#"{
            "plugins": {
                "demo": {"name": "demo", "source": "./"}
            }
        }"#;

        let catalog = Catalog::from_str(content).unwrap();

        assert_eq!(catalog.plugins["demo"].name, "demo");
    }

    #[test]
    fn parses_a_plugins_array_keyed_by_entry_name() {
        let content = r#"{
            "plugins": [
                {"name": "demo-one", "source": "./one"},
                {"name": "demo-two", "source": "./two"}
            ]
        }"#;

        let catalog = Catalog::from_str(content).unwrap();

        assert_eq!(catalog.plugins.len(), 2);
        assert_eq!(
            catalog.plugins["demo-one"].source,
            EntrySource::Local {
                path: "./one".to_string()
            }
        );
    }

    #[test]
    fn from_reader_parses_the_same_bytes_as_from_str() {
        let bytes = ANTHROPIC_SHAPED.as_bytes();

        let catalog = Catalog::from_reader(bytes).unwrap();

        assert!(catalog.plugins.contains_key("superpowers"));
    }

    #[test]
    fn from_path_reads_a_catalog_file_from_disk() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("catalog.json");
        fs::write(&path, ANTHROPIC_SHAPED).unwrap();

        let catalog = Catalog::from_path(&path).unwrap();

        assert!(catalog.plugins.contains_key("superpowers"));
    }

    #[test]
    fn missing_plugins_field_is_a_clear_error() {
        let result = Catalog::from_str(r#"{"name": "empty-catalog"}"#);

        let err = result.unwrap_err();
        assert!(err.to_string().contains("no \"plugins\" field"), "{err}");
    }

    #[test]
    fn explicit_skills_survive_the_marketplace_entry_conversion() {
        let content = r#"{
            "plugins": {
                "demo": {"name": "demo", "source": "./", "skills": ["./skills/box"]}
            }
        }"#;

        let catalog = Catalog::from_str(content).unwrap();

        assert_eq!(
            catalog.plugins["demo"].explicit_skills,
            vec!["./skills/box"]
        );
    }
}
