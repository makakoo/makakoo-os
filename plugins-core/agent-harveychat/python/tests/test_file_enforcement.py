"""Tests for the path scope preflight."""

from __future__ import annotations

import pytest

from plugins_core.agent_harveychat.python.file_enforcement import (
    PathNotInScopeError,
    PathScope,
    check_path,
    is_in_scope,
)


def test_empty_allowed_least_privilege_default():
    scope = PathScope(allowed=[], forbidden=[])
    with pytest.raises(PathNotInScopeError) as ei:
        check_path("career", scope, "/etc/passwd")
    assert ei.value.least_privilege is True
    msg = str(ei.value)
    assert "least-privilege default" in msg
    assert "/etc/passwd" in msg


def test_allowed_prefix_match_permits():
    scope = PathScope(allowed=["/var/lib/secretary"], forbidden=[])
    check_path("secretary", scope, "/var/lib/secretary/conv.db")
    check_path("secretary", scope, "/var/lib/secretary")  # exact match


def test_allowed_prefix_does_not_partial_match():
    # `/var/lib/secretary` must NOT match `/var/lib/secretary-evil`.
    scope = PathScope(allowed=["/var/lib/secretary"], forbidden=[])
    with pytest.raises(PathNotInScopeError):
        check_path("secretary", scope, "/var/lib/secretary-evil/x")


def test_forbidden_veto_wins():
    # `forbidden` denies even when `allowed` would permit.
    scope = PathScope(
        allowed=["/var/lib/secretary"],
        forbidden=["/var/lib/secretary/secret"],
    )
    check_path("secretary", scope, "/var/lib/secretary/conv.db")  # ok
    with pytest.raises(PathNotInScopeError) as ei:
        check_path("secretary", scope, "/var/lib/secretary/secret/private.db")
    assert ei.value.least_privilege is False


def test_tilde_expansion_in_scope_and_candidate():
    scope = PathScope(allowed=["~/Office"], forbidden=[])
    # The `~/Office/spec.md` candidate should expand the same way.
    check_path("secretary", scope, "~/Office/spec.md")


def test_is_in_scope_boolean_form():
    scope = PathScope(allowed=["/a"], forbidden=["/a/forbidden"])
    assert is_in_scope("x", scope, "/a/ok")
    assert not is_in_scope("x", scope, "/etc/passwd")
    assert not is_in_scope("x", scope, "/a/forbidden/file")


def test_from_slot_dict_handles_empty_lists():
    scope = PathScope.from_slot_dict({})
    assert scope.allowed == []
    assert scope.forbidden == []


def test_error_message_format_least_privilege():
    scope = PathScope(allowed=[], forbidden=[])
    with pytest.raises(PathNotInScopeError) as ei:
        check_path("career", scope, "/etc/passwd")
    msg = str(ei.value)
    assert "/etc/passwd" in msg
    assert "career" in msg
    assert "(none — least-privilege default)" in msg
    assert "(none)" in msg  # forbidden render


def test_error_message_format_with_allowed_set():
    scope = PathScope(allowed=["/a", "/b"], forbidden=["/c"])
    with pytest.raises(PathNotInScopeError) as ei:
        check_path("x", scope, "/etc/passwd")
    msg = str(ei.value)
    assert "/a, /b" in msg
    assert "/c" in msg
