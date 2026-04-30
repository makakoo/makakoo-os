# 05 — Migration from ChatStore

## Principle: No Migration Required

ChatStore is NOT replaced. It stays as the local cache. Cortex is additive.

**When `MAKAKOO_CORTEX_ENABLED=0`:** Code is bit-for-bit identical to today.
**When `MAKAKOO_CORTEX_ENABLED=1`:** ChatStore still gets every message. Cortex gets a copy.

## Dual-Write Strategy

```
Every incoming message:
  → ChatStore.add_message()     [ALWAYS]
  → Cortex.add_message()        [IF enabled]

Every outgoing response:
  → ChatStore.add_message()     [ALWAYS]
  → Cortex.add_message()        [IF enabled]
```

ChatStore = source of truth for "what happened in this session."
Cortex = source of truth for "what do I remember across all sessions."

## If You Want to Backfill ChatStore History into Cortex

Optional. Not required for the sprint to ship.

```python
# One-time backfill script
from core.chat.store import ChatStore
from core.cortex import get_cortex_client

store = ChatStore("data/chat/store.db")
cortex = get_cortex_client()

for row in store.get_all_sessions():  # hypothetical method
    channel, user_id = row["channel"], row["user_id"]
    session_id = cortex.get_or_create_session(channel, user_id)
    
    for msg in store.get_history(channel, user_id, limit=1000):
        cortex.add_message(session_id, msg["role"], msg["content"])
```

Run once, then forget about it. All new messages dual-write automatically.

## Rollback Plan

If Cortex causes problems:

1. `export MAKAKOO_CORTEX_ENABLED=0`
2. Restart gateway: `python3 -m core.chat stop && python3 -m core.chat start --daemon`
3. ChatStore still has all messages. Zero data loss.
4. Optional: `docker compose -f docker-compose.local.yml down` to stop Cortex containers.

## Roll Forward Plan

Once Cortex is stable and proven:

1. Consider making `MAKAKOO_CORTEX_ENABLED=1` the default.
2. Consider reducing ChatStore retention (e.g., keep only last 100 messages locally, let Cortex hold the long tail).
3. Consider dropping ChatStore entirely for history — use it only for offline fallback.

**These are post-sprint decisions.** Ship with dual-write first.
