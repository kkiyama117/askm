//! `askm search` and `askm list`: read-only views over registered
//! marketplaces, the installed set, and currently-enabled skills.

use anyhow::Result;
use askm::model::MarketplaceEntry;
use askm::search::{self, SearchResult};
use askm::state::InstalledPlugin;
use serde::Serialize;

use crate::commands::context::Context;
use crate::commands::marketplace::load_all_marketplaces;
use crate::commands::table;

/// Skill names to show per row before collapsing the rest into a count. Large
/// plugins ship hundreds of skills; `--json` still carries the full list.
const SKILL_PREVIEW: usize = 6;

pub fn search_cmd(ctx: &Context, query: &str, limit: usize) -> Result<()> {
    let marketplaces = load_all_marketplaces(ctx)?;
    let results = search::search(&marketplaces, &ctx.state, query, limit);
    let rows: Vec<SearchResultRow> = results.iter().map(SearchResultRow::from).collect();

    if ctx.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no matches for {query:?}");
        return Ok(());
    }
    print_search_rows(&rows);
    Ok(())
}

pub fn list_cmd(ctx: &Context, installed: bool, enabled: bool) -> Result<()> {
    if enabled {
        return list_enabled(ctx);
    }
    if installed {
        return list_installed(ctx);
    }
    list_available(ctx)
}

fn list_available(ctx: &Context) -> Result<()> {
    let marketplaces = load_all_marketplaces(ctx)?;
    let mut rows: Vec<AvailableRow> = marketplaces
        .iter()
        .flat_map(|(name, marketplace)| {
            marketplace
                .entries
                .iter()
                .map(move |entry| AvailableRow::new(name, entry))
        })
        .collect();
    rows.sort_by(|a, b| {
        a.plugin
            .cmp(&b.plugin)
            .then_with(|| a.marketplace.cmp(&b.marketplace))
    });

    if ctx.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no plugins available; register a marketplace with `askm marketplace add`");
        return Ok(());
    }
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.plugin.clone(),
                r.marketplace.clone(),
                r.version.clone().unwrap_or_else(|| "-".to_string()),
                r.description.clone().unwrap_or_default(),
            ]
        })
        .collect();
    table::print(
        &["PLUGIN", "MARKETPLACE", "VERSION", "DESCRIPTION"],
        &table_rows,
    );
    Ok(())
}

fn list_installed(ctx: &Context) -> Result<()> {
    let mut rows: Vec<&InstalledPlugin> = ctx.state.installed.values().collect();
    rows.sort_by(|a, b| {
        a.plugin
            .cmp(&b.plugin)
            .then_with(|| a.marketplace.cmp(&b.marketplace))
    });

    if ctx.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no plugins installed");
        return Ok(());
    }
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|p| {
            vec![
                p.plugin.clone(),
                p.marketplace.clone(),
                p.version.clone(),
                table::summarize_list(&p.skills, SKILL_PREVIEW),
            ]
        })
        .collect();
    table::print(&["PLUGIN", "MARKETPLACE", "VERSION", "SKILLS"], &table_rows);
    Ok(())
}

/// Currently-enabled skills, read directly from recorded state (not a live
/// filesystem scan — `askm status` is the command for that).
fn list_enabled(ctx: &Context) -> Result<()> {
    let mut rows: Vec<_> = ctx.state.links.iter().collect();
    rows.sort_by(|a, b| a.skill.cmp(&b.skill).then_with(|| a.target.cmp(&b.target)));

    if ctx.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no skills enabled");
        return Ok(());
    }
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.skill.clone(),
                r.plugin.clone(),
                r.target.clone(),
                scope_ref_label(&r.scope),
            ]
        })
        .collect();
    table::print(&["SKILL", "PLUGIN", "TARGET", "SCOPE"], &table_rows);
    Ok(())
}

fn scope_ref_label(scope: &askm::state::ScopeRef) -> String {
    match scope {
        askm::state::ScopeRef::User => "user".to_string(),
        askm::state::ScopeRef::Project { root } => format!("project:{}", root.display()),
    }
}

#[derive(Debug, Serialize)]
struct AvailableRow {
    plugin: String,
    marketplace: String,
    version: Option<String>,
    description: Option<String>,
}

impl AvailableRow {
    fn new(marketplace: &str, entry: &MarketplaceEntry) -> Self {
        Self {
            plugin: entry.name.clone(),
            marketplace: marketplace.to_string(),
            version: entry.version.clone(),
            description: entry.description.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
struct SearchResultRow {
    plugin: String,
    marketplace: String,
    version: Option<String>,
    description: Option<String>,
    matching_skills: Vec<String>,
    score: u32,
}

impl From<&SearchResult> for SearchResultRow {
    fn from(r: &SearchResult) -> Self {
        Self {
            plugin: r.id.plugin.clone(),
            marketplace: r.id.marketplace.clone(),
            version: r.version.clone(),
            description: r.description.clone(),
            matching_skills: r.matching_skills.clone(),
            score: r.score,
        }
    }
}

fn print_search_rows(rows: &[SearchResultRow]) {
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let skills = if r.matching_skills.is_empty() {
                "-".to_string()
            } else {
                table::summarize_list(&r.matching_skills, SKILL_PREVIEW)
            };
            vec![
                r.plugin.clone(),
                r.marketplace.clone(),
                r.version.clone().unwrap_or_else(|| "-".to_string()),
                skills,
                r.description.clone().unwrap_or_default(),
            ]
        })
        .collect();
    table::print(
        &[
            "PLUGIN",
            "MARKETPLACE",
            "VERSION",
            "MATCHING SKILLS",
            "DESCRIPTION",
        ],
        &table_rows,
    );
}
