---
name: multimodal
version: 0.1.0
description: |
  Look at, listen to, or watch media — images, audio, video, PDFs —
  and either (a) describe it one-shot (stateless Q&A via xiaomi-tp/
  mimo-v2-omni through switchAILocal) or (b) ingest it persistently
  into the multimodal Qdrant collection so future knowledge queries
  retrieve it by content. Use whenever a user drops media on the agent
  — a photo, voice note, screen recording, chart, diagram, PDF.
allowed-tools:
  - harvey_describe_image
  - harvey_describe_audio
  - harvey_describe_video
  - harvey_generate_image
  - harvey_knowledge_ingest
category: ai-ml
tags:
  - multimodal
  - vision
  - audio
  - video
  - rag
  - cli-agnostic
  - mcp-tool
---

# multimodal — describe vs ingest, the two media pathways

Every MCP-capable agent runtime sees five tools in this family. Picking
the right one on first try saves a round-trip and prevents a hard
failure mode documented below.

## Decision tree (read before ANY media request)

| User says… | Call | Why |
|---|---|---|
| *"what's in / describe / summarize / tell me about / watch / listen to"* a video/audio/image/pdf | `harvey_describe_image` / `_audio` / `_video` | One-shot Q&A. Stateless. No write. |
| *"add / save / remember / index / ingest / store / keep"* a video/audio/image/pdf | `harvey_knowledge_ingest` | Persistent. Chunks + embeds + writes to the `multimodal` Qdrant collection so future queries retrieve by content. |
| *"make / generate / draw / create an image of …"* | `harvey_generate_image` | Produce new media, not analyse existing. |

Describe is **not** ingest. The symmetric opposite is also true: ingest
is not describe. Mixing them up is the single most common integration
bug.

## `harvey_describe_image | _audio | _video`

```json
{
  "tool": "harvey_describe_image",
  "arguments": {
    "source": "/path/to/file.png",
    "prompt": "What does this chart show?",
    "max_completion_tokens": 1024
  }
}
```

- `source` accepts a public URL, a `data:` URI, or a local filesystem
  path (auto base64-encoded).
- `prompt` is optional; omit for a default "describe this" prompt.
- `_video` adds `fps` (sample rate) and `media_resolution` knobs.
- `_audio` runs transcription + summary.

## `harvey_knowledge_ingest`

```json
{
  "tool": "harvey_knowledge_ingest",
  "arguments": {
    "source": "https://www.youtube.com/watch?v=fdbXNWkpPMY",
    "kind": "video",
    "title": "Hermes 3 launch overview",
    "note": "cited in hermes-agent research plan"
  }
}
```

- `source` accepts the same shapes as describe, plus YouTube URLs
  (routed through `yt-dlp` transparently).
- `kind` hints the pipeline: `image` / `audio` / `video` / `pdf` /
  `text`. Autodetected from extension if omitted.
- `title` + `note` go into the document record so they surface in
  knowledge queries.
- Returns `{ingested, doc_ids, chunks, file_type, summary, errors}` —
  quote the summary back to the user and log `doc_ids` so future
  retrieval can cite them.

## `harvey_generate_image`

```json
{
  "tool": "harvey_generate_image",
  "arguments": {
    "prompt": "A red mountain reflected in a lake at sunset, photorealistic",
    "model": "ail-image",
    "size": "1024x1024"
  }
}
```

Routes through switchAILocal's `ail-image` model by default. Returns
the generated image as a base64 data URI in the `content` block.

## Rate-limit rule (non-negotiable)

If `harvey_describe_*` returns 429 / "rate limit" / "resource
exhausted":

1. The MCP handler already retries with exponential backoff. If the
   error survives retries, the model really is rate-limited.
2. **Tell the user** and ask what they want — wait, switch models,
   retry later.
3. **Do NOT** substitute a journal write, a WebFetch, or a
   "description from URL without seeing content" fallback. A line in
   the journal with a URL and no content is a confabulation that
   poisons future retrieval — the superbrain will return the URL
   literal as a top hit for "what was in that video".
4. If the user wanted the media *indexed* in the first place, pivot
   to `harvey_knowledge_ingest` — that uses a different embedding
   path (Gemini Embedding 2) and isn't blocked by mimo omni rate
   limits.

Caught live 2026-04-20: opencode 429'd on a YouTube describe, then
journaled the URL under a wikilink as "added to knowledge". The
superbrain now returns the URL literal with no content for that
query. Do not repeat this.

## Don't reach for OCR / speech-to-text libraries first

One unified omni tool covers images, audio, and video with one
interface, one API key (`AIL_API_KEY`), one audit trail. Reaching for
Tesseract / Whisper / yt-dlp-then-transcribe as a first step is
almost always wrong — those exist for edge cases (offline, extreme
length, specialty formats), not for the default request.

## Portable integration (external agentic apps)

The `harvey_*` MCP tools are implemented in Rust at
`makakoo-mcp/src/handlers/tier_b/multimodal.rs` (describe + generate)
and `tier_b/knowledge.rs` (ingest). External agents can either:

1. Connect to the Makakoo MCP stdio server (`makakoo-mcp` on PATH)
   and call the tools as-is.
2. Replicate the pattern: wrap the switchAILocal OpenAI-compatible
   API (`xiaomi-tp/mimo-v2-omni` model) for describe, and wrap a
   local Qdrant + `yt-dlp` + Gemini Embedding 2 pipeline for ingest.

The decision tree above is runtime-agnostic — port it into any agent
system prompt verbatim.

## Python callers

All three pathways are also available directly in Python, bypassing
MCP entirely:

```python
from core.llm.omni import describe_image, describe_audio, describe_video

caption = describe_image("/path/to/screenshot.png", "What does this chart show?")
transcript = describe_audio("voice_note.wav", "Transcribe and summarize.")
summary = describe_video("clip.mp4", "Main action?", fps=2, media_resolution="default")
```

## Attribution

- Describe/generate: `xiaomi-tp/mimo-v2-omni` via switchAILocal.
- Ingest pipeline: `plugins-core/agent-multimodal-knowledge/` (Makakoo
  OS) + Qdrant + Gemini Embedding 2 (3072d vectors, pgvector caps at
  2000d which is why Qdrant wins here).
