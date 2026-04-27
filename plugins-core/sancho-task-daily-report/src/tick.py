"""
Daily report tick — end-of-day Brain summary.

v0.2 Phase C.6. Every 24h (evening cadence when the user runs SANCHO
without gating to a specific hour): read today's journal, count
entries per wikilink, emit a `Brain/pages/daily-YYYY-MM-DD.md` summary
page. Pure markdown — no LLM call required — so the pipeline stays
runnable offline.

The compiled page is itself a Logseq node and picks up bidirectional
backlinks automatically: `[[daily-2026-04-21]]` in tomorrow's journal
resolves to this page, giving Sebastian a "what did I do yesterday"
jump point.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path


def _makakoo_home() -> Path:
    home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME")
    if home:
        return Path(home)
    return Path.home() / "MAKAKOO"


WIKILINK_RE = re.compile(r"\[\[([^\]]+)\]\]")


def _today_journal(home: Path) -> Path:
    today = datetime.now(timezone.utc).strftime("%Y_%m_%d")
    return home / "data" / "Brain" / "journals" / f"{today}.md"


def _parse_journal(path: Path) -> tuple[int, Counter]:
    """Return (line_count, link_counter) for the journal file."""
    if not path.exists():
        return 0, Counter()
    with open(path, encoding="utf-8") as f:
        lines = [l for l in f if l.strip()]
    links: Counter = Counter()
    for line in lines:
        for m in WIKILINK_RE.findall(line):
            links[m] += 1
    return len(lines), links


def _write_report(home: Path, line_count: int, links: Counter) -> Path:
    """Render a compact markdown page. Top 10 link counts + metadata."""
    today = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    pages_dir = home / "data" / "Brain" / "pages"
    pages_dir.mkdir(parents=True, exist_ok=True)
    out = pages_dir / f"daily-{today}.md"

    lines: list[str] = [f"- Daily report for [[{today}]]"]
    lines.append(f"  - journal entries: {line_count}")
    lines.append(f"  - unique wikilinks: {len(links)}")
    if links:
        lines.append("  - top mentions:")
        for name, count in links.most_common(10):
            lines.append(f"    - [[{name}]] × {count}")
    else:
        lines.append("  - (no wikilinks today)")

    body = "\n".join(lines) + "\n"
    # Overwrite: a second tick on the same day should refresh, not append.
    tmp = out.with_suffix(".md.tmp")
    tmp.write_text(body, encoding="utf-8")
    tmp.replace(out)
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description="Daily report SANCHO tick.")
    parser.add_argument("--task", default="daily_report", help=argparse.SUPPRESS)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    home = _makakoo_home()
    journal = _today_journal(home)
    line_count, links = _parse_journal(journal)

    if args.dry_run:
        print(
            f"daily_report: {line_count} lines, "
            f"{len(links)} unique links (dry-run)"
        )
        return 0

    out = _write_report(home, line_count, links)

    # Reflect back into today's journal so it's discoverable.
    today = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    with open(journal, "a", encoding="utf-8") as f:
        f.write(
            f"- [[SANCHO]] [[daily_report]] compiled "
            f"[[daily-{today}]] ({line_count} lines, {len(links)} links)\n"
        )
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
