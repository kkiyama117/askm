//! `askm enable|disable|status|doctor`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};
use askm::link::{self, DisableAction, EntryKind, SkillEntry, StatusReport};
use askm::paths::{default_targets, AgentTarget, Scope};
use askm::state::{LinkMode, PluginId, ScopeRef, State};
use askm::store;
use serde::Serialize;

use crate::commands::context::{all_scopes, resolve_scope, resolve_targets, Context};
use crate::commands::table;

/// Everything `enable` needs from the parsed CLI, bundled so it can be built
/// once in `commands::run` and threaded through without a long parameter list.
pub struct EnableArgs {
    pub spec: String,
    pub all: bool,
    pub path: Option<PathBuf>,
    pub targets: Option<Vec<String>>,
    pub user: bool,
    pub project: bool,
    pub copy: bool,
    pub force: bool,
}

pub fn enable_cmd(ctx: &mut Context, args: EnableArgs) -> Result<()> {
    let targets = resolve_targets(&ctx.config, args.targets.as_deref())?;
    let scope = resolve_scope(args.user, args.project, &ctx.config)?;
    let mode = if args.copy {
        LinkMode::Copy
    } else {
        LinkMode::Symlink
    };
    if args.spec.is_empty() && args.path.is_none() {
        bail!(
            "enable needs a skill name, or --path <dir> (with --all to enable every skill in it)"
        );
    }
    let plan = match &args.path {
        Some(dir) => resolve_path_plan(dir, &args)?,
        None => resolve_plan(ctx, &args)?,
    };

    for (skill, id) in &plan {
        enable_one(ctx, skill, id, &targets, &scope, mode, args.force)?;
    }
    Ok(())
}

/// Resolve the plan for `--path <dir>`: every immediate child of `<dir>` that
/// contains a `SKILL.md` (or just `<dir>/<skill>` when `--all` is not given).
/// Each skill is recorded as plugin `path@<dir>` so `disable` can prove
/// ownership of the links it creates.
fn resolve_path_plan(dir: &Path, args: &EnableArgs) -> Result<Vec<(String, PluginId)>> {
    let canonical = dir
        .canonicalize()
        .with_context(|| format!("resolving {}", dir.display()))?;
    if !canonical.is_dir() {
        bail!("{:?} is not a directory", dir.display());
    }
    let id = PluginId::new("path", canonical.to_string_lossy());
    let skills = if args.all {
        let mut names = Vec::new();
        for entry in
            fs::read_dir(&canonical).with_context(|| format!("reading {}", canonical.display()))?
        {
            let entry = entry.context("reading skills directory entry")?;
            let path = entry.path();
            if path.is_dir() && path.join("SKILL.md").is_file() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();
        names
    } else {
        let skill_dir = canonical.join(&args.spec);
        if !skill_dir.is_dir() || !skill_dir.join("SKILL.md").is_file() {
            bail!(
                "{:?} has no skill named {:?} (expected {})",
                dir.display(),
                args.spec,
                skill_dir.display()
            );
        }
        vec![args.spec.clone()]
    };
    if skills.is_empty() {
        bail!("no skills found under {:?}", dir.display());
    }
    Ok(skills.into_iter().map(|s| (s, id.clone())).collect())
}

/// The skills to enable and which installed plugin each comes from: either
/// every skill of one plugin (`--all`), or a single resolved skill.
fn resolve_plan(ctx: &Context, args: &EnableArgs) -> Result<Vec<(String, PluginId)>> {
    if args.all {
        let id = PluginId::parse(&args.spec)
            .context("with --all, the argument must be <plugin>@<marketplace>")?;
        let installed = ctx
            .state
            .installed(&id)
            .with_context(|| format!("{id} is not installed"))?;
        return Ok(installed
            .skills
            .iter()
            .map(|s| (s.clone(), id.clone()))
            .collect());
    }
    let (skill, qualifier) = parse_skill_arg(&args.spec)?;
    let id = resolve_skill(ctx, &skill, qualifier.as_ref())?;
    Ok(vec![(skill, id)])
}

/// Parse `enable`'s (non-`--all`) positional argument: a bare skill name, or
/// `<skill>@<plugin>@<marketplace>` to disambiguate.
fn parse_skill_arg(raw: &str) -> Result<(String, Option<PluginId>)> {
    let parts: Vec<&str> = raw.split('@').collect();
    match parts.as_slice() {
        [skill] if !skill.is_empty() => Ok((skill.to_string(), None)),
        [skill, plugin, marketplace]
            if [skill, plugin, marketplace].iter().all(|s| !s.is_empty()) =>
        {
            Ok((
                skill.to_string(),
                Some(PluginId::new(*plugin, *marketplace)),
            ))
        }
        _ => bail!("expected <skill> or <skill>@<plugin>@<marketplace>, got {raw:?}"),
    }
}

/// Resolve a skill name to the installed plugin it belongs to, using
/// `qualifier` when given, else searching every installed plugin's recorded
/// skill list and requiring an unambiguous match.
fn resolve_skill(ctx: &Context, skill: &str, qualifier: Option<&PluginId>) -> Result<PluginId> {
    if let Some(id) = qualifier {
        let installed = ctx
            .state
            .installed(id)
            .with_context(|| format!("{id} is not installed"))?;
        if !installed.skills.iter().any(|s| s == skill) {
            bail!(
                "{id} has no skill named {skill:?} (it has: {})",
                installed.skills.join(", ")
            );
        }
        return Ok(id.clone());
    }

    let matches: Vec<PluginId> = ctx
        .state
        .installed
        .values()
        .filter(|p| p.skills.iter().any(|s| s == skill))
        .map(|p| PluginId::new(p.plugin.clone(), p.marketplace.clone()))
        .collect();

    match matches.as_slice() {
        [] => bail!("no installed plugin has a skill named {skill:?}"),
        [one] => Ok(one.clone()),
        many => bail!(
            "{skill:?} is installed by more than one plugin ({}); qualify it as <skill>@<plugin>@<marketplace>",
            many.iter().map(PluginId::to_string).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn enable_one(
    ctx: &mut Context,
    skill: &str,
    id: &PluginId,
    targets: &[AgentTarget],
    scope: &Scope,
    mode: LinkMode,
    force: bool,
) -> Result<()> {
    let path_mode = id.plugin == "path";
    let effective_root = if path_mode {
        PathBuf::from(&id.marketplace)
    } else {
        effective_root_for(ctx, id)?
    };
    for target in targets {
        let source = path_mode.then(|| effective_root.join(skill));
        let outcome = link::enable(
            &ctx.state,
            link::EnableRequest {
                skill,
                plugin: &id.to_string(),
                plugin_root: &effective_root,
                target,
                scope,
                mode,
                force,
                source: source.as_deref(),
            },
        )
        .with_context(|| {
            format!(
                "enabling {skill:?} for target {:?} ({})",
                target.id,
                scope.label()
            )
        })?;
        let (created, link_path) = (outcome.created, outcome.link_path.clone());
        ctx.persist_state(outcome.state)?;
        report_enable(skill, &target.id, scope.label(), created, &link_path);
    }
    Ok(())
}

fn report_enable(skill: &str, target_id: &str, scope_label: &str, created: bool, link_path: &Path) {
    if created {
        println!(
            "enabled {skill} -> {} [{target_id}/{scope_label}]",
            link_path.display()
        );
    } else {
        println!("{skill} already enabled [{target_id}/{scope_label}]");
    }
}

/// Recompute the directory an installed plugin's `skills/` hangs off,
/// matching what `store::install`/`store::update` recorded as
/// `InstallOutcome::effective_root` — necessary because a `git-subdir`
/// plugin's skills live one level below `PLUGIN_ROOT`.
fn effective_root_for(ctx: &Context, id: &PluginId) -> Result<PathBuf> {
    let installed = ctx
        .state
        .installed(id)
        .with_context(|| format!("{id} is not installed"))?;
    let plugin_root = ctx
        .layout
        .plugin_root(&id.marketplace, &id.plugin, &installed.version);
    store::effective_plugin_root(&plugin_root, &installed.source)
        .with_context(|| format!("resolving the installed skills location for {id}"))
}

pub fn disable_cmd(
    ctx: &mut Context,
    skill: &str,
    targets: Option<&[String]>,
    user: bool,
    project: bool,
) -> Result<i32> {
    let targets = resolve_targets(&ctx.config, targets)?;
    let scope = resolve_scope(user, project, &ctx.config)?;
    let store_root = ctx.layout.data_dir().to_path_buf();

    let mut refused = false;
    for target in &targets {
        let outcome = link::disable(&ctx.state, skill, target, &scope, &store_root)?;
        refused |= matches!(outcome.action, DisableAction::Refused(_));
        report_disable(skill, target, &scope, &outcome.action);
        ctx.persist_state(outcome.state)?;
    }
    Ok(i32::from(refused))
}

fn report_disable(skill: &str, target: &AgentTarget, scope: &Scope, action: &DisableAction) {
    let location = format!("[{}/{}]", target.id, scope.label());
    match action {
        DisableAction::Absent => println!("{skill} was not enabled {location}"),
        DisableAction::RemovedSymlink | DisableAction::RemovedCopy => {
            println!("disabled {skill} {location}")
        }
        DisableAction::Refused(occupant) => eprintln!(
            "refusing to disable {skill:?} {location}: {occupant} — your files are safe, \
             askm only ever removes what it made itself"
        ),
    }
}

pub fn status_cmd(ctx: &Context) -> Result<()> {
    let report = link::status(&ctx.state, &default_targets(), &all_scopes()?)?;
    let entries = build_status_rows(&ctx.state, &report);
    let shadows = build_shadow_rows(&report);

    if ctx.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&StatusView { entries, shadows })?
        );
        return Ok(());
    }
    print_status(&entries, &shadows);
    Ok(())
}

pub fn doctor_cmd(ctx: &Context) -> Result<i32> {
    let report = link::status(&ctx.state, &default_targets(), &all_scopes()?)?;
    let broken: Vec<&SkillEntry> = report.entries.iter().filter(|e| e.broken).collect();
    let foreign: Vec<&SkillEntry> = report
        .entries
        .iter()
        .filter(|e| is_foreign(e.kind))
        .collect();

    if ctx.json {
        let entries = build_status_rows(&ctx.state, &report);
        let shadows = build_shadow_rows(&report);
        println!(
            "{}",
            serde_json::to_string_pretty(&StatusView { entries, shadows })?
        );
    } else {
        print_doctor_summary(&ctx.state, &report, &broken, &foreign);
    }
    Ok(i32::from(!broken.is_empty()))
}

fn print_doctor_summary(
    state: &State,
    report: &StatusReport,
    broken: &[&SkillEntry],
    foreign: &[&SkillEntry],
) {
    println!(
        "{} broken link(s), {} foreign entry(ies), {} shadow warning(s)",
        broken.len(),
        foreign.len(),
        report.shadows.len()
    );
    for entry in broken {
        let row = status_row(state, entry);
        println!(
            "  broken: {} [{}/{}] -> {}",
            row.skill, row.target, row.scope, row.plugin
        );
    }
    for entry in foreign {
        println!(
            "  foreign: {} [{}/{}] ({})",
            entry.skill,
            entry.target,
            scope_label(&entry.scope),
            kind_label(entry.kind)
        );
    }
    for shadow in &report.shadows {
        println!("  shadow: {} [{}]", shadow.skill, shadow.target);
    }
}

fn is_foreign(kind: EntryKind) -> bool {
    matches!(
        kind,
        EntryKind::ForeignSymlink | EntryKind::ForeignDirectory | EntryKind::ForeignOther
    )
}

#[derive(Debug, Serialize)]
struct StatusRow {
    skill: String,
    target: String,
    scope: String,
    plugin: String,
    state: String,
    broken: bool,
}

#[derive(Debug, Serialize)]
struct ShadowRow {
    skill: String,
    target: String,
    user_link: String,
    project_link: String,
}

#[derive(Debug, Serialize)]
struct StatusView {
    entries: Vec<StatusRow>,
    shadows: Vec<ShadowRow>,
}

fn build_status_rows(state: &State, report: &StatusReport) -> Vec<StatusRow> {
    report
        .entries
        .iter()
        .map(|e| status_row(state, e))
        .collect()
}

fn status_row(state: &State, entry: &SkillEntry) -> StatusRow {
    let plugin = state
        .link_at(&entry.link_path)
        .map(|r| r.plugin.clone())
        .unwrap_or_else(|| "-".to_string());
    StatusRow {
        skill: entry.skill.clone(),
        target: entry.target.clone(),
        scope: scope_label(&entry.scope),
        plugin,
        state: kind_label(entry.kind).to_string(),
        broken: entry.broken,
    }
}

fn build_shadow_rows(report: &StatusReport) -> Vec<ShadowRow> {
    report
        .shadows
        .iter()
        .map(|s| ShadowRow {
            skill: s.skill.clone(),
            target: s.target.clone(),
            user_link: s.user_link.display().to_string(),
            project_link: s.project_link.display().to_string(),
        })
        .collect()
}

fn scope_label(scope: &ScopeRef) -> String {
    match scope {
        ScopeRef::User => "user".to_string(),
        ScopeRef::Project { root } => format!("project:{}", root.display()),
    }
}

fn kind_label(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::ManagedSymlink => "managed (symlink)",
        EntryKind::ManagedCopy => "managed (copy)",
        EntryKind::ForeignSymlink => "foreign (symlink)",
        EntryKind::ForeignDirectory => "foreign (directory)",
        EntryKind::ForeignOther => "foreign (other)",
    }
}

fn print_status(entries: &[StatusRow], shadows: &[ShadowRow]) {
    if entries.is_empty() {
        println!("no skill entries found under any known target/scope");
    } else {
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|r| {
                vec![
                    r.skill.clone(),
                    r.target.clone(),
                    r.scope.clone(),
                    r.plugin.clone(),
                    r.state.clone(),
                    if r.broken {
                        "yes".to_string()
                    } else {
                        "no".to_string()
                    },
                ]
            })
            .collect();
        table::print(
            &["SKILL", "TARGET", "SCOPE", "PLUGIN", "STATE", "BROKEN"],
            &rows,
        );
    }
    for shadow in shadows {
        println!(
            "note: {} [{}] is enabled in both scopes; the project copy at {} shadows the user copy at {}",
            shadow.skill, shadow.target, shadow.project_link, shadow.user_link
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skill_arg_accepts_a_bare_skill_name() {
        let (skill, qualifier) = parse_skill_arg("systematic-debugging").unwrap();

        assert_eq!(skill, "systematic-debugging");
        assert!(qualifier.is_none());
    }

    #[test]
    fn parse_skill_arg_accepts_a_fully_qualified_skill() {
        let (skill, qualifier) = parse_skill_arg("debugging@superpowers@official").unwrap();

        assert_eq!(skill, "debugging");
        assert_eq!(qualifier, Some(PluginId::new("superpowers", "official")));
    }

    #[test]
    fn parse_skill_arg_rejects_a_partially_qualified_name() {
        assert!(parse_skill_arg("debugging@superpowers").is_err());
    }

    #[test]
    fn parse_skill_arg_rejects_an_empty_string() {
        assert!(parse_skill_arg("").is_err());
    }

    #[test]
    fn is_foreign_covers_every_foreign_entry_kind_but_not_managed_ones() {
        assert!(is_foreign(EntryKind::ForeignSymlink));
        assert!(is_foreign(EntryKind::ForeignDirectory));
        assert!(is_foreign(EntryKind::ForeignOther));
        assert!(!is_foreign(EntryKind::ManagedSymlink));
        assert!(!is_foreign(EntryKind::ManagedCopy));
    }
}
