---
name: multimodal-knowledge
description: Multimodal RAG agent — Gemini Embedding 2 for images, video, audio, PDFs → Qdrant
type: agent
status: active
requires:
  - GEMINI_API_KEY
  - Qdrant (localhost:6333)
---

# Multimodal Knowledge Agent

Standalone agent for semantic search across images, video, audio, and PDFs.

## Independence
This agent is COMPLETELY INDEPENDENT from core Superbrain.
- Core Superbrain: switchAILocal mistral-embed (1024d) → SQLite
- This agent: Gemini Embedding 2 (3072d) → Qdrant "multimodal" collection

## Setup
1. Set GEMINI_API_KEY in ~/MAKAKOO/.env
2. Ensure Qdrant is running: `docker compose up -d qdrant`
3. Test: `python3 agents/multimodal-knowledge/query.py "test query"`

## Usage
### Ingest
```bash
python3 agents/multimodal-knowledge/ingest.py <file_path>
```

### Search
```bash
python3 agents/multimodal-knowledge/query.py "your question"
```

## Supported Formats
- Images: PNG, JPEG, WebP, GIF
- Video: MP4, MOV (max 120s chunks)
- Audio: M4A, MP3, WAV (max 75s chunks)
- PDF: up to 5 pages per chunk
- Text: up to 6000 tokens per chunk

## Architecture
```
File → Chunker → Gemini Embedding 2 (3072d) → Qdrant "multimodal" → Query → Gemini Reasoning
```

## Components

| Script | Purpose |
|--------|---------|
| `ingest.py` | Single file ingestion (PDF, image, video, audio, text) |
| `query.py` | Semantic search against the Qdrant collection |
| `multimodal_knowledge.py` | CLI wrapper with ingest/query/stats subcommands |
| `video_ingest.py` | Batch ingest video-OCR transcripts into Qdrant |

## Dependencies
- google-genai
- qdrant-client
- numpy
- requests
- python-dotenv
