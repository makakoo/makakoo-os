# `agent-knowledge-extractor`

**Summary:** Extracts structured knowledge from raw sources (web, papers, transcripts).
**Kind:** Agent (plugin) · **Language:** Python · **Source:** `plugins-core/agent-knowledge-extractor/`
**Related walkthrough:** [09 — Ingest a document](../walkthroughs/09-ingest-document.md)

## When to use

Whenever you want a URL, a PDF, an audio file, or a transcript to be **chunked, embedded, and persisted into the knowledge index** so it's retrievable later by content. This is the ingest pathway: the extracted content becomes searchable alongside your journals and pages.

**Not for one-shot Q&A** — if you just want to ask "what's in this file?" once and move on, use the `harvey_describe_{image,audio,video}` MCP tools instead. They don't persist anything.

Words that signal this agent: **add, save, remember, index, ingest, store, keep**.

## Start / stop

Managed by the Makakoo daemon:

```sh
makakoo plugin info agent-knowledge-extractor
makakoo plugin disable agent-knowledge-extractor
makakoo plugin enable agent-knowledge-extractor
makakoo daemon restart
```

## Where it writes

- **State:** `~/MAKAKOO/state/agent-knowledge-extractor/` — extraction queue + cached parses.
- **Data (chunks + vectors):** `~/MAKAKOO/data/knowledge/<doc_id>/` — one directory per ingested source, containing chunk Markdown files and a `source.meta.json` manifest.
- **Logs:** `~/MAKAKOO/data/logs/agent-knowledge-extractor.{out,err}.log`

## Health signals

- `ps -ef | grep extractor.py` — one running process.
- `ls ~/MAKAKOO/data/knowledge/ | wc -l` — at least one directory after first ingest.
- Running `harvey_knowledge_ingest` through an infected AI CLI returns a non-zero `chunks` count.

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| Ingest returns `file-type not supported` | Type outside `pdf`, `text`, `audio`, `video`, `image` | Convert first (e.g. `pandoc input.docx -o input.md`) then ingest the converted file. |
| `chunks: 0` despite successful ingest | Extractor found no text (scanned PDF w/o OCR, empty audio, …) | OCR the source first via `harvey_describe_image`, then ingest the resulting text. |
| `rate limit` / `resource exhausted` on embedding | Embedding gateway hit a per-minute cap | `makakoo sync --embed --embed-limit 50` batches smaller; or throttle incoming ingest requests. |
| YouTube URL ingest fails with `yt-dlp not found` | `yt-dlp` not in `$PATH` | `brew install yt-dlp` or `pip install yt-dlp`. The plugin does not bundle it. |

## Capability surface

- `exec/shell` — for `yt-dlp` and text-extraction tools.
- `fs/read:$MAKAKOO_HOME/plugins/agent-knowledge-extractor`
- `fs/write:$MAKAKOO_HOME/data/knowledge`
- `net/http:*` — downloading URLs (broad by design; narrow via `makakoo perms` if needed).
- `llm/chat` — summarization step.
- `state/plugin:$MAKAKOO_HOME/state/agent-knowledge-extractor`

## Related agents

- `agent-multimodal-knowledge` — sibling that handles omni-modal understanding (describe tools). Use knowledge-extractor for ingest, multimodal-knowledge for one-shot Q&A.

## Remove permanently

```sh
makakoo plugin uninstall agent-knowledge-extractor --purge
```

Note: `--purge` also deletes `~/MAKAKOO/data/knowledge/` — your ingested document index. Omit `--purge` to keep the index for a future reinstall.
