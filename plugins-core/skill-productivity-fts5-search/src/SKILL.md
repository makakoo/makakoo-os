# FTS5 Conversation Search

**Purpose:** Fast, ranked full-text search across Harvey's conversation history in the Brain journal files.

## Quick Start

```bash
# First: index all existing journals
python3 harvey-os/skills/productivity/fts5_search/search_cli.py --reindex

# Search
python3 harvey-os/skills/productivity/fts5_search/search_cli.py "code review"

# Index stats
python3 harvey-os/skills/productivity/fts5_search/search_cli.py --stats
```

## Architecture

```
data/Brain/journals/YYYY_MM_DD.md
         │
         ▼ (indexer.py)
data/fts5/conversations.db
    ├── conversation_fts (FTS5 virtual table, BM25 ranked)
    ├── sessions (session metadata)
    └── indexed_entries (dedup hash index)
         │
         ▼ (search.py)
SearchResult objects with snippets + scores
```

## Journal Format

Entries in journals look like:
```
- [14:23] Completed code review for feature X
- [14:25] ## Session 2
- [14:26] Found bug in auth flow
```

Each `- [HH:MM]` bullet becomes an FTS entry. Session headers create session boundaries.

## CLI Commands

```bash
# Search with optional filters
harvey search "bug fix" --limit 5 --since 2026-03-01

# Show index statistics
harvey search --stats

# Re-index all journals (run after new sessions)
harvey search --reindex
```

## Re-indexing

The indexer is idempotent (hash-based dedup). Run `--reindex` to:
- Index new journal files after sessions
- Re-parse entries that may have been updated

## Python API

```python
from fts5_search import search, get_index_stats

# Basic search
results = search("code review patterns")

# With filters
results = search(
    "multimodal pipeline",
    limit=5,
    date_from="2026-03-01",
    date_to="2026-03-28",
)

for r in results:
    print(f"{r.date} {r.timestamp}: {r.snippet}")
    print(f"  session={r.session_id} score={r.score:.3f}")
```
