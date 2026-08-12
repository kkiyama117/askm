//! Plugin installation into the store.
//!
//! Installing resolves a marketplace entry's `EntrySource` into
//! `Layout::plugin_root`, validates `plugin.json`, discovers skills at the
//! Agent Plugins 1.0 fixed location, and records the result into `State`.
//! Every path the plugin exposes — a resolved `Local` source, a `Git`
//! source's `subpath`, a discovered or explicit skill directory — is checked
//! to resolve inside the plugin root before it is trusted; see
//! [`contained_within`].

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::manifest::plugin as plugin_manifest;
use crate::model::{EntrySource, MarketplaceEntry, PluginManifest};
use crate::paths::Layout;
use crate::source::{remove_path, GitSource, LocalSource, Source, SyncOutcome};
use crate::state::{now_unix, InstalledPlugin, PluginId, State};

/// Version label used when a marketplace entry does not declare a `version`.
/// Reinstalling such an entry lands in the same directory — there is no
/// other stable identifier to key a side-by-side install on.
const UNVERSIONED: &str = "unversioned";

/// Result of installing a plugin.
pub struct InstallOutcome {
    pub state: State,
    pub installed: InstalledPlugin,
    pub manifest: PluginManifest,
    pub sync: SyncOutcome,
    /// Directory the plugin's `skills/` actually hangs off. This is the plugin
    /// root for most sources, but a subdirectory of it for `git-subdir` entries,
    /// so callers linking skills must use this rather than `Layout::plugin_root`.
    pub effective_root: PathBuf,
}

/// Result of updating a plugin. The previous install, if any, is left
/// untouched on disk and returned so a caller can offer a rollback.
pub struct UpdateOutcome {
    pub state: State,
    pub installed: InstalledPlugin,
    pub previous: Option<InstalledPlugin>,
    pub manifest: PluginManifest,
    pub sync: SyncOutcome,
    /// See [`InstallOutcome::effective_root`].
    pub effective_root: PathBuf,
}

/// Install `entry` from `marketplace_name`. `marketplace_repo_root` is the
/// already-synced marketplace checkout; it is only consulted for `Local`
/// sources, whose path is relative to it.
pub fn install(
    layout: &Layout,
    state: &State,
    marketplace_name: &str,
    marketplace_repo_root: &Path,
    entry: &MarketplaceEntry,
) -> Result<InstallOutcome> {
    let version = entry
        .version
        .clone()
        .unwrap_or_else(|| UNVERSIONED.to_string());
    let plugin_root = layout.plugin_root(marketplace_name, &entry.name, &version);

    let sync = sync_source(&entry.source, marketplace_repo_root, &plugin_root)?;
    let effective_root = effective_plugin_root(&plugin_root, &entry.source)?;

    let manifest_path = plugin_manifest::locate(&effective_root)
        .with_context(|| format!("no plugin.json found under {}", effective_root.display()))?;
    let (manifest, _warnings) = plugin_manifest::load(&manifest_path)?;

    let skills = discover_skills(&effective_root, entry)?;

    let plugin_data = layout.plugin_data(marketplace_name, &entry.name);
    fs::create_dir_all(&plugin_data)
        .with_context(|| format!("creating {}", plugin_data.display()))?;

    let installed = InstalledPlugin {
        plugin: entry.name.clone(),
        marketplace: marketplace_name.to_string(),
        version,
        source: entry.source.clone(),
        installed_at: now_unix(),
        skills,
    };
    let next_state = state.record_install(installed.clone());
    next_state.save(&layout.state_file())?;

    Ok(InstallOutcome {
        state: next_state,
        installed,
        manifest,
        sync,
        effective_root,
    })
}

/// Install a (presumably newer) entry alongside whatever is already
/// installed for the same plugin. Nothing on disk for the previous version
/// is touched, so it remains available for rollback.
pub fn update(
    layout: &Layout,
    state: &State,
    marketplace_name: &str,
    marketplace_repo_root: &Path,
    entry: &MarketplaceEntry,
) -> Result<UpdateOutcome> {
    let id = PluginId::new(entry.name.clone(), marketplace_name);
    let previous = state.installed(&id).cloned();

    let InstallOutcome {
        state,
        installed,
        manifest,
        sync,
        effective_root,
    } = install(
        layout,
        state,
        marketplace_name,
        marketplace_repo_root,
        entry,
    )?;

    Ok(UpdateOutcome {
        state,
        installed,
        previous,
        manifest,
        sync,
        effective_root,
    })
}

/// Remove one installed version's directory and forget it in `State`.
/// `plugin_data` survives unless `purge` is set — the spec requires it
/// survive ordinary updates and uninstalls.
pub fn uninstall(layout: &Layout, state: &State, id: &PluginId, purge: bool) -> Result<State> {
    let installed = state
        .installed(id)
        .with_context(|| format!("{id} is not installed"))?;

    let version_dir = layout.plugin_root(&id.marketplace, &id.plugin, &installed.version);
    remove_path(&version_dir)?;

    let next_state = state.forget_install(id);
    next_state.save(&layout.state_file())?;

    if purge {
        remove_path(&layout.plugin_data(&id.marketplace, &id.plugin))?;
    }

    Ok(next_state)
}

fn sync_source(
    source: &EntrySource,
    marketplace_repo_root: &Path,
    plugin_root: &Path,
) -> Result<SyncOutcome> {
    match source {
        EntrySource::Local { path } => {
            let resolved = contained_within(marketplace_repo_root, path)
                .context("resolving local plugin source")?;
            LocalSource::new(resolved).sync(plugin_root)
        }
        EntrySource::Git {
            url,
            reference,
            sha,
            ..
        } => GitSource::new(url.clone(), reference.clone(), sha.clone()).sync(plugin_root),
    }
}

/// The directory manifest lookup and skill discovery treat as `PLUGIN_ROOT`.
/// For a `Git` source with a `subpath`, that is a containment-checked
/// subdirectory of the clone; otherwise it is `plugin_root` itself — a real
/// clone for `Git`, or, for `Local`, the symlink `LocalSource::sync` created.
///
/// Public so callers that already know an `InstalledPlugin` (e.g. the `enable`
/// CLI command, working from `State` rather than a fresh `install`/`update`
/// call) can recompute the same root `InstallOutcome::effective_root` carries,
/// from the installed plugin's own recorded `source`.
pub fn effective_plugin_root(plugin_root: &Path, source: &EntrySource) -> Result<PathBuf> {
    let subpath = match source {
        EntrySource::Git {
            subpath: Some(subpath),
            ..
        } => Some(subpath.as_str()),
        _ => None,
    };
    match subpath {
        Some(subpath) => contained_within(plugin_root, subpath),
        None => canonicalize(plugin_root),
    }
}

/// Resolve `root.join(relative)`, requiring the canonicalized result to stay
/// inside canonicalized `root`. This is the containment check the Agent
/// Plugins 1.0 spec requires for every path a plugin exposes: a `Local`
/// source's path, a `Git` source's `subpath`, and each discovered skill.
/// Symlinks are permitted as long as they resolve inside the boundary.
fn contained_within(root: &Path, relative: &str) -> Result<PathBuf> {
    let canonical_root = canonicalize(root)?;
    let candidate = root.join(relative);
    let canonical_candidate = canonicalize(&candidate)?;
    if !canonical_candidate.starts_with(&canonical_root) {
        bail!(
            "{:?} resolves to {}, which escapes the plugin root {}",
            relative,
            canonical_candidate.display(),
            canonical_root.display()
        );
    }
    Ok(canonical_candidate)
}

fn canonicalize(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("resolving {}", path.display()))
}

/// Discover a plugin's skills: `explicit_skills` if the marketplace entry
/// declared any, else the Agent Plugins 1.0 fixed location — immediate
/// children of `skills/` that contain a `SKILL.md`, no deeper recursion.
fn discover_skills(effective_root: &Path, entry: &MarketplaceEntry) -> Result<Vec<String>> {
    if !entry.explicit_skills.is_empty() {
        return discover_explicit_skills(effective_root, &entry.explicit_skills);
    }
    discover_default_skills(effective_root)
}

fn discover_default_skills(effective_root: &Path) -> Result<Vec<String>> {
    let skills_dir = effective_root.join("skills");
    if !skills_dir.is_dir() {
        return Ok(Vec::new());
    }
    let canonical_root = canonicalize(effective_root)?;

    let mut names = Vec::new();
    for entry in
        fs::read_dir(&skills_dir).with_context(|| format!("reading {}", skills_dir.display()))?
    {
        let entry = entry.with_context(|| format!("reading entry in {}", skills_dir.display()))?;
        let path = entry.path();
        if !path.is_dir() || !path.join("SKILL.md").is_file() {
            continue;
        }
        let canonical = canonicalize(&path)?;
        if !canonical.starts_with(&canonical_root) {
            bail!("skill directory {} escapes the plugin root", path.display());
        }
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

fn discover_explicit_skills(effective_root: &Path, explicit: &[String]) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for raw in explicit {
        let relative = raw.strip_prefix("./").unwrap_or(raw);
        let canonical = contained_within(effective_root, relative)
            .with_context(|| format!("resolving explicit skill path {raw:?}"))?;
        if !canonical.join("SKILL.md").is_file() {
            bail!("explicit skill path {raw:?} has no SKILL.md");
        }
        let name = canonical
            .file_name()
            .and_then(|n| n.to_str())
            .with_context(|| format!("explicit skill path {raw:?} has no final component"))?;
        names.push(name.to_string());
    }
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_plugin(root: &Path, name: &str) {
        fs::create_dir_all(root).unwrap();
        fs::write(
            root.join("plugin.json"),
            format!(r#"{{"name": "{name}", "version": "1.0.0"}}"#),
        )
        .unwrap();
    }

    fn write_skill(plugin_root: &Path, name: &str) {
        let dir = plugin_root.join("skills").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: does a thing.\n---\nBody\n"),
        )
        .unwrap();
    }

    fn demo_entry(name: &str, local_path: &str) -> MarketplaceEntry {
        MarketplaceEntry {
            name: name.to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            source: EntrySource::Local {
                path: local_path.to_string(),
            },
            category: None,
            keywords: Vec::new(),
            homepage: None,
            author: None,
            explicit_skills: Vec::new(),
        }
    }

    #[test]
    fn installing_a_local_plugin_records_discovered_skills_at_depth_one_only() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::rooted_at(temp.path().join("store"));
        let marketplace_root = temp.path().join("marketplace");
        let plugin_src = marketplace_root.join("plugins").join("demo");
        write_plugin(&plugin_src, "demo");
        write_skill(&plugin_src, "alpha");
        // A SKILL.md nested one level deeper than skills/<name> must be ignored.
        fs::create_dir_all(plugin_src.join("skills").join("alpha").join("nested")).unwrap();
        fs::write(
            plugin_src
                .join("skills")
                .join("alpha")
                .join("nested")
                .join("SKILL.md"),
            "---\nname: nested\ndescription: too deep.\n---\n",
        )
        .unwrap();
        let entry = demo_entry("demo", "plugins/demo");

        let outcome = install(
            &layout,
            &State::default(),
            "official",
            &marketplace_root,
            &entry,
        )
        .unwrap();

        assert_eq!(outcome.installed.skills, vec!["alpha".to_string()]);
        assert_eq!(outcome.installed.version, "1.0.0");
        assert!(outcome
            .state
            .installed(&PluginId::new("demo", "official"))
            .is_some());
    }

    #[test]
    fn explicit_skills_override_directory_discovery() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::rooted_at(temp.path().join("store"));
        let marketplace_root = temp.path().join("marketplace");
        let plugin_src = marketplace_root.join("plugins").join("demo");
        write_plugin(&plugin_src, "demo");
        write_skill(&plugin_src, "ignored-by-explicit-list");
        write_skill(&plugin_src, "chosen");
        let mut entry = demo_entry("demo", "plugins/demo");
        entry.explicit_skills = vec!["./skills/chosen".to_string()];

        let outcome = install(
            &layout,
            &State::default(),
            "official",
            &marketplace_root,
            &entry,
        )
        .unwrap();

        assert_eq!(outcome.installed.skills, vec!["chosen".to_string()]);
    }

    #[test]
    fn a_git_subpath_escaping_the_plugin_root_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::rooted_at(temp.path().join("store"));
        let clone_dir = layout.plugin_root("official", "demo", "1.0.0");
        write_plugin(&clone_dir, "demo");
        let source = EntrySource::Git {
            url: "unused".to_string(),
            reference: None,
            sha: None,
            subpath: Some("../../etc".to_string()),
        };

        let result = effective_plugin_root(&clone_dir, &source);

        assert!(result.is_err(), "escaping subpath must be rejected");
    }

    #[test]
    fn plugin_data_contents_survive_a_reinstall() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::rooted_at(temp.path().join("store"));
        let marketplace_root = temp.path().join("marketplace");
        let plugin_src = marketplace_root.join("plugins").join("demo");
        write_plugin(&plugin_src, "demo");
        let entry = demo_entry("demo", "plugins/demo");
        let first = install(
            &layout,
            &State::default(),
            "official",
            &marketplace_root,
            &entry,
        )
        .unwrap();
        let plugin_data = layout.plugin_data("official", "demo");
        fs::write(plugin_data.join("user-data.txt"), "keep me").unwrap();

        install(&layout, &first.state, "official", &marketplace_root, &entry).unwrap();

        assert_eq!(
            fs::read_to_string(plugin_data.join("user-data.txt")).unwrap(),
            "keep me"
        );
    }

    #[test]
    fn uninstall_removes_the_version_dir_but_leaves_plugin_data() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::rooted_at(temp.path().join("store"));
        let marketplace_root = temp.path().join("marketplace");
        let plugin_src = marketplace_root.join("plugins").join("demo");
        write_plugin(&plugin_src, "demo");
        let entry = demo_entry("demo", "plugins/demo");
        let installed = install(
            &layout,
            &State::default(),
            "official",
            &marketplace_root,
            &entry,
        )
        .unwrap();
        let plugin_data = layout.plugin_data("official", "demo");
        fs::write(plugin_data.join("user-data.txt"), "keep me").unwrap();
        let id = PluginId::new("demo", "official");
        let version_dir = layout.plugin_root("official", "demo", "1.0.0");
        assert!(version_dir.exists());

        let next_state = uninstall(&layout, &installed.state, &id, false).unwrap();

        assert!(!version_dir.exists());
        assert!(plugin_data.join("user-data.txt").exists());
        assert!(next_state.installed(&id).is_none());
    }

    #[test]
    fn uninstall_with_purge_also_removes_plugin_data() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::rooted_at(temp.path().join("store"));
        let marketplace_root = temp.path().join("marketplace");
        let plugin_src = marketplace_root.join("plugins").join("demo");
        write_plugin(&plugin_src, "demo");
        let entry = demo_entry("demo", "plugins/demo");
        let installed = install(
            &layout,
            &State::default(),
            "official",
            &marketplace_root,
            &entry,
        )
        .unwrap();
        let plugin_data = layout.plugin_data("official", "demo");
        fs::write(plugin_data.join("user-data.txt"), "keep me").unwrap();
        let id = PluginId::new("demo", "official");

        uninstall(&layout, &installed.state, &id, true).unwrap();

        assert!(!plugin_data.exists());
    }

    #[test]
    fn update_installs_a_new_version_alongside_the_old_one() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::rooted_at(temp.path().join("store"));
        let marketplace_root = temp.path().join("marketplace");
        let plugin_src = marketplace_root.join("plugins").join("demo");
        write_plugin(&plugin_src, "demo");
        let v1 = demo_entry("demo", "plugins/demo");
        let first = install(
            &layout,
            &State::default(),
            "official",
            &marketplace_root,
            &v1,
        )
        .unwrap();
        let mut v2 = demo_entry("demo", "plugins/demo");
        v2.version = Some("2.0.0".to_string());

        let outcome = update(&layout, &first.state, "official", &marketplace_root, &v2).unwrap();

        assert_eq!(outcome.installed.version, "2.0.0");
        assert_eq!(
            outcome.previous.map(|p| p.version),
            Some("1.0.0".to_string())
        );
        assert!(layout.plugin_root("official", "demo", "1.0.0").exists());
        assert!(layout.plugin_root("official", "demo", "2.0.0").exists());
    }

    #[test]
    fn install_fails_cleanly_when_plugin_json_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::rooted_at(temp.path().join("store"));
        let marketplace_root = temp.path().join("marketplace");
        fs::create_dir_all(marketplace_root.join("plugins").join("demo")).unwrap();
        let entry = demo_entry("demo", "plugins/demo");

        let result = install(
            &layout,
            &State::default(),
            "official",
            &marketplace_root,
            &entry,
        );

        assert!(result.is_err());
    }
}
