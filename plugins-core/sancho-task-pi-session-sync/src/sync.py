#!/usr/bin/env python3
"""pi_session_sync — SANCHO task that mirrors pi sessions into Brain.

Runs every 600s (10 min). For each `~/.pi/agent/sessions/*.jsonl` file
whose mtime is newer than the last synced mtime stored in the ledger:

  1. Parse the JSONL session.
  2. Render an outline-form Brain page at
     `$MAKAKOO_HOME/data/Brain/pages/pi-sessions/<session_id>.md`.
  3. Update the ledger at
     `$MAKAKOO_HOME/state/agent-pi/synced.json` with the new mtime.

Gate: file mtime change. Sessions pi hasn't touched since last tick are
skipped to keep the task cheap.

Exit 0 always — SANCHO treats non-zero as a task failure worth paging
about, but this task is "best effort". Failures log to stderr and the
ledger is left untouched so the next tick retries.
"""
from __future__ import annotations

import json
import logging
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, Iterable, List

log = logging.getLogger("harvey.sancho.pi_session_sync")


@dataclass
class SyncConfig:
    sessions_dir: Path
    brain_pages: Path
    ledger: Path


def _makakoo_home() -> Path:
    raw = (
        os.environ.get("MAKAKOO_HOME")
        or os.environ.get("HARVEY_HOME")
        or os.path.expanduser("~/MAKAKOO")
    )
    return Path(raw).expanduser().resolve()


def default_config() -> SyncConfig:
    home = _makakoo_home()
    return SyncConfig(
        sessions_dir=Path(os.path.expanduser("~/.pi/agent/sessions")),
        brain_pages=home / "data" / "Brain" / "pages" / "pi-sessions",
        ledger=home / "state" / "agent-pi" / "synced.json",
    )


def load_ledger(path: Path) -> Dict[str, float]:
    try:
        text = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return {}
    except OSError as e:
        log.warning("cannot read ledger %s: %s", path, e)
        return {}
    try:
        data = json.loads(text)
    except json.JSONDecodeError as e:
        log.warning("ledger %s is corrupt (%s) — rebuilding", path, e)
        return {}
    if not isinstance(data, dict):
        return {}
    return {k: float(v) for k, v in data.items() if isinstance(v, (int, float))}


def save_ledger(path: Path, ledger: Dict[str, float]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(ledger, indent=2, sort_keys=True), encoding="utf-8")
    tmp.replace(path)


def iter_sessions(cfg: SyncConfig, ledger: Dict[str, float]) -> Iterable[Path]:
    """Yield session files whose mtime is newer than the ledger recorded."""
    if not cfg.sessions_dir.is_dir():
        return
    for child in sorted(cfg.sessions_dir.glob("*.jsonl")):
        try:
            mtime = child.stat().st_mtime
        except OSError:
            continue
        prior = ledger.get(child.name, 0.0)
        if mtime > prior:
            yield child


def parse_session(path: Path) -> List[Dict[str, Any]]:
    entries: List[Dict[str, Any]] = []
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError as e:
        log.warning("cannot read session %s: %s", path, e)
        return []
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            entries.append(json.loads(line))
        except json.JSONDecodeError as e:
            log.debug("skipping non-JSON session line in %s: %s", path, e)
    return entries


def render_brain_page(session_id: str, entries: List[Dict[str, Any]]) -> str:
    """Outline-format Brain page (Logseq requires every line start with `- `)."""
    lines: List[str] = [
        f"- tags:: #pi #session #makakoo",
        f"- session-id:: `{session_id}`",
        f"- source:: pi-mono",
        f"- entry-count:: {len(entries)}",
        "",
        "- ## Timeline",
    ]
    for e in entries:
        ety = e.get("type", "?")
        role = e.get("role") or (e.get("event") or {}).get("role") or ""
        # Extract best-available message text.
        text = ""
        if ety == "message":
            text = (e.get("content") or e.get("text") or "").strip()
        elif ety == "event":
            ev = e.get("event") or {}
            if ev.get("type") == "message_end":
                text = (ev.get("text") or (ev.get("message") or {}).get("content") or "").strip()
        elif ety == "label":
            text = f"LABEL: {e.get('label', '')}"
        elif ety == "compaction":
            text = (
                f"COMPACTION: kept from {e.get('firstKeptEntryId')}"
                f" ({e.get('tokensBefore', '?')}→{e.get('tokensAfter', '?')} tokens)"
            )
        if not text:
            continue
        first, *rest = text.splitlines()
        label = f"{ety}{f'::{role}' if role else ''}"
        lines.append(f"  - {label}: {first}")
        for r in rest:
            if r.strip():
                lines.append(f"    - {r}")
    return "\n".join(lines) + "\n"


def sync_once(cfg: SyncConfig | None = None) -> Dict[str, Any]:
    cfg = cfg or default_config()
    cfg.brain_pages.mkdir(parents=True, exist_ok=True)
    cfg.ledger.parent.mkdir(parents=True, exist_ok=True)

    ledger = load_ledger(cfg.ledger)
    updated_count = 0
    skipped_count = 0

    for session_file in iter_sessions(cfg, ledger):
        entries = parse_session(session_file)
        if not entries:
            skipped_count += 1
            continue
        session_id = session_file.stem
        page_body = render_brain_page(session_id, entries)
        page_path = cfg.brain_pages / f"{session_id}.md"
        try:
            page_path.write_text(page_body, encoding="utf-8")
        except OSError as e:
            log.error("cannot write brain page %s: %s", page_path, e)
            continue
        try:
            ledger[session_file.name] = session_file.stat().st_mtime
        except OSError:
            pass
        updated_count += 1

    if updated_count:
        save_ledger(cfg.ledger, ledger)
    return {
        "updated": updated_count,
        "skipped": skipped_count,
        "total_ledgered": len(ledger),
    }


def main() -> int:
    logging.basicConfig(
        level=os.environ.get("LOG_LEVEL", "INFO"),
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )
    result = sync_once()
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
