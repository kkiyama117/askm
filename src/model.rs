//! Normalized domain model produced by the `manifest` parsers.
//!
//! Marketplace, plugin, and skill manifests all ship in more than one real-world
//! shape. Parsing (in `crate::manifest`) collapses those shapes into the types
//! defined here; this module owns only the normalized data, plus the one piece of
//! parsing that is genuinely part of the domain rather than any single file format:
//! [`EntrySource`]'s five-shapes-into-one `Deserialize` impl.

use std::collections::BTreeMap;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A non-fatal issue surfaced while parsing. Several parts of the spec require
/// "warn, don't fail" behavior; callers decide how to surface these.
pub type Warning = String;

/// Where a marketplace entry's plugin content comes from.
///
/// Real marketplace data expresses this as either a bare string (a local path) or
/// an object tagged by a `source` kind (`local`, `url`, `git-subdir`). `url` and
/// `git-subdir` are treated identically here: both describe a git checkout with an
/// optional subpath, ref, and pinned sha — the two tag names appear to be dialect
/// artifacts rather than a meaningful distinction.
/// Serialization emits the tagged object form the deserializer accepts, so a value
/// written to `state.json` reads back identically. `url` is used for every git
/// source: the deserializer treats `url` and `git-subdir` alike, and real data
/// already carries a `path` alongside `url`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntrySource {
    Local {
        path: String,
    },
    Git {
        url: String,
        reference: Option<String>,
        sha: Option<String>,
        subpath: Option<String>,
    },
}

impl Serialize for EntrySource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            EntrySource::Local { path } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("source", "local")?;
                map.serialize_entry("path", path)?;
                map.end()
            }
            EntrySource::Git {
                url,
                reference,
                sha,
                subpath,
            } => {
                let optional = [("ref", reference), ("sha", sha), ("path", subpath)];
                let present = optional.iter().filter(|(_, v)| v.is_some()).count();
                let mut map = serializer.serialize_map(Some(2 + present))?;
                map.serialize_entry("source", "url")?;
                map.serialize_entry("url", url)?;
                for (key, value) in optional {
                    if let Some(value) = value {
                        map.serialize_entry(key, value)?;
                    }
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for EntrySource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(EntrySourceVisitor)
    }
}

struct EntrySourceVisitor;

impl<'de> Visitor<'de> for EntrySourceVisitor {
    type Value = EntrySource;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a plugin source string or a source object")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(EntrySource::Local {
            path: v.to_string(),
        })
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(EntrySource::Local { path: v })
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut fields: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        while let Some(key) = map.next_key::<String>()? {
            let value: serde_json::Value = map.next_value()?;
            fields.insert(key, value);
        }
        entry_source_from_fields(&fields).map_err(de::Error::custom)
    }
}

fn field_str(fields: &BTreeMap<String, serde_json::Value>, key: &str) -> Option<String> {
    fields.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

fn entry_source_from_fields(
    fields: &BTreeMap<String, serde_json::Value>,
) -> Result<EntrySource, String> {
    let kind = field_str(fields, "source")
        .ok_or_else(|| "plugin source object is missing a \"source\" field".to_string())?;

    match kind.as_str() {
        "local" => {
            let path = field_str(fields, "path")
                .ok_or_else(|| "local plugin source is missing a \"path\" field".to_string())?;
            Ok(EntrySource::Local { path })
        }
        "url" | "git-subdir" => {
            let url = field_str(fields, "url")
                .ok_or_else(|| format!("{kind} plugin source is missing a \"url\" field"))?;
            Ok(EntrySource::Git {
                url,
                reference: field_str(fields, "ref"),
                sha: field_str(fields, "sha"),
                subpath: field_str(fields, "path"),
            })
        }
        other => Err(format!("unknown plugin source kind: \"{other}\"")),
    }
}

/// Author metadata shared by `plugin.json` and `marketplace.json` entries.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Author {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

/// Which marketplace.json dialect a `Marketplace` was normalized from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dialect {
    /// `.agents/plugins/marketplace.json`.
    Agents,
    /// `.claude-plugin/marketplace.json`.
    ClaudePlugin,
}

/// Which `plugin.json` schema generation a manifest declared conformance to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Conformance {
    /// Declared `$schema` matches the recognized Agent Plugins 1.0 schema.
    AgentPlugins1_0,
    /// No `$schema` declared. Real plugins (e.g. superpowers 6.2.0) ship this way
    /// and must still load.
    Legacy,
}

/// One plugin listed in a marketplace, normalized across dialects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MarketplaceEntry {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub source: EntrySource,
    pub category: Option<String>,
    pub keywords: Vec<String>,
    pub homepage: Option<String>,
    pub author: Option<Author>,
    /// Explicit `"skills": [...]` paths some entries carry, e.g. `./skills/box`.
    pub explicit_skills: Vec<String>,
}

/// A marketplace, normalized from either `marketplace.json` dialect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Marketplace {
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub entries: Vec<MarketplaceEntry>,
    pub dialect: Dialect,
}

/// A parsed `plugin.json`, an Agent Plugins 1.0 manifest with a closed schema.
///
/// `PartialEq` only (not `Eq`) because `extensions` holds `serde_json::Value`,
/// which contains floats.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PluginManifest {
    pub schema: Option<String>,
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub author: Option<Author>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub keywords: Vec<String>,
    pub extensions: serde_json::Map<String, serde_json::Value>,
    pub conformance: Conformance,
}

/// A parsed `SKILL.md`: YAML frontmatter plus the markdown body's presence is
/// implied (Phase 1 only needs the frontmatter fields).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub allowed_tools: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_source(json: &str) -> EntrySource {
        serde_json::from_str(json).expect("source should parse")
    }

    #[test]
    fn bare_dot_slash_string_is_local() {
        let source = parse_source(r#""./""#);
        assert_eq!(
            source,
            EntrySource::Local {
                path: "./".to_string()
            }
        );
    }

    #[test]
    fn bare_relative_path_string_is_local() {
        let source = parse_source(r#""./plugins/agent-sdk-dev""#);
        assert_eq!(
            source,
            EntrySource::Local {
                path: "./plugins/agent-sdk-dev".to_string()
            }
        );
    }

    #[test]
    fn local_object_normalizes_like_bare_string() {
        let source = parse_source(r#"{"source":"local","path":"./"}"#);
        assert_eq!(
            source,
            EntrySource::Local {
                path: "./".to_string()
            }
        );
    }

    #[test]
    fn url_source_with_sha_only() {
        let source =
            parse_source(r#"{"source":"url","url":"https://github.com/x/y.git","sha":"abc"}"#);
        assert_eq!(
            source,
            EntrySource::Git {
                url: "https://github.com/x/y.git".to_string(),
                reference: None,
                sha: Some("abc".to_string()),
                subpath: None,
            }
        );
    }

    #[test]
    fn url_source_with_subpath() {
        let source = parse_source(
            r#"{"source":"url","url":"https://github.com/x/y.git","path":"revenuecat","sha":"deadbeef"}"#,
        );
        assert_eq!(
            source,
            EntrySource::Git {
                url: "https://github.com/x/y.git".to_string(),
                reference: None,
                sha: Some("deadbeef".to_string()),
                subpath: Some("revenuecat".to_string()),
            }
        );
    }

    #[test]
    fn git_subdir_source_with_ref_and_sha() {
        let source = parse_source(
            r#"{"source":"git-subdir","url":"https://github.com/x/y.git","path":"plugins/api-security-testing","ref":"v1.5.5","sha":"bc78deadbeef"}"#,
        );
        assert_eq!(
            source,
            EntrySource::Git {
                url: "https://github.com/x/y.git".to_string(),
                reference: Some("v1.5.5".to_string()),
                sha: Some("bc78deadbeef".to_string()),
                subpath: Some("plugins/api-security-testing".to_string()),
            }
        );
    }

    #[test]
    fn unknown_source_kind_errors_cleanly() {
        let result: Result<EntrySource, _> =
            serde_json::from_str(r#"{"source":"ftp","url":"whatever"}"#);
        let err = result.expect_err("unknown source kind must fail, not panic");
        assert!(
            err.to_string().contains("unknown plugin source kind"),
            "{err}"
        );
    }

    #[test]
    fn source_object_missing_source_field_errors_cleanly() {
        let result: Result<EntrySource, _> = serde_json::from_str(r#"{"path":"./"}"#);
        assert!(result.is_err());
    }
}
