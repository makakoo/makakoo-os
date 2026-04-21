# Plugin Guide

Extend Makakoo OS with plugins.

## What are Plugins?

Plugins are capability-sandboxed modules that add functionality:

| Type | Purpose | Examples |
|------|---------|----------|
| `agent` | Autonomous workers | arbitrage-agent, career-manager |
| `skill` | Reusable workflows | skill-research, skill-dev |
| `sancho-task` | Scheduled jobs | watchdog, gym |
| `mcp-tool` | MCP servers | custom integrations |

## Listing Plugins

```bash
# List all installed
makakoo plugin list

# List core plugins
makakoo plugin list --core

# Show details
makakoo plugin info <name>
```

## Installing Plugins

### From Core Library

```bash
# Install a skill
makakoo plugin install skill-research-arxiv --core

# Install a watchdog
makakoo plugin install watchdog-postgres --core

# Install an agent
makakoo plugin install arbitrage --core
```

### From GitHub

```bash
# Install from git
makakoo plugin install https://github.com/user/makakoo-plugin

# Install specific version
makakoo plugin install https://github.com/user/makakoo-plugin --version 1.2.0
```

### From Local Path

```bash
# Install from local directory
makakoo plugin install ./my-plugin --allow-unsigned
```

## Managing Plugins

### Enable/Disable

```bash
# Disable temporarily
makakoo plugin disable <name>

# Re-enable
makakoo plugin enable <name>
```

### Update

```bash
# Update all plugins
makakoo plugin update

# Update specific plugin
makakoo plugin update <name>
```

### Uninstall

```bash
# Remove plugin
makakoo plugin uninstall <name>
```

## Core Plugins

Makakoo ships with 38 plugins:

### Agents

| Plugin | Purpose |
|--------|---------|
| `arbitrage` | Polymarket trading |
| `career-manager` | Job search automation |
| `harveychat` | Chat interface |

### Skills

| Plugin | Purpose |
|--------|---------|
| `skill-research-arxiv` | Research papers |
| `skill-research-blogwatcher` | Blog monitoring |
| `skill-dev-ship` | Development workflow |
| `skill-dev-orchestrator` | Multi-task coordination |
| `skill-productivity-google-workspace` | Gmail/Calendar/Drive |
| `skill-productivity-apple-notes` | Apple Notes integration |
| `skill-productivity-obsidian` | Obsidian sync |
| `skill-productivity-notion` | Notion integration |

### SANCHO Tasks

| Plugin | Purpose |
|--------|---------|
| `watchdog-postgres` | Database monitoring |
| `watchdog-switchailocal` | LLM gateway monitoring |
| `watchdog-infect` | Infection status |

### Other

| Plugin | Purpose |
|--------|---------|
| `mascot-gym` | Error classification |
| `skill-meta-loops` | Proactive improvement |
| `skill-meta-memory-retrieval` | Memory optimization |

## Plugin Configuration

Each plugin has a `plugin.toml`:

```toml
[plugin]
name = "arbitrage"
version = "0.3.1"
kind = "agent"

[capabilities]
grants = [
  "brain/read",
  "net/http:https://clob.polymarket.com/*",
  "secrets/read:POLYMARKET_API_KEY",
]

[sancho]
tasks = [
  { name = "arbitrage_tick", interval = "5m", active_hours = [6, 23] }
]
```

This defines:
- What the plugin can access (capabilities)
- When it runs (SANCHO tasks)
- Required secrets

## Security Model

Plugins run in a capability sandbox:

```
┌─────────────────────────────────────────┐
│              PLUGIN                      │
│  Declared: brain/read, net/http:api/*  │
└─────────────────┬───────────────────────┘
                  │ capability request
                  ▼
┌─────────────────────────────────────────┐
│         MAKAKOO CORE                    │
│  Checks: Is this in grants?             │
│  ✓ Yes → Allow                          │
│  ✗ No  → Deny + audit log               │
└─────────────────────────────────────────┘
```

This means even if a plugin is compromised, it can only access what you approved.

## Writing Your Own Plugin

See [Writing Plugins](writing.md) for a complete guide.

## Troubleshooting

### Plugin Won't Load

```bash
# Check logs
cat ~/.makakoo/logs/plugins/<name>/error.log

# Verify manifest
makakoo plugin info <name>

# Reinstall
makakoo plugin uninstall <name>
makakoo plugin install <name>
```

### Missing Capabilities

```bash
# See what capabilities a plugin needs
makakoo plugin info <name> | grep -A 20 grants
```

### Task Not Running

```bash
# Check SANCHO status
makakoo sancho status

# See task history
makakoo sancho history --task <task-name>
```
