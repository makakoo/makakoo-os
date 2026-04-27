"""
CRM pipeline tick — delegates to agent-career-manager's lead sync.

v0.2 Phase C.6. Runs every 2h:
  1. Walk state files under $MAKAKOO_HOME/data/career/
  2. For each lead, call `sync_lead_to_brain()` (idempotent)
  3. Append a one-line summary to today's Brain journal

This is the "daemon" replacement for what the user historically ran by
hand via cron. The plugin depends on agent-career-manager being
installed so the sync helper resolves.

Exit code 0 on success (including no leads found). Non-zero exits mark
the SANCHO handler as failed, which surfaces in `makakoo sancho status`.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path


def _makakoo_home() -> Path:
    home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME")
    if home:
        return Path(home)
    return Path.home() / "MAKAKOO"


def _append_journal_line(home: Path, line: str) -> None:
    today = datetime.now(timezone.utc).strftime("%Y_%m_%d")
    jdir = home / "data" / "Brain" / "journals"
    jdir.mkdir(parents=True, exist_ok=True)
    journal = jdir / f"{today}.md"
    if not line.startswith("- "):
        line = f"- {line}"
    with open(journal, "a", encoding="utf-8") as f:
        f.write(line + "\n")


def _collect_leads(home: Path) -> list[dict]:
    """Return every lead dict under data/career/leads/*.json. Silent
    when the dir is missing — fresh installs have no CRM state yet."""
    leads_dir = home / "data" / "career" / "leads"
    if not leads_dir.exists():
        return []
    leads: list[dict] = []
    for p in sorted(leads_dir.glob("*.json")):
        try:
            with open(p, encoding="utf-8") as f:
                leads.append(json.load(f))
        except Exception:
            # Skip malformed lead files; the CRM writer will heal next tick.
            continue
    return leads


def _sync_to_brain(home: Path, leads: list[dict]) -> tuple[int, int]:
    """Call agent-career-manager's sync_lead_to_brain for every lead.
    Returns (synced, skipped)."""
    plugin_root = home / "plugins" / "agent-career-manager" / "src"
    if not plugin_root.exists():
        # Fall back to the plugins-core source tree — useful in dev.
        plugin_root = home / "plugins-core" / "agent-career-manager" / "src"
    if str(plugin_root) not in sys.path:
        sys.path.insert(0, str(plugin_root))
    try:
        from sync_to_brain import sync_lead_to_brain  # type: ignore[import-not-found]
    except Exception as e:  # pragma: no cover — exercised via agent-career-manager install
        print(f"[crm_pipeline] sync_to_brain unavailable: {e}", file=sys.stderr)
        return (0, len(leads))

    synced = skipped = 0
    for lead in leads:
        try:
            sync_lead_to_brain(lead)
            synced += 1
        except Exception as e:
            print(f"[crm_pipeline] sync failed for {lead.get('id')}: {e}", file=sys.stderr)
            skipped += 1
    return synced, skipped


def main() -> int:
    parser = argparse.ArgumentParser(description="CRM pipeline SANCHO tick.")
    parser.add_argument("--task", default="crm_pipeline", help=argparse.SUPPRESS)
    parser.add_argument("--dry-run", action="store_true", help="Count leads, skip writes.")
    args = parser.parse_args()

    home = _makakoo_home()
    leads = _collect_leads(home)
    if args.dry_run:
        print(f"crm_pipeline: {len(leads)} leads (dry-run)")
        return 0

    synced, skipped = _sync_to_brain(home, leads)
    line = (
        f"- [[SANCHO]] [[crm_pipeline]] {len(leads)} leads "
        f"({synced} synced, {skipped} skipped)"
    )
    _append_journal_line(home, line)
    print(line.lstrip("- "))
    return 0


if __name__ == "__main__":
    sys.exit(main())
