"""Safe Markdown-table row parsing.

Every table parser in this plugin was splitting a data line on ``|``
and indexing cells by position — a client or project name containing
a literal pipe character shifted every downstream index, so
``_parse_money(cells[5])`` read a non-money string and the row was
silently dropped. That silent corruption rippled into ``ytd_total``,
the threshold watchdog, the dashboard, the pipeline summary, and
every ``mark-paid`` row lookup.

``parse_row`` takes the expected column count and either returns
exactly that many trimmed cells or returns ``None`` — never a
wrongly-shifted list. Callers route ``None`` rows to
:func:`log_malformed_row` which appends to
``$MAKAKOO_HOME/state/skill-freelance-office/malformed_rows.jsonl``
for later inspection via ``freelance-office doctor``.
"""
from __future__ import annotations

import json
import os
from datetime import datetime, timezone
from pathlib import Path
from typing import List, Optional


def parse_row(line: str, expected_cols: int) -> Optional[List[str]]:
    """Split ``line`` on ``|`` and return exactly ``expected_cols``
    trimmed cells, or ``None`` if the row is malformed.

    A row is malformed if it does not start with ``|`` or if the cell
    count does not match ``expected_cols``. ``None`` signals "skip
    this row safely" to the caller, which should route to
    :func:`log_malformed_row` so the user can find it later.
    """
    if not line.startswith("|"):
        return None
    stripped = line.strip().strip("|")
    cells = [c.strip() for c in stripped.split("|")]
    if len(cells) != expected_cols:
        return None
    return cells


def state_dir() -> Path:
    """Resolve ``$MAKAKOO_HOME/state/skill-freelance-office/``.

    Falls back to ``~/MAKAKOO/state/skill-freelance-office/`` when the
    env var is unset. The directory is created on first write.
    """
    env = os.environ.get("MAKAKOO_HOME")
    root = Path(env).expanduser() if env else Path.home() / "MAKAKOO"
    return root / "state" / "skill-freelance-office"


def malformed_rows_path() -> Path:
    return state_dir() / "malformed_rows.jsonl"


def log_malformed_row(
    kind: str,
    line: str,
    *,
    source: Optional[Path] = None,
    reason: str = "cell count mismatch",
    expected_cols: Optional[int] = None,
) -> Path:
    """Append one malformed-row record to ``malformed_rows.jsonl``.

    ``kind`` is a short label ("earnings" / "expenses:equipment" / …).
    ``source`` is the file the line was read from, when known.
    Returns the sentinel path.
    """
    path = malformed_rows_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    record = {
        "ts": datetime.now(tz=timezone.utc).isoformat(timespec="seconds"),
        "kind": kind,
        "source": str(source) if source else None,
        "expected_cols": expected_cols,
        "reason": reason,
        "line": line.rstrip("\n"),
    }
    with path.open("a", encoding="utf-8") as f:
        f.write(json.dumps(record, ensure_ascii=False) + "\n")
    return path


def malformed_row_count() -> int:
    """Total rows recorded in the sentinel file — 0 when the file
    does not exist. Used by ``freelance-office doctor``."""
    p = malformed_rows_path()
    if not p.is_file():
        return 0
    with p.open("r", encoding="utf-8") as f:
        return sum(1 for _ in f)
