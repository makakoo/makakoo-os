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


def test_browser_approval_yes_grants_and_runs_read(monkeypatch):
    bridge = HarveyBridge(BridgeConfig())
    calls = []

    def fake_execute_tool(name, args):
        calls.append((name, args))
        if name == "grant_action_access":
            return "Granted. browser/domain host=iberia.com browser=default writable until later."
        if name == "operator_browser_read":
            return "Iberia flight page summary"
        raise AssertionError(name)

    monkeypatch.setattr("core.agent.harvey_agent.execute_tool", fake_execute_tool)
    history = [
        {
            "role": "user",
            "content": "navigate to iberia and check flights to madrid from disseldorf for 26 May",
        },
        {
            "role": "assistant",
            "content": (
                'Grant me Chrome access? grant_action_access(action="browser/control", '
                'target="iberia.com flights düsseldorf madrid 26 may")'
            ),
        },
    ]

    out = bridge._maybe_handle_browser_approval("yes", history)

    assert "Granted." in out
    assert "Iberia flight page summary" in out
    assert calls[0] == (
        "grant_action_access",
        {
            "action": "browser/control",
            "target": "iberia.com flights düsseldorf madrid 26 may",
            "duration": "1h",
            "label": "browser approval from chat",
        },
    )
    assert calls[1] == (
        "operator_browser_read",
        {
            "url": "https://iberia.com",
            "query": "navigate to iberia and check flights to madrid from disseldorf for 26 May",
            "browser": "default",
            "timeout_seconds": 90,
        },
    )


def test_browser_approval_yes_ignored_without_recent_browser_prompt(monkeypatch):
    bridge = HarveyBridge(BridgeConfig())

    def fake_execute_tool(name, args):  # pragma: no cover - must not be called
        raise AssertionError(name)

    monkeypatch.setattr("core.agent.harvey_agent.execute_tool", fake_execute_tool)
    assert bridge._maybe_handle_browser_approval("yes", [{"role": "assistant", "content": "Need anything else?"}]) is None


def test_history_sanitizer_drops_browser_grant_loop_dead_ends():
    bridge = HarveyBridge(BridgeConfig())
    clean = bridge._sanitize_history(
        [
            {
                "role": "assistant",
                "content": (
                    "Aggregators are JS-rendered — can't get prices without a real browser.\n\n"
                    "Want me to try Kiwi or Expedia via fetch? Or grant browser access?"
                ),
            },
            {"role": "assistant", "content": "Need the full URL as the target.\n\nSay yes."},
            {"role": "assistant", "content": "One more step — grant the exact URL:\n\nyes"},
            {"role": "assistant", "content": "Iberia hard-blocks automated fetching. Need real browser session."},
        ]
    )
    assert clean == [{"role": "assistant", "content": "Iberia hard-blocks automated fetching. Need real browser session."}]
