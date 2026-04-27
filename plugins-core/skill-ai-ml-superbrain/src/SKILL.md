---
name: superbrain
description: "Use when querying Harvey's unified knowledge layer — searches Brain pages, journals, multimodal documents, and events across Qdrant + PostgreSQL + filesystem in parallel."
version: 1.0.0
tags: [knowledge, search, memory, qdrant, embeddings, brain]
---

# Superbrain — Unified Knowledge Layer

## What It Does

Searches ALL of Harvey's knowledge systems in a single query:
- **Brain pages** (233 entity profiles) — via Qdrant "brain" collection
- **Brain journals** (25 daily journals) — via Qdrant "brain" collection
- **Multimodal documents** (164 vectors: PDFs, videos, audio) — via Qdrant "multimodal" collection
- **Events** (structured activity log) — via PostgreSQL
- **Brain filesystem** (keyword fallback) — always works, zero dependencies

Results are merged, re-ranked (Brain content gets authority boost), and optionally synthesized via LLM.

## Trigger Commands

| Phrase | Effect |
|--------|--------|
| `superbrain query "question"` | Search all systems, return synthesized answer |
| `superbrain status` | Show available backends and vector counts |
| `superbrain sync` | Embed Brain pages/journals into vector store |
| `superbrain sync --force` | Re-embed everything (ignore change detection) |

## Usage

### CLI
```bash
python3 $HARVEY_HOME/harvey-os/core/superbrain/superbrain.py status
python3 $HARVEY_HOME/harvey-os/core/superbrain/superbrain.py query "What trading strategies exist?"
python3 $HARVEY_HOME/harvey-os/core/superbrain/superbrain.py sync
```

### Python API
```python
from core.superbrain.superbrain import Superbrain

sb = Superbrain()  # auto-detects available backends
result = sb.query("What do I know about Karpathy?")
print(result.answer)
print(result.sources)  # ranked list of SearchHit objects
```

### Custom Providers
```python
from core.superbrain.superbrain import Superbrain
from core.superbrain.providers import GeminiEmbedding, QdrantStore, ChromaStore, LocalEmbedding

# Use Qdrant (default if Docker running)
sb = Superbrain(embedding=GeminiEmbedding(), vector_store=QdrantStore())

# Use Chroma (no server needed)
sb = Superbrain(embedding=LocalEmbedding(), vector_store=ChromaStore())

# Auto-detect (picks best available)
sb = Superbrain()
```

## Architecture

```
Query → embed question → search in parallel:
  ├─ Qdrant "brain"       (pages + journals, 3072-dim Gemini)
  ├─ Qdrant "multimodal"  (PDFs, videos, audio, 3072-dim Gemini)
  ├─ PostgreSQL events     (structured, recent-first)
  └─ Brain filesystem      (keyword grep, always works)
       ↓
  Merge + re-rank (Brain 1.2x boost, journals 1.1x boost)
       ↓
  Synthesize via switchAILocal (optional)
       ↓
  QueryResult(answer, sources, systems_queried, query_time_sec)
```

## Degradation

| Available | Behavior |
|-----------|----------|
| Gemini + Qdrant + PG | Full semantic search across all systems |
| OpenAI + Chroma | Semantic search, local vector store |
| SentenceTransformers + Chroma | All local, no API keys needed |
| Nothing | Brain filesystem grep (always works) |

## Files

| File | Purpose |
|------|---------|
| `core/superbrain/superbrain.py` | Main class: auto-detect, query, sync, ingest |
| `core/superbrain/providers.py` | EmbeddingProvider + VectorStore abstractions |
| `core/superbrain/config.py` | Env var loading + tunables |
| `core/superbrain/brain_sync.py` | Legacy standalone Brain → Qdrant sync |
| `core/superbrain/query.py` | Legacy standalone multi-system query |
| `core/superbrain/ingest.py` | Event/trade/CRM ingestion |
| `core/superbrain/db.py` | PostgreSQL helper |
| `core/superbrain/schema.sql` | PG tables: events, trades, crm_leads |

## Current State

- Brain vectors: 250 (225 pages + 25 journals)
- Multimodal vectors: 164 (PDFs, videos with transcripts, audio)
- Embedding: Gemini Embedding 001 (3072-dim)
- Vector store: Qdrant (Docker, localhost:6333)
