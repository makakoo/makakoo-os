# HarveyChat Cortex Memory

HarveyChat Cortex Memory is Makakoo OS' native long-term memory layer for the external chat gateway. It lives inside `plugins-core/lib-harvey-core/src/core/cortex/` and stores memory in the existing local HarveyChat SQLite database. It is not a separate Cortex service and it does not require Docker, Postgres, Redis, or a second repository.

## What it adds

- Durable cross-session memory for HarveyChat.
- PII-scrubbed memory extraction from user/assistant turns.
- SQLite FTS5 retrieval before each LLM turn.
- Bounded `## Relevant Local Memory` prompt injection.
- Explicit cross-channel aliases so Telegram and Discord can share the same person memory.
- Fail-open behavior: memory errors are logged and chat still works.

## Runtime flow

```text
Telegram / Discord
  -> core.chat.gateway.HarveyChat.handle_message()
  -> ChatStore writes normal conversation history
  -> CortexMemory creates/updates session
  -> CortexMemory.search(user message)
  -> HarveyBridge injects relevant local memory
  -> LLM/tool loop runs through switchAILocal
  -> CortexMemory.record_turn(user, assistant)
```

ChatStore remains the fast recent-history store. Cortex is an augmenting long-term memory layer.

## Configuration

Edit `~/MAKAKOO/data/chat/config.json`:

```json
{
  "cortex": {
    "enabled": true,
    "memory_limit": 5,
    "min_confidence": 0.7,
    "min_importance": 0.4,
    "pii_scrubbing": true,
    "max_memory_chars": 500,
    "max_prompt_memory_chars": 1200,
    "max_memory_age_days": 365,
    "app_id": "makakoo-harveychat"
  }
}
```

Environment overrides:

| Variable | Meaning |
|---|---|
| `MAKAKOO_CORTEX_ENABLED` | enable/disable Cortex Memory |
| `HARVEY_CORTEX_ENABLED` | compatibility alias for enabled only |
| `MAKAKOO_CORTEX_MEMORY_LIMIT` | max memories retrieved per turn |
| `MAKAKOO_CORTEX_MIN_CONFIDENCE` | minimum extraction confidence |
| `MAKAKOO_CORTEX_MIN_IMPORTANCE` | minimum extraction importance |
| `MAKAKOO_CORTEX_PII_SCRUBBING` | enable/disable PII scrubbing |
| `MAKAKOO_CORTEX_MAX_MEMORY_CHARS` | max stored memory length |
| `MAKAKOO_CORTEX_MAX_PROMPT_CHARS` | max injected prompt memory block |
| `MAKAKOO_CORTEX_MAX_AGE_DAYS` | memory retention window |

## Cross-channel aliases

Aliases are explicit. Do not merge identities by display name.

```bash
cd ~/MAKAKOO
PYTHONPATH=plugins/lib-harvey-core/src python3 - <<'PY'
from core.cortex import CortexConfig, CortexMemory

memory = CortexMemory('data/chat/conversations.db', CortexConfig(enabled=True))
memory.set_alias('telegram', '<telegram-user-id>', 'person:sebastian', 'Sebastian Telegram')
memory.set_alias('discord', '<discord-user-id>', 'person:sebastian', 'Sebastian Discord')
PY
```

After aliases exist, a memory learned in Telegram can be recalled in Discord, and the reverse.

## Inspecting memory

```bash
sqlite3 ~/MAKAKOO/data/chat/conversations.db \
  "SELECT memory_type, confidence, importance, content FROM cortex_memories ORDER BY created_at DESC LIMIT 10;"

sqlite3 ~/MAKAKOO/data/chat/conversations.db \
  "SELECT channel, channel_user_id, person_id, label FROM cortex_user_aliases ORDER BY channel;"
```

## Status and rollback

`/status` in HarveyChat includes:

```text
Cortex Memory: enabled
```

Disable and restart to roll back:

```bash
MAKAKOO_CORTEX_ENABLED=0 PYTHONPATH=. python3 -m core.chat start
```

Or set `"cortex": { "enabled": false }` in `data/chat/config.json` and restart HarveyChat.
