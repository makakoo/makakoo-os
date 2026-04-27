"""Brain journal attribution.

Every line written to today's Brain journal by the slot's gateway
gets prefixed with `[agent:<slot_id>] `. The supervisor pre-issues a
write grant for the journal directory bound to this slot, so even if
the gateway forgets to call `prefix_line`, the audit log still
attributes the write correctly. This module is the user-friendly
path: it produces well-formed Logseq outliner lines and writes them
through the makakoo journal API surface.
"""

from __future__ import annotations

from datetime import datetime, timezone
from pathlib import Path


PREFIX_TEMPLATE = "[agent:{slot_id}] "


def prefix_line(slot_id: str, body: str) -> str:
    """Return `body` prefixed with the canonical `[agent:<slot>]`
    tag. If the body already starts with the prefix, do not double-
    prefix (idempotent for safety)."""
    prefix = PREFIX_TEMPLATE.format(slot_id=slot_id)
    if body.startswith(prefix):
        return body
    return prefix + body


def journal_path(makakoo_home: Path, day: datetime | None = None) -> Path:
    """Locked path: `<makakoo>/data/Brain/journals/YYYY_MM_DD.md`."""
    if day is None:
        day = datetime.now(timezone.utc)
    name = day.strftime("%Y_%m_%d") + ".md"
    return makakoo_home / "data" / "Brain" / "journals" / name


def render_logseq_bullet(slot_id: str, body: str) -> str:
    """Render a Logseq outliner bullet (`- `) with the slot prefix.
    Multi-line bodies are NOT auto-indented — caller is responsible
    for outliner indentation if they want sub-bullets."""
    line = prefix_line(slot_id, body)
    return f"- {line}\n"


def append_journal_line(
    makakoo_home: Path,
    slot_id: str,
    body: str,
    *,
    day: datetime | None = None,
) -> Path:
    """Append a single attributed line to today's journal.
    Returns the journal path written."""
    path = journal_path(makakoo_home, day)
    path.parent.mkdir(parents=True, exist_ok=True)
    bullet = render_logseq_bullet(slot_id, body)
    with path.open("a", encoding="utf-8") as f:
        f.write(bullet)
    return path
