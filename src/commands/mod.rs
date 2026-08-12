//! Dispatch from parsed CLI arguments to command implementations. Kept thin:
//! each command's real logic lives in its own module, organized by domain
//! (marketplace registration, plugin installs, skill projection, discovery).

mod context;
mod discover;
mod marketplace;
mod plugin;
mod skills;
mod table;

use anyhow::Result;

use crate::cli::{Cli, Command, MarketplaceCommand};
use context::Context;

/// Run the parsed CLI. Returns the process exit code: `0` on ordinary
/// success, non-zero when `doctor` finds a broken link or `disable` refuses
/// to touch something it did not create — both are successful *runs* of the
/// command, but the caller (a script, most likely) should still be able to
/// tell something needs attention.
pub fn run(cli: Cli) -> Result<i32> {
    let mut ctx = Context::load(cli.store_root.as_deref(), cli.json)?;

    match cli.command {
        Command::Marketplace { action } => run_marketplace(&mut ctx, action).map(|()| 0),
        Command::Search { query, limit } => discover::search_cmd(&ctx, &query, limit).map(|()| 0),
        Command::Install { plugin, version } => {
            plugin::install(&mut ctx, &plugin, version.as_deref()).map(|()| 0)
        }
        Command::Uninstall { plugin, purge } => {
            plugin::uninstall(&mut ctx, &plugin, purge).map(|()| 0)
        }
        Command::Update { plugin } => plugin::update(&mut ctx, plugin.as_deref()).map(|()| 0),
        Command::List { installed, enabled } => {
            discover::list_cmd(&ctx, installed, enabled).map(|()| 0)
        }
        Command::Enable {
            spec,
            all,
            path,
            targets,
            user,
            project,
            copy,
            force,
        } => skills::enable_cmd(
            &mut ctx,
            skills::EnableArgs {
                spec: spec.unwrap_or_default(),
                all,
                path,
                targets,
                user,
                project,
                copy,
                force,
            },
        )
        .map(|()| 0),
        Command::Disable {
            skill,
            targets,
            user,
            project,
        } => skills::disable_cmd(&mut ctx, &skill, targets.as_deref(), user, project),
        Command::Status => skills::status_cmd(&ctx).map(|()| 0),
        Command::Doctor => skills::doctor_cmd(&ctx),
    }
}

fn run_marketplace(ctx: &mut Context, action: MarketplaceCommand) -> Result<()> {
    match action {
        MarketplaceCommand::Add { source, name } => marketplace::add(ctx, &source, name.as_deref()),
        MarketplaceCommand::List => marketplace::list(ctx),
        MarketplaceCommand::Remove { name } => marketplace::remove(ctx, &name),
        MarketplaceCommand::Update { name } => marketplace::update(ctx, name.as_deref()),
    }
}
