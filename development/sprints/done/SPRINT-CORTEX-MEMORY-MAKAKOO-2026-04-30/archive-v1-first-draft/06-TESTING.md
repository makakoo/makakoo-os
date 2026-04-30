# 06 — Testing Plan

## Unit Tests

### `tests/cortex/test_memory.py`

```python
import pytest
import sqlite3
from core.cortex.memory import CortexMemory, CortexConfig

class TestCortexMemory:
    @pytest.fixture
    def memory(self, tmp_path):
        db = tmp_path / "test.db"
        return CortexMemory(str(db), CortexConfig(enabled=True, pii_scrubbing=False))

    def test_init_schema(self, memory):
        # Schema created on init
        with sqlite3.connect(memory.db_path) as conn:
            cursor = conn.execute("SELECT name FROM sqlite_master WHERE type='table'")
            tables = {r[0] for r in cursor.fetchall()}
            assert "cortex_sessions" in tables
            assert "cortex_memories" in tables
            assert "cortex_memories_fts" in tables

    def test_get_or_create_session(self, memory):
        sid1 = memory.get_or_create_session("discord", "123")
        assert len(sid1) == 36  # UUID
        
        sid2 = memory.get_or_create_session("discord", "123")
        assert sid1 == sid2  # Cached

    def test_create_and_search_memory(self, memory):
        memory.create_memory("I like owls", "discord", "123", importance=0.8)
        results = memory.search("owls", "discord", "123")
        assert len(results) == 1
        assert "owls" in results[0]["content"]

    def test_search_no_match(self, memory):
        memory.create_memory("I like owls", "discord", "123")
        results = memory.search("elephants", "discord", "123")
        assert len(results) == 0

    def test_cross_channel_same_user(self, memory):
        memory.create_memory("I like owls", "discord", "456")
        # Same user_id on different channel — if we want shared memory
        # This depends on user_id mapping; test the behavior we decide on

    def test_temporal_decay(self, memory):
        memory.create_memory("Old news", "discord", "123", importance=0.1)
        # Fast-forward expiry
        with sqlite3.connect(memory.db_path) as conn:
            conn.execute(
                "UPDATE cortex_memories SET expires_at = datetime('now', '-1 day')"
            )
            conn.commit()
        
        results = memory.search("Old news", "discord", "123")
        assert len(results) == 0  # Pruned

    def test_delete_user_memories(self, memory):
        memory.create_memory("Secret", "discord", "123")
        count = memory.delete_user_memories("discord", "123")
        assert count == 1
        results = memory.search("Secret", "discord", "123")
        assert len(results) == 0

    def test_pii_scrubbing(self, memory):
        memory.config.pii_scrubbing = True
        # This requires presidio installed; skip if not available
        pytest.importorskip("presidio_analyzer")
        
        mid = memory.create_memory("My SSN is 123-45-6789", "discord", "123")
        with sqlite3.connect(memory.db_path) as conn:
            cursor = conn.execute("SELECT content FROM cortex_memories WHERE id = ?", (mid,))
            row = cursor.fetchone()
            assert "123-45-6789" not in row[0]
```

### `tests/cortex/test_bridge_memory.py`

```python
def test_format_memories_empty():
    bridge = HarveyBridge(mock_config)
    assert bridge._format_memories([]) == ""
    assert bridge._format_memories(None) == ""

def test_format_memories_truncate():
    bridge = HarveyBridge(mock_config)
    long_mem = {"content": "x" * 300}
    result = bridge._format_memories([long_mem])
    assert "..." in result
    assert len(result) < 300

def test_system_prompt_with_memories():
    bridge = HarveyBridge(mock_config)
    memories = [{"content": "User likes owls"}]
    prompt = bridge._build_system_prompt("discord", memories=memories)
    assert "Relevant Context from Past Conversations" in prompt
    assert "User likes owls" in prompt

def test_system_prompt_without_memories():
    bridge = HarveyBridge(mock_config)
    prompt = bridge._build_system_prompt("discord", memories=None)
    assert "Relevant Context" not in prompt
```

## Integration Tests

### Test 1: Full Message Flow

```python
import asyncio
from core.chat.gateway import HarveyChat

async def test_message_flow():
    chat = HarveyChat()
    
    # Simulate Discord message
    response = await chat.handle_message(
        "discord", "test-user", "Test", "Hello Harvey"
    )
    assert response  # Got a response
    
    # Verify session created in SQLite
    import sqlite3
    with sqlite3.connect(chat.store.db_path) as conn:
        cursor = conn.execute("SELECT COUNT(*) FROM cortex_sessions")
        assert cursor.fetchone()[0] == 1
```

### Test 2: Cross-Channel Memory

```python
async def test_cross_channel():
    chat = HarveyChat()
    
    # Discord message
    r1 = await chat.handle_message("discord", "user-1", "Seb", "I like owls")
    
    # Telegram message from same user
    r2 = await chat.handle_message("telegram", "user-1", "Seb", "What do I like?")
    
    # r2 should reference owls (if cross-channel memory is enabled)
    # Note: This depends on whether user_id is shared across channels
```

### Test 3: Disabled Cortex

```python
import os
os.environ["MAKAKOO_CORTEX_ENABLED"] = "0"

async def test_disabled():
    chat = HarveyChat()
    assert chat.cortex is None
    
    response = await chat.handle_message("discord", "123", "Test", "Hello")
    assert response  # Still works without Cortex
```

### Test 4: `/clear` Resets Session

```python
async def test_clear():
    chat = HarveyChat()
    
    await chat.handle_message("discord", "user-1", "Seb", "My password is secret123")
    await chat.handle_message("discord", "user-1", "Seb", "/clear")
    
    # Session cache should be cleared
    key = "discord:user-1"
    assert key not in chat.cortex._session_cache
```

### Test 5: Memory Survives Restart

```python
def test_memory_survives_restart():
    db_path = "/tmp/test_cortex.db"
    
    # First instance
    m1 = CortexMemory(db_path, CortexConfig())
    m1.create_memory("I like owls", "discord", "123")
    del m1
    
    # Second instance (simulating restart)
    m2 = CortexMemory(db_path, CortexConfig())
    results = m2.search("owls", "discord", "123")
    assert len(results) == 1
```

## Manual QA Checklist

- [ ] Discord message → session row created in `cortex_sessions`
- [ ] Memory row created in `cortex_memories` after assistant response
- [ ] `SELECT * FROM cortex_memories_fts WHERE cortex_memories_fts MATCH 'owls'` returns results
- [ ] Telegram message → same user_id, different app_id, memories accessible if configured
- [ ] Long conversation → old memories still searchable
- [ ] `/clear` → new session created on next message, old memories still in LTM
- [ ] `MAKAKOO_CORTEX_ENABLED=0` → no cortex tables touched, normal operation
- [ ] Delete `cortex_memories` row → `cortex_memories_fts` stays in sync (trigger test)
- [ ] PII in message → scrubbed in `cortex_memories`, raw in ChatStore `messages`
- [ ] Expired memory → pruned on next search, not returned

## Performance Benchmarks

| Metric | Target | Method |
|---|---|---|
| Memory search (FTS5) | <50ms | `time` around `memory.search()` |
| Session creation | <20ms | `time` around `get_or_create_session()` |
| Memory creation | <30ms | `time` around `create_memory()` |
| Total overhead per turn | <100ms | Compare `MAKAKOO_CORTEX_ENABLED=0` vs `=1` |
| RAM usage (Cortex module) | <50MB | `ps` or `memory_profiler` |
| SQLite DB growth | <1MB/day | Monitor `store.db` size |

## Debugging

```bash
# Check sessions
sqlite3 data/chat/store.db "SELECT * FROM cortex_sessions;"

# Check memories
sqlite3 data/chat/store.db "SELECT * FROM cortex_memories ORDER BY created_at DESC LIMIT 10;"

# Test FTS5 search
sqlite3 data/chat/store.db "SELECT * FROM cortex_memories_fts WHERE cortex_memories_fts MATCH 'migration';"

# Check triggers work
sqlite3 data/chat/store.db "DELETE FROM cortex_memories WHERE id = 'test-id'; SELECT COUNT(*) FROM cortex_memories_fts WHERE rowid = (SELECT rowid FROM cortex_memories WHERE id = 'test-id');"
```
