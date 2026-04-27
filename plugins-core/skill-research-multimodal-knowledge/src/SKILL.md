---
name: multimodal-knowledge
description: "Use when asked to index, embed, ingest, or search video, audio, PDF, image, or text content. Trigger phrases: 'index this video', 'embed this audio', 'add this PDF to knowledge', 'search my knowledge base', 'what did that video say about...', 'do i have anything about...', 'ingest this file', 'store in knowledge base'. Embeds content using Gemini Embedding 2 (3072-dim), stores in PostgreSQL 16 pgvector, reasons with MiniMax-M2.7."
category: research
version: 1.0.0
dependencies: []
tags:
  - gemini
  - embedding
  - vector-search
  - multimodal
  - rag
  - pgvector
  - knowledge-base
metadata:
  hermes:
    tags:
      - gemini
      - embedding
      - vector-search
      - multimodal
      - rag
      - knowledge-base
    related_skills:
      - knowledge-extraction
---

# Multimodal Knowledge — Skill Manifest

**Category:** research
**Producer:** multimodal-knowledge
**Type:** RAG pipeline wrapper for Harvey OS

## What This Skill Does

Embeds and queries video, audio, PDF, image, and text content using Gemini Embedding 2
(3072-dim, L2-normalized) stored in a PostgreSQL 16 + pgvector database (harvey_brain),
with MiniMax-M2.7 reasoning over retrieved context.

## Trigger Phrases

Primary (ingest):
- "index this video"
- "embed this audio"
- "add this PDF to knowledge"
- "ingest this file"
- "store this in my knowledge base"

Primary (query):
- "search my knowledge base"
- "what did that video say about..."
- "do i have anything about..."
- "find content about..."
- "query my knowledge"

Secondary (stats):
- "how much do i have stored"
- "knowledge stats"
- "show my indexed content"

## Operating Procedure

### Ingest

```bash
python3 $HARVEY_HOME/harvey-os/skills/research/multimodal-knowledge/multimodal_knowledge.py ingest <file_path> --title "..."
```

Steps:
1. Read file bytes from `<file_path>`
2. Detect MIME type from filename extension
3. Run `rag.ingest()` which: detects content type → chunks → embeds via Gemini → stores to PostgreSQL
4. Register document as a Layer 6 artifact in `data/Brain/artifacts/registry.jsonl`
5. Return chunk count and PostgreSQL doc IDs

### Query

```bash
python3 $HARVEY_HOME/harvey-os/skills/research/multimodal-knowledge/multimodal_knowledge.py query "what is the key finding?" --top-k 5
```

Steps:
1. Embed query text via Gemini Embedding 2
2. Search PostgreSQL via HNSW cosine similarity (`<=>` operator)
3. Send top-k chunks + query to MiniMax-M2.7 for reasoning
4. Return answer with source citations

### Stats

```bash
python3 $HARVEY_HOME/harvey-os/skills/research/multimodal-knowledge/multimodal_knowledge.py stats
```

Returns total document count and breakdown by content type (video, audio, pdf, image, text).

## Content Types & Chunking

| Type   | Chunk Size         | Method                    |
|--------|--------------------|---------------------------|
| text   | ~6000 tokens       | character/spell split     |
| PDF    | 5 pages            | PyMuPDF                   |
| audio  | 75 seconds         | pydub                    |
| video  | 120 seconds        | moviepy                  |
| image  | whole file         | Gemini multimodal embed  |

## Environment Variables

Required in `~/MAKAKOO/.env`:
- `GEMINI_API_KEY` — Gemini Embedding 2 (embedding)
- `LLM_API_KEY` — API key for switchAILocal gateway (default: sk-test-123)
- `LLM_BASE_URL` — defaults to `http://localhost:18080/v1`
- `LLM_MODEL` — defaults to `auto` (Cortex Router picks best available provider)

PostgreSQL (no password needed — peer auth as `sebastian`):
- `POSTGRES_HOST=localhost`
- `POSTGRES_PORT=5434`
- `POSTGRES_DB=harvey_brain`

## Integration with Harvey OS

- Ingest results are registered as Layer 6 artifacts via `publish_artifact()` in the memory substrate
- Artifact stores: name (title), type (content_type), producer ("multimodal-knowledge"), content (text excerpt or summary)
- PostgreSQL doc IDs are stored in the artifact's `consumed_by` field for traceability
- Skill is listed in CLAUDE.md under the research category

## File Location

```
harvey-os/skills/research/multimodal-knowledge/
├── SKILL.md           — this file
└── multimodal_knowledge.py  — CLI tool (ingest, query, stats)
```
