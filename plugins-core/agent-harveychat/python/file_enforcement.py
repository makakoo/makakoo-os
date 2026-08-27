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

import re
import unicodedata
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional


_FILESYSTEM_KEYS = frozenset(
    {
        "path",
        "paths",
        "filepath",
        "filepaths",
        "file",
        "files",
        "dir",
        "dirs",
        "directory",
        "directories",
        "dirname",
        "folder",
        "folders",
        "cwd",
        "workdir",
        "working_dir",
        "working_directory",
        "target",
        "targets",
        "output",
        "outputs",
        "destination",
        "destinations",
        "dest",
        "dst",
        "source",
        "sources",
        "src",
        "filename",
        "filenames",
        "fullpath",
        "fullpaths",
    }
)
_FILESYSTEM_SUFFIXES = (
    "_path",
    "_paths",
    "_file",
    "_files",
    "_dir",
    "_dirs",
    "_directory",
    "_directories",
    "_folder",
    "_folders",
    "_cwd",
    "_target",
    "_targets",
    "_output",
    "_outputs",
    "_destination",
    "_destinations",
    "_dest",
    "_dst",
    "_source",
    "_sources",
    "_src",
    "_filename",
    "_filenames",
    "_fullpath",
    "_fullpaths",
)


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


def _resolve(path: str) -> Path:
    """Normalize traversal and resolve existing symlinks.

    `strict=False` preserves not-yet-created write targets while still
    resolving every existing ancestor. Component containment below avoids
    string-prefix collisions such as `/tmp/allowed` vs `/tmp/allowed-evil`.

    This is a check-time snapshot: a symlink swapped in after this
    preflight but before the write (TOCTOU) is not caught here — the
    Rust MCP/grant layer is the authoritative write-time enforcer.
    """
    return Path(path).expanduser().resolve(strict=False)


def _case_insensitive_filesystem(path: Path) -> bool:
    """Detect case-folding without creating a probe file.

    APFS commonly resolves differently-cased spellings to the same inode while
    preserving the caller's spelling in ``Path.resolve(strict=False)``. Walk to
    an existing ancestor and ask ``samefile`` about a swapped-case spelling.
    Linux case-sensitive filesystems keep the spellings distinct.
    """
    probe = path
    while not probe.exists() and probe != probe.parent:
        probe = probe.parent
    while probe != probe.parent:
        swapped = probe.with_name(probe.name.swapcase())
        if swapped != probe:
            try:
                return swapped.exists() and probe.samefile(swapped)
            except OSError:
                # Detection uncertainty must not disable case/Unicode folding
                # at a security boundary. Over-deny rather than permit an alias.
                return True
        probe = probe.parent
    return False


def _contains(prefix: Path, candidate: Path) -> bool:
    casefold = _case_insensitive_filesystem(prefix) or _case_insensitive_filesystem(
        candidate
    )
    prefix_parts = prefix.parts
    candidate_parts = candidate.parts
    if casefold:
        prefix_parts = tuple(
            unicodedata.normalize("NFC", part).casefold() for part in prefix_parts
        )
        candidate_parts = tuple(
            unicodedata.normalize("NFC", part).casefold() for part in candidate_parts
        )
    return candidate_parts[: len(prefix_parts)] == prefix_parts


def is_filesystem_key(key: str) -> bool:
    """Return whether an MCP-style argument name can carry a local path."""
    key = _normalize_argument_key(key)
    if key in _FILESYSTEM_KEYS or key.endswith(_FILESYSTEM_SUFFIXES):
        return True
    # Some tool schemas concatenate words instead of using snake/camel case.
    # Keep this list specific so unrelated keys such as ``profile`` or
    # ``target_language`` do not become path-bearing by suffix accident.
    squashed = key.replace("_", "")
    return squashed.endswith(
        (
            "filepath",
            "filepaths",
            "filename",
            "filenames",
            "fullpath",
            "fullpaths",
            "dirname",
            "workdir",
            "workingdirectory",
        )
    )


def _normalize_argument_key(key: str) -> str:
    """Normalize snake/kebab/camel/Pascal argument names to snake case."""
    key = re.sub(r"(?<=[a-z0-9])(?=[A-Z])", "_", key)
    key = re.sub(r"[^A-Za-z0-9]+", "_", key)
    return key.strip("_").casefold()


def _looks_like_remote_source(value: str) -> bool:
    """Accept a real URI scheme at byte zero, never a path containing ``://``."""
    if value.startswith("data:"):
        return True
    match = re.match(r"^([A-Za-z][A-Za-z0-9+.-]*):\/\/", value)
    return bool(match and match.group(1).casefold() != "file")


def filesystem_paths(arguments: object) -> List[str]:
    """Walk nested tool arguments and return every local path candidate.

    Arrays inherit their parent key. Only exact ``source``/``sources`` keys
    accept URLs and data URIs; ``source_path`` remains a filesystem path.
    """

    found: List[str] = []

    def walk(key: Optional[str], value: object) -> None:
        if isinstance(value, dict):
            for child_key, child in value.items():
                walk(str(child_key), child)
        elif isinstance(value, (list, tuple)):
            for child in value:
                walk(key, child)
        elif isinstance(value, str) and key is not None and is_filesystem_key(key):
            normalized_key = _normalize_argument_key(key)
            if normalized_key in {"source", "sources"} and (
                _looks_like_remote_source(value)
            ):
                return
            found.append(value)

    walk(None, arguments)
    return found


def check_path(slot_id: str, scope: PathScope, candidate: str) -> None:
    """Raise PathNotInScopeError if `candidate` is not permitted.

    `candidate` is normalized via expanduser; `scope.allowed` and
    `scope.forbidden` likewise.
    """
    cand = _resolve(candidate)

    # Forbidden veto wins.
    for f in scope.forbidden:
        f_norm = _resolve(f)
        if _contains(f_norm, cand):
            raise PathNotInScopeError(
                slot_id, candidate, scope.allowed, scope.forbidden, False
            )

    if not scope.allowed:
        raise PathNotInScopeError(
            slot_id, candidate, scope.allowed, scope.forbidden, True
        )

    for a in scope.allowed:
        a_norm = _resolve(a)
        if _contains(a_norm, cand):
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
