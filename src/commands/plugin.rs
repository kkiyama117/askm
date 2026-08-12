//! `askm install|uninstall|update` (the plugin-level `update`, distinct from
//! `marketplace update`).

use anyhow::{bail, Context as _, Result};
use askm::model::{Marketplace, MarketplaceEntry};
use askm::state::PluginId;
use askm::store;

use crate::commands::context::Context;
use crate::commands::marketplace::load_marketplace;

pub fn install(ctx: &mut Context, raw_id: &str, version: Option<&str>) -> Result<()> {
    let id = PluginId::parse(raw_id)?;
    let marketplace = load_marketplace(ctx, &id.marketplace)?;
    let entry = find_entry(&marketplace, &id.plugin)?;
    check_version(entry, version)?;

    let repo_root = ctx.layout.marketplace_cache(&id.marketplace);
    let outcome = store::install(&ctx.layout, &ctx.state, &id.marketplace, &repo_root, entry)
        .with_context(|| format!("installing {id}"))?;
    ctx.state = outcome.state;

    println!(
        "installed {id} {} ({} skill(s))",
        outcome.installed.version,
        outcome.installed.skills.len()
    );
    Ok(())
}

pub fn uninstall(ctx: &mut Context, raw_id: &str, purge: bool) -> Result<()> {
    let id = PluginId::parse(raw_id)?;
    let next_state = store::uninstall(&ctx.layout, &ctx.state, &id, purge)
        .with_context(|| format!("uninstalling {id}"))?;
    ctx.state = next_state;
    let purged = if purge { " (purged plugin data)" } else { "" };
    println!("uninstalled {id}{purged}");
    Ok(())
}

pub fn update(ctx: &mut Context, raw_id: Option<&str>) -> Result<()> {
    let ids = ids_to_update(ctx, raw_id)?;
    for id in ids {
        update_one(ctx, &id)?;
    }
    Ok(())
}

fn ids_to_update(ctx: &Context, raw_id: Option<&str>) -> Result<Vec<PluginId>> {
    match raw_id {
        Some(raw) => Ok(vec![PluginId::parse(raw)?]),
        None => ctx
            .state
            .installed
            .keys()
            .map(|key| PluginId::parse(key))
            .collect(),
    }
}

fn update_one(ctx: &mut Context, id: &PluginId) -> Result<()> {
    let marketplace = load_marketplace(ctx, &id.marketplace)?;
    let entry = find_entry(&marketplace, &id.plugin)?;
    let repo_root = ctx.layout.marketplace_cache(&id.marketplace);
    let outcome = store::update(&ctx.layout, &ctx.state, &id.marketplace, &repo_root, entry)
        .with_context(|| format!("updating {id}"))?;
    ctx.state = outcome.state;
    report_update(id, &outcome.previous, &outcome.installed.version);
    Ok(())
}

fn report_update(
    id: &PluginId,
    previous: &Option<askm::state::InstalledPlugin>,
    new_version: &str,
) {
    match previous {
        Some(previous) if previous.version != new_version => {
            println!("updated {id}: {} -> {new_version}", previous.version);
        }
        Some(_) => println!("{id} already at {new_version}"),
        None => println!("installed {id} {new_version}"),
    }
}

fn find_entry<'a>(marketplace: &'a Marketplace, plugin: &str) -> Result<&'a MarketplaceEntry> {
    marketplace
        .entries
        .iter()
        .find(|e| e.name == plugin)
        .with_context(|| {
            format!(
                "no plugin named {plugin:?} in marketplace {:?}",
                marketplace.name
            )
        })
}

fn check_version(entry: &MarketplaceEntry, requested: Option<&str>) -> Result<()> {
    let Some(requested) = requested else {
        return Ok(());
    };
    let Some(actual) = &entry.version else {
        bail!("--version {requested:?} was given but the marketplace entry declares no version");
    };
    if versions_match(requested, actual) {
        return Ok(());
    }
    bail!(
        "requested version {requested:?} does not match the marketplace-listed version {actual:?} \
         (askm installs whatever version the marketplace currently lists; it cannot fetch a \
         different version on demand — try `askm marketplace update` first if you expected {actual:?} \
         to have moved)"
    );
}

fn versions_match(requested: &str, actual: &str) -> bool {
    match (
        semver::Version::parse(requested),
        semver::Version::parse(actual),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => requested == actual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_match_compares_semver_values_not_just_strings() {
        assert!(versions_match("1.0.0", "1.0.0"));
        assert!(versions_match("1.0.0", "v1.0.0".trim_start_matches('v')));
    }

    #[test]
    fn versions_match_falls_back_to_string_equality_for_non_semver_versions() {
        assert!(versions_match("release-42", "release-42"));
        assert!(!versions_match("release-42", "release-43"));
    }

    #[test]
    fn find_entry_errors_clearly_when_the_plugin_is_absent() {
        let marketplace = Marketplace {
            name: "official".to_string(),
            display_name: None,
            description: None,
            entries: Vec::new(),
            dialect: askm::model::Dialect::Agents,
        };

        let err = find_entry(&marketplace, "missing-plugin").unwrap_err();

        assert!(err.to_string().contains("missing-plugin"), "{err}");
    }
}
