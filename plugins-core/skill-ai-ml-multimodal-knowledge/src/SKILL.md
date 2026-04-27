# Multimodal Knowledge — Gemini Embedding 2 RAG

> **Runtime code lives at `agents/multimodal-knowledge/`.**
> This SKILL.md is the documentation reference only.

**Harvey's superpower for semantic search across any file type.**

## What It Does

Ingests PDF, images, video, audio, and text → Gemini Embedding 2 (3072 dims) → PostgreSQL/pgvector (harvey_brain). Supports semantic Q&A across all media types via nearest-neighbor search + reasoning.

## Pipeline

```
File → Chunker (size-based) → Gemini Embedding 2 (3072d) → pgvector (HNSW) → Query → Reasoning (Gemini/Codex)
```

## Chunking Rules

| Type | Chunk Size | Method |
|------|-----------|--------|
| PDF | 5 pages | PyMuPDF |
| Video | 120s | MoviePy/ffmpeg |
| Audio | 75s | pydub |
| Images | 1 file | direct |
| Text | 6000 tokens | split |

## Usage

### Ingest a file
```bash
python3 $HARVEY_HOME/harvey-os/skills/ai-ml/multimodal-knowledge/ingest.py <file_path>
```

### Search
```bash
python3 $HARVEY_HOME/harvey-os/skills/ai-ml/multimodal-knowledge/query.py "your question"
```

### Ingest URL (YouTube, webpage)
```bash
python3 $HARVEY_HOME/harvey-os/skills/ai-ml/multimodal-knowledge/ingest_url.py <url>
```

## Config

- **GEMINI_API_KEY**: Set in ~/MAKAKOO/.env (never commit real keys)
- **DB**: harvey_brain@localhost:5434 (pgvector)
- **Table**: multimodal_documents

## Scripts

- `ingest.py` — single file ingestion
- `ingest_url.py` — YouTube/video URL ingestion
- `query.py` — semantic search + reasoning
- `batch_ingest.py` — bulk ingestion from directory

## Verification

Tested on:
- Audio: "Wojenna tułaczka" (16min, 13 chunks) ✅
- Video: "Karpathy autoresearch" (24min, 13 chunks) ✅

Search queries return correct chunks with 0.57-0.70 similarity.
