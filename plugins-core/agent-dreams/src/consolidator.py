#!/usr/bin/env python3
"""
Harvey Dreams — Memory Consolidation Engine.

Inspired by claurst's SANCHO system. Harvey "dreams" during idle time
to consolidate Brain memories:

  1. Orient  — Scan Brain state, identify what needs attention
  2. Gather  — Read recent journals, find patterns and gaps
  3. Consolidate — Update entity pages, strengthen connections, resolve conflicts
  4. Prune   — Remove stale entries, keep index compact

Three-gate trigger prevents over/under-dreaming:
  - Time gate: minimum N hours since last dream
  - Session gate: minimum N sessions since last dream
  - Lock gate: no concurrent dreams

Usage:
    from core.dreams.consolidator import DreamEngine
    engine = DreamEngine()

    if engine.should_dream():
        report = engine.dream()
        print(report)

    # Or run as CLI:
    python3 consolidator.py           # Dream if gates allow
    python3 consolidator.py --force   # Dream regardless of gates
    python3 consolidator.py --status  # Show gate status
"""

import json
import logging
import os
import time
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from pathlib import Path
from typing import Dict, List, Optional

log = logging.getLogger("harvey.dreams")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
BRAIN_DIR = Path(HARVEY_HOME) / "data" / "Brain"
DREAM_STATE = Path(HARVEY_HOME) / "data" / "dream_state.json"


@dataclass
class DreamState:
    """Persistent state for the dream engine."""
    last_dream_ts: float = 0.0
    last_dream_iso: str = ""
    sessions_since_dream: int = 0
    total_dreams: int = 0
    total_pages_updated: int = 0
    total_pages_pruned: int = 0
    total_conflicts_resolved: int = 0
    is_dreaming: bool = False


@dataclass
class DreamReport:
    """Result of a dream cycle."""
    phase_results: Dict[str, dict]
    pages_updated: int = 0
    pages_created: int = 0
    pages_pruned: int = 0
    orphans_found: int = 0
    conflicts_resolved: int = 0
    duration_sec: float = 0
    timestamp: str = ""


class DreamEngine:
    """Harvey's memory consolidation engine."""

    # ── Gate thresholds ───────────────────────────────────────
    MIN_HOURS_BETWEEN_DREAMS = 12
    MIN_SESSIONS_BETWEEN_DREAMS = 3

    def __init__(self):
        self.brain_dir = BRAIN_DIR
        self.pages_dir = BRAIN_DIR / "pages"
        self.journals_dir = BRAIN_DIR / "journals"
        self.state = self._load_state()

    # ═══════════════════════════════════════════════════════════
    #  Three-Gate System
    # ═══════════════════════════════════════════════════════════

    def should_dream(self) -> bool:
        """Check all three gates. ALL must pass."""
        return (
            self._time_gate()
            and self._session_gate()
            and self._lock_gate()
        )

    def _time_gate(self) -> bool:
        """Minimum time since last dream."""
        if self.state.last_dream_ts == 0:
            return True  # Never dreamed before
        hours = (time.time() - self.state.last_dream_ts) / 3600
        return hours >= self.MIN_HOURS_BETWEEN_DREAMS

    def _session_gate(self) -> bool:
        """Minimum sessions since last dream."""
        return self.state.sessions_since_dream >= self.MIN_SESSIONS_BETWEEN_DREAMS

    def _lock_gate(self) -> bool:
        """No concurrent dreams."""
        return not self.state.is_dreaming

    def gate_status(self) -> dict:
        """Human-readable gate status."""
        hours_since = 0
        if self.state.last_dream_ts:
            hours_since = (time.time() - self.state.last_dream_ts) / 3600

        return {
            "time_gate": {
                "passed": self._time_gate(),
                "hours_since_last": round(hours_since, 1),
                "threshold": self.MIN_HOURS_BETWEEN_DREAMS,
            },
            "session_gate": {
                "passed": self._session_gate(),
                "sessions_since_last": self.state.sessions_since_dream,
                "threshold": self.MIN_SESSIONS_BETWEEN_DREAMS,
            },
            "lock_gate": {
                "passed": self._lock_gate(),
                "is_dreaming": self.state.is_dreaming,
            },
            "should_dream": self.should_dream(),
            "total_dreams": self.state.total_dreams,
            "last_dream": self.state.last_dream_iso or "never",
        }

    def record_session(self):
        """Call this at session start to increment session counter."""
        self.state.sessions_since_dream += 1
        self._save_state()

    # ═══════════════════════════════════════════════════════════
    #  Dream Cycle
    # ═══════════════════════════════════════════════════════════

    def dream(self, force: bool = False) -> DreamReport:
        """
        Execute a full dream cycle.

        Four phases:
        1. Orient  — Survey Brain state
        2. Gather  — Find patterns in recent journals
        3. Consolidate — Update pages, resolve conflicts
        4. Prune   — Clean up stale/orphan content
        """
        if not force and not self.should_dream():
            return DreamReport(
                phase_results={"error": "Gates not passed"},
                timestamp=datetime.now().isoformat(),
            )

        start = time.time()
        self.state.is_dreaming = True
        self._save_state()

        try:
            # Import wiki ops
            import sys
            sys.path.insert(0, str(Path(HARVEY_HOME) / "harvey-os"))
            from core.superbrain.wiki import WikiOps

            wiki = WikiOps()
            report = DreamReport(
                phase_results={},
                timestamp=datetime.now().isoformat(),
            )

            # Phase 1: Orient
            log.info("Dream Phase 1: Orient")
            lint_report = wiki.lint()
            report.phase_results["orient"] = {
                "total_pages": lint_report["total_pages"],
                "total_journals": lint_report["total_journals"],
                "orphans": lint_report["orphan_count"],
                "missing": lint_report["missing_count"],
                "high_value_missing": len(lint_report.get("high_value_missing", [])),
            }
            report.orphans_found = lint_report["orphan_count"]

            # Phase 2: Gather — compile recent unprocessed journals
            log.info("Dream Phase 2: Gather")
            compile_result = wiki.compile_all(since_days=7, dry_run=False)
            report.phase_results["gather"] = compile_result
            report.pages_updated = compile_result.get("total_updated", 0)
            report.pages_created = compile_result.get("total_created", 0)

            # Phase 3: Consolidate — rebuild index with fresh data
            log.info("Dream Phase 3: Consolidate")
            index_content = wiki.build_index()
            report.phase_results["consolidate"] = {
                "index_lines": index_content.count("\n"),
                "action": "index_rebuilt",
            }

            # Phase 4: Prune — identify and clean up empty pages
            log.info("Dream Phase 4: Prune")
            prune_result = self._prune_empty_pages(wiki)
            report.phase_results["prune"] = prune_result
            report.pages_pruned = prune_result.get("pruned", 0)

            report.duration_sec = round(time.time() - start, 2)

            # Log to Brain
            wiki.log_op("dream", (
                f"Dream #{self.state.total_dreams + 1}: "
                f"{report.pages_updated} updated, "
                f"{report.pages_created} created, "
                f"{report.pages_pruned} pruned, "
                f"{report.duration_sec}s"
            ))

            # Update state
            self.state.last_dream_ts = time.time()
            self.state.last_dream_iso = datetime.now().isoformat()
            self.state.sessions_since_dream = 0
            self.state.total_dreams += 1
            self.state.total_pages_updated += report.pages_updated
            self.state.total_pages_pruned += report.pages_pruned

            log.info("Dream complete: %s", report.phase_results)
            return report

        finally:
            self.state.is_dreaming = False
            self._save_state()

    def _prune_empty_pages(self, wiki) -> dict:
        """Remove truly empty pages (<20 chars, no inbound links)."""
        pruned = []
        skipped = []

        for f in self.pages_dir.glob("*.md"):
            try:
                content = f.read_text(encoding="utf-8", errors="replace")
            except Exception:
                continue

            # Skip system pages
            if f.stem in ("Brain Index", "Brain Log", "Harvey OS Index"):
                continue

            # Only prune truly empty pages (not stubs with content)
            if len(content.strip()) < 20:
                # Check inbound links before deleting
                has_inbound = False
                for other in self.pages_dir.glob("*.md"):
                    if other == f:
                        continue
                    try:
                        other_content = other.read_text(encoding="utf-8", errors="replace")
                        if f"[[{f.stem}]]" in other_content:
                            has_inbound = True
                            break
                    except Exception:
                        continue

                if not has_inbound:
                    pruned.append(f.stem)
                    f.unlink()
                else:
                    skipped.append(f.stem)

        return {
            "pruned": len(pruned),
            "pruned_pages": pruned[:20],
            "skipped_with_inbound": len(skipped),
        }

    # ── State Persistence ─────────────────────────────────────

    def _load_state(self) -> DreamState:
        if DREAM_STATE.exists():
            try:
                data = json.loads(DREAM_STATE.read_text())
                return DreamState(**data)
            except Exception:
                pass
        return DreamState()

    def _save_state(self):
        DREAM_STATE.parent.mkdir(parents=True, exist_ok=True)
        DREAM_STATE.write_text(json.dumps({
            "last_dream_ts": self.state.last_dream_ts,
            "last_dream_iso": self.state.last_dream_iso,
            "sessions_since_dream": self.state.sessions_since_dream,
            "total_dreams": self.state.total_dreams,
            "total_pages_updated": self.state.total_pages_updated,
            "total_pages_pruned": self.state.total_pages_pruned,
            "total_conflicts_resolved": self.state.total_conflicts_resolved,
            "is_dreaming": self.state.is_dreaming,
        }, indent=2))

    # ── Display ───────────────────────────────────────────────

    def print_status(self):
        status = self.gate_status()
        print(f"\n{'=' * 45}")
        print(f"  Harvey Dream Engine")
        print(f"{'=' * 45}")
        print(f"  Total dreams: {status['total_dreams']}")
        print(f"  Last dream:   {status['last_dream']}")
        print()
        for gate_name in ["time_gate", "session_gate", "lock_gate"]:
            g = status[gate_name]
            icon = "✅" if g["passed"] else "❌"
            if gate_name == "time_gate":
                print(f"  {icon} Time:    {g['hours_since_last']}h / {g['threshold']}h")
            elif gate_name == "session_gate":
                print(f"  {icon} Session: {g['sessions_since_last']} / {g['threshold']}")
            else:
                print(f"  {icon} Lock:    {'free' if g['passed'] else 'DREAMING'}")
        print()
        print(f"  Should dream: {'YES' if status['should_dream'] else 'NO'}")
        print(f"{'=' * 45}\n")

    def print_report(self, report: DreamReport):
        print(f"\n{'=' * 45}")
        print(f"  Dream Report")
        print(f"{'=' * 45}")
        print(f"  Duration:  {report.duration_sec}s")
        print(f"  Updated:   {report.pages_updated} pages")
        print(f"  Created:   {report.pages_created} pages")
        print(f"  Pruned:    {report.pages_pruned} pages")
        print(f"  Orphans:   {report.orphans_found}")
        for phase, result in report.phase_results.items():
            print(f"  [{phase}]: {result}")
        print(f"{'=' * 45}\n")


# ═══════════════════════════════════════════════════════════════
#  CLI
# ═══════════════════════════════════════════════════════════════

if __name__ == "__main__":
    import sys
    logging.basicConfig(level=logging.INFO, format="%(asctime)s [%(name)s] %(message)s")

    engine = DreamEngine()

    if "--status" in sys.argv:
        engine.print_status()
    elif "--force" in sys.argv:
        report = engine.dream(force=True)
        engine.print_report(report)
    else:
        if engine.should_dream():
            report = engine.dream()
            engine.print_report(report)
        else:
            print("Gates not passed. Use --force to dream anyway, or --status to check.")
