from core.chat.bridge import HarveyBridge
from core.chat.config import BridgeConfig


class FakeAgent:
    def __init__(self):
        self.system_prompt = ""
        self._last_reasoning_content = ""

    def process(self, message, history, system_prompt, channel, task_id=None, store=None):
        self.system_prompt = system_prompt
        return "ok"


def test_format_memories_empty():
    bridge = HarveyBridge(BridgeConfig())
    assert bridge._format_memories([]) == ""


def test_format_memories_bounded_and_warning():
    bridge = HarveyBridge(BridgeConfig())
    block = bridge._format_memories([
        {"content": "owl mascots\nwith newlines", "memory_type": "preference", "confidence": 0.91, "created_at": 1777561356.0}
    ])
    assert "Relevant Local Memory" in block
    assert "prefer the current user message" in block
    assert "owl mascots with newlines" in block
    assert "confidence 0.91" in block


def test_send_passes_enriched_prompt():
    bridge = HarveyBridge(BridgeConfig())
    fake = FakeAgent()
    bridge.agent = fake
    out = bridge.send("what mascot?", [], memories=[{"content": "Sebastian prefers owls", "memory_type": "preference"}])
    assert out == "ok"
    assert "Relevant Local Memory" in fake.system_prompt
    assert "Sebastian prefers owls" in fake.system_prompt
