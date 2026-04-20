#!/usr/bin/env python3
"""
Tests for Delegate Tool — subagent spawning with depth limiting,
restricted toolsets, and concurrent child limiting.

Run with: python3 harvey-os/core/improve/tests_delegate_tool.py -v
"""

import os
import sys
import threading
import time
import pytest
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", ".."))


class TestToolsetRestrictions:
    """Test: blocked tools are stripped from toolsets."""

    def test_blocked_toolsets_removed(self):
        """Delegation, clarify, memory, code_execution toolsets are stripped."""
        from harvey_os.core.improve.delegate_tool import _resolve_toolset

        result = _resolve_toolset(
            ["delegation", "clarify", "terminal"],
            ["delegation", "clarify", "terminal", "file", "web"],
        )
        assert "delegation" not in result
        assert "clarify" not in result
        assert "terminal" in result

    def test_resolve_toolset_intersection(self):
        """Subagent cannot gain tools the parent doesn't have."""
        from harvey_os.core.improve.delegate_tool import _resolve_toolset

        result = _resolve_toolset(["terminal", "web"], ["terminal"])
        assert "web" not in result
        assert "terminal" in result

    def test_resolve_toolset_fallback_to_default(self):
        """No parent toolsets → use DEFAULT_TOOLSETS minus blocked."""
        from harvey_os.core.improve.delegate_tool import _resolve_toolset

        result = _resolve_toolset(None, None)
        blocked = {"delegation", "clarify", "memory", "code_execution"}
        for t in result:
            assert t not in blocked


class TestDepthLimiting:
    """Test: depth limiting prevents subagents from spawning grandchildren."""

    def test_depth_2_rejected(self):
        """Depth >= MAX_DEPTH returns error JSON."""
        from harvey_os.core.improve.delegate_tool import delegate_task

        fake_agent = type("FakeAgent", (), {"enabled_toolsets": ["terminal"]})()
        result = delegate_task(
            goal="test",
            parent_agent=fake_agent,
            depth=2,
        )
        parsed = json.loads(result)
        assert "error" in parsed
        assert "depth limit" in parsed["error"].lower()

    def test_depth_0_allowed(self):
        """Depth 0 is allowed and returns results."""
        from harvey_os.core.improve.delegate_tool import delegate_task

        fake_agent = type("FakeAgent", (), {"enabled_toolsets": ["terminal"]})()
        result = delegate_task(
            goal="test",
            parent_agent=fake_agent,
            depth=0,
        )
        parsed = json.loads(result)
        assert "error" not in parsed or parsed.get("error") is None


class TestConcurrentChildren:
    """Test: concurrent child limiting via MAX_CONCURRENT_CHILDREN."""

    def test_batch_capped_at_max_concurrent(self):
        """Batch of 5 tasks is silently capped to MAX_CONCURRENT_CHILDREN."""
        from harvey_os.core.improve.delegate_tool import (
            delegate_task,
            MAX_CONCURRENT_CHILDREN,
        )

        fake_agent = type("FakeAgent", (), {"enabled_toolsets": ["terminal"]})()
        tasks = [{"goal": f"task {i}"} for i in range(5)]

        result = delegate_task(
            tasks=tasks,
            parent_agent=fake_agent,
            depth=0,
        )
        parsed = json.loads(result)
        assert len(parsed["results"]) <= MAX_CONCURRENT_CHILDREN

    def test_single_task_no_threadpool(self):
        """Single task uses direct execution path (no thread pool overhead)."""
        from harvey_os.core.improve.delegate_tool import delegate_task

        fake_agent = type("FakeAgent", (), {"enabled_toolsets": ["terminal"]})()
        result = delegate_task(
            goal="simple task",
            parent_agent=fake_agent,
            depth=0,
        )
        parsed = json.loads(result)
        assert "results" in parsed
        assert len(parsed["results"]) == 1


class TestDelegateResult:
    """Test: DelegateResult dataclass fields."""

    def test_delegate_result_fields(self):
        """DelegateResult has all required fields."""
        from harvey_os.core.improve.delegate_tool import DelegateResult

        result = DelegateResult(
            session_id="test_123",
            status="completed",
            summary="Did the thing",
            duration_seconds=1.5,
            model="auto",
            exit_reason="completed",
            api_calls=3,
            tokens={"input": 100, "output": 200},
            tool_trace=[
                {
                    "tool": "terminal",
                    "args_bytes": 50,
                    "result_bytes": 100,
                    "status": "ok",
                }
            ],
        )

        assert result.session_id == "test_123"
        assert result.status == "completed"
        assert result.summary == "Did the thing"
        assert result.duration_seconds == 1.5
        assert result.model == "auto"
        assert result.exit_reason == "completed"
        assert result.api_calls == 3
        assert result.tokens == {"input": 100, "output": 200}
        assert len(result.tool_trace) == 1
        assert result.error is None


class TestSpawnDelegate:
    """Test: spawn_delegate convenience function."""

    def test_spawn_delegate_returns_delegate_result(self):
        """spawn_delegate returns a DelegateResult object."""
        from harvey_os.core.improve.delegate_tool import spawn_delegate

        result = spawn_delegate(
            goal="What is 2+2?",
            allowed_tools=["terminal"],
            max_iterations=5,
            depth=0,
        )

        assert hasattr(result, "session_id")
        assert hasattr(result, "status")
        assert hasattr(result, "summary")
        assert hasattr(result, "duration_seconds")


class TestSessionManagement:
    """Test: session listing and status functions."""

    def test_list_active_sessions_returns_list(self):
        """list_active_sessions returns a list (possibly empty)."""
        from harvey_os.core.improve.delegate_tool import list_active_sessions

        sessions = list_active_sessions()
        assert isinstance(sessions, list)

    def test_get_session_status_nonexistent(self):
        """get_session_status returns None for unknown session."""
        from harvey_os.core.improve.delegate_tool import get_session_status

        result = get_session_status("nonexistent_session_xyz")
        assert result is None


class TestDelegateTaskSchema:
    """Test: DELEGATE_TASK_SCHEMA is well-formed for tool registration."""

    def test_schema_has_required_fields(self):
        """DELEGATE_TASK_SCHEMA has name, description, parameters."""
        from harvey_os.core.improve.delegate_tool import DELEGATE_TASK_SCHEMA

        assert DELEGATE_TASK_SCHEMA["name"] == "delegate_task"
        assert "description" in DELEGATE_TASK_SCHEMA
        assert "parameters" in DELEGATE_TASK_SCHEMA
        params = DELEGATE_TASK_SCHEMA["parameters"]
        assert params["type"] == "object"
        assert "properties" in params
        assert "goal" in params["properties"]
        assert "tasks" in params["properties"]


class TestBuildChildSystemPrompt:
    """Test: system prompt building for child agents."""

    def test_build_prompt_includes_goal(self):
        """System prompt contains the goal."""
        from harvey_os.core.improve.delegate_tool import _build_child_system_prompt

        prompt = _build_child_system_prompt(
            goal="Fix the bug in auth.py",
            context="Error: NoneType has no attribute 'user'",
            allowed_tools=["terminal", "file"],
        )

        assert "Fix the bug in auth.py" in prompt
        assert "Error: NoneType has no attribute" in prompt
        assert "terminal" in prompt
        assert "file" in prompt

    def test_build_prompt_no_context(self):
        """System prompt without context is still well-formed."""
        from harvey_os.core.improve.delegate_tool import _build_child_system_prompt

        prompt = _build_child_system_prompt(goal="Run tests")
        assert "Run tests" in prompt


class TestErrorHandling:
    """Test: error cases return proper JSON error responses."""

    def test_no_goal_no_tasks(self):
        """Neither goal nor tasks → error JSON."""
        from harvey_os.core.improve.delegate_tool import delegate_task

        fake_agent = type("FakeAgent", (), {"enabled_toolsets": ["terminal"]})()
        result = delegate_task(parent_agent=fake_agent, depth=0)
        parsed = json.loads(result)
        assert "error" in parsed

    def test_empty_goal(self):
        """Empty goal string → error JSON."""
        from harvey_os.core.improve.delegate_tool import delegate_task

        fake_agent = type("FakeAgent", (), {"enabled_toolsets": ["terminal"]})()
        result = delegate_task(goal="   ", parent_agent=fake_agent, depth=0)
        parsed = json.loads(result)
        assert "error" in parsed

    def test_task_missing_goal(self):
        """Task in batch missing goal → error JSON."""
        from harvey_os.core.improve.delegate_tool import delegate_task

        fake_agent = type("FakeAgent", (), {"enabled_toolsets": ["terminal"]})()
        result = delegate_task(
            tasks=[{"goal": "valid"}, {"context": "no goal here"}],
            parent_agent=fake_agent,
            depth=0,
        )
        parsed = json.loads(result)
        assert "error" in parsed
        assert "goal" in parsed["error"].lower()


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
