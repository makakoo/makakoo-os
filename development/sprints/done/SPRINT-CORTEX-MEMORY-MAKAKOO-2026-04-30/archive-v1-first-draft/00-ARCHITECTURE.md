# 00 — Architecture (Native Integration)

## Principle

Cortex is NOT a separate service. It is a **Python module inside Makakoo OS** that uses the **same SQLite** database Makakoo already has. No Docker. No Postgres. No Redis. No FastAPI server.

```
Discord ──┐
Telegram ─┼──→ Gateway ──→ ChatStore (SQLite, data/chat/store.db)
          │       │            ├── messages table (existing)
          │       │            ├── sessions table (NEW)
          │       │            ├── memories table (NEW)
          │       │            └── memory_embeddings (NEW, sqlite-vec)
          │       │
          │       └──────→ Cortex Module (Python, same process)
          │                    ├── SessionManager
          │                    ├── MemoryStore (SQLite)
          │                    ├── Summarizer (LLM call)
          │                    └── PII Scrubber (presidio)
          │
          └──────────────────────────────────→ LLM (switchAILocal)
                                               ↑
                                         Memories injected
                                         into system prompt
```

## What Gets Added to Makakoo OS

```
core/cortex/
├── __init__.py          # exports
├── config.py            # CortexConfig
├── session.py           # SessionManager
├── memory.py            # MemoryStore (SQLite-backed)
├── search.py            # Vector + text search
├── summarizer.py        # Auto session summaries via LLM
├── scrubber.py          # PII detection/removal
└── models.py            # dataclasses

core/chat/
├── store.py             # Add sessions + memories tables
├── bridge.py            # Memory injection
└── gateway.py           # Session lifecycle
```

## SQLite Schema

Same `data/chat/store.db`. Three new tables:

```sql
-- Sessions
CREATE TABLE cortex_sessions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    title TEXT,
    summary TEXT,
    message_count INTEGER DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(user_id, app_id)
);

-- Memories (long-term)
CREATE TABLE cortex_memories (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    content TEXT NOT NULL,
    importance_score REAL DEFAULT 0.5,
    access_count INTEGER DEFAULT 0,
    last_accessed TIMESTAMP,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMP
);

-- Memory search index (FTS5 for text, sqlite-vec for vectors)
CREATE VIRTUAL TABLE cortex_memories_fts USING fts5(content, content='cortex_memories', content_rowid='rowid');

-- Or with sqlite-vec:
-- CREATE VIRTUAL TABLE cortex_memory_vectors USING vec0(embedding float[384]);
```

## Key Design Decisions

### 1. No Separate Database

Cortex uses the **same SQLite file** as ChatStore: `data/chat/store.db`. Just new tables. One connection. One backup. One file to copy.

### 2. No Vector Database

Options for memory search:
- **FTS5** (built into SQLite): Full-text search on memory content. Fast. Good enough for most cases.
- **sqlite-vec** (extension): Lightweight vector search. If we want semantic similarity.
- **Hybrid**: FTS5 for text, embeddings for semantic. Start with FTS5, add vectors later.

**Recommendation for sprint: FTS5 only.** Simple, no dependencies, works today.

### 3. No Redis

Session cache is an **in-memory dict** inside the Cortex module. If Makakoo restarts, sessions are reloaded from SQLite on first access.

```python
_session_cache: Dict[str, str] = {}  # key: "channel:user_id" → session_id
```

### 4. No FastAPI Server

Cortex is imported as a Python module:

```python
from core.cortex import CortexMemory

memory = CortexMemory(db_path="data/chat/store.db")
session_id = memory.get_or_create_session("discord", "123")
memories = memory.search("migration", "discord", "123")
```

### 5. PII Scrubbing

Use Microsoft's Presidio as a Python library (not a service):

```python
from presidio_analyzer import AnalyzerEngine
from presidio_anonymizer import AnonymizerEngine

analyzer = AnalyzerEngine()
anonymizer = AnonymizerEngine()

results = analyzer.analyze(text=content, language="en")
scrubbed = anonymizer.anonymize(text=content, analyzer_results=results)
```

Install: `pip install presidio-analyzer presidio-anonymizer`

### 6. Summarization

Auto-summarize sessions after N messages via LLM call (same switchAILocal):

```python
def summarize_session(messages: List[str]) -> str:
    prompt = "Summarize this conversation in 2 sentences:\n" + "\n".join(messages)
    return llm_call(prompt, max_tokens=100)
```

Run async (thread pool) so it doesn't block the chat response.

### 7. Temporal Decay

Memories have an `expires_at` field. Background task (or on every search) prunes old memories:

```sql
DELETE FROM cortex_memories WHERE expires_at < datetime('now');
```

Decay formula: `expires_at = created_at + (importance_score * 365 days)`
- High importance (0.9) → ~1 year
- Low importance (0.1) → ~36 days

## User Identity

Same as before: `f"{channel}:{user_id}"` (e.g., `"discord:1499167392503300157"`).

Cross-channel memory: if user_id is the same across Discord and Telegram, memories are shared.

## Data Flow (One Message)

```
1. User sends "What about the migration?" on Discord
   ↓
2. Gateway.handle_message("discord", "123", "Seb", "What about the migration?")
   ↓
3. ChatStore.add_message(...)  ← existing messages table
   ↓
4. IF CORTEX_ENABLED:
      session = cortex.get_session("discord", "123")
      cortex.add_message(session, "user", "What about the migration?")
      memories = cortex.search("What about the migration?", "discord:123")
   ↓
5. Bridge.build_system_prompt(memories=memories)
   ← base prompt + "## Relevant Past Context\n- (2026-04-22) You discussed AWS → DO migration..."
   ↓
6. HarveyAgent.process(message, history, enriched_prompt)
   ↓
7. LLM → "You decided to migrate to DigitalOcean on April 22..."
   ↓
8. ChatStore.add_message("assistant", response)
   ↓
9. IF CORTEX_ENABLED:
      cortex.add_message(session, "assistant", response)
      cortex.create_memory("User decided to migrate to DigitalOcean")
      IF message_count % 4 == 0:
          cortex.summarize_session(session)  # async
   ↓
10. Gateway sends response to Discord
```

## What Stays Unchanged

| Component | Status |
|---|---|
| Discord/Telegram adapters | Unchanged |
| Gateway dispatch logic | Unchanged |
| Tool system (TOOL_DISPATCH) | Unchanged |
| LLM calls (switchAILocal) | Unchanged |
| File sending (`[[SEND_FILE:...]]`) | Unchanged |
| Brain journaling | Unchanged |
| ChatStore (messages table) | Unchanged |

## What Gets Added

| Component | File | Description |
|---|---|---|
| SessionManager | `core/cortex/session.py` | Create/get sessions per user |
| MemoryStore | `core/cortex/memory.py` | SQLite-backed memory CRUD |
| Search | `core/cortex/search.py` | FTS5 + optional vector search |
| Summarizer | `core/cortex/summarizer.py` | LLM-based session summaries |
| PII Scrubber | `core/cortex/scrubber.py` | Presidio-based PII removal |
| Config | `core/cortex/config.py` | CortexConfig dataclass |
| Integration | `core/chat/store.py` | New tables in existing DB |
| Integration | `core/chat/bridge.py` | Memory injection |
| Integration | `core/chat/gateway.py` | Session lifecycle |
