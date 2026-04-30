# 01 — Cortex Local Setup (Native)

## Goal

Cortex runs as a Python module inside Makakoo OS. No Docker. No external services. Just `pip install` and go.

## Step 1: Add Dependencies

Add to Makakoo's `requirements.txt` or `pyproject.toml`:

```
presidio-analyzer>=2.2.0
presidio-anonymizer>=2.2.0
spacy>=3.7.0
# For sqlite-vec (optional, for vector search later):
# sqlite-vec>=0.1.0
```

Install:

```bash
cd ~/MAKAKOO/plugins/lib-harvey-core
pip install -e ".[cortex]"
# OR directly:
pip install presidio-analyzer presidio-anonymizer spacy
python -m spacy download en_core_web_lg
```

## Step 2: Initialize Database

Cortex adds tables to the **existing** `data/chat/store.db`. Run once:

```bash
cd ~/MAKAKOO/plugins/lib-harvey-core/src
python3 -c "
from core.cortex.memory import CortexMemory
m = CortexMemory('~/MAKAKOO/data/chat/store.db')
m.init_schema()
print('Cortex schema initialized')
"
```

Or automatically on first boot if tables don't exist.

## Step 3: Enable in Config

Add to `data/chat/config.json`:

```json
{
  "cortex": {
    "enabled": true,
    "memory_limit": 5,
    "auto_summarize_after": 4,
    "pii_scrubbing": true,
    "temporal_decay": 0.05,
    "min_importance": 0.3,
    "max_memory_age_days": 365
  }
}
```

Or via env vars:

```bash
export MAKAKOO_CORTEX_ENABLED=1
export MAKAKOO_CORTEX_MEMORY_LIMIT=5
export MAKAKOO_CORTEX_PII_SCRUBBING=1
```

## Step 4: Verify

```python
from core.cortex import CortexMemory

memory = CortexMemory()

# Create session
sid = memory.get_or_create_session("discord", "123")
print(f"Session: {sid}")

# Store a message
memory.add_message(sid, "user", "I like owls")

# Search memories
results = memory.search("what do I like?", "discord", "123")
print(results)
# Expected: [{"content": "I like owls", "importance_score": 0.5, ...}]
```

## Step 5: Start Makakoo

```bash
cd ~/MAKAKOO/plugins/lib-harvey-core/src
python3 -m core.chat start --daemon
```

That's it. No containers. No `docker compose`. Just Python + SQLite.

## Resource Usage

| Component | RAM | Notes |
|---|---|---|
| SQLite (existing) | ~10 MB | Already running |
| Presidio models | ~100 MB | Loaded on first PII scan |
| Cortex module | ~20 MB | In-memory session cache |
| **Total added** | **~130 MB** | One-time, not per-user |

## Uninstall / Disable

```bash
# Disable
export MAKAKOO_CORTEX_ENABLED=0

# Remove tables (optional)
sqlite3 data/chat/store.db "DROP TABLE IF EXISTS cortex_sessions; DROP TABLE IF EXISTS cortex_memories; DROP TABLE IF EXISTS cortex_memories_fts;"
```

Makakoo works exactly as before.
