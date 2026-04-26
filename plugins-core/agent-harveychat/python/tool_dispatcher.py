"""Tool whitelist preflight.

Defense-in-depth gate: the Rust MCP/grant layer is the
authoritative scope enforcer. This Python preflight returns a
friendlier error message to the LLM before it even calls the
tool, so the slot's persona doesn't waste tokens on a 403 it
could have predicted.

Locked Q6: empty `tools` + `inherit_baseline = true` → permit
anything; empty `tools` + `inherit_baseline = false` → deny
everything (least-privilege default).
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import List


@dataclass
class ToolScope:
    """Mirror of the Rust slot.toml `[slot]` tool fields."""

    tools: List[str]
    inherit_baseline: bool

    @classmethod
    def from_slot_dict(cls, slot: dict) -> "ToolScope":
        return cls(
            tools=list(slot.get("tools", []) or []),
            inherit_baseline=bool(slot.get("inherit_baseline", False)),
        )


class ToolNotInScopeError(Exception):
    """Raised when a tool call is rejected by the preflight."""

    def __init__(self, slot_id: str, candidate: str, allowed: List[str]):
        self.slot_id = slot_id
        self.candidate = candidate
        self.allowed = allowed
        if not allowed:
            allowed_render = "(none — least-privilege default)"
        else:
            allowed_render = ", ".join(allowed)
        super().__init__(
            f"tool '{candidate}' is not in scope for slot '{slot_id}'; "
            f"allowed: {allowed_render}"
        )


def check_tool(slot_id: str, scope: ToolScope, candidate: str) -> None:
    """Raise ToolNotInScopeError if `candidate` is not permitted.

    Locked semantics:
      * `tools` non-empty → `candidate` must be in `tools`
      * `tools` empty + `inherit_baseline=true` → permit anything
      * `tools` empty + `inherit_baseline=false` → deny everything
    """
    if scope.tools:
        if candidate not in scope.tools:
            raise ToolNotInScopeError(slot_id, candidate, scope.tools)
        return
    if scope.inherit_baseline:
        return
    raise ToolNotInScopeError(slot_id, candidate, [])
