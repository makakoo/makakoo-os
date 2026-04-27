# `agent-multimodal-knowledge`

**Summary:** Multimodal RAG — image, video, audio, PDF understanding via Gemini Embedding 2, persisted into the unified Brain SQLite + vector store.
**Kind:** Agent (plugin) · **Language:** Python · **Source:** `plugins-core/agent-multimodal-knowledge/`
**Exposes MCP tools:** `harvey_describe_image`, `harvey_describe_audio`, `harvey_describe_video`, (plus delegate routes for `harvey_knowledge_ingest`)

## When to use

One-shot descriptions of media — *"what's in this image"*, *"summarize this audio"*, *"watch this video"*. No persistence, just an answer.

For persistent ingest of the same files into the knowledge index, use [`agent-knowledge-extractor`](./agent-knowledge-extractor.md) via `harvey_knowledge_ingest`. The two are siblings: this one answers; that one remembers.

## Prerequisites

- A configured model provider (via `makakoo setup model-provider`) that exposes the `mimo-v2-omni` alias (or equivalent). `switchAILocal` ships with this alias by default; other providers need mapping.
- `AIL_API_KEY` env var if your provider requires it.

## Start / stop

Managed by the daemon:

```sh
makakoo plugin info agent-multimodal-knowledge
makakoo plugin disable agent-multimodal-knowledge
makakoo plugin enable agent-multimodal-knowledge
makakoo daemon restart
```

## Where it writes

- **State:** `~/MAKAKOO/state/agent-multimodal-knowledge/` — temp buffers for in-flight media.
- **Logs:** `~/MAKAKOO/data/logs/agent-multimodal-knowledge.{out,err}.log`
- **Knowledge chunks (when routed through ingest):** `~/MAKAKOO/data/knowledge/` (shared with `agent-knowledge-extractor`).

## Health signals

- Any `harvey_describe_{image,audio,video}` call from an infected CLI returns a non-empty response.
- Logs show successful HTTP 200 from the configured embedding/model gateway.
- `ps -ef | grep query.py` — one running process.

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| `unknown provider for model mimo-v2-omni` | Model alias not registered with your gateway | Set an explicit model name via the tool's optional `model` arg, or switch provider. |
| `rate limit` / `429` | Gateway per-minute cap | Slow down the call rate. The MCP wrapper retries with exponential backoff automatically. |
| YouTube URL describe fails | `yt-dlp` missing or restricted by region | `brew install yt-dlp`. For region blocks, re-run with a VPN or via a different peer (e.g. a Tytus pod). |
| Describe returns a URL literal as the "answer" | **Anti-pattern:** a rate-limited describe was journaled as a workaround | Do NOT journal URLs to paper over rate limits. Retry when the gateway is healthy, or route to `harvey_knowledge_ingest` directly (different embedding path). |

## Capability surface

- `llm/chat` — describe + summarize pipeline.
- `llm/embed` — chunk embedding.
- `net/http:*` — downloading media.
- `fs/read:$MAKAKOO_HOME/plugins/agent-multimodal-knowledge`
- `fs/write:$MAKAKOO_HOME/data/knowledge`

## Remove permanently

```sh
makakoo plugin uninstall agent-multimodal-knowledge --purge
```

If you remove this agent, `harvey_describe_*` tools disappear from every infected CLI's MCP tool list. The Brain and existing knowledge chunks are unaffected — they belong to `agent-knowledge-extractor` and the unified Brain store.
