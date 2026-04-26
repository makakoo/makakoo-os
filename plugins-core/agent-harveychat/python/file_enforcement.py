"""Filesystem path scope preflight.

Defense-in-depth gate: the Rust MCP/grant layer is the
authoritative scope enforcer. This Python preflight returns a
friendlier error before the LLM tries to write a forbidden path.

Locked semantics (Phase 3 of v1, mirrored here):
  * `forbidden_paths` is a hard veto. A path matching any forbidden
    prefix is denied even if it also matches an allowed prefix.
  * Empty `allowed_paths` → least-privilege default: deny everything.
  * Otherwise: prefix-match against `allowed_paths` (after
    tilde-expansion).
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path
from typing import List


@dataclass
class PathScope:
    allowed: List[str]
    forbidden: List[str]

    @classmethod
    def from_slot_dict(cls, slot: dict) -> "PathScope":
        return cls(
            allowed=list(slot.get("allowed_paths", []) or []),
            forbidden=list(slot.get("forbidden_paths", []) or []),
        )


class PathNotInScopeError(Exception):
    def __init__(
        self,
        slot_id: str,
        candidate: str,
        allowed: List[str],
        forbidden: List[str],
        least_privilege: bool,
    ):
        self.slot_id = slot_id
        self.candidate = candidate
        self.allowed = allowed
        self.forbidden = forbidden
        self.least_privilege = least_privilege
        if least_privilege:
            allowed_render = "(none — least-privilege default)"
        elif not allowed:
            allowed_render = "(none)"
        else:
            allowed_render = ", ".join(allowed)
        forbidden_render = ", ".join(forbidden) if forbidden else "(none)"
        super().__init__(
            f"path '{candidate}' is not in scope for slot '{slot_id}'; "
            f"allowed: {allowed_render}; forbidden: {forbidden_render}"
        )


def _expand(path: str) -> str:
    return os.path.expanduser(path)


def _normalize_prefix(prefix: str) -> str:
    """Trailing-slash insensitive prefix match base."""
    p = _expand(prefix)
    return p.rstrip("/")


def check_path(slot_id: str, scope: PathScope, candidate: str) -> None:
    """Raise PathNotInScopeError if `candidate` is not permitted.

    `candidate` is normalized via expanduser; `scope.allowed` and
    `scope.forbidden` likewise.
    """
    cand = _expand(candidate)

    # Forbidden veto wins.
    for f in scope.forbidden:
        f_norm = _normalize_prefix(f)
        if cand == f_norm or cand.startswith(f_norm + "/"):
            raise PathNotInScopeError(
                slot_id, candidate, scope.allowed, scope.forbidden, False
            )

    if not scope.allowed:
        raise PathNotInScopeError(
            slot_id, candidate, scope.allowed, scope.forbidden, True
        )

    for a in scope.allowed:
        a_norm = _normalize_prefix(a)
        if cand == a_norm or cand.startswith(a_norm + "/"):
            return

    raise PathNotInScopeError(
        slot_id, candidate, scope.allowed, scope.forbidden, False
    )


def is_in_scope(slot_id: str, scope: PathScope, candidate: str) -> bool:
    """Boolean variant for code paths that prefer to branch."""
    try:
        check_path(slot_id, scope, candidate)
        return True
    except PathNotInScopeError:
        return False
