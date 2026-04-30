# 02 — Memory Module Design

## Files

```text
plugins/lib-harvey-core/src/core/cortex/
├── __init__.py
├── config.py
├── identity.py
├── memory.py
├── extractor.py
├── scrubber.py
└── models.py
```

## SQLite schema

All tables live in the existing HarveyChat DB selected by `ChatConfig.db_path`.

```sql
CREATE TABLE IF NOT EXISTS cortex_user_aliases (
    channel TEXT NOT NULL,
    channel_user_id TEXT NOT NULL,
    person_id TEXT NOT NULL,
    label TEXT,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL,
    PRIMARY KEY (channel, channel_user_id)
);

CREATE TABLE IF NOT EXISTS cortex_sessions (
    id TEXT PRIMARY KEY,
    person_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    channel_user_id TEXT NOT NULL,
    title TEXT,
    summary TEXT,
    message_count INTEGER NOT NULL DEFAULT 0,
    active INTEGER NOT NULL DEFAULT 1,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL,
    ended_at REAL
);

CREATE INDEX IF NOT EXISTS idx_cortex_sessions_active
    ON cortex_sessions(person_id, app_id, channel, active, updated_at);

CREATE TABLE IF NOT EXISTS cortex_memories (
    id TEXT PRIMARY KEY,
    person_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    memory_type TEXT NOT NULL,
    content TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 0.0,
    importance_score REAL NOT NULL DEFAULT 0.5,
    source_channel TEXT,
    source_channel_user_id TEXT,
    source_session_id TEXT,
    source_message_id INTEGER,
    access_count INTEGER NOT NULL DEFAULT 0,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL,
    last_accessed REAL,
    expires_at REAL,
    metadata TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_cortex_memories_person
    ON cortex_memories(person_id, app_id, created_at DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS cortex_memories_fts USING fts5(
    content,
    memory_type UNINDEXED,
    content='cortex_memories',
    content_rowid='rowid'
);
```

FTS triggers:

```sql
CREATE TRIGGER IF NOT EXISTS cortex_memories_ai
AFTER INSERT ON cortex_memories BEGIN
    INSERT INTO cortex_memories_fts(rowid, content, memory_type)
    VALUES (new.rowid, new.content, new.memory_type);
END;

CREATE TRIGGER IF NOT EXISTS cortex_memories_ad
AFTER DELETE ON cortex_memories BEGIN
    INSERT INTO cortex_memories_fts(cortex_memories_fts, rowid, content, memory_type)
    VALUES ('delete', old.rowid, old.content, old.memory_type);
END;

CREATE TRIGGER IF NOT EXISTS cortex_memories_au
AFTER UPDATE ON cortex_memories BEGIN
    INSERT INTO cortex_memories_fts(cortex_memories_fts, rowid, content, memory_type)
    VALUES ('delete', old.rowid, old.content, old.memory_type);
    INSERT INTO cortex_memories_fts(rowid, content, memory_type)
    VALUES (new.rowid, new.content, new.memory_type);
END;
```

Use REAL timestamps via `time.time()` to match existing ChatStore style.

## Identity resolver

Default mapping:

```python
person_id = f"channel:{channel}:{channel_user_id}"
```

But support aliases:

```python
resolve_person_id("discord", "149...") -> "person:sebastian"
resolve_person_id("telegram", "123...") -> "person:sebastian"
```

Add helper:

```python
set_alias(channel: str, channel_user_id: str, person_id: str, label: str | None = None) -> None
```

Initial sprint does not need UI for alias setup. Tests can create aliases directly. Manual dogfood can insert rows with sqlite3 or helper.

## Public API

```python
class CortexMemory:
    def __init__(self, db_path: str, config: CortexConfig): ...

    def resolve_person_id(self, channel: str, channel_user_id: str) -> str: ...
    def set_alias(self, channel: str, channel_user_id: str, person_id: str, label: str | None = None) -> None: ...

    def get_or_create_session(self, channel: str, channel_user_id: str, username: str | None = None) -> str: ...
    def end_session(self, channel: str, channel_user_id: str) -> None: ...
    def increment_session_count(self, session_id: str, by: int = 1) -> None: ...

    def create_memory(self, candidate: MemoryCandidate, source: MemorySource) -> str | None: ...
    def search(self, query: str, channel: str, channel_user_id: str, limit: int | None = None) -> list[MemoryRecord]: ...
    def delete_memory(self, memory_id: str, person_id: str | None = None) -> bool: ...
    def delete_person_memories(self, person_id: str) -> int: ...
```

## Search query safety

Never pass raw user text directly to `MATCH`.

Implement `sanitize_fts_query(text)`:

- keep alphanumeric tokens longer than 2 chars
- drop punctuation/operators
- quote tokens if needed
- max 12 tokens
- if no tokens, return empty result without querying

Example:

```python
"What about AWS -> DO migration???" -> "what about aws migration"
```

## Memory extraction

`extractor.py` creates candidates. It must be conservative.

```python
@dataclass
class MemoryCandidate:
    content: str
    memory_type: Literal["preference", "decision", "fact", "project_context", "identity", "summary"]
    confidence: float
    importance_score: float
    metadata: dict = field(default_factory=dict)
```

MVP extractor may be rule-based first:

High-confidence patterns:

- user says `remember ...`
- user says `log that ...`
- user says `I prefer ...`
- user says `my X is ...` where X is non-secret preference/identity
- assistant confirms a project decision after user asked planning/spec work

Do not extract from assistant response alone.

For non-explicit memory, require confidence >= 0.80.

## Scrubbing

`scrubber.py` should expose:

```python
def scrub_memory_text(text: str, pii_enabled: bool = True) -> ScrubResult: ...
```

Fallback regex must catch at least:

- AWS access keys
- GitHub tokens
- OpenAI-style keys
- generic `password=...`, `token=...`, `api_key=...`
- SSN-like `123-45-6789`
- emails may be replaced unless needed for context

If scrubbing is enabled and scrubber errors, do not write the memory.

## Expiration

Set `expires_at` based on importance unless `memory_type` is identity/preference/decision with high importance.

Simple MVP rule:

```python
lifespan_days = max(30, min(config.max_memory_age_days, int(importance_score * config.max_memory_age_days)))
expires_at = now + lifespan_days * 86400
```

Identity/preference/decision with `importance_score >= 0.75` may get `expires_at = None`.

## Dedupe

MVP dedupe can be simple exact normalized content dedupe:

- lower
- collapse whitespace
- strip punctuation at ends

Before insert, skip if same `person_id`, `app_id`, normalized content exists.

Semantic dedupe is later.
