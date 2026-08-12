//! Parsing and normalization for `marketplace.json`.
//!
//! Two dialects exist in the wild:
//! - `.agents/plugins/marketplace.json` (checked first)
//! - `.claude-plugin/marketplace.json` (fallback, and cross-checked when both
//!   files exist)
//!
//! Both normalize into the same [`Marketplace`] type. Fields unknown to a given
//! dialect's raw struct are dropped silently by serde's default behavior — never
//! a hard error.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::model::{Author, Dialect, EntrySource, Marketplace, MarketplaceEntry, Warning};

const AGENTS_RELATIVE: [&str; 3] = [".agents", "plugins", "marketplace.json"];
const CLAUDE_PLUGIN_RELATIVE: [&str; 2] = [".claude-plugin", "marketplace.json"];

/// Load a marketplace from a plugin repository checkout, preferring
/// `.agents/plugins/marketplace.json` when both dialect files exist.
pub fn load_from_repo(repo_root: &Path) -> Result<(Marketplace, Vec<Warning>)> {
    let agents_path: std::path::PathBuf = AGENTS_RELATIVE.iter().collect();
    let agents_path = repo_root.join(agents_path);
    let claude_path: std::path::PathBuf = CLAUDE_PLUGIN_RELATIVE.iter().collect();
    let claude_path = repo_root.join(claude_path);

    if agents_path.is_file() {
        let (marketplace, mut warnings) = load_agents_file(&agents_path)?;
        if claude_path.is_file() {
            let reconciled = reconcile_with_secondary(marketplace, &claude_path, &mut warnings);
            return Ok((reconciled, warnings));
        }
        return Ok((marketplace, warnings));
    }

    if claude_path.is_file() {
        return load_claude_plugin_file(&claude_path);
    }

    bail!(
        "no marketplace.json found under {} (checked .agents/plugins and .claude-plugin)",
        repo_root.display()
    );
}

/// Reconcile the primary (`.agents`) load with the secondary (`.claude-plugin`)
/// file: warn when their plugin-name sets disagree, and backfill descriptive
/// metadata the primary omits.
///
/// The two dialects are not redundant in practice — real repositories carry a
/// terse `.agents` listing next to a `.claude-plugin` one with the actual
/// descriptions and keywords. Identity and `source` still come from `.agents`;
/// only fields it leaves empty are filled, so search has something to match on.
/// If the secondary fails to parse, the primary load is returned untouched.
fn reconcile_with_secondary(
    primary: Marketplace,
    claude_path: &Path,
    warnings: &mut Vec<Warning>,
) -> Marketplace {
    let Ok((secondary, _)) = load_claude_plugin_file(claude_path) else {
        return primary;
    };

    let primary_names: BTreeSet<&str> = primary.entries.iter().map(|e| e.name.as_str()).collect();
    let secondary_names: BTreeSet<&str> =
        secondary.entries.iter().map(|e| e.name.as_str()).collect();

    if primary_names != secondary_names {
        warnings.push(format!(
            "plugin names differ between .agents/plugins/marketplace.json {primary_names:?} \
             and .claude-plugin/marketplace.json {secondary_names:?}; using the .agents listing"
        ));
    }

    backfill(primary, secondary)
}

/// Fill each empty descriptive field of `primary` from `secondary`.
fn backfill(primary: Marketplace, secondary: Marketplace) -> Marketplace {
    let entries = primary
        .entries
        .into_iter()
        .map(
            |entry| match secondary.entries.iter().find(|e| e.name == entry.name) {
                Some(other) => backfill_entry(entry, other),
                None => entry,
            },
        )
        .collect();

    Marketplace {
        description: primary.description.or(secondary.description),
        entries,
        ..primary
    }
}

fn backfill_entry(entry: MarketplaceEntry, other: &MarketplaceEntry) -> MarketplaceEntry {
    MarketplaceEntry {
        version: entry.version.or_else(|| other.version.clone()),
        description: entry.description.or_else(|| other.description.clone()),
        category: entry.category.or_else(|| other.category.clone()),
        homepage: entry.homepage.or_else(|| other.homepage.clone()),
        author: entry.author.or_else(|| other.author.clone()),
        keywords: or_empty(entry.keywords, &other.keywords),
        explicit_skills: or_empty(entry.explicit_skills, &other.explicit_skills),
        ..entry
    }
}

fn or_empty(primary: Vec<String>, fallback: &[String]) -> Vec<String> {
    if primary.is_empty() {
        return fallback.to_vec();
    }
    primary
}

fn load_agents_file(path: &Path) -> Result<(Marketplace, Vec<Warning>)> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    parse_agents(&content)
}

fn load_claude_plugin_file(path: &Path) -> Result<(Marketplace, Vec<Warning>)> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    parse_claude_plugin(&content)
}

pub fn parse_agents(content: &str) -> Result<(Marketplace, Vec<Warning>)> {
    let file: AgentsMarketplaceFile = serde_json::from_str(content)
        .context("failed to parse .agents/plugins/marketplace.json")?;
    Ok((file.into(), Vec::new()))
}

pub fn parse_claude_plugin(content: &str) -> Result<(Marketplace, Vec<Warning>)> {
    let file: ClaudePluginMarketplaceFile =
        serde_json::from_str(content).context("failed to parse .claude-plugin/marketplace.json")?;
    Ok((file.into(), Vec::new()))
}

fn merge_keywords(keywords: Vec<String>, tags: Vec<String>) -> Vec<String> {
    if keywords.is_empty() {
        tags
    } else {
        keywords
    }
}

// --- .agents/plugins/marketplace.json ---------------------------------------

#[derive(Debug, Deserialize)]
struct AgentsMarketplaceFile {
    name: String,
    #[serde(default)]
    interface: Option<AgentsInterface>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    plugins: Vec<AgentsEntry>,
}

#[derive(Debug, Deserialize)]
struct AgentsInterface {
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgentsEntry {
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
    tags: Vec<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    author: Option<Author>,
    #[serde(default)]
    skills: Vec<String>,
}

impl From<AgentsEntry> for MarketplaceEntry {
    fn from(e: AgentsEntry) -> Self {
        MarketplaceEntry {
            name: e.name,
            version: e.version,
            description: e.description,
            source: e.source,
            category: e.category,
            keywords: merge_keywords(e.keywords, e.tags),
            homepage: e.homepage,
            author: e.author,
            explicit_skills: e.skills,
        }
    }
}

impl From<AgentsMarketplaceFile> for Marketplace {
    fn from(file: AgentsMarketplaceFile) -> Self {
        Marketplace {
            name: file.name,
            display_name: file.interface.and_then(|i| i.display_name),
            description: file.description,
            entries: file
                .plugins
                .into_iter()
                .map(MarketplaceEntry::from)
                .collect(),
            dialect: Dialect::Agents,
        }
    }
}

// --- .claude-plugin/marketplace.json -----------------------------------------

#[derive(Debug, Deserialize)]
struct ClaudePluginMarketplaceFile {
    name: String,
    #[serde(default)]
    metadata: Option<ClaudePluginMetadata>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    plugins: Vec<ClaudePluginEntry>,
}

#[derive(Debug, Deserialize)]
struct ClaudePluginMetadata {
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudePluginEntry {
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
    tags: Vec<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    author: Option<Author>,
    #[serde(default)]
    skills: Vec<String>,
}

impl From<ClaudePluginEntry> for MarketplaceEntry {
    fn from(e: ClaudePluginEntry) -> Self {
        MarketplaceEntry {
            name: e.name,
            version: e.version,
            description: e.description,
            source: e.source,
            category: e.category,
            keywords: merge_keywords(e.keywords, e.tags),
            homepage: e.homepage,
            author: e.author,
            explicit_skills: e.skills,
        }
    }
}

impl From<ClaudePluginMarketplaceFile> for Marketplace {
    fn from(file: ClaudePluginMarketplaceFile) -> Self {
        let description = file
            .description
            .or_else(|| file.metadata.and_then(|m| m.description));
        Marketplace {
            name: file.name,
            display_name: None,
            description,
            entries: file
                .plugins
                .into_iter()
                .map(MarketplaceEntry::from)
                .collect(),
            dialect: Dialect::ClaudePlugin,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_CLAUDE_PLUGIN_JSON: &str = r#"{
        "name": "ecc",
        "owner": {"name": "Affaan Mustafa", "email": "me@affaanmustafa.com"},
        "metadata": {"description": "Harness-native ECC skills, hooks, rules"},
        "plugins": [
            {
                "name": "ecc",
                "source": "./",
                "description": "Harness-native ECC operator layer",
                "version": "2.2.0",
                "author": {"name": "Affaan Mustafa", "email": "me@affaanmustafa.com"},
                "homepage": "https://ecc.tools",
                "repository": "https://github.com/affaan-m/ECC",
                "license": "MIT",
                "keywords": ["agents", "skills"],
                "category": "workflow",
                "tags": ["agents", "skills"],
                "strict": false
            }
        ]
    }"#;

    const REAL_AGENTS_JSON: &str = r#"{
        "name": "ecc",
        "interface": {"displayName": "ECC"},
        "plugins": [
            {
                "name": "ecc",
                "version": "2.2.0",
                "source": {"source": "local", "path": "./"},
                "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"},
                "category": "Productivity"
            }
        ]
    }"#;

    #[test]
    fn real_claude_plugin_marketplace_parses() {
        let (marketplace, warnings) = parse_claude_plugin(REAL_CLAUDE_PLUGIN_JSON).unwrap();

        assert_eq!(marketplace.name, "ecc");
        assert_eq!(marketplace.dialect, Dialect::ClaudePlugin);
        assert_eq!(
            marketplace.description.as_deref(),
            Some("Harness-native ECC skills, hooks, rules")
        );
        assert_eq!(marketplace.entries.len(), 1);
        assert_eq!(marketplace.entries[0].name, "ecc");
        assert_eq!(
            marketplace.entries[0].source,
            EntrySource::Local {
                path: "./".to_string()
            }
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn real_agents_marketplace_parses() {
        let (marketplace, warnings) = parse_agents(REAL_AGENTS_JSON).unwrap();

        assert_eq!(marketplace.name, "ecc");
        assert_eq!(marketplace.dialect, Dialect::Agents);
        assert_eq!(marketplace.display_name.as_deref(), Some("ECC"));
        assert_eq!(marketplace.entries.len(), 1);
        assert_eq!(marketplace.entries[0].name, "ecc");
        assert_eq!(
            marketplace.entries[0].source,
            EntrySource::Local {
                path: "./".to_string()
            }
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn agents_dialect_borrows_descriptions_the_claude_plugin_file_has() {
        // The real ecc repo ships exactly this pair: a terse `.agents` listing
        // and a `.claude-plugin` one carrying the description and keywords.
        // Losing them would leave `askm search` nothing to match on.
        let temp = tempfile::tempdir().unwrap();
        write_agents_file(temp.path(), REAL_AGENTS_JSON);
        write_claude_plugin_file(temp.path(), REAL_CLAUDE_PLUGIN_JSON);

        let (marketplace, warnings) = load_from_repo(temp.path()).unwrap();

        assert_eq!(marketplace.dialect, Dialect::Agents);
        assert!(warnings.is_empty(), "names agree: {warnings:?}");
        assert!(
            marketplace.description.is_some(),
            "marketplace description should be borrowed from the secondary file"
        );
        assert!(
            marketplace.entries[0].description.is_some(),
            "entry description should be borrowed from the secondary file"
        );
        // Identity and source still come from the primary dialect.
        assert_eq!(
            marketplace.entries[0].source,
            EntrySource::Local {
                path: "./".to_string()
            }
        );
        assert_eq!(
            marketplace.entries[0].category.as_deref(),
            Some("Productivity")
        );
    }

    #[test]
    fn equivalent_content_normalizes_to_equivalent_marketplace() {
        let agents_json = r#"{
            "name": "demo",
            "description": "A demo marketplace",
            "plugins": [
                {
                    "name": "demo-plugin",
                    "version": "1.0.0",
                    "source": {"source": "local", "path": "./"},
                    "category": "tools",
                    "keywords": ["one", "two"]
                }
            ]
        }"#;
        let claude_plugin_json = r#"{
            "name": "demo",
            "metadata": {"description": "A demo marketplace"},
            "plugins": [
                {
                    "name": "demo-plugin",
                    "version": "1.0.0",
                    "source": "./",
                    "category": "tools",
                    "keywords": ["one", "two"]
                }
            ]
        }"#;

        let (from_agents, _) = parse_agents(agents_json).unwrap();
        let (from_claude_plugin, _) = parse_claude_plugin(claude_plugin_json).unwrap();

        assert_eq!(from_agents.name, from_claude_plugin.name);
        assert_eq!(from_agents.description, from_claude_plugin.description);
        assert_eq!(from_agents.entries, from_claude_plugin.entries);
        assert_ne!(from_agents.dialect, from_claude_plugin.dialect);
    }

    #[test]
    fn load_from_repo_prefers_agents_dialect_when_both_present() {
        let temp = tempfile::tempdir().unwrap();
        write_agents_file(temp.path(), REAL_AGENTS_JSON);
        write_claude_plugin_file(temp.path(), REAL_CLAUDE_PLUGIN_JSON);

        let (marketplace, warnings) = load_from_repo(temp.path()).unwrap();

        assert_eq!(marketplace.dialect, Dialect::Agents);
        assert!(
            warnings.is_empty(),
            "plugin names agree, expected no warnings: {warnings:?}"
        );
    }

    #[test]
    fn load_from_repo_warns_when_dialects_disagree_on_plugin_names() {
        let temp = tempfile::tempdir().unwrap();
        let agents_json = r#"{
            "name": "demo",
            "plugins": [{"name": "only-in-agents", "source": "./"}]
        }"#;
        let claude_plugin_json = r#"{
            "name": "demo",
            "plugins": [{"name": "only-in-claude-plugin", "source": "./"}]
        }"#;
        write_agents_file(temp.path(), agents_json);
        write_claude_plugin_file(temp.path(), claude_plugin_json);

        let (marketplace, warnings) = load_from_repo(temp.path()).unwrap();

        assert_eq!(marketplace.dialect, Dialect::Agents);
        assert_eq!(marketplace.entries[0].name, "only-in-agents");
        assert!(warnings.iter().any(|w| w.contains("plugin names differ")));
    }

    #[test]
    fn load_from_repo_falls_back_to_claude_plugin_dialect() {
        let temp = tempfile::tempdir().unwrap();
        write_claude_plugin_file(temp.path(), REAL_CLAUDE_PLUGIN_JSON);

        let (marketplace, _warnings) = load_from_repo(temp.path()).unwrap();

        assert_eq!(marketplace.dialect, Dialect::ClaudePlugin);
    }

    #[test]
    fn load_from_repo_errors_when_neither_file_exists() {
        let temp = tempfile::tempdir().unwrap();

        assert!(load_from_repo(temp.path()).is_err());
    }

    #[test]
    fn explicit_skills_array_is_captured() {
        let json = r#"{
            "name": "demo",
            "plugins": [{"name": "p", "source": "./", "skills": ["./skills/box", "./skills/other"]}]
        }"#;

        let (marketplace, _) = parse_claude_plugin(json).unwrap();

        assert_eq!(
            marketplace.entries[0].explicit_skills,
            vec!["./skills/box", "./skills/other"]
        );
    }

    #[test]
    fn unknown_fields_in_either_dialect_are_ignored_not_errors() {
        let agents_json = r#"{
            "name": "demo",
            "someBrandNewField": {"nested": true},
            "plugins": [{"name": "p", "source": "./", "someOtherField": 1}]
        }"#;

        let result = parse_agents(agents_json);

        assert!(result.is_ok());
    }

    fn write_agents_file(root: &Path, content: &str) {
        let dir = root.join(".agents").join("plugins");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("marketplace.json"), content).unwrap();
    }

    fn write_claude_plugin_file(root: &Path, content: &str) {
        let dir = root.join(".claude-plugin");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("marketplace.json"), content).unwrap();
    }
}
