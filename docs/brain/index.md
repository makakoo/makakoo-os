# Brain Guide

Makakoo's persistent memory system.

## What is the Brain?

The Brain is Makakoo's memory — all your notes, decisions, and knowledge in one place.

```
~/MAKAKOO/Brain/
├── journals/           # Daily logs
│   ├── 2026_04_20.md  # Today
│   ├── 2026_04_19.md
│   └── ...
├── pages/              # Structured knowledge
│   ├── projects/
│   ├── decisions/
│   └── people/
└── superbrain.db       # Vector embeddings
```

## Two Types of Memory

### Journals

Daily logs with timestamps. Makakoo writes to these automatically.

**Format:**
```markdown
- [[timestamp]] Did X
- [[timestamp]] Decided Y
- [[timestamp]] Note Z
```

**Example journal entry:**
```markdown
- [[2026-04-20 09:15:23]] Started new project "Atlas"
- [[2026-04-20 09:45:00]] Chose PostgreSQL over MySQL
- [[2026-04-20 14:30:00]] Bug found in auth module
```

### Pages

Structured documents for persistent knowledge.

**File structure:**
```markdown
~/MAKAKOO/Brain/pages/
├── projects/
│   ├── atlas.md
│   └── web-app.md
├── decisions/
│   ├── database-choice.md
│   └── architecture.md
├── people/
│   └── team-contacts.md
└── reference/
    ├── api-docs.md
    └── setup-guide.md
```

## Writing to the Brain

### Automatic (Recommended)

When using an infected CLI, Makakoo writes automatically:

```
> Remember I chose PostgreSQL
> Note that the retry limit is 3
> Add to my project notes that we're using microservices
```

### Manual

```bash
# Add to today's journal
echo "- [[timestamp]] Did X" >> ~/MAKAKOO/Brain/journals/$(date +%Y_%m_%d).md

# Create a page
cat > ~/MAKAKOO/Brain/pages/projects/my-project.md << 'EOF'
# My Project

## Status
In progress

## Tech Stack
- Backend: PostgreSQL
- Frontend: React

## Decisions
- [x] Chose microservices (2026-04-20)
EOF
```

## Querying the Brain

### Simple Search

```bash
# Full-text search
makakoo search "PostgreSQL"

# With limit
makakoo search "project" --limit 20
```

### Semantic Query

```bash
# Ask a question (uses vectors + LLM)
makakoo query "what database did I choose?"

# With specific model
makakoo query "what projects am I working on?" --model ail-compound
```

### LLM Synthesis

The `query` command:
1. Converts your question to a vector
2. Finds relevant entries (FTS + vectors)
3. Synthesizes an answer with LLM
4. Returns answer with citations

```
$ makakoo query "what did I decide about the architecture?"

Based on your journals and pages, you decided on:

1. **Microservices architecture** (2026-04-20)
   - Each service has its own database
   - API gateway for routing
   
2. **PostgreSQL for all services**
   - Better for complex queries
   - Supports JSONB for flexible schemas

Citations:
- ~/MAKAKOO/Brain/pages/decisions/architecture.md
- ~/MAKAKOO/Brain/journals/2026_04_20.md
```

## Page Format

### Basic Page

```markdown
# Page Title

## Summary
Brief description.

## Details
Detailed information.

## Related
- [[Other Page]]
- [[Another Page]]
```

### Project Page

```markdown
# Project Name

## Status
#status/active #project/my-project

## Overview
What this project is about.

## Tech Stack
- Backend: 
- Frontend: 
- Infrastructure: 

## Decisions
- [x] Chose X (2026-04-20)
- [ ] Need to decide Y

## Current Tasks
- [ ] Task 1
- [ ] Task 2

## Notes
Additional information.
```

### Decision Page

```markdown
# Decision: [Title]

**Date:** YYYY-MM-DD
**Status:** Decided / Pending / Reversed

## Context
Why this decision was needed.

## Options Considered
1. Option A
   - Pros: ...
   - Cons: ...
2. Option B

## Decision
Chose: Option A

## Rationale
Why this was the right choice.

## Consequences
What this affects.
```

## Tags

Makakoo understands tags:

```markdown
# Project Status
#status/active
#status/completed
#status/on-hold

# Project Names  
#project/atlas
#project/web-app

# People
#person/john
#team/backend
```

## Query Examples

### What did I work on?

```
$ makakoo query "what did I work on yesterday?"

Yesterday (2026-04-19) you:
1. Worked on the Atlas project
2. Fixed authentication bug
3. Had a call with the team about architecture
```

### What decisions have I made?

```
$ makakoo query "what architectural decisions have I made?"

Architectural decisions:
1. Microservices over monolith (2026-04-20)
2. PostgreSQL over MySQL (2026-04-20)
3. React over Vue (2026-04-18)
4. GitHub Actions for CI/CD (2026-04-15)
```

### What do I know about X?

```
$ makakoo query "what do I know about AI agents?"

From your notes and research:
- AI agents need memory systems
- Context windows limit long-term memory
- Semantic search improves retrieval
- Vector databases enable similarity search
```

## Best Practices

### 1. Write Frequently

More entries = better answers:

```
> Remember that X
> Note that Y
> I decided Z
```

### 2. Use Consistent Naming

```markdown
# Good
#project/atlas
#person/john
#status/active

# Bad
#proj1
#employee-123
#in-progress
```

### 3. Create Pages for Projects

```
~/MAKAKOO/Brain/pages/projects/
~/MAKAKOO/Brain/pages/decisions/
~/MAKAKOO/Brain/pages/people/
```

### 4. Link Related Pages

```markdown
## Related
- [[Project Name]]
- [[Architecture Decision]]
```

### 5. Review Weekly

```bash
# Read last week's journal
cat ~/MAKAKOO/Brain/journals/2026_04_13.md
```

## Automatic Maintenance

SANCHO runs these Brain tasks:

| Task | When | What |
|------|------|------|
| `wiki_lint` | Daily | Fix broken links |
| `memory_consolidation` | Daily | Optimize storage |
| `superbrain_sync` | Hourly | Sync vectors |

## Troubleshooting

### No Results from Query

```bash
# Add more entries
> Remember that I use X for Y

# Rebuild index
makakoo sancho run index_rebuild
```

### Brain Not Accessible

```bash
# Check directory
ls -la ~/MAKAKOO/Brain/

# Rebuild from scratch
rm ~/MAKAKOO/Brain/superbrain.db
makakoo sancho run index_rebuild
```

### Slow Searches

```bash
# Check index size
du -sh ~/MAKAKOO/Brain/superbrain.db

# Optimize
makakoo sancho run memory_consolidation
```

## API Reference

### MCP Tools

```json
// brain_read
{
  "tool": "makakoo_brain_read",
  "params": {
    "path": "journals/2026_04_20.md"
  }
}

// brain_write
{
  "tool": "makakoo_brain_write",
  "params": {
    "entry": "- Did X (timestamp)",
    "journal": true
  }
}

// superbrain_query
{
  "tool": "makakoo_superbrain_query",
  "params": {
    "question": "what did I decide about X?"
  }
}
```

## See Also

- [Concepts Overview](../concepts/index.md) — Brain in context
- [Superbrain Search](../concepts/superbrain.md) — Technical details
- [Query Reference](../user-manual/makakoo-query.md) — CLI options


## HarveyChat Cortex Memory

Brain is the global journal/wiki memory. HarveyChat Cortex Memory is a separate chat-facing recall layer that stores durable memories in `data/chat/conversations.db` and injects relevant local memory into HarveyChat prompts.

Use Cortex when the external chat gateway should remember facts across sessions or across Telegram/Discord aliases. Use Brain for canonical project notes, journals, pages, and superbrain search.

See [HarveyChat Cortex Memory](../agents/harveychat-cortex-memory.md).
