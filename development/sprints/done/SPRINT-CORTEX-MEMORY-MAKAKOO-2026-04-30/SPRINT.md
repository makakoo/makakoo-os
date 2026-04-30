# SPRINT: Native Cortex Memory for Makakoo HarveyChat

**Date:** 2026-04-30  
**Owner:** Sebastian / Harvey  
**Status:** completed — shipped 2026-04-30  
**Predecessor:** `TYTUS-CHAT-CORTEX-RESEARCH-2026-04-30`  
**First-draft archive:** `archive-v1-first-draft/`

## Executive decision

This sprint does **not** integrate the hosted Traylinx Cortex FastAPI service.

This sprint builds a **Cortex-inspired native memory layer inside Makakoo OS** for HarveyChat:

- same process as HarveyChat
- same SQLite database configured by `ChatConfig.db_path`
- FTS5 search first
- no Docker
- no Postgres
- no Redis
- no HTTP service
- default **off** behind `MAKAKOO_CORTEX_ENABLED=1`
- fail-open: if memory fails, HarveyChat continues ChatStore-only

Why: Makakoo is Sebastian's local cognitive extension. Personal assistant memory should be local, inspectable, low-ops, and reversible. Real Traylinx Cortex remains the right tool for TytusOS hosted/multi-tenant chat, not this Makakoo sprint.

## Non-negotiable scope boundaries

### In scope

- Local long-term memory for HarveyChat conversations.
- Conservative memory extraction from user+assistant turns.
- Cross-channel identity aliasing for Sebastian across Discord/Telegram/future channels.
- Memory retrieval and injection into HarveyChat system prompt.
- PII/secret scrubbing before long-term memory writes.
- Tests proving disabled mode keeps old behavior.
- Tests proving restart durability and fail-open behavior.

### Out of scope

- Hosted Traylinx Cortex API.
- `/v1/session`, `/v1/chat`, `/v1/memory/search` HTTP client.
- Postgres/pgvector, Redis, Celery, Docker Compose.
- sqlite-vec embeddings.
- Full semantic search.
- Backfilling all historic ChatStore messages.
- TytusOS UI work.
- Replacing Brain, auto-memory, or superbrain.

## Current code reality

Code paths are under:

```text
plugins/lib-harvey-core/src/core/
```

Relevant existing files:

```text
plugins/lib-harvey-core/src/core/chat/config.py
plugins/lib-harvey-core/src/core/chat/store.py
plugins/lib-harvey-core/src/core/chat/gateway.py
plugins/lib-harvey-core/src/core/chat/bridge.py
```

Current HarveyChat default DB is not `store.db`; it is whatever `ChatConfig.db_path` resolves to. At time of spec review, default is:

```text
$MAKAKOO_HOME/data/chat/conversations.db
```

Implementation must use `self.store.db_path` or `config.db_path`. No hardcoded database filename.

## Deliverables

### New module

```text
plugins/lib-harvey-core/src/core/cortex/
├── __init__.py
├── config.py          # CortexConfig dataclass + env parsing helpers
├── identity.py        # channel identity → canonical person mapping
├── memory.py          # SQLite schema + CRUD + FTS5 search
├── extractor.py       # conservative memory candidate extraction
├── scrubber.py        # PII/secret scrubbing, fail-closed when configured
└── models.py          # dataclasses / typed dicts for sessions, memories, candidates
```

### Modified existing code

```text
plugins/lib-harvey-core/src/core/chat/config.py   # add CortexConfig section
plugins/lib-harvey-core/src/core/chat/gateway.py  # session lifecycle + search + post-turn memory write
plugins/lib-harvey-core/src/core/chat/bridge.py   # memory prompt injection
```

### Tests

```text
plugins/lib-harvey-core/src/core/cortex/tests/
├── test_config.py
├── test_identity.py
├── test_memory_store.py
├── test_extractor.py
├── test_scrubber.py
└── test_chat_integration.py
```

If local project test convention demands another test root, adapt, but keep tests near `core/cortex` unless there is a strong reason not to.

## Feature flag behavior

| Mode | Expected behavior |
|---|---|
| `MAKAKOO_CORTEX_ENABLED=0` or unset | No Cortex object, no Cortex schema creation, no memory search, no memory write. HarveyChat works exactly as before. |
| `MAKAKOO_CORTEX_ENABLED=1` | Cortex schema initialized lazily against `ChatConfig.db_path`, memory search injected into prompt, memory candidates written after successful responses. |
| Cortex init/search/write raises | Log warning, continue ChatStore-only for that turn. Never crash chat. |

## Ship gates

Automated:

1. Unit tests pass for config, identity mapping, memory CRUD, FTS triggers, scrubber, extractor.
2. Integration test with mocked bridge proves `MAKAKOO_CORTEX_ENABLED=0` does not create Cortex tables.
3. Integration test with mocked bridge proves `MAKAKOO_CORTEX_ENABLED=1` creates session + memory row and injects memory on follow-up.
4. Restart test proves memory survives new `CortexMemory` instance using same temp DB.
5. Cross-channel alias test proves `discord:<id>` and `telegram:<id>` can both map to `person:sebastian` and retrieve shared memory.
6. PII/secret test proves raw SSN/API-key-like values are not stored in `cortex_memories.content`.
7. Fail-open test proves broken Cortex search/write still returns bridge response.

Manual:

1. Start HarveyChat with Cortex disabled; `/status`, normal chat, `/clear` still work.
2. Start HarveyChat with Cortex enabled; send “Remember: I prefer owl mascots.”
3. Restart HarveyChat; ask “What mascot style do I prefer?”
4. Confirm memory appears in bridge prompt block and answer references owl mascots.
5. Send same follow-up from second mapped channel; same memory is found.
6. Paste fake secret; confirm `cortex_memories` contains redacted content only.

## Success definition

Sprint is done when HarveyChat has safe, local, cross-session memory with no external service dependency and no regression when disabled.

It is **not** done when code merely stores assistant responses. Memory must be extracted, scrubbed, provenance-tagged, searchable, and fail-open.

## Results (implementation + closeout 2026-04-30)

Implemented native Cortex Memory MVP in `plugins/lib-harvey-core/src/core/cortex/` and wired HarveyChat config, gateway, bridge, and store integration.

What shipped:

- `CortexConfig` with config-file + env override support.
- SQLite schema for aliases, sessions, memories, FTS5 index, and sync triggers.
- Session epoch model with `/clear` ending active Cortex session.
- Cross-channel alias API for shared `person_id` memory.
- Conservative rule-based extractor; no raw assistant-response dumping.
- Fallback PII/secret scrubber with credential/SSN/email redaction.
- FTS5 prefix search with query sanitization.
- Bridge memory prompt block: bounded, system-context only, current user message wins conflicts.
- Gateway fail-open integration for init/search/write failures.
- Focused tests under `plugins/lib-harvey-core/src/core/cortex/tests/`.

Verification:

```bash
PYTHONPYCACHEPREFIX=/tmp/pycache-cortex python3 -m py_compile \
  plugins/lib-harvey-core/src/core/cortex/*.py \
  plugins/lib-harvey-core/src/core/chat/config.py \
  plugins/lib-harvey-core/src/core/chat/store.py \
  plugins/lib-harvey-core/src/core/chat/bridge.py \
  plugins/lib-harvey-core/src/core/chat/gateway.py

PYTHONPATH=plugins/lib-harvey-core/src PYTHONPYCACHEPREFIX=/tmp/pycache-cortex \
  python3 -m pytest plugins/lib-harvey-core/src/core/cortex/tests -q
```

Result: `16 passed`.

Known follow-ups:

- Manual Discord/Telegram dogfood still needed with real HarveyChat daemon.
- User-facing alias management command/UI not included.
- Semantic embeddings/sqlite-vec not included.
- Historic ChatStore backfill intentionally not included.


Closeout dogfood:

- Live config updated with `cortex.enabled=true` in `data/chat/config.json` (local state, not committed).
- Real Sebastian aliases set in live `data/chat/conversations.db`:
  - `telegram:746496145` → `person:sebastian`
  - `discord:870588390846889996` → `person:sebastian`
- Live DB dogfood PASS: Telegram-created memory recalled through Discord alias after new `HarveyChat` instance.
- `/status` now includes `Cortex Memory: enabled|disabled`.
- Real daemon restarted with Cortex-enabled config; duplicate stale poller killed; final daemon PID healthy with Telegram polling 200 OK and no conflict loop.
