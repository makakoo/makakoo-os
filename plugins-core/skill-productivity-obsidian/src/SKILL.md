---
name: obsidian
description: Read, search, and create notes in any Obsidian vault.
---

# Obsidian Vault Plugin

Read, search, and create notes in any Obsidian vault. Separate from Makakoo Brain.

## Configuration

```bash
# Set your Obsidian vault path
export OBSIDIAN_VAULT_PATH=~/Documents/MyVault

# Or use Makakoo Brain as vault
export OBSIDIAN_VAULT_PATH=~/MAKAKOO/data/Brain
```

## Usage

```bash
# Show vault status
makakoo skill obsidian status

# List all notes
makakoo skill obsidian list

# List in folder
makakoo skill obsidian list projects/

# Read a note
makakoo skill obsidian read "Project Name"

# Search notes
makakoo skill obsidian search "keyword"

# Create a note
makakoo skill obsidian create "New Note" "# Title\n\nContent here"

# Add to today's journal
makakoo skill obsidian journal "Did something important"

# Sync from Makakoo Brain
makakoo skill obsidian sync
```

## Commands

| Command | Description |
|---------|-------------|
| `status` | Show vault status |
| `list [folder]` | List all notes or notes in folder |
| `read <name>` | Read a note |
| `search <query>` | Search notes by content |
| `create <name> <content>` | Create a new note |
| `journal <content>` | Add to today's journal |
| `sync` | Sync from Makakoo Brain |
| `help` | Show help |

## Vault Format

Uses standard Obsidian markdown:
- Frontmatter: `key:: value`
- Wikilinks: `[[Page Name]]`
- Tags: `#tag`
- Bullet points: `- item`

## Sync Feature

```bash
# Sync Makakoo Brain pages to Obsidian vault
makakoo skill obsidian sync
```

This copies pages from `~/MAKAKOO/data/Brain/pages/` to `OBSIDIAN_VAULT_PATH/brain-sync/`
