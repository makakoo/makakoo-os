# Multimodal Knowledge Agent — Gemini Embedding 2 + Qdrant

**Semantic search across any file type: PDF, images, video, audio, text, and video-OCR transcripts.**

## Architecture

```
File/Transcript → Chunker → Gemini Embedding 2 (3072d) → Qdrant (COSINE) → Query → Results
```

## Components

| Script | Purpose |
|--------|---------|
| `ingest.py` | Single file ingestion (PDF, image, video, audio, text) |
| `query.py` | Semantic search against the Qdrant collection |
| `multimodal_knowledge.py` | CLI wrapper with ingest/query/stats subcommands |
| `video_ingest.py` | Batch ingest video-OCR transcripts into Qdrant |

## Chunking Rules

| Type | Chunk Size | Method |
|------|-----------|--------|
| PDF | 5 pages | PyMuPDF |
| Video | 30s segments | ffmpeg |
| Audio | 75s segments | pydub |
| Images | 1 file | direct |
| Text | 6000 chars | split |
| Video Transcript | 6000 chars, 500 overlap | text split |

## Usage

### Ingest a single file
```bash
python3 ingest.py <file_path> [title]
```

### Ingest all video-OCR transcripts
```bash
python3 video_ingest.py           # ingest all pending transcripts
python3 video_ingest.py --dry-run # preview what would be ingested
```

Scans `~/MAKAKOO/data/video-ocr/videos/` for subdirectories containing `transcript.txt` or `jina_transcript.txt`. Skips videos already present in Qdrant (checked by `doc_id` prefix `vt_{video_id}_*`).

### Search the knowledge base
```bash
python3 query.py "your question" [content_type_filter]
```

### CLI wrapper (ingest + query + stats)
```bash
python3 multimodal_knowledge.py ingest <file> --title "..."
python3 multimodal_knowledge.py query "question text"
python3 multimodal_knowledge.py stats
```

## Config

| Variable | Default | Description |
|----------|---------|-------------|
| `GEMINI_API_KEY` | from `.env` | Gemini API key |
| `QDRANT_URL` | `http://localhost:6333` | Qdrant endpoint |

Environment loaded from `~/MAKAKOO/.env`.

## Qdrant Collection

- **Name:** `multimodal`
- **Vector size:** 3072 (Gemini Embedding 2)
- **Distance:** Cosine
- **Payload fields:** `doc_id`, `title`, `content_type`, `filename`, `chunk_index`, `chunk_total`, `text_content`, `metadata`

### Content Types

- `video`, `audio`, `image`, `pdf`, `text` — from `ingest.py`
- `video_transcript` — from `video_ingest.py` (video-OCR pipeline output)

## Verification

Tested on:
- Audio: "Wojenna tułaczka" (16min, 13 chunks)
- Video: "Karpathy autoresearch" (24min, 13 chunks)
- Search queries return correct chunks with 0.57-0.70 similarity
