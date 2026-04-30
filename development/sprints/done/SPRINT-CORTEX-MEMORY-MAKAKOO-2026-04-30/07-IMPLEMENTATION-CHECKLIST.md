# 07 — Implementation Checklist

Use this as execution order. Do not start Phase 4 before Phases 1–3 tests exist.

## Phase 0 — Preflight

- [ ] Confirm current git status.
- [ ] Confirm no active unrelated changes in `plugins/lib-harvey-core/src/core/chat/`.
- [ ] Read `core/chat/config.py`, `store.py`, `gateway.py`, `bridge.py` before editing.
- [ ] Keep first-draft docs in `archive-v1-first-draft/` untouched.

## Phase 1 — Config and schema

- [ ] Add `core/cortex/config.py`.
- [ ] Add `CortexConfig` to `ChatConfig`.
- [ ] Parse config JSON `cortex` section.
- [ ] Add env overrides.
- [ ] Add `core/cortex/models.py`.
- [ ] Add `core/cortex/memory.py` schema init.
- [ ] Add tests for config and schema.

Exit gate:

- [ ] `MAKAKOO_CORTEX_ENABLED=0` creates no tables through gateway integration test.
- [ ] direct `CortexMemory(... enabled=True)` creates expected tables.

## Phase 2 — Identity and sessions

- [ ] Add `core/cortex/identity.py` or identity methods in memory store.
- [ ] Implement alias table helpers.
- [ ] Implement active session epoch model.
- [ ] Implement `end_session()`.
- [ ] Add tests for alias and session behavior.

Exit gate:

- [ ] Discord and Telegram aliases can resolve to same `person_id`.
- [ ] `/clear` equivalent ends session but leaves memories.

## Phase 3 — Memory CRUD/search/extraction/scrubbing

- [ ] Implement FTS-safe query sanitizer.
- [ ] Implement `create_memory()` with thresholds and dedupe.
- [ ] Implement `search()` with pruning/access-count.
- [ ] Implement fallback scrubber.
- [ ] Optional: Presidio lazy integration.
- [ ] Implement rule-based extractor.
- [ ] Implement `record_turn()`.
- [ ] Add unit tests.

Exit gate:

- [ ] raw assistant response is never blindly stored.
- [ ] fake secrets are redacted or candidate write is skipped.
- [ ] search works after restart.

## Phase 4 — Bridge injection

- [ ] Add `memories` arg to `HarveyBridge.send()`.
- [ ] Add `_format_memories()`.
- [ ] Add memory block to `_build_system_prompt()`.
- [ ] Add bridge tests with mocked agent.

Exit gate:

- [ ] prompt block bounded.
- [ ] memory conflict warning present.
- [ ] no fake history messages.

## Phase 5 — Gateway integration

- [ ] Initialize Cortex best-effort in `HarveyChat.__init__`.
- [ ] Capture user message ID from ChatStore.
- [ ] Get/create Cortex session on normal messages.
- [ ] Search memories before bridge fast path.
- [ ] Pass memories to `_bridge_send_with_file_hints()`.
- [ ] Record turn after successful bridge response.
- [ ] End Cortex session on `/clear`.
- [ ] Add fail-open integration tests.

Exit gate:

- [ ] Chat works with Cortex disabled.
- [ ] Chat works with Cortex enabled.
- [ ] Search/write failures do not crash chat.

## Phase 6 — Manual dogfood

- [ ] Start HarveyChat disabled; smoke test.
- [ ] Start HarveyChat enabled; remember/follow-up/restart test.
- [ ] Cross-channel alias manual test if both IDs available.
- [ ] Fake-secret DB inspection.
- [ ] Check logs for raw message leaks.

## Phase 7 — Documentation closeout

- [ ] Update this sprint `Results` section in `SPRINT.md`.
- [ ] Add commands used and test results.
- [ ] Log significant result to Brain journal.
- [ ] If implementation is deferred, record blockers explicitly.

## Hard no-go signs

Stop and revise if any appears:

- implementation starts adding Docker/Postgres/Redis
- `core/cortex/client.py` HTTP client appears
- memory write stores `response[:500]`
- disabled mode creates `cortex_*` tables
- PII scrub failure stores raw candidate
- gateway crash caused by Cortex exception
- current ChatStore behavior changes when disabled
