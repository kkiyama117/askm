//! Parsing for `SKILL.md`: YAML frontmatter between `---` lines, then a markdown
//! body.
//!
//! The official client-implementation guide mandates lenient parsing: everything
//! except a missing/empty `name`, a missing/empty `description`, or unparseable
//! frontmatter should warn and still load.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::model::{SkillManifest, Warning};

const MAX_NAME_LEN: usize = 64;
const MAX_DESCRIPTION_LEN: usize = 1024;
const MAX_COMPATIBILITY_LEN: usize = 500;

#[derive(Debug, Deserialize)]
struct RawFrontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    compatibility: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<String>,
}

/// Read and parse `<dir>/SKILL.md`, using `dir`'s own name as the expected skill
/// name for the mismatch check.
pub fn load_from_dir(dir: &Path) -> Result<(SkillManifest, Vec<Warning>)> {
    let path = dir.join("SKILL.md");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let expected_name = dir.file_name().and_then(|n| n.to_str());
    parse(&content, expected_name)
}

/// Parse SKILL.md content. `expected_name` is the skill's parent directory name,
/// used only to produce a mismatch warning — it never causes a hard error.
pub fn parse(content: &str, expected_name: Option<&str>) -> Result<(SkillManifest, Vec<Warning>)> {
    let (frontmatter_src, _body) = split_frontmatter(content)?;

    let raw: RawFrontmatter = serde_yaml_ng::from_str(&frontmatter_src)
        .context("failed to parse SKILL.md frontmatter as YAML")?;

    let name = raw
        .name
        .filter(|n| !n.trim().is_empty())
        .context("SKILL.md frontmatter is missing a non-empty \"name\"")?;
    let description = raw
        .description
        .filter(|d| !d.trim().is_empty())
        .context("SKILL.md frontmatter is missing a non-empty \"description\"")?;

    let mut warnings = Vec::new();
    validate_name(&name, expected_name, &mut warnings);
    validate_description(&description, &mut warnings);
    if let Some(compatibility) = &raw.compatibility {
        validate_compatibility(compatibility, &mut warnings);
    }

    let allowed_tools = raw
        .allowed_tools
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();

    let manifest = SkillManifest {
        name,
        description,
        license: raw.license,
        compatibility: raw.compatibility,
        metadata: raw.metadata,
        allowed_tools,
    };

    Ok((manifest, warnings))
}

/// Split SKILL.md content into (frontmatter, body). The file must start with a
/// `---` line and contain a second `---` line that closes the block.
fn split_frontmatter(content: &str) -> Result<(String, String)> {
    let mut lines = content.lines();

    let Some(first) = lines.next() else {
        bail!("SKILL.md is empty; expected YAML frontmatter delimited by \"---\"");
    };
    if first.trim_end() != "---" {
        bail!("SKILL.md is missing YAML frontmatter (must start with a \"---\" line)");
    }

    let mut frontmatter_lines = Vec::new();
    let mut body_lines = Vec::new();
    let mut closed = false;
    for line in lines {
        if !closed && line.trim_end() == "---" {
            closed = true;
            continue;
        }
        if closed {
            body_lines.push(line);
        } else {
            frontmatter_lines.push(line);
        }
    }

    if !closed {
        bail!("SKILL.md frontmatter is not closed with a second \"---\" line");
    }

    Ok((frontmatter_lines.join("\n"), body_lines.join("\n")))
}

fn validate_name(name: &str, expected_name: Option<&str>, warnings: &mut Vec<Warning>) {
    if name.len() > MAX_NAME_LEN {
        warnings.push(format!(
            "skill name \"{name}\" is {} characters, over the {MAX_NAME_LEN}-character limit",
            name.len()
        ));
    }

    let valid_charset = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let no_edge_hyphen = !name.starts_with('-') && !name.ends_with('-');
    if !valid_charset || !no_edge_hyphen {
        warnings.push(format!(
            "skill name \"{name}\" should use only lowercase letters, digits, and hyphens, \
             with no leading or trailing hyphen"
        ));
    }

    if let Some(expected) = expected_name {
        if expected != name {
            warnings.push(format!(
                "skill name \"{name}\" does not match its directory name \"{expected}\""
            ));
        }
    }
}

fn validate_description(description: &str, warnings: &mut Vec<Warning>) {
    if description.len() > MAX_DESCRIPTION_LEN {
        warnings.push(format!(
            "skill description is {} characters, over the {MAX_DESCRIPTION_LEN}-character limit",
            description.len()
        ));
    }
}

fn validate_compatibility(compatibility: &str, warnings: &mut Vec<Warning>) {
    if compatibility.len() > MAX_COMPATIBILITY_LEN {
        warnings.push(format!(
            "skill compatibility string is {} characters, over the {MAX_COMPATIBILITY_LEN}-character limit",
            compatibility.len()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_skill_md_parses_with_no_warnings() {
        let content = "---\nname: brainstorming\ndescription: Explore ideas before implementation.\n---\n\n# Brainstorming\n";

        let (manifest, warnings) = parse(content, Some("brainstorming")).unwrap();

        assert_eq!(manifest.name, "brainstorming");
        assert_eq!(manifest.description, "Explore ideas before implementation.");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    #[test]
    fn real_brainstorming_frontmatter_parses() {
        let content = concat!(
            "---\n",
            "name: brainstorming\n",
            "description: \"You MUST use this before any creative work - creating features, ",
            "building components, adding functionality, or modifying behavior. Explores user ",
            "intent, requirements and design before implementation.\"\n",
            "---\n\n# Brainstorming Ideas Into Designs\n",
        );

        let (manifest, warnings) = parse(content, Some("brainstorming")).unwrap();

        assert_eq!(manifest.name, "brainstorming");
        assert!(warnings.is_empty());
    }

    #[test]
    fn name_dir_mismatch_warns_but_still_loads() {
        let content = "---\nname: my-skill\ndescription: Does a thing.\n---\nBody\n";

        let (manifest, warnings) = parse(content, Some("other-dir")).unwrap();

        assert_eq!(manifest.name, "my-skill");
        assert!(warnings
            .iter()
            .any(|w| w.contains("does not match its directory name")));
    }

    #[test]
    fn missing_description_is_a_hard_error() {
        let content = "---\nname: my-skill\n---\nBody\n";

        let result = parse(content, None);

        assert!(result.is_err());
    }

    #[test]
    fn empty_name_is_a_hard_error() {
        let content = "---\nname: \"\"\ndescription: Does a thing.\n---\nBody\n";

        let result = parse(content, None);

        assert!(result.is_err());
    }

    #[test]
    fn missing_frontmatter_is_a_hard_error() {
        let content = "# Just markdown, no frontmatter at all\n";

        let result = parse(content, None);

        assert!(result.is_err());
    }

    #[test]
    fn unclosed_frontmatter_is_a_hard_error() {
        let content =
            "---\nname: my-skill\ndescription: Does a thing.\nBody without closing delimiter\n";

        let result = parse(content, None);

        assert!(result.is_err());
    }

    #[test]
    fn oversized_name_warns_but_still_loads() {
        let long_name = "a".repeat(65);
        let content = format!("---\nname: {long_name}\ndescription: Does a thing.\n---\nBody\n");

        let (manifest, warnings) = parse(&content, None).unwrap();

        assert_eq!(manifest.name, long_name);
        assert!(warnings
            .iter()
            .any(|w| w.contains("over the 64-character limit")));
    }

    #[test]
    fn uppercase_name_warns_but_still_loads() {
        let content = "---\nname: MySkill\ndescription: Does a thing.\n---\nBody\n";

        let (manifest, warnings) = parse(content, None).unwrap();

        assert_eq!(manifest.name, "MySkill");
        assert!(warnings.iter().any(|w| w.contains("lowercase letters")));
    }

    #[test]
    fn allowed_tools_splits_on_whitespace() {
        let content =
            "---\nname: my-skill\ndescription: Does a thing.\nallowed-tools: \"Read Edit  Bash\"\n---\nBody\n";

        let (manifest, _warnings) = parse(content, None).unwrap();

        assert_eq!(manifest.allowed_tools, vec!["Read", "Edit", "Bash"]);
    }

    #[test]
    fn absent_allowed_tools_is_empty_vec() {
        let content = "---\nname: my-skill\ndescription: Does a thing.\n---\nBody\n";

        let (manifest, _warnings) = parse(content, None).unwrap();

        assert!(manifest.allowed_tools.is_empty());
    }

    #[test]
    fn metadata_map_parses() {
        let content =
            "---\nname: my-skill\ndescription: Does a thing.\nmetadata:\n  category: writing\n  level: beginner\n---\nBody\n";

        let (manifest, _warnings) = parse(content, None).unwrap();

        assert_eq!(
            manifest.metadata.get("category"),
            Some(&"writing".to_string())
        );
        assert_eq!(
            manifest.metadata.get("level"),
            Some(&"beginner".to_string())
        );
    }

    #[test]
    fn load_from_dir_reads_skill_md_and_uses_dir_name() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: Does a thing.\n---\nBody\n",
        )
        .unwrap();

        let (manifest, warnings) = load_from_dir(&skill_dir).unwrap();

        assert_eq!(manifest.name, "my-skill");
        assert!(warnings.is_empty());
    }
}
