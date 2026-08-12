//! Command-line surface for `askm`, defined with `clap`'s derive API.
//!
//! Argument *grammar* notes that don't fit neatly into a single `--help` line:
//!
//! - Plugin identity is always `<plugin>@<marketplace>` (see
//!   `askm::state::PluginId`) — required because the same plugin name can be
//!   listed by more than one registered marketplace.
//! - `enable`'s positional argument names *a skill* by default: either
//!   `<skill>`, or `<skill>@<plugin>@<marketplace>` to disambiguate a skill
//!   name installed by more than one plugin. Passing `--all` switches its
//!   meaning entirely: it then names *a whole plugin* as `<plugin>@<marketplace>`,
//!   and every skill that plugin has is enabled.
//! - `disable` only ever takes a bare skill name. Disabling acts on whatever
//!   currently occupies a target's skills directory rather than resolving a
//!   source plugin, so it never needs a plugin qualifier.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "askm",
    version,
    about = "Agent skills manager: install skill plugins and project them into agent skills directories."
)]
pub struct Cli {
    /// Root directory for askm's own store (plugins, marketplace cache, state,
    /// config). Overrides the OS-standard XDG locations this build would
    /// otherwise use. Also settable via ASKM_ROOT; the flag wins if both are
    /// given. This is what makes askm testable without touching a real home
    /// directory.
    #[arg(long, global = true, env = "ASKM_ROOT", value_name = "PATH")]
    pub store_root: Option<PathBuf>,

    /// Emit machine-readable JSON instead of a human-readable table. Applies
    /// only to read-only commands: search, list, status.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manage registered marketplaces.
    Marketplace {
        #[command(subcommand)]
        action: MarketplaceCommand,
    },
    /// Fuzzy-search plugin and skill names, descriptions, and keywords across
    /// every registered marketplace.
    Search {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Install a plugin from a registered marketplace.
    Install {
        /// `<plugin>@<marketplace>`.
        plugin: String,
        /// Assert the marketplace-listed version matches this before
        /// installing. askm always installs whatever version the marketplace
        /// currently lists — this flag only makes a script fail loudly if
        /// that version has moved out from under it; it cannot fetch a
        /// different version on demand.
        #[arg(long)]
        version: Option<String>,
    },
    /// Remove an installed plugin.
    Uninstall {
        /// `<plugin>@<marketplace>`.
        plugin: String,
        /// Also delete the plugin's persistent PLUGIN_DATA directory. Without
        /// this, PLUGIN_DATA survives so a later reinstall keeps it.
        #[arg(long)]
        purge: bool,
    },
    /// Re-install one (or, if omitted, every) installed plugin from its
    /// marketplace's currently-cached listing. Run `marketplace update`
    /// first to fetch a newer listing — this command does not sync on its own.
    Update {
        /// `<plugin>@<marketplace>`. Omit to update everything installed.
        plugin: Option<String>,
    },
    /// List plugins. With no flags, lists every plugin available across
    /// registered marketplaces.
    List {
        /// Only plugins that are installed.
        #[arg(long)]
        installed: bool,
        /// Only skills that are currently enabled somewhere. Takes
        /// precedence over `--installed` if both are given.
        #[arg(long)]
        enabled: bool,
    },
    /// Project a skill from an installed plugin into one or more agents'
    /// skills directories — the `systemctl enable` of this tool.
    Enable {
        /// `<skill>`, or `<skill>@<plugin>@<marketplace>` to disambiguate a
        /// skill name installed by more than one plugin. With `--all`, this
        /// instead names the whole plugin as `<plugin>@<marketplace>`.
        /// Omitted only with `--all --path`, which enables every skill in the
        /// directory instead.
        spec: Option<String>,
        /// Enable every skill the named plugin has. Changes `spec`'s
        /// grammar — see above.
        #[arg(long)]
        all: bool,
        /// Project from an arbitrary skills directory instead of an installed
        /// plugin: `<dir>/<skill>` is linked directly, and with `--all` every
        /// immediate child of `<dir>` containing a `SKILL.md` is enabled.
        /// The link is recorded as plugin `path@<dir>`.
        #[arg(long, value_name = "DIR")]
        path: Option<PathBuf>,
        /// Comma-separated agent target ids (e.g. `agents,claude`). Defaults
        /// to the config's default target list.
        #[arg(long = "target", value_delimiter = ',', value_name = "IDS")]
        targets: Option<Vec<String>>,
        /// Enable in the user scope (default).
        #[arg(long, conflicts_with = "project")]
        user: bool,
        /// Enable in the project scope, resolved from the current directory:
        /// the nearest ancestor containing a `.git` directory, or the current
        /// directory itself if none is found.
        #[arg(long, conflicts_with = "user")]
        project: bool,
        /// Materialize a real copy instead of a symlink, for clients that
        /// don't follow symlinks.
        #[arg(long)]
        copy: bool,
        /// Replace whatever askm itself previously projected at this path.
        /// Never overrides the safety rule: a directory or symlink askm did
        /// not create is refused regardless of this flag.
        #[arg(long)]
        force: bool,
    },
    /// Remove a skill's projection from a target's skills directory. Refuses,
    /// rather than deletes, anything askm cannot prove it created itself.
    Disable {
        /// Bare skill name.
        skill: String,
        #[arg(long = "target", value_delimiter = ',', value_name = "IDS")]
        targets: Option<Vec<String>>,
        #[arg(long, conflicts_with = "project")]
        user: bool,
        #[arg(long, conflicts_with = "user")]
        project: bool,
    },
    /// Show every skill entry askm can see across all known targets and both
    /// scopes, and any project/user shadowing between them.
    Status,
    /// Check the store for problems: broken links, foreign entries sharing a
    /// skills directory, and project/user shadowing. Exits non-zero if any
    /// askm-managed link is broken.
    Doctor,
}

#[derive(Debug, Subcommand)]
pub enum MarketplaceCommand {
    /// Register a marketplace from a git URL or a local directory
    /// (autodetected: an existing directory is treated as local, anything
    /// else must look like a git URL).
    Add {
        source: String,
        /// Name to register under. Defaults to the repository or directory's
        /// own name.
        #[arg(long)]
        name: Option<String>,
    },
    /// List registered marketplaces.
    List,
    /// Un-register a marketplace and delete its local cache.
    Remove { name: String },
    /// Re-sync one (or, if omitted, every) registered marketplace's listing.
    Update { name: Option<String> },
}
