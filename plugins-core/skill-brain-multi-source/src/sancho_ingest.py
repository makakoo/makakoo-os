#!/usr/bin/env python3
"""SANCHO multi-source brain ingest.

Walks every registered brain source every N minutes, collects docs newer than
the per-source cursor, and triggers the Rust `makakoo sync` indexer when
anything changed. The Rust indexer owns the actual source-labeled writes into
`brain_docs`; this task is only the scheduler/cursor layer.

Cursor file: `$MAKAKOO_HOME/state/skill-brain-multi-source/cursors.json`
Format: `{source_name: last_seen_mtime_epoch}`

Exit codes:
  0 — clean tick (zero or more sources had new docs)
  1 — recoverable error (config missing, one source unreadable)
  2 — fatal (no registry could load at all)
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import time
import traceback
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))


CURSOR_FILENAME = "cursors.json"


def _makakoo_home() -> Path:
    home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME")
    if home:
        return Path(home).expanduser()
    return Path.home() / "MAKAKOO"


def _state_dir() -> Path:
    d = _makakoo_home() / "state" / "skill-brain-multi-source"
    d.mkdir(parents=True, exist_ok=True)
    return d


def _cursor_path() -> Path:
    return _state_dir() / CURSOR_FILENAME


def _load_cursors() -> dict:
    path = _cursor_path()
    if not path.exists():
        return {}
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError):
        return {}


def _save_cursors(cursors: dict) -> None:
    path = _cursor_path()
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(cursors, indent=2))
    tmp.replace(path)


def _dispatch_to_pipeline(source_name: str, docs: list) -> int:
    """Count new docs. Real pipeline runs once per tick via `_run_makakoo_sync`."""
    return len(docs)


def _run_makakoo_sync() -> dict:
    if os.environ.get("PYTEST_CURRENT_TEST"):
        return {"ran": False, "reason": "pytest"}
    exe = shutil.which("makakoo")
    if not exe:
        return {"ran": False, "reason": "makakoo not on PATH"}
    env = os.environ.copy()
    env["MAKAKOO_HOME"] = str(_makakoo_home())
    try:
        proc = subprocess.run(
            [exe, "sync"],
            env=env,
            capture_output=True,
            text=True,
            timeout=300,
        )
    except Exception as e:
        return {"ran": False, "reason": str(e)}
    return {
        "ran": True,
        "ok": proc.returncode == 0,
        "code": proc.returncode,
        "stdout": proc.stdout.strip()[-500:],
        "stderr": proc.stderr.strip()[-500:],
    }


def tick() -> int:
    try:
        import config as cfg  # type: ignore
    except ImportError:
        print(json.dumps({"status": "error", "reason": "brain_sources config module missing"}))
        return 2

    registry = cfg.load_registry()
    cursors = _load_cursors()
    now = time.time()

    per_source: dict = {}
    total_new = 0
    errors: list = []

    for src in registry.sources:
        last_seen = cursors.get(src.name, 0.0)
        new_docs = []
        try:
            for doc in src.iter_docs():
                if doc.mtime > last_seen:
                    new_docs.append(doc)
        except Exception as e:
            errors.append({"source": src.name, "error": str(e)})
            traceback.print_exc(file=sys.stderr)
            continue

        count = _dispatch_to_pipeline(src.name, new_docs)
        per_source[src.name] = count
        total_new += count

        if new_docs:
            cursors[src.name] = max(d.mtime for d in new_docs)
        else:
            # Advance cursor to "now" so next tick only sees truly-new mtimes
            cursors[src.name] = now

    _save_cursors(cursors)

    sync_result = {"ran": False, "reason": "no new docs"}
    if total_new > 0:
        sync_result = _run_makakoo_sync()
        if sync_result.get("ran") and not sync_result.get("ok", False):
            errors.append({"source": "makakoo sync", "error": sync_result})

    result = {
        "status": "ok" if not errors else "partial",
        "sources": len(registry.sources),
        "new_docs_total": total_new,
        "per_source": per_source,
        "sync": sync_result,
    }
    if errors:
        result["errors"] = errors
    print(json.dumps(result))
    return 0 if not errors else 1


def main() -> int:
    try:
        return tick()
    except Exception:
        traceback.print_exc()
        return 1


if __name__ == "__main__":
    sys.exit(main())
