# 06 — Testing and QA Plan

## Test principles

- No live Discord/Telegram.
- No live switchAILocal/LLM calls.
- No production DB writes.
- Use temp SQLite DBs.
- Mock `HarveyBridge.send()` for integration tests.
- Disabled mode must be proven, not assumed.

## Unit tests

### `test_config.py`

Cases:

- default `CortexConfig.enabled is False`
- config JSON loads `cortex` section
- env overrides config file
- invalid env values fall back safely or raise clear config error

### `test_identity.py`

Cases:

- default identity maps to `channel:<channel>:<id>`
- alias insert maps Discord user to `person:sebastian`
- alias insert maps Telegram user to same `person:sebastian`
- alias update changes label/person correctly

### `test_memory_store.py`

Cases:

- schema creates all `cortex_*` tables and FTS table
- disabled mode does not initialize schema through gateway path
- create session returns UUID-like string
- `/clear` equivalent `end_session()` marks active=0
- new session after `end_session()` has different ID
- create memory writes row and FTS trigger row
- update/delete keeps FTS in sync
- search finds expected memory
- search with punctuation/operator text does not crash
- expired memory pruned/not returned
- exact duplicate memory skipped
- access count increments on search result
- restart/new instance can search old memory

### `test_extractor.py`

Cases:

- `Remember: I prefer owl mascots` -> preference candidate
- `log that we chose SQLite for Makakoo memory` -> decision/project candidate
- generic “thanks” -> no candidate
- assistant-only claim -> no candidate
- candidate below confidence threshold not written
- long candidate content truncated/rejected according to config

### `test_scrubber.py`

Cases:

- fake SSN redacted
- `api_key=sk-...` redacted
- GitHub token-like value redacted
- AWS key-like value redacted
- scrubber failure with `pii_scrubbing=true` prevents write
- scrubber disabled stores non-secret normal text; still redact obvious secrets if fallback is designed that way

### `test_bridge_memory.py`

Cases:

- `_format_memories([]) == ""`
- memory block has title `Relevant Local Memory`
- block includes current-user-overrides-memory warning
- content truncates at per-memory limit
- total block under configured/default max
- multiline content flattened
- `send(..., memories=...)` passes enriched prompt to mocked agent

## Integration tests

### Disabled mode no-op

Setup:

- temp DB
- `MAKAKOO_CORTEX_ENABLED=0`
- mocked bridge response: `ok`
- instantiate `HarveyChat(config)` with temp DB
- call `handle_message(...)`

Assert:

- response `ok`
- ChatStore `messages` has user+assistant rows
- no `cortex_%` tables exist
- bridge prompt does not include memory block

### Enabled mode first turn writes memory

Setup:

- temp DB
- `config.cortex.enabled=True`
- mocked bridge response: `Noted. I will remember you prefer owl mascots.`
- send `Remember: I prefer owl mascots.`

Assert:

- `cortex_sessions` has active row
- `cortex_memories` has scrubbed preference row
- memory provenance has source channel/user/session

### Enabled mode follow-up injects memory

Setup:

- create memory manually or via first turn
- send `What mascot style do I prefer?`
- mock bridge captures `system_prompt`

Assert:

- prompt contains `Relevant Local Memory`
- prompt contains `owl mascots`
- response returned normally

### Cross-channel alias

Setup:

- map Discord ID and Telegram ID to `person:sebastian`
- create memory from Discord
- search from Telegram

Assert:

- same memory returned

### Fail-open search

Setup:

- monkeypatch `cortex.search` to raise
- bridge mocked response `still works`

Assert:

- response `still works`
- no exception to caller

### Fail-open write

Setup:

- monkeypatch `cortex.record_turn` to raise
- bridge mocked response

Assert:

- response still returned
- ChatStore assistant row still written

## Manual QA

Run from:

```bash
cd ~/MAKAKOO/plugins/lib-harvey-core/src
```

### Disabled smoke

```bash
MAKAKOO_CORTEX_ENABLED=0 python3 -m core.chat start --daemon
```

Expected:

- starts
- chat works
- no memory block in logs/prompts

### Enabled smoke

```bash
MAKAKOO_CORTEX_ENABLED=1 python3 -m core.chat start --daemon
```

Send:

```text
Remember: I prefer owl mascots for Harvey.
```

Then restart and send:

```text
What mascot style do I prefer?
```

Expected: answer references owl mascots.

### DB inspection

```bash
sqlite3 "$MAKAKOO_HOME/data/chat/conversations.db" \
  "SELECT memory_type, confidence, content FROM cortex_memories ORDER BY created_at DESC LIMIT 5;"
```

### Fake secret smoke

Send fake secret:

```text
Remember this fake token for the test: api_key=sk-test-1234567890abcdef
```

Expected DB content does not include the raw fake token.

## Performance targets

| Metric | Target |
|---|---:|
| session get/create | <20ms p95 local |
| FTS memory search | <50ms p95 for 10k memories |
| record turn with rule extractor | <50ms p95 excluding optional Presidio cold start |
| prompt memory block | <1200 chars default |
| added RAM with Presidio not loaded | <20MB |

Presidio cold start may exceed target; lazy-load and avoid in disabled mode.
