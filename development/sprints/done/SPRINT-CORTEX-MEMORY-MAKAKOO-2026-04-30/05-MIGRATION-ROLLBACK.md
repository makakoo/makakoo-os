# 05 — Migration, Rollback, and Data Safety

## No required migration

This sprint is additive. ChatStore remains the source of truth for recent conversation history.

When disabled, Cortex must not create tables or write data.

When enabled, Cortex creates `cortex_*` tables in the same SQLite DB and writes extracted long-term memories only.

## No historical backfill in MVP

Do not backfill old ChatStore history during implementation.

Reasons:

- old assistant responses may contain contaminated claims
- PII/secret scrub risk is higher on bulk import
- memory extraction needs dogfood before bulk trust
- backfill is not needed to prove current-turn value

Optional future backfill must be a separate script/sprint with dry-run, review report, and deletion plan.

## Rollback

Disable:

```bash
MAKAKOO_CORTEX_ENABLED=0 python3 -m core.chat start --daemon
```

Expected rollback result:

- HarveyChat starts normally.
- Existing ChatStore history still available.
- `cortex_*` tables may remain on disk but are ignored.
- No memory injection appears in prompts.
- No new `cortex_*` writes happen.

## Table removal

Only for manual cleanup after backup:

```bash
sqlite3 "$MAKAKOO_HOME/data/chat/conversations.db" \
  "DROP TABLE IF EXISTS cortex_user_aliases; \
   DROP TABLE IF EXISTS cortex_sessions; \
   DROP TABLE IF EXISTS cortex_memories; \
   DROP TABLE IF EXISTS cortex_memories_fts;"
```

Do not put table drops in code.

## Data safety invariants

- Raw ChatStore messages are untouched.
- Cortex never deletes ChatStore rows.
- `/clear` does not delete long-term memory.
- Memory deletion APIs operate only on `cortex_memories` unless explicitly named otherwise.
- If memory write fails after ChatStore write, response still succeeds.

## Privacy controls in MVP

Required:

- global enable/disable flag
- PII/secret scrubbing before memory write
- `delete_person_memories(person_id)` API
- `delete_memory(memory_id, person_id)` API

Not required:

- user-facing Telegram/Discord commands for deletion
- Settings UI
- per-row “forget this” affordance

## Inspectability

Provide debugging snippets in docs/tests:

```bash
sqlite3 "$MAKAKOO_HOME/data/chat/conversations.db" \
  "SELECT memory_type, confidence, importance_score, content, datetime(created_at, 'unixepoch') FROM cortex_memories ORDER BY created_at DESC LIMIT 10;"
```

```bash
sqlite3 "$MAKAKOO_HOME/data/chat/conversations.db" \
  "SELECT person_id, app_id, channel, active, message_count FROM cortex_sessions ORDER BY updated_at DESC LIMIT 10;"
```

## Future migration path

If native memory proves useful, later sprints can add:

1. semantic embeddings via sqlite-vec or Qdrant bridge
2. Brain promotion for high-confidence project decisions
3. explicit `/remember`, `/forget`, `/memories` commands
4. historical backfill with review
5. hosted Traylinx Cortex sync for multi-device cloud memory

None of these belong in this sprint.
