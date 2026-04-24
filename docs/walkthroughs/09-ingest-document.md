# Walkthrough 09 — Teach Harvey about a document

## What you'll do

Feed a PDF, a YouTube URL, an audio file, or any other document into Makakoo's knowledge index so Harvey can retrieve it by content — not just by filename. The indexed chunks become searchable alongside your journals and pages.

**Time:** about 5 minutes (depends on document length). **Prerequisites:** [Walkthrough 01](./01-fresh-install-mac.md), [Walkthrough 05 Path 1](./05-ask-harvey.md) (infected AI CLI working), a configured model provider (Path 2 of walkthrough 05).

## Ingest vs describe — pick the right tool

Makakoo exposes **two** MCP tools that look similar but do different things:

| You want to… | Use | Why |
|---|---|---|
| One-time Q&A about a file: *"what's in this video?"* | `harvey_describe_video` (or `_image`, `_audio`) | Stateless. Output is just an answer. Nothing stored. |
| Make the file retrievable later: *"add this paper to my notes"* | `harvey_knowledge_ingest` | Chunks + embeds + writes to the knowledge index. Future `makakoo query` calls retrieve it by content. |

Words that signal ingest (don't reach for `describe` here): **add, save, remember, index, ingest, store, keep**.

Words that signal describe (don't reach for `ingest`): **what's in, summarize, tell me about, watch, listen to, read**.

Full rationale + the incident that motivated this distinction is in the auto-memory entry `feedback_knowledge_ingest_vs_describe`.

## Steps

### 1. Pick a source

Any of these work as inputs to `harvey_knowledge_ingest`:

- An absolute path to a local PDF, audio file, or video
- A YouTube URL (routed through `yt-dlp` automatically)
- A public HTTPS URL for a PDF / media file
- A plain text file

For this walkthrough, use a small public PDF. If you don't have one handy:

```sh
curl -o ~/Downloads/sample.pdf https://www.pdf995.com/samples/pdf.pdf
```

Expected output:

```text
  % Total    % Received % Xferd  Average Speed   Time    Time     Time  Current
                                 Dload  Upload   Total   Spent    Left  Speed
100  6225k  100  6225k    0     0  2144k      0  0:00:02  0:00:02 --:--:-- 2143k
```

### 2. Confirm your AI CLI sees the MCP tool

Open Claude Code (or any infected CLI):

```sh
claude
```

In the prompt, ask:

```text
List the MCP tools you have access to.
```

You should see `harvey_knowledge_ingest` in the list, alongside `harvey_describe_*`, `harvey_browse`, `brain_search`, etc.

### 3. Ingest the document

In the same AI CLI session, ask:

```text
Use harvey_knowledge_ingest to add ~/Downloads/sample.pdf to the knowledge index with title "PDF995 sample".
```

**What happens:**
1. The AI CLI recognizes the phrase and composes a tool call:
   ```json
   {
     "source": "/Users/you/Downloads/sample.pdf",
     "kind": "pdf",
     "title": "PDF995 sample",
     "note": "added via walkthrough 09"
   }
   ```
2. The agent downloads / reads the file, extracts text (for PDFs via `pdfplumber` or `poppler`), splits into overlapping chunks, runs each through the embedding model, and writes both the chunk text and its vector into `~/MAKAKOO/data/knowledge/`.
3. The return value lands back in your CLI:
   ```json
   {
     "ingested": 1,
     "doc_ids": ["k_20260424_a1b2c3d4"],
     "chunks": 8,
     "file_type": "pdf",
     "summary": "PDF995 sample: a 2-page test PDF used for testing PDF processing tools. Includes a simple headline 'Sample PDF' and placeholder Lorem Ipsum text...",
     "errors": []
   }
   ```

You should see a summary sentence or two plus a chunk count. The chunk count scales with document length (roughly 1 chunk per 200–400 tokens of content).

### 4. Find the document back

Later — in the same session or a new one — you can retrieve what you ingested:

```text
Use brain_search to find what I ingested about "PDF sample".
```

Expected (abbreviated):

```text
Found 3 matching chunks (from "PDF995 sample", ingested 2026-04-24):
1. [chunk 1 of 8] "Sample PDF. This is a simple example of a PDF document..."
2. [chunk 3 of 8] "...Lorem ipsum dolor sit amet, consectetur adipiscing elit..."
3. ...
```

The chunks are findable by full-text search (FTS5) AND by semantic similarity (if you opted into embeddings with `makakoo sync --embed`).

### 5. See the knowledge index at the filesystem level

```sh
ls ~/MAKAKOO/data/knowledge/ | head -5
```

Expected output (one directory per ingested source):

```text
k_20260424_a1b2c3d4
k_20260423_0f9e8a22
k_20260420_5c2b1d30
```

Each directory contains the chunk Markdown + a `source.meta.json` manifest.

## What just happened?

- **The `multimodal` agent did the heavy lifting.** `agent-multimodal-knowledge` (or its unified SQLite successor per `project_brain_orchestration_sprint`) receives the MCP call, downloads / reads the source, chunks it, embeds it, persists it.
- **Embeddings and full-text search are different paths.** FTS5 works on every chunk immediately. Vector search needs `makakoo sync --embed` to run, which batches embedding requests to your configured gateway.
- **You can re-ingest to refresh.** Calling `harvey_knowledge_ingest` on the same source replaces the prior entry's chunks, preserving the `doc_id`. Useful when you update a local file.
- **The source never leaves your machine** unless your embedding / LLM provider is remote. Local providers (Ollama, a local switchAILocal gateway) make the whole pipeline offline.
- **`harvey_knowledge_ingest` has a different blast radius from `harvey_describe_*`.** Ingest writes persistent state. Describe does not. When rate limits bite the describe path, **never** journal the URL as a workaround — that's been shown to poison the retrieval layer (see `feedback_knowledge_ingest_vs_describe` for the 2026-04-20 incident).

## If something went wrong

| Symptom | Fix |
|---|---|
| AI CLI doesn't find `harvey_knowledge_ingest` | The multimodal agent didn't start. Check `ps -ef \| grep multimodal`, or `makakoo daemon restart`. |
| Ingest fails with `file-type not supported` | `harvey_knowledge_ingest` handles `pdf`, `text`, `audio`, `video`, `image`, and URL routing. Unsupported types (e.g. `.docx`) need conversion first — `pandoc input.docx -o input.md`, then ingest the `.md`. |
| Chunk count is 0 but `ingested: 1` | The extractor couldn't find any text (scanned PDF with no OCR, empty audio, etc.). Run `harvey_describe_image` on a scanned PDF page first to OCR it, then feed the OCR'd text to `harvey_knowledge_ingest`. |
| `rate limit` / `resource exhausted` on describe during ingest | The embedding model hit a per-minute cap. `makakoo sync --embed --embed-limit 50` batches a smaller number of embeddings at once. |
| YouTube URL ingest fails with `yt-dlp not found` | Install: `brew install yt-dlp` (macOS) or `pip install yt-dlp`. The knowledge-extractor plugin does NOT bundle yt-dlp (by design — it's a big binary and many users have their own install). |

## Next

- [Walkthrough 10 — Mascot mission](./10-mascot-mission.md) — see one of the guardian mascots fire a scheduled mission using the Brain you've been populating.
- [Walkthrough 11 — Connect Tytus](./11-connect-tytus.md) — pair your Mac with a remote pod so the knowledge you've ingested is reachable from anywhere.
