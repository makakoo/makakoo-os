---
name: logseq
description: Connect Makakoo Brain to Logseq app for graph view and app integration.
---

# Logseq Integration

Connect Makakoo Brain to Logseq desktop/app for optional graph view.

## Two Modes

### Mode 1: Filesystem (No Logseq needed)
Makakoo Brain already uses Logseq markdown format. Works standalone.

### Mode 2: Logseq App Integration
Connect to Logseq app for:
- Visual graph view of all wikilinks
- Page relationships
- Backlinks
- Daily notes journal
- Community plugins

## Configuration

```bash
# No config needed - Brain IS Logseq format
# Just point Logseq app to your Brain folder:
export BRAIN_API_URL=http://127.0.0.1:12315  # Optional Logseq API
```

## Setup

1. Install Logseq app from https://logseq.com
2. Open Logseq → Settings → Advanced → Choose folder
3. Select: `~/MAKAKOO/data/Brain`
4. Done! Use graph view, backlinks, queries

## Usage

```bash
# Show status
makakoo skill logseq status

# List pages
makakoo skill logseq pages

# Search Brain
makakoo skill logseq search "query"

# Add to journal
makakoo skill logseq journal "Did something important"
```

## Brain Structure

```
~/MAKAKOO/data/Brain/
├── pages/              # Your notes (wikilinks format)
│   ├── Project.md
│   └── Ideas.md
└── journals/           # Daily journals
    ├── 2026_04_20.md
    └── 2026_04_19.md
```

## Graph View

With Logseq app connected:
- Click "Graph" in sidebar
- See all your pages as nodes
- Wikilinks become edges
- Community detection clusters

## Commands

| Command | Description |
|---------|-------------|
| `status` | Show Brain and Logseq status |
| `connect` | Print setup instructions |
| `pages` | List recent pages |
| `search <query>` | Search pages |
| `journal <content>` | Add to today's journal |
