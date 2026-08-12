//! Skill projection into agent skills directories: the `systemctl enable` analogue.
//!
//! A skill lives once under a plugin's `PLUGIN_ROOT/skills/<skill>` in the store and
//! is *projected* into one or more agents' skills directories, normally by symlink.
//! This module creates, removes, and inspects those projections.
//!
//! # The safety rule
//!
//! [`disable`] is the most safety-critical function in the whole project. A real
//! agent skills directory (e.g. `~/.agents/skills`) can contain hand-made symlinks
//! to unrelated projects and real directories a user created by hand, side by side
//! with entries `askm` manages. `disable` must never destroy either of those.
//!
//! The rule it enforces: an entry is only ever removed when it is a symlink (never
//! a real file or directory) **and** either
//!   (a) [`State::link_at`] has a record for that exact path — proof `askm` created
//!       it — or
//!   (b) it has no such record, but its target resolves inside the `askm` store
//!       itself (a recovery path for a lost or hand-edited state file).
//!
//! A [`LinkMode::Copy`] entry is a real directory, not a symlink, but `askm` did
//! create it; it is only removed when a state record proves that, via the same
//! `link_at` check applied on the non-symlink path. There is no other code path in
//! this file that calls `remove_dir_all` or `remove_file` on an entry it did not
//! just prove ownership of.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use thiserror::Error;

use crate::paths::{AgentTarget, Scope};
use crate::state::{now_unix, LinkMode, LinkRecord, ScopeRef, State};

#[cfg(not(unix))]
compile_error!(
    "askm::link requires a Unix target: it creates symlinks via \
     std::os::unix::fs::symlink. Windows support is not implemented in this build."
);

/// What `askm` found occupying a link path it was asked to write to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occupant {
    /// A real directory `askm` has no record of creating.
    Directory,
    /// A real file.
    File,
    /// A symlink `askm` has no record of creating.
    ForeignSymlink,
    /// Recorded in state, but pointing somewhere other than what was just
    /// requested (e.g. a different plugin version) — still requires `force`.
    StaleManaged,
}

impl fmt::Display for Occupant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Each variant reads as a complete clause, including who owns the entry.
        // `StaleManaged` is the one case askm *did* create, so it cannot share a
        // generic "askm did not create it" suffix bolted on by the caller.
        let label = match self {
            Occupant::Directory => "a directory askm did not create is there",
            Occupant::File => "a file askm did not create is there",
            Occupant::ForeignSymlink => "a symlink askm did not create is there",
            Occupant::StaleManaged => {
                "an askm-managed entry is there, but it points somewhere else"
            }
        };
        f.write_str(label)
    }
}

/// Errors from [`enable`] that a caller might want to branch on.
#[derive(Debug, Error)]
pub enum EnableError {
    #[error("refusing to enable {skill:?} at {path:?}: {occupant} (use --force to replace it)")]
    Occupied {
        skill: String,
        path: PathBuf,
        occupant: Occupant,
    },
}

/// Everything [`enable`] needs to know about the skill being projected. A struct
/// rather than a long positional argument list, since several fields share a type.
#[derive(Debug, Clone)]
pub struct EnableRequest<'a> {
    pub skill: &'a str,
    /// `<plugin>@<marketplace>` identity recorded against the resulting link.
    pub plugin: &'a str,
    /// `PLUGIN_ROOT` for the installed version; the skill lives at
    /// `<plugin_root>/skills/<skill>`.
    pub plugin_root: &'a Path,
    pub target: &'a AgentTarget,
    pub scope: &'a Scope,
    pub mode: LinkMode,
    pub force: bool,
}

/// Result of a successful [`enable`] call.
#[derive(Debug, Clone)]
pub struct EnableOutcome {
    /// State with the link recorded. The caller is responsible for persisting it.
    pub state: State,
    pub link_path: PathBuf,
    pub points_to: PathBuf,
    /// `false` when this call was a no-op because the link already matched.
    pub created: bool,
}

/// What [`disable`] did (or refused to do).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisableAction {
    /// Nothing was at the link path.
    Absent,
    /// Removed an askm-managed symlink.
    RemovedSymlink,
    /// Removed an askm-managed copy (a real directory `askm` created, proven by state).
    RemovedCopy,
    /// Left the entry alone: the safety rule in action.
    Refused(Occupant),
}

/// Result of a [`disable`] call. `disable` itself never errors on a safety refusal
/// — refusing to delete someone's data is success, not failure — so this always
/// carries an updated (possibly unchanged) `State`.
#[derive(Debug, Clone)]
pub struct DisableOutcome {
    pub state: State,
    pub action: DisableAction,
}

/// How one entry in an agent's skills directory relates to `askm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    ManagedSymlink,
    ManagedCopy,
    ForeignSymlink,
    ForeignDirectory,
    ForeignOther,
}

/// One entry observed in a target's skills directory, for `status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    pub skill: String,
    pub target: String,
    pub scope: ScopeRef,
    pub link_path: PathBuf,
    /// What the entry points to, if it is a symlink (managed or foreign).
    pub points_to: Option<PathBuf>,
    pub kind: EntryKind,
    /// `true` for a symlink whose target no longer exists.
    pub broken: bool,
}

/// A skill enabled in both project and user scope for the same target. Per the
/// Agent Skills convention project overrides user, so this is a warning, not an
/// error: the user-scope entry is simply shadowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowWarning {
    pub skill: String,
    pub target: String,
    pub user_link: PathBuf,
    pub project_link: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    pub entries: Vec<SkillEntry>,
    pub shadows: Vec<ShadowWarning>,
}

/// Project a skill from a plugin's store directory into a target's skills
/// directory, by symlink or by recursive copy. Idempotent: calling it again with
/// the same request is a no-op success, not an error.
pub fn enable(state: &State, request: EnableRequest<'_>) -> Result<EnableOutcome> {
    let link_path = skill_link_path(request.target, request.scope, request.skill)?;
    let points_to = skill_source_path(request.plugin_root, request.skill);
    ensure_parent_dir(&link_path)?;

    let Some(meta) = read_occupant(&link_path)? else {
        materialize(&points_to, &link_path, request.mode)?;
        return Ok(record_enable(state, &request, link_path, points_to));
    };

    let recorded = state.link_at(&link_path);
    if is_idempotent(recorded, &meta, &link_path, &points_to, request.mode) {
        return Ok(EnableOutcome {
            state: state.clone(),
            link_path,
            points_to,
            created: false,
        });
    }

    if !request.force {
        return Err(EnableError::Occupied {
            skill: request.skill.to_string(),
            path: link_path.clone(),
            occupant: classify_occupant(recorded, &meta),
        }
        .into());
    }

    remove_occupant(&link_path, &meta, recorded)?;
    materialize(&points_to, &link_path, request.mode)?;
    Ok(record_enable(state, &request, link_path, points_to))
}

/// Remove a skill's projection from a target's skills directory. See the module
/// doc comment for the safety rule this enforces. Removing an absent link is a
/// no-op success.
///
/// `store_root` is the root `askm`'s own store lives under (e.g.
/// [`crate::paths::Layout::data_dir`]) — used only as a recovery path when a
/// symlink `askm` made has lost its state record.
pub fn disable(
    state: &State,
    skill: &str,
    target: &AgentTarget,
    scope: &Scope,
    store_root: &Path,
) -> Result<DisableOutcome> {
    let link_path = skill_link_path(target, scope, skill)?;
    let Some(meta) = read_occupant(&link_path)? else {
        return Ok(DisableOutcome {
            state: state.clone(),
            action: DisableAction::Absent,
        });
    };

    let recorded = state.link_at(&link_path);
    if !meta.file_type().is_symlink() {
        return disable_non_symlink(state, &link_path, recorded);
    }

    let askm_owns_it = recorded.is_some() || symlink_target_inside_store(&link_path, store_root);
    if !askm_owns_it {
        return Ok(DisableOutcome {
            state: state.clone(),
            action: DisableAction::Refused(Occupant::ForeignSymlink),
        });
    }

    fs::remove_file(&link_path)
        .with_context(|| format!("removing symlink {}", link_path.display()))?;
    Ok(DisableOutcome {
        state: state.forget_link(&link_path),
        action: DisableAction::RemovedSymlink,
    })
}

/// Report every entry found under the given targets and scopes, and any
/// project/user shadowing between them.
pub fn status(state: &State, targets: &[AgentTarget], scopes: &[Scope]) -> Result<StatusReport> {
    let mut entries = Vec::new();
    for target in targets {
        for scope in scopes {
            entries.extend(scan_target_scope(state, target, scope)?);
        }
    }
    let shadows = detect_shadows(&entries);
    Ok(StatusReport { entries, shadows })
}

fn skill_link_path(target: &AgentTarget, scope: &Scope, skill: &str) -> Result<PathBuf> {
    Ok(target
        .skills_dir(scope)
        .with_context(|| format!("resolving skills directory for target {:?}", target.id))?
        .join(skill))
}

fn skill_source_path(plugin_root: &Path, skill: &str) -> PathBuf {
    plugin_root.join("skills").join(skill)
}

fn ensure_parent_dir(link_path: &Path) -> Result<()> {
    let parent = link_path
        .parent()
        .with_context(|| format!("{} has no parent directory", link_path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))
}

/// Read what is at `link_path` without following a symlink, or `None` if nothing
/// is there.
fn read_occupant(link_path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(link_path) {
        Ok(meta) => Ok(Some(meta)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("checking {}", link_path.display())),
    }
}

fn is_idempotent(
    recorded: Option<&LinkRecord>,
    meta: &fs::Metadata,
    link_path: &Path,
    points_to: &Path,
    mode: LinkMode,
) -> bool {
    let Some(recorded) = recorded else {
        return false;
    };
    if recorded.points_to != points_to || recorded.mode != mode {
        return false;
    }
    match mode {
        LinkMode::Symlink => {
            meta.file_type().is_symlink()
                && fs::read_link(link_path).ok().as_deref() == Some(points_to)
        }
        LinkMode::Copy => meta.is_dir(),
    }
}

fn classify_occupant(recorded: Option<&LinkRecord>, meta: &fs::Metadata) -> Occupant {
    if recorded.is_some() {
        return Occupant::StaleManaged;
    }
    if meta.file_type().is_symlink() {
        Occupant::ForeignSymlink
    } else if meta.is_dir() {
        Occupant::Directory
    } else {
        Occupant::File
    }
}

/// Remove whatever is occupying `link_path` so `enable` can replace it under
/// `force`. A symlink is always safe to swap. A real file or directory is only
/// ever removed here when `recorded` proves `askm` made it as a [`LinkMode::Copy`]
/// — this is the same proof [`disable`] requires, so `force` can never be used to
/// delete a user's real data.
fn remove_occupant(
    link_path: &Path,
    meta: &fs::Metadata,
    recorded: Option<&LinkRecord>,
) -> Result<()> {
    if meta.file_type().is_symlink() {
        return fs::remove_file(link_path)
            .with_context(|| format!("removing existing symlink {}", link_path.display()));
    }

    let recorded_copy = recorded.is_some_and(|r| r.mode == LinkMode::Copy);
    if !recorded_copy {
        anyhow::bail!(
            "refusing to replace {}: real data is there and askm did not create it \
             (force does not override this safety check)",
            link_path.display()
        );
    }
    fs::remove_dir_all(link_path)
        .with_context(|| format!("removing existing directory {}", link_path.display()))
}

fn materialize(points_to: &Path, link_path: &Path, mode: LinkMode) -> Result<()> {
    match mode {
        LinkMode::Symlink => create_symlink(points_to, link_path),
        LinkMode::Copy => copy_recursive(points_to, link_path)
            .with_context(|| format!("copying {} to {}", points_to.display(), link_path.display())),
    }
}

fn create_symlink(points_to: &Path, link_path: &Path) -> Result<()> {
    std::os::unix::fs::symlink(points_to, link_path)
        .with_context(|| format!("linking {} -> {}", link_path.display(), points_to.display()))
}

/// Recursively copy `src` to `dst`, creating `dst` itself. Used by
/// [`LinkMode::Copy`] for clients that don't follow symlinks. Any symlink inside
/// `src` is copied by its resolved contents (`std::fs::copy` follows symlinks),
/// not preserved as a symlink — acceptable for v1's container-transfer use case.
fn copy_recursive(src: &Path, dst: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(src) {
        let entry = entry.context("walking source skill directory")?;
        let relative = entry
            .path()
            .strip_prefix(src)
            .expect("walkdir entries are rooted at src");
        let target = dst.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)
                .with_context(|| format!("creating {}", target.display()))?;
        } else {
            fs::copy(entry.path(), &target).with_context(|| {
                format!("copying {} to {}", entry.path().display(), target.display())
            })?;
        }
    }
    Ok(())
}

fn record_enable(
    state: &State,
    request: &EnableRequest<'_>,
    link_path: PathBuf,
    points_to: PathBuf,
) -> EnableOutcome {
    let record = LinkRecord {
        skill: request.skill.to_string(),
        plugin: request.plugin.to_string(),
        target: request.target.id.clone(),
        scope: ScopeRef::from(request.scope),
        link_path: link_path.clone(),
        points_to: points_to.clone(),
        mode: request.mode,
        created_at: now_unix(),
    };
    EnableOutcome {
        state: state.record_link(record),
        link_path,
        points_to,
        created: true,
    }
}

/// A real file or directory sits at `link_path`. Only ever removed when `recorded`
/// proves `askm` created it as a [`LinkMode::Copy`] — the safety rule.
fn disable_non_symlink(
    state: &State,
    link_path: &Path,
    recorded: Option<&LinkRecord>,
) -> Result<DisableOutcome> {
    let recorded_copy = recorded.is_some_and(|r| r.mode == LinkMode::Copy);
    if !recorded_copy {
        let occupant = if link_path.is_dir() {
            Occupant::Directory
        } else {
            Occupant::File
        };
        return Ok(DisableOutcome {
            state: state.clone(),
            action: DisableAction::Refused(occupant),
        });
    }
    fs::remove_dir_all(link_path).with_context(|| format!("removing {}", link_path.display()))?;
    Ok(DisableOutcome {
        state: state.forget_link(link_path),
        action: DisableAction::RemovedCopy,
    })
}

/// Whether `link_path`'s symlink target resolves inside `store_root`. A recovery
/// path for a symlink `askm` made that has no state record (state file lost or
/// hand-edited): still safe to remove because it demonstrably points into askm's
/// own managed store rather than at user data. A dangling target can't be proven
/// either way, so it is treated as *not* inside the store — refuse, don't guess.
fn symlink_target_inside_store(link_path: &Path, store_root: &Path) -> bool {
    let Ok(resolved) = fs::canonicalize(link_path) else {
        return false;
    };
    let Ok(store_root) = fs::canonicalize(store_root) else {
        return false;
    };
    resolved.starts_with(store_root)
}

fn scan_target_scope(
    state: &State,
    target: &AgentTarget,
    scope: &Scope,
) -> Result<Vec<SkillEntry>> {
    let skills_dir = target.skills_dir(scope)?;
    let read_dir = match fs::read_dir(&skills_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", skills_dir.display())),
    };

    let mut out = Vec::new();
    for entry in read_dir {
        let entry = entry.with_context(|| format!("reading entry in {}", skills_dir.display()))?;
        out.push(describe_entry(state, target, scope, &entry.path())?);
    }
    Ok(out)
}

fn describe_entry(
    state: &State,
    target: &AgentTarget,
    scope: &Scope,
    link_path: &Path,
) -> Result<SkillEntry> {
    let skill = link_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .with_context(|| format!("{} has no file name", link_path.display()))?;
    let meta = fs::symlink_metadata(link_path)
        .with_context(|| format!("reading {}", link_path.display()))?;

    if let Some(record) = state.link_at(link_path) {
        return Ok(managed_entry(target, scope, link_path, &skill, record));
    }
    if meta.file_type().is_symlink() {
        return Ok(foreign_symlink_entry(target, scope, link_path, &skill));
    }

    let kind = if meta.is_dir() {
        EntryKind::ForeignDirectory
    } else {
        EntryKind::ForeignOther
    };
    Ok(SkillEntry {
        skill,
        target: target.id.clone(),
        scope: ScopeRef::from(scope),
        link_path: link_path.to_path_buf(),
        points_to: None,
        kind,
        broken: false,
    })
}

fn managed_entry(
    target: &AgentTarget,
    scope: &Scope,
    link_path: &Path,
    skill: &str,
    record: &LinkRecord,
) -> SkillEntry {
    let broken = record.mode == LinkMode::Symlink && !record.points_to.exists();
    let kind = match record.mode {
        LinkMode::Symlink => EntryKind::ManagedSymlink,
        LinkMode::Copy => EntryKind::ManagedCopy,
    };
    SkillEntry {
        skill: skill.to_string(),
        target: target.id.clone(),
        scope: ScopeRef::from(scope),
        link_path: link_path.to_path_buf(),
        points_to: Some(record.points_to.clone()),
        kind,
        broken,
    }
}

fn foreign_symlink_entry(
    target: &AgentTarget,
    scope: &Scope,
    link_path: &Path,
    skill: &str,
) -> SkillEntry {
    let raw_target = fs::read_link(link_path).ok();
    let broken = match &raw_target {
        Some(t) => !resolve_symlink_target(link_path, t).exists(),
        None => false,
    };
    SkillEntry {
        skill: skill.to_string(),
        target: target.id.clone(),
        scope: ScopeRef::from(scope),
        link_path: link_path.to_path_buf(),
        points_to: raw_target,
        kind: EntryKind::ForeignSymlink,
        broken,
    }
}

/// A symlink's raw target may be relative to the symlink's own directory rather
/// than to the current working directory; resolve it accordingly before checking
/// existence.
fn resolve_symlink_target(link_path: &Path, raw_target: &Path) -> PathBuf {
    if raw_target.is_absolute() {
        return raw_target.to_path_buf();
    }
    match link_path.parent() {
        Some(parent) => parent.join(raw_target),
        None => raw_target.to_path_buf(),
    }
}

fn detect_shadows(entries: &[SkillEntry]) -> Vec<ShadowWarning> {
    let mut shadows = Vec::new();
    for user_entry in entries.iter().filter(|e| e.scope == ScopeRef::User) {
        let project_entry = entries.iter().find(|e| {
            e.target == user_entry.target
                && e.skill == user_entry.skill
                && matches!(e.scope, ScopeRef::Project { .. })
        });
        if let Some(project_entry) = project_entry {
            shadows.push(ShadowWarning {
                skill: user_entry.skill.clone(),
                target: user_entry.target.clone(),
                user_link: user_entry.link_path.clone(),
                project_link: project_entry.link_path.clone(),
            });
        }
    }
    shadows
}

// These are pure-logic unit tests. Shadow detection specifically needs a
// `ScopeRef::User` entry, and `status()` only ever produces one by scanning the
// real `Scope::User` (`$HOME`) — not something a hermetic test suite should do.
// Testing `detect_shadows` directly against hand-built entries covers the same
// logic without touching the filesystem at all. The rest of the behavior
// (creating, removing, and scanning real entries) is covered by the FS-backed
// integration tests in `tests/link_*.rs`, which stick to `Scope::Project` only.
#[cfg(test)]
mod tests {
    use super::*;

    fn entry(skill: &str, target: &str, scope: ScopeRef, link_path: &str) -> SkillEntry {
        SkillEntry {
            skill: skill.to_string(),
            target: target.to_string(),
            scope,
            link_path: PathBuf::from(link_path),
            points_to: None,
            kind: EntryKind::ManagedSymlink,
            broken: false,
        }
    }

    #[test]
    fn detect_shadows_fires_when_the_same_skill_is_enabled_in_both_scopes() {
        let entries = vec![
            entry(
                "demo-skill",
                "agents",
                ScopeRef::User,
                "/home/u/.agents/skills/demo-skill",
            ),
            entry(
                "demo-skill",
                "agents",
                ScopeRef::Project {
                    root: PathBuf::from("/work/repo"),
                },
                "/work/repo/.agents/skills/demo-skill",
            ),
        ];

        let shadows = detect_shadows(&entries);

        assert_eq!(shadows.len(), 1);
        assert_eq!(shadows[0].skill, "demo-skill");
        assert_eq!(shadows[0].target, "agents");
        assert_eq!(
            shadows[0].user_link,
            PathBuf::from("/home/u/.agents/skills/demo-skill")
        );
    }

    #[test]
    fn detect_shadows_is_silent_when_a_skill_is_enabled_in_only_one_scope() {
        let entries = vec![entry(
            "demo-skill",
            "agents",
            ScopeRef::Project {
                root: PathBuf::from("/work/repo"),
            },
            "/work/repo/.agents/skills/demo-skill",
        )];

        assert!(detect_shadows(&entries).is_empty());
    }

    #[test]
    fn detect_shadows_ignores_matching_skills_under_different_targets() {
        let entries = vec![
            entry(
                "demo-skill",
                "agents",
                ScopeRef::User,
                "/home/u/.agents/skills/demo-skill",
            ),
            entry(
                "demo-skill",
                "claude",
                ScopeRef::Project {
                    root: PathBuf::from("/work/repo"),
                },
                "/work/repo/.claude/skills/demo-skill",
            ),
        ];

        assert!(detect_shadows(&entries).is_empty());
    }

    #[test]
    fn resolve_symlink_target_joins_a_relative_target_against_the_links_parent_directory() {
        let link_path = Path::new("/home/u/.agents/skills/demo-skill");

        let resolved = resolve_symlink_target(link_path, Path::new("../elsewhere"));

        assert_eq!(
            resolved,
            PathBuf::from("/home/u/.agents/skills/../elsewhere")
        );
    }

    #[test]
    fn resolve_symlink_target_leaves_an_absolute_target_untouched() {
        let link_path = Path::new("/home/u/.agents/skills/demo-skill");

        let resolved = resolve_symlink_target(link_path, Path::new("/store/elsewhere"));

        assert_eq!(resolved, PathBuf::from("/store/elsewhere"));
    }
}
