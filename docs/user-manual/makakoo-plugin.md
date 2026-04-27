# `makakoo plugin` — CLI reference

Plugins are the extension unit of Makakoo OS. Each plugin is a directory
with a `plugin.toml` manifest and an entrypoint script. Plugins can
contribute SANCHO tasks, MCP tools, SKILL.md skill fragments, agent
definitions, and infect fragments that get embedded into CLI hosts. The
plugin registry lives at `$MAKAKOO_HOME/plugins/`; the lock file is
`$MAKAKOO_HOME/plugins.lock`.

Install sources: local path, `git+<url>[@<ref>]` (pinned to a semver tag
or 40-char SHA by default), `https://` tarball, or `--core` (resolves
against the shipped `plugins-core/` tree).

## Subcommand overview

| Subcommand | Purpose |
|---|---|
| `plugin list [--json]` | All installed plugins with version + blake3 hash. |
| `plugin info <name>` | Parsed manifest + lock entry for one plugin. |
| `plugin install <source> [--core] [--blake3 H] [--sha256 H] [--allow-unstable-ref]` | Install a plugin. |
| `plugin uninstall <name> [--purge]` | Remove. `--purge` also wipes the state dir. |
| `plugin enable <name>` | Re-enable a soft-disabled plugin. |
| `plugin disable <name>` | Soft-disable without removing. |
| `plugin update <name>` | Re-fetch and reinstall from the recorded source. |
| `plugin outdated` | List plugins whose upstream ref has drifted (dry-run, no writes). |
| `plugin start <name>` | Start a service-kind or agent-kind plugin. |
| `plugin stop <name>` | Stop a service-kind or agent-kind plugin. |
| `plugin restart <name>` | Stop then start. |
| `plugin status <name>` | Probe a service/agent plugin's health endpoint. |
| `plugin sync` | Batch-reinstall every plugin from `plugins-core/` into the live tree (use after a bulk source migration). |

## Key use patterns

### Install a plugin from git

```sh
# pin to a semver tag (required by default; use --allow-unstable-ref for branches)
makakoo plugin install git+https://github.com/acme/skill-research-arxiv@v1.2.0

# install from plugins-core/ (core-shipped plugins)
makakoo plugin install skill-brain-multi-source --core

# verify it is live
makakoo plugin info skill-brain-multi-source
```

### Soft-disable a plugin without losing it

```sh
# disable (SANCHO tasks, MCP tools, and infect fragments are skipped on next load)
makakoo plugin disable mascot-gym

# re-enable later
makakoo plugin enable mascot-gym

# a daemon restart is needed to pick up the change
makakoo daemon uninstall && makakoo daemon install
```

## Plugin kinds

| Kind | Description |
|---|---|
| `skill` | Exposes one or more `SKILL.md` fragments to infected CLIs. |
| `agent` | A service with `start` / `stop` / `health` lifecycle commands. |
| `lib` | Python helper library with no SANCHO tasks or MCP tools. |
| `wiki` | Documentation-only; contributes no runtime behaviour. |

## Related commands

- [`makakoo-distro.md`](makakoo-distro.md) — install curated plugin bundles
- [`makakoo-sancho.md`](makakoo-sancho.md) — SANCHO tasks registered by plugins
- [`makakoo-daemon.md`](makakoo-daemon.md) — daemon must restart to pick up plugin changes
- [`../plugins/writing.md`](../plugins/writing.md) — how to author a plugin
- [`../plugins/index.md`](../plugins/) — the plugin ecosystem catalog

## Common gotcha

**`plugin install` refuses a git ref like `main` with "unstable ref".**
By default, `git+<url>@<ref>` requires the ref to be a semver tag or a
40-character SHA. Branch names are rejected to prevent silent drift when the
branch moves. Pass `--allow-unstable-ref` to override for development
workflows, but pin to a tag for anything you want reproduced reliably.
