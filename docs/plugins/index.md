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
| `skill-productivity-obsidian` | Read/write Obsidian vault |
| `skill-productivity-notion` | Notion integration |
| `skill-productivity-logseq` | Connect Logseq app to Brain |

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

---

## skill-productivity-logseq

Connect Makakoo Brain to Logseq app for graph view and app integration.

### What it does

Makakoo Brain already uses Logseq markdown format. This plugin makes it easy to:
- See your Brain as a visual graph
- Use Logseq backlinks and queries
- Access community plugins

### Setup

```bash
# Install
makakoo plugin install skill-productivity-logseq --core

# Then connect Logseq app:
# 1. Open Logseq app
# 2. Settings → Advanced → Choose folder
# 3. Select: ~/MAKAKOO/data/Brain
```

### Usage

```bash
# Check status
makakoo skill logseq status

# List pages
makakoo skill logseq pages

# Search Brain
makakoo skill logseq search "polymarket"

# Add to today's journal
makakoo skill logseq journal "Did something important"

# Print setup instructions
makakoo skill logseq connect
```


### Logseq App Features

Once connected:
- **Graph View** - Visual map of all wikilinks
- **Backlinks** - See all pages linking to a page
- **Queries** - Datalog queries on your Brain
- **Daily Notes** - Journal view
- **Plugins** - Community plugins work

---

## skill-productivity-obsidian

Read, write, and sync any Obsidian vault.


### What it does

Manage multiple knowledge bases:
- Read/write any Obsidian vault
- Sync from Makakoo Brain to Obsidian
- Search across vaults

### Setup

```bash
# Install
makakoo plugin install skill-productivity-obsidian --core

# Configure vault path
export OBSIDIAN_VAULT_PATH=~/Documents/MyVault
```

### Usage

```bash
# Check vault status
makakoo skill obsidian status

# List notes
makakoo skill obsidian list
makakoo skill obsidian list projects/

# Read a note
makakoo skill obsidian read "Project Name"

# Search
makakoo skill obsidian search "keyword"

# Create note
makakoo skill obsidian create "New Note" "# Title\n\nContent here"

# Add to journal
makakoo skill obsidian journal "Did X today"

# Sync from Brain to Obsidian vault
makakoo skill obsidian sync
```

### Vault Format

Both use standard Logseq markdown:
- Frontmatter: `key:: value`
- Wikilinks: `[[Page Name]]`
- Tags: `#tag`
- Bullet points: `- item`

---

## skill-productivity-apple-notes

Access Apple Notes from Makakoo.

### Setup

```bash
makakoo plugin install skill-productivity-apple-notes --core
```

### Usage

```bash
makakoo skill apple-notes list
makakoo skill apple-notes read "Note Name"
```
