# Makakoo User Manual

Every command, every chapter, every way to do a thing.

**New here?** Start with [Getting started](../getting-started.md) for a
step-by-step install guide, or the [Use cases](../use-cases.md) page
for "I want to X" recipes.

## By task (start here if you have a goal in mind)

| Chapter | What it covers |
|---|---|
| [Setup wizard](setup-wizard.md) | The 6 sections (persona, brain, cli-agent, terminal, model-provider, infect) walked through end-to-end. |
| [Write-access grants (`makakoo perms`)](makakoo-perms.md) | Grant / revoke / audit runtime write permissions. |

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
| [setup](setup-wizard.md) | Interactive re-runnable wizard (persona / brain / cli-agent / terminal / model-provider / infect) |
| [install](../getting-started.md) | One-shot installer umbrella — distro + daemon + infect + health + optional setup |
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
| [brain](setup-wizard.md#sections) | Multi-source brain registry (`list / add / remove / set-default / sync / init`) |
| [status](makakoo-status.md) | Show system status |
| [completion](makakoo-completion.md) | Shell completion setup |
| [adapter](makakoo-adapter.md) | Manage AI adapters |
| [mcp](makakoo-mcp.md) | MCP server management |

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

# Update plugins
makakoo plugin update

# Disable/enable
makakoo plugin disable my-plugin
makakoo plugin enable my-plugin
```

### SANCHO Tasks

```bash
# Show all tasks
makakoo sancho status

# Trigger a task manually
makakoo sancho run dream

# Show task history
makakoo sancho history --limit 20
```

### Secrets

```bash
# Set a secret
makakoo secret set POLYMARKET_API_KEY

# List secrets
makakoo secret list

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
