# Why askm is built this way

This document records the reasoning behind `askm`'s design. The README says
what the tool does; this says why it does it that way, including the decisions
that could reasonably have gone differently and the ones that are load-bearing.

## The problem

Agent skills are managed separately by every client. Claude Code keeps its own
plugin cache, Codex and Gemini and Cursor each scan their own directories, and
the same skill ends up installed several times with no single answer to "which
of these is actually active?"

The state this tool was written against was concretely: seven marketplaces
cloned under one client's cache, alongside a hand-maintained skills directory
holding a mix of real directories and hand-made symlinks pointing at unrelated
checkouts. Nothing recorded where any of it came from.

## The core idea: one store, many projections

`askm` copies the `systemctl enable` model. A unit file lives once in the
system; enabling it creates a symlink in a `.wants/` directory. Disabling
removes the link, not the unit.

Here, a skill lives once in `askm`'s store, and enabling it symlinks it into an
agent's skills directory. The consequences that make this the right model:

- **No duplication.** One copy on disk regardless of how many agents use it.
- **Enabling is cheap and reversible.** It creates or removes a link; it never
  re-downloads or deletes plugin content.
- **The filesystem stays the source of truth for agents.** Every client already
  scans its skills directory. `askm` needs no cooperation from any of them —
  it writes links they already know how to read.

This was validated before it was built: the hand-made symlinks in the target
environment were already being picked up by the agents reading them, which is
direct evidence that symlink projection works against real clients.

## Why the store lives entirely under XDG

The store could have gone in `~/.agents/plugins/`. The Agent Plugins 1.0 spec
even uses that path in its own example (`PLUGIN_ROOT=/home/alex/.agents/plugins/devtools`),
so it would have been defensible, and it would have let other clients discover
`askm`-installed plugins directly.

It went under XDG instead — `~/.local/share/askm/`, `~/.cache/askm/`,
`~/.local/state/askm/`, `~/.config/askm/` — because the whole point of the tool
is to *absorb* the differences between clients rather than to become another
client competing for the shared namespace. `~/.agents/` is a space several
tools read and write; putting `askm`'s bookkeeping there would make `askm` one
more source of the exact confusion it exists to remove.

The practical payoff: the only thing `askm` ever writes into an agent's
namespace is the skill links themselves. The cache can be deleted without
losing what is enabled; the store can be relocated wholesale with
`--store-root`.

That flag is not a testing afterthought. Because `askm` owns its own root, the
entire tool can be pointed at a scratch directory, which is what makes the test
suite hermetic — no test needs the real home directory, so no test can damage
it.

## Why per-skill granularity

Plugins are large. The ECC plugin used in development ships 282 skills. Every
skill's name and description is loaded into an agent's context at startup, so
enabling a plugin wholesale is not free — it is a real context cost paid on
every session.

Per-skill enabling keeps that cost proportional to what is actually wanted.
`--all` exists for the case where a whole plugin is genuinely desired.

## The spec landscape, and what it does not cover

Two external specs apply, and the distinction between them matters because one
of them leaves a hole that `askm` has to fill.

**Agent Skills** ([agentskills.io](https://agentskills.io)) defines what is
*inside* a skill: `SKILL.md` frontmatter (`name`, `description`, plus optional
`license`, `compatibility`, `metadata`, `allowed-tools`) and the optional
`scripts/`, `references/`, `assets/` directories.

It deliberately does **not** mandate where skill directories live. `.agents/skills/`
is a widely-adopted convention documented in the client-implementation guide,
not a requirement. Two conventions from that guide are followed here: skills are
directories containing a `SKILL.md` at depth 1, and **project-level skills
override user-level ones** — which is why `status` reports shadowing rather
than treating it as an error.

The same guide prescribes *lenient* parsing: a skill whose `name` disagrees with
its directory, or violates the character set, should warn and load anyway. The
parsers follow this. Only a missing `name`/`description` or unparseable
frontmatter is fatal.

**Agent Plugins 1.0** ([agent-plugins.org](https://agent-plugins.org)) defines
the package around skills: a closed-schema `plugin.json`, skills discovered at
`skills/` **depth 1 only** (recursion is forbidden), path containment (nothing a
plugin exposes may resolve outside its root), and `PLUGIN_DATA` that must
survive updates. All four are enforced.

**Neither spec standardizes marketplaces.** Registries are listed in Agent
Plugins' `FUTURE_CONSIDERATIONS.md`, not in the spec. That is the hole, and it
is why the next section exists.

## Why marketplace parsing is so defensive

Because marketplaces are unstandardized, real-world data is inconsistent, and
`askm` has to normalize it rather than pick a winner.

Two dialects exist side by side in the wild:

| | `.agents/plugins/marketplace.json` | `.claude-plugin/marketplace.json` |
|---|---|---|
| display name | `interface.displayName` | — |
| description | often absent | usually present |
| source | object | string *or* object |

And a plugin's `source` appears in **five** shapes, all of which had to collapse
into one internal type:

```
"./"                                                             → Local
"./plugins/agent-sdk-dev"                                        → Local
{"source":"local","path":"./"}                                   → Local
{"source":"url","url":"…git","path":"revenuecat","sha":"…"}      → Git
{"source":"git-subdir","url":"…","path":"…","ref":"v1.5.5",…}    → Git
```

`url` and `git-subdir` carry the same fields and are treated identically; the
two tag names appear to be dialect artifacts rather than a real distinction.

When both dialect files exist, `.agents` wins on identity and `source` — but
only there. An early version preferred `.agents` wholesale, which silently
discarded the descriptions and keywords that only the `.claude-plugin` file
carries, leaving `askm search` with nothing to match on for real plugins. The
current behavior backfills the empty descriptive fields from the secondary file.

One more compatibility wrinkle: some marketplace entries enumerate their skills
explicitly (`"skills": ["./skills/box", …]`) rather than relying on the fixed
`skills/` location. That contradicts Agent Plugins 1.0's fixed discovery, so it
is honored as a compatibility path — and still containment-checked.

## The safety rule

This is the one invariant the whole design rests on, so it is stated in the
strongest terms the code allows.

A real skills directory is a shared space. It contains entries `askm` created,
next to real directories a user maintains by hand, next to symlinks pointing at
unrelated checkouts. `disable` operating on a *path* rather than on *proof of
ownership* would eventually delete someone's work.

So an entry is removed only when it is a symlink **and** either:

- `state.json` records that `askm` created a link at exactly that path, or
- it has no record, but its target resolves inside the `askm` store (a recovery
  path for a lost or hand-edited state file).

A `--copy`-mode entry is a real directory rather than a symlink, and is removed
only when a state record proves `askm` created it. Everything else is refused,
and `--force` does not override this — force may replace links, never real data.

Two consequences worth knowing:

- **Refusal is reported as a feature, not a failure.** The message says the
  user's files are safe. It exits non-zero so scripts notice, but the wording is
  aimed at a human reading it.
- **Local-source plugins depend on the state record.** Their store path is a
  symlink to an external directory, so links to their skills resolve *outside*
  the store and cannot satisfy the second condition. If `state.json` is lost,
  `askm` will refuse to remove its own links to them. That degrades toward
  refusing-too-much rather than deleting-too-much, which is the correct
  direction to fail.

## Smaller decisions

**Git by subprocess, not libgit2.** `git` is already a hard dependency of a tool
that clones marketplaces. Linking libgit2 would add build complexity and binary
weight to duplicate something already installed. Values taken from marketplace
data (`url`, `ref`, `sha`) are rejected if they begin with `-`, so untrusted
listing data cannot smuggle in a flag.

**JSON config, not TOML.** No TOML crate is in the dependency set, and
`serde_json` already is. A hand-rolled TOML subset parser would be new code to
maintain for a cosmetic gain.

**Atomic writes, no file locking.** `state.json` is written to a temporary file
and renamed, so an interrupted write cannot corrupt it. Locking against
concurrent `askm` processes was considered and dropped: it needs a held handle
across the read-modify-write cycle, and this is a single-user CLI. The rename
keeps the file always-valid; if concurrent invocations become real, locking can
be added behind `State::save`.

**Version directories side by side.** Installing a new version does not remove
the old one, so a bad update can be rolled back, and different projects can pin
different versions.

**Unix only for v1.** Symlink creation on Windows requires elevated privileges
or developer mode. Rather than silently misbehaving, the link module fails to
compile off Unix. `--copy` mode is the intended path to Windows support.

## Known limitations

- **No network catalog fetching.** No HTTP client is in the dependency set, so
  catalogs load from a file or from bytes already in memory. Adding network
  support means handing a response body to `Catalog::from_reader`; the module
  needs no other change.
- **`--copy` does not auto-refresh.** A copied skill is a point-in-time snapshot
  and is only refreshed by re-enabling.
- **`update` does not sync marketplaces.** `marketplace update` and `update` are
  deliberately separate steps, so refreshing a listing and changing what is
  installed are distinct, composable decisions.
