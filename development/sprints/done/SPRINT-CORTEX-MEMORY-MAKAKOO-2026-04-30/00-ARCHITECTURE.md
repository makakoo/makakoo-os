# 00 — Architecture

## Principle

Native Cortex Memory is a Makakoo-local memory layer for HarveyChat. It borrows the useful ideas from Traylinx Cortex — sessions, durable memory, summaries later, PII scrubbing, search — but implements only the local subset needed by Sebastian's Makakoo assistant.

```text
Discord ──┐
Telegram ─┼──→ Gateway ──→ ChatStore SQLite (`ChatConfig.db_path`)
Future  ──┘       │             ├── messages      existing
                  │             ├── sessions      existing HarveyChat session cache
                  │             ├── cortex_*      new native memory tables
                  │
                  ├──→ CortexMemory module
                  │       ├── identity resolver
                  │       ├── active memory session
                  │       ├── FTS5 memory search
                  │       ├── conservative extractor
                  │       └── PII/secret scrubber
                  │
                  └──→ Bridge → HarveyAgent → switchAILocal
                               ↑
                     relevant memories injected into system prompt
```

No service boundary. No network. No containers.

## Relation to existing memory systems

| System | Role | This sprint changes it? |
|---|---|---|
| ChatStore `messages` | Short-term chat transcript and recent history | No schema replacement; still source of truth for recent turns. |
| Brain journals/pages | Narrative project memory, manually/agent-written | No. Cortex can complement but not replace. |
| auto-memory | Curated durable cross-session lessons | No. Cortex is chat-connected local LTM. |
| superbrain/Qdrant | Broad retrieval across Brain/media/etc. | No. Cortex is low-latency per-person chat memory. |
| Traylinx Cortex | Hosted production cognitive service | Not integrated in this sprint. |

## Data flow per normal message

```text
1. Channel adapter calls HarveyChat.handle_message(channel, user_id, username, text).
2. Gateway handles commands/routing as today.
3. ChatStore.add_message(...) writes raw user message as today.
4. If Cortex enabled:
   a. Resolve canonical person_id via CortexIdentityResolver.
   b. Get/create active cortex session for person/app/channel.
   c. Search long-term memories using current user text.
5. Gateway calls bridge with `memories=[...]`.
6. Bridge appends a system prompt block:
   "## Relevant Local Memory"
   - [preference, 2026-04-30, confidence 0.82] Sebastian prefers owl mascots.
7. HarveyAgent responds.
8. ChatStore.add_message(...) writes raw assistant response as today.
9. If Cortex enabled:
   a. Extract memory candidates from user text + assistant response + channel metadata.
   b. Scrub candidate content.
   c. Write only candidates above confidence/importance thresholds.
10. Return response to channel.
```

## Commands and special cases

| Event | Cortex behavior |
|---|---|
| `/status` | No memory search/write. Optional status line may later include Cortex enabled/disabled; not required MVP. |
| `/clear` | Clears active HarveyChat task/context as today. Ends current Cortex session epoch and creates a new one on next message. Does **not** delete long-term memories. |
| `/forget` | Out of scope unless already exists. Future sprint. |
| workflow-routed task | Out of scope for first integration unless it reaches bridge fast path. Do not block workflows on Cortex. |
| bridge/LLM failure | Do not write memory. Memory extraction only after successful assistant response. |

## Session model

Do **not** use one forever session per `user_id/app_id`.

Use session epochs:

- one active session per `(person_id, app_id, channel)` at a time
- `/clear` ends current active session
- restart reloads latest active session or creates one
- long-term memories are scoped to `person_id`, not session

## Memory model

Memory rows are not raw transcript snippets by default. They are extracted facts/preferences/decisions with provenance.

Examples of acceptable memories:

- `Sebastian prefers concise, caveman-style replies in internal Makakoo work.`
- `Sebastian chose native SQLite memory over hosted Traylinx Cortex for Makakoo HarveyChat on 2026-04-30.`
- `Project TytusOS pod-readiness work uses ProjectWannolot.`

Examples of unacceptable memories:

- assistant response first 500 chars
- raw API keys/secrets/passwords
- unverified claims with low confidence
- transient task chatter like “Thinking...” or “I’ll do that”

## Search model

MVP uses SQLite FTS5 only.

Reasons:

- no additional dependency
- available in Python SQLite on macOS/Linux in most builds
- enough for first iteration
- semantic search can be a later `sqlite-vec`/embedding phase

Search should combine:

- FTS5 rank
- importance score
- recency/decay
- access count optional

Return top `memory_limit` results.

## Failure model

Cortex Memory is never allowed to make chat unavailable.

| Failure | Behavior |
|---|---|
| schema init fails | log warning; `self.cortex = None`; continue ChatStore-only |
| search fails | log warning; send no memories for this turn |
| extractor fails | log warning; skip writes |
| scrubber fails while scrubbing enabled | skip candidate write, log warning |
| FTS unavailable | init fails and feature disables itself |

## Security/privacy model

- Raw ChatStore remains unchanged. It already stores raw local chat history.
- `cortex_memories.content` must be scrubbed before write.
- Memory prompt injection must be bounded by count and chars.
- Memories are system prompt context, not fake assistant/user messages.
- Every memory row has source/provenance so bad memories can be inspected/deleted.
