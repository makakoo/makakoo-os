"""Tests for the path scope preflight."""

from __future__ import annotations

import pytest

from plugins_core.agent_harveychat.python import file_enforcement as enforcement
from plugins_core.agent_harveychat.python.file_enforcement import (
    PathNotInScopeError,
    PathScope,
    check_path,
    is_in_scope,
    filesystem_paths,
    is_filesystem_key,
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


def test_forbidden_veto_handles_case_aliases_on_case_insensitive_filesystems(tmp_path):
    allowed = tmp_path / "allowed"
    forbidden = allowed / "secret"
    forbidden.mkdir(parents=True)
    candidate = allowed / "SECRET" / "private.db"
    scope = PathScope(allowed=[str(allowed)], forbidden=[str(forbidden)])

    if (allowed / "SECRET").exists():
        with pytest.raises(PathNotInScopeError):
            check_path("secretary", scope, str(candidate))
    else:
        # On a case-sensitive filesystem this is a genuinely different path.
        check_path("secretary", scope, str(candidate))


def test_component_comparison_exercises_both_case_sensitivity_branches(monkeypatch):
    prefix = enforcement.Path("/scope/Secret")
    candidate = enforcement.Path("/scope/secret/file")
    monkeypatch.setattr(enforcement, "_case_insensitive_filesystem", lambda _path: True)
    assert enforcement._contains(prefix, candidate)
    monkeypatch.setattr(enforcement, "_case_insensitive_filesystem", lambda _path: False)
    assert not enforcement._contains(prefix, candidate)


def test_unicode_normalization_alias_is_forbidden_on_insensitive_filesystems(
    monkeypatch,
):
    prefix = enforcement.Path("/scope/s\N{LATIN SMALL LETTER E WITH ACUTE}cret")
    candidate = enforcement.Path("/scope/se\N{COMBINING ACUTE ACCENT}cret/file")
    monkeypatch.setattr(enforcement, "_case_insensitive_filesystem", lambda _path: True)
    assert enforcement._contains(prefix, candidate)
    monkeypatch.setattr(enforcement, "_case_insensitive_filesystem", lambda _path: False)
    assert not enforcement._contains(prefix, candidate)


def test_tilde_expansion_in_scope_and_candidate():
    scope = PathScope(allowed=["~/Office"], forbidden=[])
    # The `~/Office/spec.md` candidate should expand the same way.
    check_path("secretary", scope, "~/Office/spec.md")


def test_parent_traversal_cannot_escape_allowed_prefix():
    scope = PathScope(allowed=["~/Office"], forbidden=[])
    with pytest.raises(PathNotInScopeError):
        check_path("secretary", scope, "~/Office/../CV/private.md")


def test_existing_symlink_cannot_escape_allowed_prefix(tmp_path):
    allowed = tmp_path / "allowed"
    outside = tmp_path / "outside"
    allowed.mkdir()
    outside.mkdir()
    (allowed / "escape").symlink_to(outside, target_is_directory=True)
    scope = PathScope(allowed=[str(allowed)], forbidden=[])
    with pytest.raises(PathNotInScopeError):
        check_path("secretary", scope, str(allowed / "escape" / "secret"))


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


@pytest.mark.parametrize(
    "key",
    ["myfilepath", "outFilePath", "workingDirectory", "OUTPUT-FILE"],
)
def test_concatenated_and_camel_case_filesystem_keys_are_detected(key):
    assert is_filesystem_key(key)


@pytest.mark.parametrize(
    "key",
    [
        "filename",
        "filenames",
        "fileName",
        "FILENAME",
        "src",
        "dst",
        "SRC",
        "fullpath",
        "fullpaths",
        "fullPath",
        "output_filename",
        "backup_dst",
        "avatar_src",
        "logFullPath",
    ],
)
def test_filename_src_dst_fullpath_keys_are_detected(key):
    assert is_filesystem_key(key)


def test_remote_source_exemption_requires_scheme_at_start_and_rejects_file_uri():
    assert filesystem_paths({"source": "https://example.com/a"}) == []
    assert filesystem_paths({"source": "data:text/plain,hello"}) == []
    assert filesystem_paths({"source": "/tmp/secret://payload"}) == [
        "/tmp/secret://payload"
    ]
    assert filesystem_paths({"source": "file:///tmp/secret"}) == [
        "file:///tmp/secret"
    ]
