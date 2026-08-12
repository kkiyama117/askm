---
name: askm
description: "Manage agent skills with askm: register marketplaces, install skill plugins, and project skills into agent skills directories (.agents, .claude, .codex, .gemini, .cursor, .opencode). Use when the user asks to install, enable, disable, search, or otherwise manage skills or skill plugins."
---

# askm — agent skills manager

`askm` is `systemctl enable` for Agent Skills: one local store of installed
skill plugins, projected by symlink into whichever agent's skills directory
you point it at. Install a plugin once, then enable individual skills — or a
whole plugin's worth — into one agent or several, per project or for your
user account.

## When to Use

Use this skill whenever the user asks to:
- install or remove a skill plugin (`askm install` / `uninstall` / `update`)
- enable or disable a skill for an agent (`askm enable` / `disable`)
- find a skill to install (`askm search`, `askm list`)
- register a marketplace of plugins (`askm marketplace add`)
- check what is enabled and where (`askm status`, `askm doctor`)
- create their own skill plugin and make it installable

## Quick Start

```bash
askm marketplace add https://github.com/<owner>/<repo>   # git URL or local path
askm marketplace update <name>                           # REQUIRED after add — add only registers
askm search <query>                                      # fuzzy, across every marketplace
askm install <plugin>@<marketplace>
askm enable <skill> --project                             # -> ./.agents/skills/<skill>
askm status                                               # what's enabled, and where
```

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

Global flags: `--store-root <path>` (or `ASKM_ROOT`) relocates the whole
store — use it to test without touching a real home directory. `--json`
emits machine-readable output on `search`, `list`, `status`, `doctor`.

## Key Rules

- **Plugin identity is always `<plugin>@<marketplace>`** — the same plugin
  name can exist in more than one registered marketplace.
- **`enable` takes a skill name** by default: `<skill>`, or
  `<skill>@<plugin>@<marketplace>` to disambiguate. `--all` changes the
  argument to name a whole plugin (`<plugin>@<marketplace>`) and enables
  every skill it has. `disable` only ever takes a bare skill name.
- **`marketplace add` does not sync.** Run `marketplace update <name>`
  before `search`/`install` against a freshly added marketplace, or you get
  `no marketplace.json found under .../cache/marketplaces/<name>`.
- **Targets and scopes:** `--target` takes comma-separated agent ids
  (`agents,claude,codex,gemini,cursor,opencode`); default is `agents`
  (the cross-client convention — skills there are visible to every
  compliant agent). Default scope is user (`~/.agents/skills/`); `--project`
  resolves the nearest ancestor with `.git` and projects into
  `./.agents/skills/`. Project overrides user (shadowing is flagged, not
  conflicting).
- **The safety rule:** askm only removes what it created. `disable` (and
  `enable --force`) refuse — exit non-zero — to touch a directory or
  symlink askm cannot prove it made. That refusal is the feature.
- **`--copy`** materializes a real directory instead of a symlink, for
  clients that don't follow symlinks.

## Creating Your Own Skill Plugin

A plugin is a directory with a `plugin.json` manifest and a `skills/`
directory of `SKILL.md` files, listed in a `marketplace.json`:

```
my-repo/
├── .agents/plugins/marketplace.json     # or .claude-plugin/marketplace.json
└── my-plugin/
    ├── plugin.json
    └── skills/
        └── my-skill/
            └── SKILL.md
```

`plugin.json` (name is required; 1-64 chars, `a-z0-9-.`, no `--` or `..`):

```json
{"name": "my-plugin", "version": "1.0.0", "description": "My first plugin"}
```

`skills/my-skill/SKILL.md` — YAML frontmatter between `---` lines with
`name` and `description` required (name: lowercase letters, digits, hyphens):

```markdown
---
name: my-skill
description: A skill I wrote myself.
---
Body.
```

`.agents/plugins/marketplace.json` — `source` is a relative path (local) or
a git URL object:

```json
{
  "name": "my-marketplace",
  "plugins": [
    {"name": "my-plugin", "source": "./my-plugin", "version": "1.0.0", "description": "My first plugin"}
  ]
}
```

Then: `askm marketplace add <repo-path>` (a local path is symlinked in
place, so the plugin stays editable at its original location) →
`marketplace update` → `install` → `enable`.

## Store Layout

Everything askm owns lives under XDG base directories (or `--store-root`),
never inside an agent's own `.agents`/`.claude` namespace:

```
data/plugins/<marketplace>/<plugin>/<version>/   # PLUGIN_ROOT, one per install
data/plugin-data/<marketplace>/<plugin>/         # PLUGIN_DATA, survives updates
cache/marketplaces/<name>/                       # synced marketplace checkouts
state/state.json                                 # what's installed, what's linked
config/config.json                               # registered marketplaces, defaults
```
