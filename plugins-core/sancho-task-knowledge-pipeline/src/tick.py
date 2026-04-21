"""
Knowledge pipeline tick — run the daily mine → extract → integrate pass.

v0.2 Phase C.6. Replaces the old cron-driven script that ran:
  1. agent-knowledge-extractor/miner.py     — walks new docs
  2. agent-knowledge-extractor/extractor.py — semantic extraction
  3. agent-knowledge-extractor/integrator.py — merge into Brain

Each stage is called in-process (import + call main()) so a failure
halts the pipeline with a clear log line instead of silently running
the next stage on broken state.

Non-fatal if a stage has no work — the `already-processed` check at
the miner level is what keeps the idempotency guarantee.
"""

from __future__ import annotations

import argparse
import os
import sys
import traceback
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


def _plugin_src(home: Path, plugin: str) -> Path | None:
    """Return the src/ dir of an installed plugin, or the plugins-core
    source tree in dev installs. None if neither is present."""
    installed = home / "plugins" / plugin / "src"
    if installed.exists():
        return installed
    source = home / "plugins-core" / plugin / "src"
    if source.exists():
        return source
    return None


def _run_stage(home: Path, plugin: str, module: str) -> tuple[bool, str]:
    """Import + call main() for one stage. Returns (ok, one_line_msg)."""
    src = _plugin_src(home, plugin)
    if src is None:
        return False, f"{module}: plugin {plugin!r} not installed"
    if str(src) not in sys.path:
        sys.path.insert(0, str(src))

    try:
        imported = __import__(module)
    except Exception as e:
        return False, f"{module}: import failed: {e}"
    fn = getattr(imported, "main", None)
    if fn is None:
        return False, f"{module}: module has no main()"
    try:
        rc = fn()
    except SystemExit as se:
        rc = int(se.code or 0)
    except Exception as e:
        tb = traceback.format_exc().splitlines()[-1]
        return False, f"{module}: raised {e!r} ({tb})"
    if rc not in (None, 0):
        return False, f"{module}: exit {rc}"
    return True, f"{module}: ok"


def main() -> int:
    parser = argparse.ArgumentParser(description="Knowledge pipeline SANCHO tick.")
    parser.add_argument("--task", default="knowledge_pipeline", help=argparse.SUPPRESS)
    parser.add_argument("--dry-run", action="store_true", help="List stages, skip execution.")
    args = parser.parse_args()

    home = _makakoo_home()

    stages = [
        ("agent-knowledge-extractor", "miner"),
        ("agent-knowledge-extractor", "extractor"),
        ("agent-knowledge-extractor", "integrator"),
    ]

    if args.dry_run:
        for plugin, module in stages:
            print(f"would run {plugin}/{module}")
        return 0

    results = []
    for plugin, module in stages:
        ok, msg = _run_stage(home, plugin, module)
        results.append((ok, msg))
        if not ok:
            break  # halt on first failure — downstream depends on upstream

    ok_count = sum(1 for ok, _ in results if ok)
    status = "ok" if all(ok for ok, _ in results) else "halted"
    summary = (
        f"- [[SANCHO]] [[knowledge_pipeline]] {status}: "
        f"{ok_count}/{len(stages)} stages"
    )
    _append_journal_line(home, summary)
    print(summary.lstrip("- "))
    for ok, msg in results:
        prefix = "  ✓" if ok else "  ✗"
        print(f"{prefix} {msg}")

    return 0 if status == "ok" else 1


if __name__ == "__main__":
    sys.exit(main())
