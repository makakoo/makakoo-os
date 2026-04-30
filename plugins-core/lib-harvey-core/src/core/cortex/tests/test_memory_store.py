import sqlite3

from core.cortex import CortexConfig, CortexMemory
from core.cortex.memory import sanitize_fts_query
from core.cortex.models import MemoryCandidate, MemorySource


def test_schema_session_memory_search_restart(tmp_path):
    db = tmp_path / "chat.db"
    m = CortexMemory(str(db), CortexConfig(enabled=True, pii_scrubbing=True))
    sid = m.get_or_create_session("discord", "u1", username="Seb")
    assert sid == m.get_or_create_session("discord", "u1")
    mid = m.create_memory(
        MemoryCandidate("I prefer owl mascots api_key=sk-test-1234567890abcdef", "preference", 0.9, 0.8),
        MemorySource("discord", "u1", sid, 7),
    )
    assert mid
    results = m.search("what mascot style do I prefer???", "discord", "u1")
    assert len(results) == 1
    assert "owl mascots" in results[0]["content"]
    assert "sk-test" not in results[0]["content"]

    m2 = CortexMemory(str(db), CortexConfig(enabled=True))
    assert m2.search("owl mascot", "discord", "u1")


def test_alias_cross_channel(tmp_path):
    m = CortexMemory(str(tmp_path / "chat.db"), CortexConfig(enabled=True))
    m.set_alias("discord", "d1", "person:sebastian")
    m.set_alias("telegram", "t1", "person:sebastian")
    sid = m.get_or_create_session("discord", "d1")
    m.create_memory(
        MemoryCandidate("Sebastian prefers owls", "preference", 0.9, 0.8),
        MemorySource("discord", "d1", sid, 1),
    )
    assert m.search("owls", "telegram", "t1")


def test_end_session_creates_new_epoch(tmp_path):
    m = CortexMemory(str(tmp_path / "chat.db"), CortexConfig(enabled=True))
    sid1 = m.get_or_create_session("discord", "u1")
    m.end_session("discord", "u1")
    sid2 = m.get_or_create_session("discord", "u1")
    assert sid1 != sid2


def test_delete_updates_fts(tmp_path):
    m = CortexMemory(str(tmp_path / "chat.db"), CortexConfig(enabled=True))
    sid = m.get_or_create_session("discord", "u1")
    mid = m.create_memory(
        MemoryCandidate("SQLite memory decision", "decision", 0.9, 0.8),
        MemorySource("discord", "u1", sid, 1),
    )
    assert m.search("SQLite", "discord", "u1")
    assert m.delete_memory(mid)
    assert not m.search("SQLite", "discord", "u1")


def test_sanitize_fts_query_safe():
    assert sanitize_fts_query('what about AWS -> DO migration???') == 'what* OR about* OR aws* OR migration*'
