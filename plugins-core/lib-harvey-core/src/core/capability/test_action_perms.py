from __future__ import annotations

import os
from pathlib import Path

import pytest

from core.agent.harvey_agent import execute_tool
from core.capability.action_perms import ActionGrantArgs, action_scope, grant_action
from core.capability.perms_core import PermsError


@pytest.fixture(autouse=True)
def isolated_home(tmp_path, monkeypatch):
    home = tmp_path / "MAKAKOO"
    for d in ["config", "state", "logs"]:
        (home / d).mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("MAKAKOO_HOME", str(home))
    monkeypatch.setenv("HARVEY_HOME", str(home))
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("HARVEY_PLUGIN", "harveychat-telegram")
    monkeypatch.setenv("HARVEY_TELEGRAM_CHAT_ID", "123")

    # Empty allowlist means permissive config if config loading falls back;
    # if a real config is consulted, non-Telegram unit calls below bypass via
    # direct grant_action. The env is isolated for grant provenance either way.
    yield home


def test_action_grant_requires_origin_turn_id():
    with pytest.raises(PermsError) as e:
        grant_action(
            ActionGrantArgs(
                action="shell/run",
                target="printf hello",
                plugin="harveychat-telegram",
                origin_turn_id="",
            )
        )
    assert "origin_turn_id required" in str(e.value)


def test_operator_run_command_requires_exact_action_grant(monkeypatch):
    monkeypatch.setenv("HARVEY_PLUGIN", "harveychat")
    denied = execute_tool("operator_run_command", {"command": "printf hello"})
    assert "no active action grant" in denied

    granted = grant_action(
        ActionGrantArgs(
            action="shell/run",
            target="printf hello",
            plugin="harveychat",
            origin_turn_id="turn-1",
            duration="1h",
        )
    )
    assert "Granted." in granted
    grants_json = Path(os.environ["MAKAKOO_HOME"]).joinpath(
        "config", "user_grants.json"
    )
    assert action_scope("shell/run", "printf hello") in grants_json.read_text()

    allowed = execute_tool("operator_run_command", {"command": "printf hello"})
    assert allowed == "exit=0\nhello"

    mismatch = execute_tool("operator_run_command", {"command": "printf goodbye"})
    assert "no active action grant" in mismatch


def test_operator_hard_blocks_destructive_commands_even_before_grant(monkeypatch):
    monkeypatch.setenv("HARVEY_PLUGIN", "harveychat")
    out = execute_tool("operator_run_command", {"command": "sudo whoami"})
    assert "rejected" in out
    assert "hard-blocked" in out

    grant = execute_tool(
        "grant_action_access",
        {
            "action": "shell/run",
            "target": "sudo whoami",
            "duration": "1h",
            "user_turn_id": "turn-2",
        },
    )
    assert "refused even with grant" in grant


def test_list_action_grants(monkeypatch):
    monkeypatch.setenv("HARVEY_PLUGIN", "harveychat")
    grant_action(
        ActionGrantArgs(
            action="shell/run",
            target="printf listed",
            plugin="harveychat",
            origin_turn_id="turn-3",
            duration="1h",
        )
    )
    out = execute_tool("list_action_grants", {})
    assert "Action grants:" in out
    assert "action:shell/run:" in out
    assert "printf listed" in out
