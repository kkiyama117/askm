//! Parsing for `plugin.json`, the Agent Plugins 1.0 plugin manifest.
//!
//! The 1.0 schema is closed: only a fixed set of top-level fields is permitted.
//! Legacy plugins (no `$schema`, e.g. superpowers 6.2.0) must still load, so
//! conformance is a spectrum rather than pass/fail — see [`Conformance`].

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::model::{Author, Conformance, PluginManifest, Warning};

const SCHEMA_1_0: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
const MAX_NAME_LEN: usize = 64;
const KNOWN_TOP_LEVEL_FIELDS: &[&str] = &[
    "$schema",
    "name",
    "version",
    "description",
    "author",
    "homepage",
    "repository",
    "license",
    "keywords",
    "extensions",
];

#[derive(Debug, Deserialize)]
struct RawPluginManifest {
    #[serde(rename = "$schema")]
    schema: Option<String>,
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<Author>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    extensions: serde_json::Map<String, Value>,
}

/// Locate a plugin's manifest, preferring the flat `<root>/plugin.json` layout
/// and falling back to the legacy `<root>/.claude-plugin/plugin.json` location.
pub fn locate(plugin_root: &Path) -> Option<PathBuf> {
    let flat = plugin_root.join("plugin.json");
    if flat.is_file() {
        return Some(flat);
    }
    let nested = plugin_root.join(".claude-plugin").join("plugin.json");
    nested.is_file().then_some(nested)
}

pub fn load(path: &Path) -> Result<(PluginManifest, Vec<Warning>)> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    parse(&content)
}

pub fn parse(content: &str) -> Result<(PluginManifest, Vec<Warning>)> {
    let value: Value = serde_json::from_str(content).context("plugin.json is not valid JSON")?;

    let mut warnings = Vec::new();
    warn_unknown_top_level_fields(&value, &mut warnings);

    let raw: RawPluginManifest =
        serde_json::from_value(value).context("plugin.json does not match the expected shape")?;

    let conformance = resolve_conformance(raw.schema.as_deref())?;

    let name = raw
        .name
        .filter(|n| !n.trim().is_empty())
        .context("plugin.json is missing a non-empty \"name\"")?;
    enforce_name_rules(&name, conformance, &mut warnings)?;

    let manifest = PluginManifest {
        schema: raw.schema,
        name,
        version: raw.version,
        description: raw.description,
        author: raw.author,
        homepage: raw.homepage,
        repository: raw.repository,
        license: raw.license,
        keywords: raw.keywords,
        extensions: raw.extensions,
        conformance,
    };

    Ok((manifest, warnings))
}

fn warn_unknown_top_level_fields(value: &Value, warnings: &mut Vec<Warning>) {
    let Some(map) = value.as_object() else { return };
    for key in map.keys() {
        if !KNOWN_TOP_LEVEL_FIELDS.contains(&key.as_str()) {
            warnings.push(format!(
                "plugin.json has an unknown top-level field \"{key}\"; ignoring it"
            ));
        }
    }
}

fn resolve_conformance(schema: Option<&str>) -> Result<Conformance> {
    match schema {
        None => Ok(Conformance::Legacy),
        Some(s) if s == SCHEMA_1_0 => Ok(Conformance::AgentPlugins1_0),
        Some(other) => bail!("plugin.json declares an unrecognized $schema: \"{other}\""),
    }
}

fn enforce_name_rules(
    name: &str,
    conformance: Conformance,
    warnings: &mut Vec<Warning>,
) -> Result<()> {
    let Some(violation) = plugin_name_violation(name) else {
        return Ok(());
    };
    match conformance {
        Conformance::AgentPlugins1_0 => bail!(violation),
        Conformance::Legacy => {
            warnings.push(violation);
            Ok(())
        }
    }
}

/// Agent Plugins 1.0 name rules: 1-64 chars, `a-z0-9-.` only, first/last char
/// alphanumeric, no `--` or `..`. Returns a description of the violation, if any.
fn plugin_name_violation(name: &str) -> Option<String> {
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
        return Some(format!(
            "plugin name \"{name}\" must be 1-{MAX_NAME_LEN} characters"
        ));
    }

    let valid_charset = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.');
    if !valid_charset {
        return Some(format!(
            "plugin name \"{name}\" must contain only lowercase letters, digits, hyphens, and dots"
        ));
    }

    let first_last_alphanumeric = name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name.ends_with(|c: char| c.is_ascii_alphanumeric());
    if !first_last_alphanumeric {
        return Some(format!(
            "plugin name \"{name}\" must start and end with an alphanumeric character"
        ));
    }

    if name.contains("--") || name.contains("..") {
        return Some(format!(
            "plugin name \"{name}\" must not contain \"--\" or \"..\""
        ));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_manifest_with_no_schema_loads() {
        let content = r#"{
            "name": "superpowers",
            "description": "Core skills library for Claude Code",
            "version": "6.2.0",
            "author": {"name": "Jesse Vincent", "email": "jesse@fsck.com"},
            "homepage": "https://github.com/obra/superpowers",
            "repository": "https://github.com/obra/superpowers",
            "license": "MIT",
            "keywords": ["skills", "tdd"]
        }"#;

        let (manifest, warnings) = parse(content).unwrap();

        assert_eq!(manifest.name, "superpowers");
        assert_eq!(manifest.conformance, Conformance::Legacy);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    #[test]
    fn recognized_1_0_schema_is_agent_plugins_1_0() {
        let content = format!(r#"{{"$schema": "{SCHEMA_1_0}", "name": "my-plugin"}}"#);

        let (manifest, warnings) = parse(&content).unwrap();

        assert_eq!(manifest.conformance, Conformance::AgentPlugins1_0);
        assert!(warnings.is_empty());
    }

    #[test]
    fn unrecognized_schema_is_a_hard_error() {
        let content =
            r#"{"$schema": "https://example.com/other.schema.json", "name": "my-plugin"}"#;

        let result = parse(content);

        let err = result.expect_err("unrecognized $schema must error");
        assert!(err.to_string().contains("unrecognized $schema"), "{err}");
    }

    #[test]
    fn unknown_top_level_field_warns_and_still_loads() {
        let content = r#"{"name": "my-plugin", "totallyMadeUpField": 42}"#;

        let (manifest, warnings) = parse(content).unwrap();

        assert_eq!(manifest.name, "my-plugin");
        assert!(warnings.iter().any(|w| w.contains("totallyMadeUpField")));
    }

    #[test]
    fn missing_name_is_a_hard_error() {
        let content = r#"{"description": "no name here"}"#;

        assert!(parse(content).is_err());
    }

    #[test]
    fn invalid_name_errors_under_1_0_but_warns_under_legacy() {
        let legacy = r#"{"name": "Not_Valid!"}"#;
        let (manifest, warnings) = parse(legacy).unwrap();
        assert_eq!(manifest.name, "Not_Valid!");
        assert!(!warnings.is_empty());

        let strict = format!(r#"{{"$schema": "{SCHEMA_1_0}", "name": "Not_Valid!"}}"#);
        assert!(parse(&strict).is_err());
    }

    #[test]
    fn name_with_double_hyphen_violates_1_0_rules() {
        let content = format!(r#"{{"$schema": "{SCHEMA_1_0}", "name": "my--plugin"}}"#);

        assert!(parse(&content).is_err());
    }

    #[test]
    fn extensions_object_is_preserved() {
        let content = r#"{"name": "my-plugin", "extensions": {"vendor.custom": {"flag": true}}}"#;

        let (manifest, _warnings) = parse(content).unwrap();

        assert!(manifest.extensions.contains_key("vendor.custom"));
    }

    #[test]
    fn locate_prefers_flat_layout_over_nested() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("plugin.json"), "{}").unwrap();
        std::fs::create_dir_all(temp.path().join(".claude-plugin")).unwrap();
        std::fs::write(temp.path().join(".claude-plugin").join("plugin.json"), "{}").unwrap();

        let found = locate(temp.path()).unwrap();

        assert_eq!(found, temp.path().join("plugin.json"));
    }

    #[test]
    fn locate_falls_back_to_nested_layout() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(".claude-plugin")).unwrap();
        std::fs::write(temp.path().join(".claude-plugin").join("plugin.json"), "{}").unwrap();

        let found = locate(temp.path()).unwrap();

        assert_eq!(
            found,
            temp.path().join(".claude-plugin").join("plugin.json")
        );
    }

    #[test]
    fn locate_returns_none_when_absent() {
        let temp = tempfile::tempdir().unwrap();

        assert!(locate(temp.path()).is_none());
    }
}
