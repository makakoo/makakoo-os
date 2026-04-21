# Distro Guide

Choose the right Makakoo OS distribution for your needs.

## What is a Distro?

A distro (distribution) is a curated bundle of plugins for a specific use case.

```
┌─────────────────────────────────────────────────────────────┐
│                    MAKAKOO OS                               │
│                     (Kernel)                                 │
│           Core: Brain, SANCHO, MCP, Plugins                  │
└─────────────────────────────────────────────────────────────┘
                            │
        ┌───────────────────┼───────────────────┐
        ▼                   ▼                   ▼
   ┌─────────┐         ┌─────────┐         ┌─────────┐
   │ minimal │         │  core  │         │ trader  │
   │  5 plugins    │   │ 15 plugins   │   │ 20 plugins   │
   └─────────┘         └─────────┘         └─────────┘
```

## Available Distros

### minimal

For beginners or minimal setup.

```toml
# distros/minimal.toml
[distro]
name = "minimal"
version = "0.1.0"
summary = "Just the essentials"

[includes]
- brain
- sancho
- mcp-server

[plugins]
# Core only (5 plugins)
```

**Includes:**
- Brain (journals + pages)
- Superbrain (search)
- SANCHO (proactive tasks)
- MCP server
- Basic plugins

**Best for:**
- New users
- Limited disk space
- Minimal setup

---

### core

The standard experience.

```toml
# distros/core.toml
[distro]
name = "core"
version = "0.1.0"
summary = "Standard Makakoo experience"

[includes]
- minimal
- productivity
- monitoring

[plugins]
# Core + productivity (15 plugins)
```

**Includes everything in minimal plus:**
- Development skills
- Productivity integrations
- Monitoring watchdogs
- Error classification
- Memory tools

**Best for:**
- Most users
- General productivity
- Software development

---

### sebastian

Sebastian's personal setup (dogfood).

```toml
# distros/sebastian.toml
[distro]
name = "seastian"
version = "0.1.0"
summary = "Full-featured personal setup"

[includes]
- core
- research
- trading

[plugins]
# Everything in core + more
```

**Includes:**
- All core plugins
- Research tools (ArXiv, blog watching)
- AI/ML integrations
- Custom Harvey-specific tools

**Best for:**
- Power users
- Researchers
- AI/ML practitioners

---

### creator

For writers, streamers, and creators.

```toml
# distros/creator.toml
[distro]
name = "creator"
version = "0.1.0"
summary = "For writers, streamers, creators"

[includes]
- core
- content
- social

[plugins]
# Core + creative tools
```

**Includes:**
- All core plugins
- Content research
- YouTube/blog tools
- Social media integrations
- Creative writing skills

**Best for:**
- Writers
- Streamers
- Content creators
- Social media managers

---

### trader

For market-facing autonomous agents.

```toml
# distros/trader.toml
[distro]
name = "trader"
version = "0.1.0"
summary = "Market-facing autonomous agents"

[includes]
- core
- trading
- finance

[plugins]
# Core + trading tools
```

**Includes:**
- All core plugins
- Arbitrage agent
- Market monitoring
- Financial analysis
- Trading skills

**Best for:**
- Day traders
- Quantitative researchers
- Crypto traders

---

## Comparison Matrix

| Feature | minimal | core | sebastian | creator | trader |
|---------|---------|------|-----------|---------|--------|
| Brain | ✅ | ✅ | ✅ | ✅ | ✅ |
| SANCHO | ✅ | ✅ | ✅ | ✅ | ✅ |
| MCP | ✅ | ✅ | ✅ | ✅ | ✅ |
| Development skills | ❌ | ✅ | ✅ | ❌ | ❌ |
| Research tools | ❌ | ❌ | ✅ | ✅ | ❌ |
| Trading agent | ❌ | ❌ | ✅ | ❌ | ✅ |
| Content tools | ❌ | ❌ | ❌ | ✅ | ❌ |
| Social media | ❌ | ❌ | ❌ | ✅ | ❌ |
| Plugins | 5 | 15 | 25 | 20 | 20 |

## Using Distros

### Install with Distro

```bash
# Install specific distro
makakoo distro install minimal
makakoo distro install core
makakoo distro install creator
makakoo distro install trader

# Install with install
curl -fsSL https://makakoo.com/install | sh -s -- --distro core
```

### List Available Distros

```bash
makakoo distro list
```

Output:
```
Available distros:
  minimal   - Just the essentials (5 plugins)
  core     - Standard Makakoo experience (15 plugins)
  sebastian - Full-featured personal setup (25 plugins)
  creator  - For writers/streamers (20 plugins)
  trader   - Market-facing agents (20 plugins)
  
  Installed: core
```

### Show Distro Contents

```bash
makakoo distro info core
```

Output:
```
Distro: core v0.1.0
Plugins: 15

Included:
  - brain (journals, pages, superbrain)
  - sancho (proactive tasks)
  - mcp-server (40+ tools)
  - skill-dev-ship
  - skill-research-arxiv
  - watchdog-postgres
  - watchdog-switchailocal
  - mascot-gym
  - ...and 6 more
```

### Switch Distro

```bash
# Switch to different distro
makakoo distro switch minimal

# Preview switch
makakoo distro switch minimal --dry-run
```

## Creating Custom Distros

### Save Current Setup

```bash
# Save as new distro
makakoo distro save my-custom

# With specific plugins
makakoo distro save my-custom --plugins brain,research,trading
```

### Create Manually

Create `~/.makakoo/distros/my-custom.toml`:

```toml
[distro]
name = "my-custom"
version = "0.1.0"
summary = "My custom Makakoo setup"
author = "you@example.com"

[plugins]
# Specify plugins
skill-research-arxiv = "0.1.0"
skill-dev-ship = "0.1.0"
arbitrage = "0.3.1"
```

### Install Custom Distro

```bash
# From local file
makakoo distro install ~/.makakoo/distros/my-custom.toml

# From URL
makakoo distro install https://example.com/my-distro.toml
```

## Distro Files

### File Location

```
~/.makakoo/distros/           # User distros
~/.makakoo/share/distros/    # System distros
```

### Manifest Format

```toml
[distro]
name = "distro-name"
version = "0.1.0"
summary = "Brief description"
description = """
Longer description with details.
"""
author = "Author Name <email@example.com>"
homepage = "https://example.com/distro"

[distro.plugins]
# Plugin name -> version constraint
core-brain = ">=0.1"
skill-research = "^0.2"
arbitrage = "~0.3"

[distro.config]
# Optional default config
[distro.config.sancho]
default_interval = "1h"

[distro.config.brain]
auto_create_journals = true
```

## Distribution Bundle

A distro bundle is a reproducible installation:

```bash
# Export distro as single file
makakoo distro bundle core --output core-distro.tar.gz

# Import on another machine
makakoo distro import core-distro.tar.gz
```

The bundle includes:
- Plugin manifests
- Version pins
- Configuration
- Capability grants

## Best Practices

### 1. Start Small

Begin with `minimal` or `core`, add plugins as needed.

### 2. Add Incrementally

```bash
# Add one plugin at a time
makakoo plugin install skill-research-arxiv
makakoo plugin install arbitrage
```

### 3. Save Custom Setup

After adding plugins:

```bash
makakoo distro save my-setup
```

### 4. Version Pins

For reproducibility:

```toml
[distro.plugins]
skill-research = "1.2.3"  # Exact version
arbitrage = "^0.3.0"     # Compatible
```

## Troubleshooting

### Plugin Not in Distro

```bash
# Add manually
makakoo plugin install <plugin-name>
```

### Version Conflict

```bash
# Check versions
makakoo plugin list

# Update plugin
makakoo plugin update <plugin-name>
```

### Corrupt Distro

```bash
# Reset to default
makakoo distro reset core
```

## See Also

- [Plugin Guide](../plugins/index.md) — Plugin details
- [Installation Guide](../getting-started.md) — Install with distro
- [Contributing Guide](../development/contributing.md) — Share your distro
