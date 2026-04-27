---
name: agents
version: 0.1.0
description: |
  Meta-lifecycle tools for the Makakoo agent scaffold — list every
  installed agent, inspect one in detail, scaffold a new agent
  directory, install a pre-built agent from a source directory, or
  remove an existing one. Useful for any external agent runtime that
  wants to see what Makakoo agents are available or automate their
  lifecycle.
allowed-tools:
  - agent_list
  - agent_info
  - agent_create
  - agent_install
  - agent_uninstall
category: meta
tags:
  - agent-lifecycle
  - meta
  - cli-agnostic
  - mcp-tool
---

# agents — scaffold, install, inspect, uninstall

Makakoo's agent scaffold lives under `$MAKAKOO_HOME/agents/<name>/`.
Each agent directory has an `agent.toml` that describes its kind,
entry point, patrol interval, and metadata. Five MCP tools cover the
full lifecycle.

## When to reach for agent_* tools

| Situation | Tool |
|---|---|
| *"Show me every agent installed"* | `agent_list` |
| *"What does agent X actually do?"* | `agent_info` |
| *"Create a new agent called Y for task Z"* | `agent_create` |
| *"Install this agent bundle into Makakoo"* | `agent_install` |
| *"Remove agent X"* | `agent_uninstall` |

External runtimes use these to surface the Makakoo agent population
to their own users — e.g. a LangChain orchestrator that wants to
decide "delegate to career-manager vs. arbitrage-agent vs. multimodal-
knowledge". Reading `agent_list` + `agent_info` gives the orchestrator
the ground truth without bundling any Makakoo code.

## `agent_list` — read-only listing

```json
{
  "tool": "agent_list",
  "arguments": {}
}
```

No parameters. Returns an array of `AgentSpec` objects — every agent
parseable from `$MAKAKOO_HOME/agents/<name>/agent.toml`. Fields per
entry:

```json
{
  "name": "career-manager",
  "kind": "python",
  "entry": "src/agent.py",
  "description": "CRM + inbound interest triage",
  "version": "1.0.0",
  "created_at": "2026-04-11T09:12:44Z",
  "maintainer": "Makakoo OS contributors",
  "patrol_interval_min": 30
}
```

## `agent_info` — inspect one

```json
{
  "tool": "agent_info",
  "arguments": {
    "name": "career-manager"
  }
}
```

Returns the same `AgentSpec` object. Returns `null` if no such
agent exists — no error. Cheap; prefer over grepping agent.toml by
hand.

## `agent_create` — scaffold a new agent

```json
{
  "tool": "agent_create",
  "arguments": {
    "name": "lead-enricher",
    "kind": "python",
    "description": "Enrich inbound lead records with LinkedIn + company data"
  }
}
```

- `name` — lowercase-kebab, same rules as plugin names. Rejects
  duplicates and invalid characters at the scaffold layer.
- `kind` — one of `python`, `rust`, `shell`. Determines the stub
  entry file the scaffold drops.
- `description` — short human summary; goes into agent.toml and the
  README stub.

Returns the new `AgentSpec`. The scaffold writes `agent.toml`,
`README.md`, and a language-appropriate stub entry file (for python
it's `src/agent.py` with a `main()` placeholder).

## `agent_install` — import a pre-built agent

```json
{
  "tool": "agent_install",
  "arguments": {
    "src_dir": "/path/to/unpacked/agent-bundle"
  }
}
```

`src_dir` must contain an `agent.toml`. The installer copies the
directory into `$MAKAKOO_HOME/agents/<name>/` (name pulled from the
manifest). Rejects duplicates.

## `agent_uninstall` — remove a running-safe

```json
{
  "tool": "agent_uninstall",
  "arguments": {
    "name": "lead-enricher"
  }
}
```

Deletes the agent directory. **Refuses** to delete if any file inside
holds an exclusive `fs2` lock — i.e. the agent is actively running.
Stop the agent first (`makakoo daemon status` / platform-specific
restart) then retry. This guard prevents yanking the rug out from
under a live process via a stray MCP call.

Returns `{ok: true}` on success. On refusal, surfaces the locked path
in the error message so you know which file is held.

## Portable integration (external agentic apps)

The handlers live in Rust:
- Tier-A read-only: `makakoo-mcp/src/handlers/tier_a/agents.rs`
  (`agent_list`, `agent_info`).
- Tier-B mutating: `makakoo-mcp/src/handlers/tier_b/agents.rs`
  (`agent_create`, `agent_install`, `agent_uninstall`).

Underlying logic sits in `makakoo_core::agents::AgentScaffold`. No
direct filesystem assumptions — `AgentScaffold::new(home)` takes an
explicit home path, so tests can run against an ephemeral tempdir.

External agent runtimes can either:

1. **Connect to `makakoo-mcp`** and call the tools via the stdio
   transport — the standard path.
2. **Shell out to `makakoo` CLI** — every tool above has a matching
   subcommand (e.g. `makakoo agent list`) for humans and scripts.

The five-tool set is **sufficient** for external lifecycle automation;
new tools for rename, clone, or duplicate are deliberately out of
scope (trivially expressible as uninstall + create).

## Don't confuse `agent_*` with `plugin_*`

Agents live at `$MAKAKOO_HOME/agents/<name>/` and are typically
long-running autonomous processes with their own entry points.
Plugins live at `$MAKAKOO_HOME/plugins/<name>/` and include SANCHO
task handlers, MCP tool handlers, bootstrap fragments, and anything
else the kernel loads on boot. Lifecycle tools for plugins
(`plugin install`, `plugin uninstall`, etc.) live under the
`makakoo plugin` CLI — not covered by this skill.
