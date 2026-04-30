import asyncio
import sqlite3

from core.chat.config import ChatConfig, BridgeConfig
from core.chat.gateway import HarveyChat
from core.cortex.config import CortexConfig


class FakeBridge:
    def __init__(self, response="ok"):
        self.response = response
        self.memories_seen = None
        self._last_reasoning_content = ""

    def send(self, message, history, channel="telegram", file_sender=None, task_id=None, store=None, memories=None):
        self.memories_seen = memories or []
        return self.response


def test_disabled_mode_no_cortex_tables(tmp_path, monkeypatch):
    monkeypatch.delenv("MAKAKOO_CORTEX_ENABLED", raising=False)
    cfg = ChatConfig(bridge=BridgeConfig(), cortex=CortexConfig(enabled=False), db_path=str(tmp_path / "chat.db"), log_to_brain=False)
    chat = HarveyChat(cfg)
    chat.bridge = FakeBridge("ok")
    response = asyncio.run(chat.handle_message("discord", "u1", "Seb", "hello"))
    assert response == "ok"
    with sqlite3.connect(cfg.db_path) as conn:
        tables = {r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    assert "messages" in tables
    assert not any(t.startswith("cortex_") for t in tables)


def test_enabled_mode_writes_and_injects_memory(tmp_path, monkeypatch):
    monkeypatch.delenv("MAKAKOO_CORTEX_ENABLED", raising=False)
    cfg = ChatConfig(bridge=BridgeConfig(), cortex=CortexConfig(enabled=True), db_path=str(tmp_path / "chat.db"), log_to_brain=False)
    chat = HarveyChat(cfg)
    chat.bridge = FakeBridge("noted")
    asyncio.run(chat.handle_message("discord", "u1", "Seb", "Remember: I prefer owl mascots"))

    with sqlite3.connect(cfg.db_path) as conn:
        row = conn.execute("SELECT content FROM cortex_memories").fetchone()
    assert row and "owl mascots" in row[0]

    chat.bridge = FakeBridge("ok")
    asyncio.run(chat.handle_message("discord", "u1", "Seb", "what mascot style prefer?"))
    assert chat.bridge.memories_seen
    assert "owl mascots" in chat.bridge.memories_seen[0]["content"]


def test_fail_open_search(tmp_path, monkeypatch):
    monkeypatch.delenv("MAKAKOO_CORTEX_ENABLED", raising=False)
    cfg = ChatConfig(bridge=BridgeConfig(), cortex=CortexConfig(enabled=True), db_path=str(tmp_path / "chat.db"), log_to_brain=False)
    chat = HarveyChat(cfg)
    chat.bridge = FakeBridge("still works")
    def boom(*a, **k):
        raise RuntimeError("broken search")
    chat.cortex.search = boom
    assert asyncio.run(chat.handle_message("discord", "u1", "Seb", "hello")) == "still works"
