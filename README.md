# askm — agent skills manager

`askm` is `systemctl enable` for [Agent Skills](https://agentskills.io):
one local store of installed skill plugins, projected by symlink into
whichever agent's skills directory you point it at (`.agents`, `.claude`,
`.codex`, `.gemini`, `.cursor`, `.opencode`). Install a plugin once, then
enable individual skills — or a whole plugin's worth — into one agent or
several, per project or for your user account, without duplicating files.

It follows the [Agent Skills](https://agentskills.io) and
[Agent Plugins 1.0](https://agent-plugins.org) specs: `plugin.json` manifests,
`SKILL.md` frontmatter, and `marketplace.json` listings in either the
`.agents/plugins/` or `.claude-plugin/` dialect.

For the reasoning behind these choices, see [docs/why.md](docs/why.md).

## Store layout

Everything `askm` owns lives under XDG base directories (or a single root via
`--store-root` / `ASKM_ROOT`) — never inside an agent's own `.agents`/`.claude`
namespace, so it can be blown away without touching what's enabled anywhere:

```
data/
  plugins/<marketplace>/<plugin>/<version>/   # PLUGIN_ROOT, one per install
  plugin-data/<marketplace>/<plugin>/         # PLUGIN_DATA, survives updates
cache/marketplaces/<name>/                    # synced marketplace checkouts
state/state.json                              # what's installed, what's linked
config/config.json                            # registered marketplaces, defaults
```

`config.json`, not `config.toml`: no TOML crate is in this project's
dependency set, and `serde_json` already is, so JSON needed no new code and no
new dependency. A missing config file is not an error — it yields the same
defaults as a fresh install (target `agents`, scope `user`).

## Commands

```
askm marketplace add <url-or-path> [--name N]   # git URL or local path, autodetected
askm marketplace list | remove <name> | update [name]

askm search <query> [--limit N]                 # fuzzy search, across all marketplaces
askm install <plugin>@<marketplace> [--version V]
askm uninstall <plugin>@<marketplace> [--purge]
askm update [<plugin>@<marketplace>]             # re-install from the cached listing
askm list [--installed] [--enabled]

askm enable <skill> [--target ids] [--user|--project] [--all] [--copy] [--force]
askm disable <skill> [--target ids] [--user|--project]
askm status
askm doctor
```

Global flags: `--store-root <path>` (or `ASKM_ROOT`) points the whole store
somewhere other than the OS default — this is what makes the tool testable
without touching a real home directory. `--json` gives machine-readable output
on the read-only commands (`search`, `list`, `status`, and `doctor`'s report).

Plugin identity is always `<plugin>@<marketplace>`, since the same plugin name
can appear in more than one registered marketplace. `enable`'s argument is a
skill by default (`<skill>`, or `<skill>@<plugin>@<marketplace>` if that name
is installed by more than one plugin); `--all` repurposes it to name a whole
plugin instead (`<plugin>@<marketplace>`), enabling every skill it has.
`disable` only ever takes a bare skill name — disabling acts on whatever
currently occupies a target's skills directory, not on a specific plugin's
record of it. `--target` takes a comma-separated list of agent ids and
defaults to the config's target list (`agents` out of the box, since that's
the cross-client convention every compliant agent scans). `--project` resolves
the project root by walking up from the current directory to the nearest
`.git`, falling back to the current directory itself.

## The safety rule

`askm` only ever removes what it created. Every skill it projects is recorded
in `state.json`; `disable` deletes a symlink or a `--copy`-made directory only
when that record proves `askm` made it — or, as a recovery path for a lost
state file, when a symlink's target still resolves inside `askm`'s own store.
Anything else sharing a skills directory — a hand-made directory, a symlink
you created pointing elsewhere — is left exactly alone. `enable --force` can
repoint a symlink `askm` itself manages, but that same rule still applies: it
cannot be used to delete real data `askm` cannot prove it created. When
`disable` (or a forced `enable`) meets one of these foreign entries, it prints
what's in the way and why, and exits non-zero — that refusal is the feature
working, not a failure.

`status` and `doctor` report every entry `askm` can see across all known
targets and both scopes (project overrides user, so a skill enabled in both is
flagged as shadowed, not conflicting), classifying each as managed, foreign,
or broken. `doctor` exits non-zero if anything managed is actually broken.

## Development

```
cargo test                              # unit + integration tests
cargo clippy --all-targets -- -D warnings
cargo fmt
```

Integration tests in `tests/cli_*.rs` drive the built binary and are
hermetic: every invocation gets its own `--store-root` and `HOME` (so
`Scope::User` resolution never touches a real home directory either) via
tempdirs in `tests/common/mod.rs`.
