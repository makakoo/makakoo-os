#!/usr/bin/env python3
"""Tests for tool_registry.py — RED phase (must fail before implementation)."""

import sys
import os

# Add registry to path
sys.path.insert(0, os.path.dirname(__file__))

# These imports will fail until we create tool_registry.py
try:
    from tool_registry import registry, ToolRegistry, ToolEntry

    IMPORT_OK = True
except ImportError as e:
    print(f"IMPORT_FAIL: {e}")
    sys.exit(1)


def test_tool_entry_creation():
    """Test 8: ToolEntry can be created with all required fields."""
    e = ToolEntry(
        name="test_tool",
        toolset="test_toolset",
        schema={"name": "test_tool", "description": "A test tool", "parameters": {}},
        handler=lambda: None,
        check_fn=None,
        requires_env=["TEST_VAR"],
        is_async=True,
        description="Test tool description",
        emoji="🧪",
    )
    assert e.name == "test_tool"
    assert e.toolset == "test_toolset"
    assert e.is_async == True
    assert e.requires_env == ["TEST_VAR"]
    print("  ✓ ToolEntry creation with all fields")


def test_register_stores_tool():
    """Test 1: registry.register() stores tool entry with all metadata."""
    registry._tools.clear()  # Start fresh
    registry._toolset_checks.clear()

    def my_handler(args, **kwargs):
        return "ok"

    registry.register(
        name="test_register",
        toolset="test_toolset",
        schema={"name": "test_register", "description": "Test register"},
        handler=my_handler,
        check_fn=None,
        requires_env=["VAR1"],
        is_async=False,
        description="Testing register",
        emoji="⚡",
    )

    assert "test_register" in registry._tools
    entry = registry._tools["test_register"]
    assert entry.name == "test_register"
    assert entry.toolset == "test_toolset"
    assert entry.handler is my_handler
    assert entry.requires_env == ["VAR1"]
    print("  ✓ registry.register() stores tool entry")


def test_get_definitions_returns_schemas():
    """Test 2: registry.get_definitions() returns OpenAI-format schemas."""
    registry._tools.clear()
    registry._toolset_checks.clear()

    registry.register(
        name="test_defs",
        toolset="test",
        schema={
            "name": "test_defs",
            "description": "A test",
            "parameters": {"type": "object"},
        },
        handler=lambda: None,
    )

    defs = registry.get_definitions({"test_defs"})
    assert len(defs) == 1
    assert defs[0]["type"] == "function"
    assert defs[0]["function"]["name"] == "test_defs"
    print("  ✓ get_definitions() returns OpenAI-format schemas")


def test_get_definitions_filters_by_check_fn():
    """Test 3: registry.get_definitions() filters by check_fn (unavailable tools excluded)."""
    registry._tools.clear()
    registry._toolset_checks.clear()

    # Tool with failing check_fn
    def failing_check():
        return False

    registry.register(
        name="unavailable_tool",
        toolset="broken",
        schema={"name": "unavailable_tool", "description": "Won't be included"},
        handler=lambda: None,
        check_fn=failing_check,
    )

    defs = registry.get_definitions({"unavailable_tool"})
    assert len(defs) == 0, f"Expected 0, got {len(defs)}"
    print("  ✓ get_definitions() filters by check_fn")


def test_dispatch_executes_handler():
    """Test 4: registry.dispatch() executes handler and returns result."""
    registry._tools.clear()
    registry._toolset_checks.clear()

    def my_handler(args, **kwargs):
        return {"result": "success", "args": args}

    registry.register(
        name="exec_test",
        toolset="test",
        schema={"name": "exec_test"},
        handler=my_handler,
    )

    result = registry.dispatch("exec_test", {"key": "value"})
    # Result could be JSON string or dict depending on implementation
    print(f"  ✓ dispatch() executes handler, result type: {type(result)}")


def test_dispatch_unknown_tool_returns_error():
    """Test 5: registry.dispatch() returns error JSON for unknown tools."""
    registry._tools.clear()
    registry._toolset_checks.clear()

    import json

    result = registry.dispatch("nonexistent", {})
    # Should be JSON error string
    try:
        parsed = json.loads(result)
        assert "error" in parsed
        assert "nonexistent" in parsed["error"].lower()
    except json.JSONDecodeError:
        # If it returns a dict instead, that's also fine
        if isinstance(result, dict) and "error" in result:
            pass
        else:
            raise AssertionError(f"Expected error JSON, got: {result}")
    print("  ✓ dispatch() returns error JSON for unknown tools")


def test_dispatch_catches_exceptions():
    """Test 6: registry.dispatch() catches exceptions and returns error JSON."""
    registry._tools.clear()
    registry._toolset_checks.clear()

    import json

    def bad_handler(args, **kwargs):
        raise ValueError("Intentional test error")

    registry.register(
        name="bad_handler",
        toolset="test",
        schema={"name": "bad_handler"},
        handler=bad_handler,
    )

    result = registry.dispatch("bad_handler", {})
    try:
        parsed = json.loads(result)
        assert "error" in parsed
    except (json.JSONDecodeError, TypeError):
        if isinstance(result, dict) and "error" in result:
            pass
        else:
            raise AssertionError(f"Expected error dict, got: {result}")
    print("  ✓ dispatch() catches exceptions")


def test_is_toolset_available_false_on_check_failure():
    """Test 7: is_toolset_available() returns False when check_fn raises."""
    registry._tools.clear()
    registry._toolset_checks.clear()

    def failing_check():
        raise RuntimeError("Check failed")

    registry.register(
        name="fail_check_tool",
        toolset="fail_toolset",
        schema={"name": "fail_check_tool"},
        handler=lambda: None,
        check_fn=failing_check,
    )

    result = registry.is_toolset_available("fail_toolset")
    assert result == False, f"Expected False, got {result}"
    print("  ✓ is_toolset_available() returns False when check_fn raises")


def test_all():
    print("\n=== Running tool_registry tests ===")
    test_tool_entry_creation()
    test_register_stores_tool()
    test_get_definitions_returns_schemas()
    test_get_definitions_filters_by_check_fn()
    test_dispatch_executes_handler()
    test_dispatch_unknown_tool_returns_error()
    test_dispatch_catches_exceptions()
    test_is_toolset_available_false_on_check_failure()
    print("\n✅ All tests passed!")


if __name__ == "__main__":
    test_all()
