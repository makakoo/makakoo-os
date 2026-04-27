# Superbrain — Harvey's Global Knowledge Layer

You have access to `superbrain`, a CLI tool that gives you access to Sebastian's entire knowledge base (Brain, entity graph, events).

## Available Commands

```bash
superbrain query "your question"     # Search + LLM-synthesized answer
superbrain search "keywords"         # Fast FTS5 keyword search (no LLM)
superbrain context                   # Get compact memory context (~300 tokens)
superbrain stack "query"             # Memory context tailored to a query
superbrain gods                      # Most important entities
superbrain neighbors "entity"        # Entity relationships
superbrain remember "what happened"  # Log something to Brain
superbrain status                    # Health check
superbrain sync                      # Re-index Brain (after changes)
```

## When to Use

- **Before saying "I don't know"** about Sebastian's projects, contacts, or history → `superbrain search "topic"`
- **For context** when starting work → `superbrain context` (inject into your reasoning)
- **After significant work** → `superbrain remember "what you did"`
- **To understand relationships** → `superbrain neighbors "project name"`

## Brain Location

Sebastian's Brain lives at `~/MAKAKOO/data/Brain/`:
- Journals: `~/MAKAKOO/data/Brain/journals/YYYY_MM_DD.md` (daily diary, newest at bottom)
- Pages: `~/MAKAKOO/data/Brain/pages/` (entity profiles)
- Every line starts with `- ` (outliner format)
- Bidirectional links: `[[Entity Name]]`

## Key Entities

Run `superbrain gods` to see the current top entities. Typical ones include:
Traylinx, switchAILocal, Harvey OS, Arbitrage Agent, Tytus, Polymarket, OpenClaw.
