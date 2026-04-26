"""Tests for the tool whitelist preflight."""

from __future__ import annotations

import pytest

from plugins_core.agent_harveychat.python.tool_dispatcher import (
    ToolNotInScopeError,
    ToolScope,
    check_tool,
)


def test_explicit_whitelist_permits_listed():
    scope = ToolScope(tools=["brain_search", "write_file"], inherit_baseline=False)
    check_tool("secretary", scope, "brain_search")  # no raise


def test_explicit_whitelist_rejects_others():
    scope = ToolScope(tools=["brain_search"], inherit_baseline=False)
    with pytest.raises(ToolNotInScopeError) as ei:
        check_tool("secretary", scope, "run_command")
    assert ei.value.candidate == "run_command"
    assert ei.value.allowed == ["brain_search"]
    msg = str(ei.value)
    assert "run_command" in msg
    assert "secretary" in msg
    assert "brain_search" in msg


def test_empty_tools_with_baseline_permits_all():
    scope = ToolScope(tools=[], inherit_baseline=True)
    check_tool("secretary", scope, "anything")  # no raise


def test_empty_tools_without_baseline_least_privilege_default():
    scope = ToolScope(tools=[], inherit_baseline=False)
    with pytest.raises(ToolNotInScopeError) as ei:
        check_tool("career", scope, "brain_search")
    msg = str(ei.value)
    assert "least-privilege default" in msg
    assert ei.value.allowed == []


def test_from_slot_dict_extracts_tools_and_baseline():
    slot = {
        "tools": ["a", "b"],
        "inherit_baseline": True,
    }
    scope = ToolScope.from_slot_dict(slot)
    assert scope.tools == ["a", "b"]
    assert scope.inherit_baseline is True


def test_from_slot_dict_handles_missing_fields():
    scope = ToolScope.from_slot_dict({})
    assert scope.tools == []
    assert scope.inherit_baseline is False
