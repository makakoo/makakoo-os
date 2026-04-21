"""
Python bridge to the Rust swarm dispatch queue.

v0.2 Phase D.4. The Rust SwarmDispatchHandler drains pending dispatch
requests from `$MAKAKOO_HOME/state/swarm/queue.jsonl` on every SANCHO
tick. This module is the Python-side producer: any agent, skill, or
HarveyChat path can enqueue a team/agent dispatch without needing a
Rust bridge or MCP call.

Design goals:
  * Byte-compatible line format with the Rust reader — we write
    exactly the JSON shape `serde` expects (`{"kind":"team", ...}` /
    `{"kind":"agent", ...}`) so Rust doesn't need a separate parser.
  * At-least-once semantics via JSONL append + unique ids.
  * No dependencies beyond stdlib — this is a shim, not an engine.

Example:
    >>> from core.orchestration.swarm_dispatch_queue import enqueue_team
    >>> qid = enqueue_team("research_team", "what is lope?", parallelism=3)
    >>> qid
    'q-20260421T013045.123456-1a2b3c4d'
"""

from __future__ import annotations

import json
import os
import secrets
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, Optional


def _makakoo_home() -> Path:
    """Resolve $MAKAKOO_HOME, falling back to $HARVEY_HOME or ~/MAKAKOO."""
    home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME")
    if home:
        return Path(home)
    return Path.home() / "MAKAKOO"


def queue_dir(home: Optional[Path] = None) -> Path:
    """Match the Rust `queue_dir` resolver."""
    home = home or _makakoo_home()
    return home / "state" / "swarm"


def queue_path(home: Optional[Path] = None) -> Path:
    return queue_dir(home) / "queue.jsonl"


def receipts_path(home: Optional[Path] = None) -> Path:
    return queue_dir(home) / "receipts.jsonl"


def _mint_id() -> str:
    """Time-ordered unique id — tracks the Rust `mint_id` pattern."""
    ts = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%f")
    tail = secrets.token_hex(4)
    return f"q-{ts}-{tail}"


def _now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")


def _append_line(path: Path, obj: Dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    line = json.dumps(obj, separators=(",", ":"), ensure_ascii=False)
    # Append + fsync matches what the Rust writer does. Tests rely on the
    # line being visible to the next open call with no extra flush.
    with open(path, "a", encoding="utf-8") as f:
        f.write(line + "\n")
        f.flush()
        try:
            os.fsync(f.fileno())
        except OSError:
            # Not all filesystems support fsync on a regular file handle.
            # The write is already append-only so the loss window is tiny.
            pass


def enqueue_team(
    team: str,
    prompt: str,
    *,
    parallelism: Optional[int] = None,
    model: Optional[str] = None,
    home: Optional[Path] = None,
) -> str:
    """Enqueue a TeamDispatchRequest. Returns the queue id."""
    qid = _mint_id()
    entry: Dict[str, Any] = {
        "kind": "team",
        "id": qid,
        "enqueued_at": _now_iso(),
        "team": team,
        "prompt": prompt,
    }
    if parallelism is not None:
        entry["parallelism"] = int(parallelism)
    if model:
        entry["model"] = model
    _append_line(queue_path(home), entry)
    return qid


def enqueue_agent(
    name: str,
    task: str,
    prompt: str,
    *,
    model: Optional[str] = None,
    parent_run_id: Optional[str] = None,
    home: Optional[Path] = None,
) -> str:
    """Enqueue a single-agent DispatchRequest. Returns the queue id."""
    qid = _mint_id()
    entry: Dict[str, Any] = {
        "kind": "agent",
        "id": qid,
        "enqueued_at": _now_iso(),
        "name": name,
        "task": task,
        "prompt": prompt,
    }
    if model:
        entry["model"] = model
    if parent_run_id:
        entry["parent_run_id"] = parent_run_id
    _append_line(queue_path(home), entry)
    return qid


def queue_depth(home: Optional[Path] = None) -> int:
    """Number of queue lines minus receipted ids. Read-only."""
    qp = queue_path(home)
    if not qp.exists():
        return 0
    with open(qp, encoding="utf-8") as f:
        queue_ids = {
            json.loads(line)["id"]
            for line in f
            if line.strip()
        }
    rp = receipts_path(home)
    if rp.exists():
        with open(rp, encoding="utf-8") as f:
            receipted = {
                json.loads(line)["id"]
                for line in f
                if line.strip()
            }
        queue_ids -= receipted
    return len(queue_ids)


__all__ = [
    "enqueue_agent",
    "enqueue_team",
    "queue_dir",
    "queue_path",
    "queue_depth",
    "receipts_path",
]
