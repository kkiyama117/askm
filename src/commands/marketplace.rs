//! `askm marketplace add|list|remove|update`.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};
use askm::manifest::marketplace as marketplace_manifest;
use askm::model::{EntrySource, Marketplace};
use askm::source::{GitSource, LocalSource, MarketplaceRegistry, Source};

use crate::commands::context::Context;

pub fn add(ctx: &mut Context, source_arg: &str, name: Option<&str>) -> Result<()> {
    let (source, entry_source, derived_name) = detect_source(source_arg)?;
    let name = name.map(str::to_string).unwrap_or(derived_name);

    let registry = MarketplaceRegistry::new(&ctx.layout);
    let (marketplace, warnings, outcome) = registry
        .add(&name, source.as_ref())
        .with_context(|| format!("adding marketplace {name:?}"))?;

    let next_config = ctx.config.with_marketplace(&name, entry_source);
    ctx.persist_config(next_config)?;

    let revision = outcome
        .revision
        .map(|r| format!(", at {r}"))
        .unwrap_or_default();
    println!(
        "added marketplace {name:?} ({} plugin(s)){revision}",
        marketplace.entries.len()
    );
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    Ok(())
}

pub fn list(ctx: &Context) -> Result<()> {
    if ctx.json {
        let names: Vec<&String> = ctx.config.marketplaces.keys().collect();
        println!("{}", serde_json::to_string_pretty(&names)?);
        return Ok(());
    }
    if ctx.config.marketplaces.is_empty() {
        println!("no marketplaces registered; add one with `askm marketplace add <url-or-path>`");
        return Ok(());
    }
    for (name, source) in &ctx.config.marketplaces {
        println!("{name}  {}", describe_source(source));
    }
    Ok(())
}

pub fn remove(ctx: &mut Context, name: &str) -> Result<()> {
    if !ctx.config.marketplaces.contains_key(name) {
        bail!("no marketplace named {name:?} is registered");
    }
    MarketplaceRegistry::new(&ctx.layout).remove(name)?;
    let next_config = ctx.config.without_marketplace(name);
    ctx.persist_config(next_config)?;
    println!("removed marketplace {name:?}");
    Ok(())
}

pub fn update(ctx: &Context, name: Option<&str>) -> Result<()> {
    let names: Vec<String> = match name {
        Some(n) => {
            if !ctx.config.marketplaces.contains_key(n) {
                bail!("no marketplace named {n:?} is registered");
            }
            vec![n.to_string()]
        }
        None => ctx.config.marketplaces.keys().cloned().collect(),
    };

    let registry = MarketplaceRegistry::new(&ctx.layout);
    for name in names {
        let entry_source = &ctx.config.marketplaces[&name];
        let source = source_from_entry(entry_source);
        let (marketplace, _warnings, outcome) = registry
            .update(&name, source.as_ref())
            .with_context(|| format!("updating marketplace {name:?}"))?;
        let changed = if outcome.changed {
            "updated"
        } else {
            "already up to date"
        };
        println!(
            "{name}: {changed} ({} plugin(s))",
            marketplace.entries.len()
        );
    }
    Ok(())
}

/// Load a registered marketplace's currently-cached listing.
pub fn load_marketplace(ctx: &Context, name: &str) -> Result<Marketplace> {
    let (marketplace, _warnings) = marketplace_manifest::load_from_repo(
        &ctx.layout.marketplace_cache(name),
    )
    .with_context(|| {
        format!("loading cached marketplace {name:?} (try `askm marketplace update {name}`)")
    })?;
    Ok(marketplace)
}

/// Load every registered marketplace's currently-cached listing, paired with
/// its registered name.
pub fn load_all_marketplaces(ctx: &Context) -> Result<Vec<(String, Marketplace)>> {
    ctx.config
        .marketplaces
        .keys()
        .map(|name| load_marketplace(ctx, name).map(|m| (name.clone(), m)))
        .collect()
}

fn detect_source(raw: &str) -> Result<(Box<dyn Source>, EntrySource, String)> {
    let path = Path::new(raw);
    if path.is_dir() {
        let canonical = path
            .canonicalize()
            .with_context(|| format!("resolving {raw:?}"))?;
        let name = derive_name_from_path(&canonical)?;
        let entry_source = EntrySource::Local {
            path: canonical.to_string_lossy().into_owned(),
        };
        return Ok((Box::new(LocalSource::new(canonical)), entry_source, name));
    }
    if looks_like_git_url(raw) {
        let name = derive_name_from_url(raw)?;
        let entry_source = EntrySource::Git {
            url: raw.to_string(),
            reference: None,
            sha: None,
            subpath: None,
        };
        return Ok((
            Box::new(GitSource::new(raw.to_string(), None, None)),
            entry_source,
            name,
        ));
    }
    bail!("could not tell whether {raw:?} is a local path or a git URL; pass an existing directory or a URL");
}

fn looks_like_git_url(raw: &str) -> bool {
    raw.starts_with("http://")
        || raw.starts_with("https://")
        || raw.starts_with("git@")
        || raw.starts_with("ssh://")
        || raw.ends_with(".git")
}

fn derive_name_from_path(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .with_context(|| format!("cannot derive a marketplace name from {}", path.display()))
}

fn derive_name_from_url(url: &str) -> Result<String> {
    let trimmed = url.trim_end_matches('/').trim_end_matches(".git");
    trimmed
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .with_context(|| format!("cannot derive a marketplace name from {url:?}"))
}

fn source_from_entry(entry: &EntrySource) -> Box<dyn Source> {
    match entry {
        EntrySource::Local { path } => Box::new(LocalSource::new(PathBuf::from(path))),
        EntrySource::Git {
            url,
            reference,
            sha,
            ..
        } => Box::new(GitSource::new(url.clone(), reference.clone(), sha.clone())),
    }
}

fn describe_source(entry: &EntrySource) -> String {
    match entry {
        EntrySource::Local { path } => format!("local:{path}"),
        EntrySource::Git { url, .. } => format!("git:{url}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_name_from_url_strips_the_dot_git_suffix() {
        let name =
            derive_name_from_url("https://github.com/obra/superpowers-marketplace.git").unwrap();

        assert_eq!(name, "superpowers-marketplace");
    }

    #[test]
    fn derive_name_from_url_handles_a_trailing_slash() {
        let name =
            derive_name_from_url("https://github.com/obra/superpowers-marketplace/").unwrap();

        assert_eq!(name, "superpowers-marketplace");
    }

    #[test]
    fn looks_like_git_url_recognizes_common_forms() {
        assert!(looks_like_git_url("https://github.com/x/y.git"));
        assert!(looks_like_git_url("git@github.com:x/y.git"));
        assert!(!looks_like_git_url("./local/path"));
    }

    #[test]
    fn detect_source_rejects_input_that_is_neither_a_path_nor_a_url() {
        let result = detect_source("not-a-real-path-or-url");

        assert!(result.is_err());
    }
}
