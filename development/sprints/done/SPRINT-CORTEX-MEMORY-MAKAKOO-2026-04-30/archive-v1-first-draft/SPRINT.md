# SPRINT: Cortex Memory for Makakoo Agents

**Date:** 2026-04-30
**Owner:** Sebastian (Makakoo OS)
**Status:** queued
**Predecessor:** `TYTUS-CHAT-CORTEX-RESEARCH-2026-04-30`

## Scope

Wire Traylinx Cortex into Makakoo OS as a **local memory backend** for chat-connected agents. Cortex runs on the same machine as Makakoo (localhost), no Sentinel auth, no external hosting. Provides persistent cross-session memory, auto-summarization, and PII scrubbing for Harvey's conversations on Discord, Telegram, and future channels.

Makakoo's existing chat adapters (Discord, Telegram), gateway, bridge, and tool system stay untouched. Only the **history store** and **memory injection** layers change.

## Deliverables

- [ ] Cortex runs locally on Sebastian's machine (Docker Compose or native)
- [ ] `core/cortex/client.py` — thin HTTP client wrapping Cortex `/v1/session`, `/v1/chat`, `/v1/memory/search`
- [ ] `core/chat/store.py` — dual-write mode: ChatStore (local SQLite) + Cortex (Postgres)
- [ ] `core/chat/bridge.py` — memory injection: query Cortex memories before LLM call, append to system prompt
- [ ] `core/chat/gateway.py` — session lifecycle: create/get Cortex session on first message, add messages to session
- [ ] Feature flag `MAKAKOO_CORTEX_ENABLED` — default off, toggle via env var
- [ ] Fallback: if Cortex is down, degrade to ChatStore-only (no memory injection)
- [ ] Test: verify Discord + Telegram both feed into same user memory graph
- [ ] Test: verify memory survives Makakoo restart
- [ ] Test: verify PII scrubbing is on by default, can be disabled per-user

## Ship Gate

1. `python3 -m core.chat start --daemon` boots with `MAKAKOO_CORTEX_ENABLED=1`
2. Send message on Discord → Cortex session created, message stored
3. Restart Makakoo, send follow-up on same Discord channel → Cortex recalls previous context
4. Send message on Telegram as same user → Cortex surfaces Discord memory in system prompt
5. `MAKAKOO_CORTEX_ENABLED=0` → runs exactly as before (ChatStore-only)
6. Kill Cortex container → Makakoo degrades gracefully, no crash

## Architecture (simplified)

```
Discord ──┐
Telegram ─┼──→ Gateway ──→ ChatStore (SQLite, local cache)
          │       │
          │       └──────→ Cortex Client ──→ Cortex API (localhost:8000)
          │                                   ├── Postgres + pgvector
          │                                   └── Redis
          │
          └──────────────────────────────────→ LLM (switchAILocal)
                                               ↑
                                         Memories injected
                                         into system prompt
```

## Files to touch

```
core/cortex/
├── __init__.py
├── client.py          # Cortex HTTP client
├── config.py          # CortexConfig dataclass
└── models.py          # Pydantic models for Cortex responses

core/chat/
├── store.py           # Dual-write: add_message writes to both stores
├── bridge.py          # Memory injection before LLM call
└── gateway.py         # Session create/get on first message

data/chat/config.json  # Add cortex section
```

## Results (post-ship)
(TBD)
