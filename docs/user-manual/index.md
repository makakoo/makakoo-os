# Makakoo User Manual

Every command, every chapter, every way to do a thing.

**New here?** Start with [Getting started](../getting-started.md) for a
step-by-step install guide, or the [Use cases](../use-cases.md) page
for "I want to X" recipes.

## By task (start here if you have a goal in mind)

| Chapter | What it covers |
|---|---|
| [Setup wizard](setup-wizard.md) | The setup sections (persona, updates, brain, cli-agent, terminal, lope, model-provider, infect) walked through end-to-end. |
| [Write-access grants (`makakoo perms`)](makakoo-perms.md) | Grant / revoke / audit runtime write permissions. |
| [Multi-bot subagents (`makakoo agent`)](agent.md) | Create, run, and tear down scoped agent slots. Telegram, Slack, Discord, WhatsApp, Voice, Email, and Web are transports, not infected hosts. |
| [Agent sessions (`makakoo agent-session`)](makakoo-agent-session.md) | Durable child-agent work sessions with compact result handles and verification gates. |
| [Handle reads (`makakoo handle`)](makakoo-handle.md) | Bounded reads from durable Makakoo handles such as `agent-artifact://...`. |
| [Skill security & auditing](makakoo-skill-security.md) | Plugin preflight security scans, overrides, and manual SkillSpector audits. |
| [HarveyChat Cortex Memory](../agents/harveychat-cortex-memory.md) | Configure long-term memory and cross-channel aliases for HarveyChat. |
| [Brain Network (`makakoo network`)](makakoo-network.md) | Opt-in Makakoo-to-Makakoo Brain federation via Octopus signed MCP. |

*(More task-oriented chapters coming — brain sources, adapter
selection, plugin authoring. Until those land, look up the individual
command below.)*

## Synopsis

```bash
makakoo <command> [options] [arguments]
```

## Commands

| Command | Description |
|---------|-------------|
| [setup](setup-wizard.md) | Interactive re-runnable wizard (persona / updates / brain / cli-agent / terminal / lope / model-provider / infect) |
| [install](../getting-started.md) | One-shot installer umbrella — distro + daemon + infect + health + optional setup |
| [update](makakoo-update.md) | Primary self-update command. Auto-detects cargo / brew / curl-pipe and updates `makakoo` + `makakoo-mcp`. |
| [upgrade](makakoo-upgrade.md) | Legacy alias for `makakoo update`. |
| [query](makakoo-query.md) | Search the Brain with LLM synthesis |
| [search](makakoo-search.md) | Full-text search the Brain |
| [infect](makakoo-infect.md) | Infect AI CLIs with shared brain |
| [uninfect](makakoo-uninfect.md) | Remove infection from CLIs |
| [plugin](makakoo-plugin.md) | Install and manage plugins |
| [sancho](makakoo-sancho.md) | Manage proactive tasks |
| [daemon](makakoo-daemon.md) | Control the background daemon |
| [distro](makakoo-distro.md) | Manage distro bundles |
| [secret](makakoo-secret.md) | Manage secrets |
| [perms](makakoo-perms.md) | Runtime write-access grants (v0.3 / hardened in v0.3.1-v0.3.2) |
| [sync](makakoo-sync.md) | Index on-disk Brain journals, pages, and auto-memory into FTS5. Use after manual file edits. |
| [memory](makakoo-memory.md) | Memory diagnostics and maintenance. |
| [skill](makakoo-skill-security.md) | Run a plugin skill or execute security audits |
| [status](makakoo-status.md) | Show system status |
| [completion](makakoo-completion.md) | Shell completion setup |
| [adapter](makakoo-adapter.md) | Manage AI adapters |
| [mcp](makakoo-mcp.md) | MCP server management |
| [agent](agent.md) | Multi-bot subagents — slot lifecycle + transports (v2.0) |
| [agent-session](makakoo-agent-session.md) | Durable child-agent sessions — open/eval/read/gate without flooding parent context |
| [handle](makakoo-handle.md) | Bounded reads from Makakoo handles such as `agent-artifact://...` |
| [network](makakoo-network.md) | Opt-in Brain Network federation across Makakoo installs |

## Global Options

| Flag | Description |
|------|-------------|
| `-h, --help` | Show help |
| `-v, --verbose` | Enable verbose output |
| `--version` | Show version |

## Examples

### Query the Brain

```bash
# Ask a question
makakoo query "what did I decide about the database?"

# Search with filters
makakoo query "trading strategies" --model ail-compound
```

### Search Full-Text

```bash
# Basic search
makakoo search "polymarket"

# Limit results
makakoo search "arbitrage" --limit 10
```

### Manage Plugins

```bash
# List installed plugins
makakoo plugin list

# Install a plugin
makakoo plugin install skill-research-arxiv --core

# Update one plugin, or every updatable plugin
makakoo plugin update <name>
makakoo plugin update --all

# Disable/enable
makakoo plugin disable my-plugin
makakoo plugin enable my-plugin
```

### Agent sessions

```bash
# Create a durable child-agent work record
makakoo agent-session open --name repo-audit --role explore --task "Inspect plugin install flow" --workspace .

# Complete sync v1 evaluation and read only the evidence section
makakoo agent-session eval repo-audit --wait
makakoo agent-session read repo-audit --section EVIDENCE

# Attach a verification gate and keep full logs behind a handle
makakoo agent-session gate repo-audit --name tests --cwd . --cmd "cargo test -p makakoo-core agent_session"
```

Full reference: [makakoo-agent-session.md](makakoo-agent-session.md) and [makakoo-handle.md](makakoo-handle.md).

### SANCHO Tasks

```bash
# Show all tasks
makakoo sancho status

# Trigger due tasks once
makakoo sancho tick
```

### Secrets

```bash
# Set a secret
makakoo secret set POLYMARKET_API_KEY

# Delete a secret
makakoo secret delete POLYMARKET_API_KEY
```

### Daemon

```bash
# Check daemon status
makakoo daemon status

# Restart daemon
makakoo daemon restart

# View logs
makakoo daemon logs --lines 50
```

### Infection

```bash
# Preview infection
makakoo infect --global --dry-run

# Apply infection
makakoo infect --global

# Infect specific CLIs
makakoo infect --target claude,gemini

# Remove infection
makakoo uninfect --global
```

### Update

```bash
# Auto-detect install method + update both binaries
makakoo update

# Preview without spawning
makakoo update --dry-run

# Update + refresh bootstrap fragments in every infected CLI / IDE slot
makakoo update --reinfect

# Force a specific method (rare — when auto-detect picks the wrong path)
makakoo update --method brew
makakoo update --method cargo
makakoo update --method curl-pipe

# Update Cargo install from a local checkout instead of the public repo
makakoo update --source ~/makakoo-os
```

Full reference: [makakoo-update.md](makakoo-update.md). Task-oriented walkthrough: [docs/upgrade.md](../upgrade.md). `makakoo upgrade` remains a legacy alias.

### Write-access grants

```bash
# Show baseline + active grants
makakoo perms list

# Grant 1h write access to a directory outside the baseline
makakoo perms grant ~/code/scratch/ --for 1h

# Revoke the grant (also releases a rate-limit slot since v0.3.1)
makakoo perms revoke --path last

# See every grant / revoke / denial since yesterday
makakoo perms audit --since 1d

# Forensic: why did a grant get refused?
makakoo perms audit --since 10m --json | jq 'select(.result == "denied")'
```

Full reference: [makakoo-perms.md](makakoo-perms.md). For the
conversational flow — when an agent's `write_file` gets rejected and
offers to grant itself access — see the "Grant write access in
conversation" section of [quickstart.md](../quickstart.md).

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error |
| 2 | Invalid arguments |
| 3 | Permission denied |
| 4 | Resource not found |
| 5 | Daemon not running |
