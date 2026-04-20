"""
Memory Promoter — Active Memory promotion pipeline for Harvey OS.

Scores recall_stats entries using a 6-component weighted algorithm
(inspired by OpenClaw Active Memory, adapted for single-user Brain).
Promotes top candidates to Brain pages with full audit trail.

Architecture:
    RecallTracker.rebuild_stats()  →  recall_stats table (aggregated)
    MemoryPromoter.rank_candidates()  →  scored, filtered, sorted list
    MemoryPromoter.promote()  →  Brain page updates + event emission

Usage:
    from core.memory.memory_promoter import MemoryPromoter
    promoter = MemoryPromoter()
    report = promoter.promote(dry_run=True)   # Preview
    report = promoter.promote()               # Execute
"""

import json
import logging
import math
import os
import sqlite3
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional, Tuple

log = logging.getLogger("harvey.memory.promoter")

from core.paths import harvey_home as _harvey_home

HARVEY_HOME = _harvey_home()
DB_PATH = os.path.join(HARVEY_HOME, "data", "superbrain.db")
BRAIN_DIR = os.path.join(HARVEY_HOME, "data", "Brain")
JOURNALS_DIR = os.path.join(BRAIN_DIR, "journals")
PAGES_DIR = os.path.join(BRAIN_DIR, "pages")


class MemoryPromoter:
    """
    6-component scoring + promotion pipeline.

    Scoring components (weights sum to 1.0):
      frequency     (0.22): log(signal_count) / log(10)
      relevance     (0.28): average search score across recalls
      diversity     (0.18): max(unique_queries, unique_days) / 5
      recency       (0.17): exp(-(ln2/half_life) * age_days)
      consolidation (0.10): spaced repetition — temporal spacing across days
      conceptual    (0.05): entity/tag richness from content

    Weight rationale vs OpenClaw:
      +3% diversity (different contexts matter more for single user)
      +2% recency (Sebastian's attention shifts fast)
      -2% frequency, -2% relevance (Brain is curated, less noise)
      -1% conceptual (Harvey's entity graph already rich)
    """

    # ─── Weights ──────────────────────────────────────────────────
    W_FREQUENCY = 0.22
    W_RELEVANCE = 0.28
    W_DIVERSITY = 0.18
    W_RECENCY = 0.17
    W_CONSOLIDATION = 0.10
    W_CONCEPTUAL = 0.05

    # ─── Promotion gates ─────────────────────────────────────────
    MIN_RECALL_COUNT = 3       # At least 3 recalls
    MIN_UNIQUE_QUERIES = 2     # From at least 2 different questions
    MIN_SCORE = 0.70           # Minimum composite score
    MAX_AGE_DAYS = 45          # Don't promote stale entries
    MAX_PROMOTIONS_PER_RUN = 8 # Cap per execution

    # ─── Decay parameters ────────────────────────────────────────
    RECENCY_HALF_LIFE = 21     # 21-day half-life
    PHASE_BOOST_MAX = 0.08     # Max boost from consolidation encounters

    def __init__(self, db_path: str = None):
        self.db_path = db_path or DB_PATH

    # ═══════════════════════════════════════════════════════════════
    #  SCORING
    # ═══════════════════════════════════════════════════════════════

    def score(self, entry: dict) -> Tuple[float, dict]:
        """
        Score a single recall_stats entry.
        Returns (composite_score, component_breakdown).
        """
        freq = self._frequency(entry)
        rel = self._relevance(entry)
        div = self._diversity(entry)
        rec = self._recency(entry)
        con = self._consolidation(entry)
        cpt = self._conceptual(entry)
        phase = self._phase_boost(entry)

        composite = (
            self.W_FREQUENCY * freq
            + self.W_RELEVANCE * rel
            + self.W_DIVERSITY * div
            + self.W_RECENCY * rec
            + self.W_CONSOLIDATION * con
            + self.W_CONCEPTUAL * cpt
            + phase
        )

        components = {
            "frequency": round(freq, 4),
            "relevance": round(rel, 4),
            "diversity": round(div, 4),
            "recency": round(rec, 4),
            "consolidation": round(con, 4),
            "conceptual": round(cpt, 4),
            "phase_boost": round(phase, 4),
            "composite": round(min(1.0, composite), 4),
        }

        return min(1.0, composite), components

    def _frequency(self, e: dict) -> float:
        """log1p(signal_count) / log1p(10), clamped [0,1]."""
        signals = (e.get("recall_count") or 0) + (e.get("consolidation_hits") or 0)
        if signals <= 0:
            return 0.0
        return min(1.0, math.log1p(signals) / math.log1p(10))

    def _relevance(self, e: dict) -> float:
        """Average score across all recalls."""
        count = max(1, e.get("recall_count") or 1)
        total = e.get("total_score") or 0.0
        return min(1.0, total / count)

    def _diversity(self, e: dict) -> float:
        """max(unique_queries, unique_days) / 5, clamped [0,1]."""
        uq = e.get("unique_queries") or 0
        ud = e.get("unique_days") or 0
        return min(1.0, max(uq, ud) / 5.0)

    def _recency(self, e: dict) -> float:
        """Exponential decay: e^(-(ln2/T) * age_days)."""
        last = e.get("last_recalled_at")
        if not last:
            return 0.1
        try:
            last_dt = datetime.fromisoformat(last.replace("Z", "+00:00").split("+")[0])
            age = (datetime.now() - last_dt).days
        except (ValueError, TypeError):
            return 0.1
        lam = math.log(2) / self.RECENCY_HALF_LIFE
        return math.exp(-lam * max(0, age))

    def _consolidation(self, e: dict) -> float:
        """
        Spaced repetition: temporal spacing * temporal span.

        Memories recalled across different days score higher than
        memories recalled many times on one day.

        Formula (adapted from OpenClaw):
          unique_days == 0: 0.0
          unique_days == 1: 0.2
          else: 0.55 * log1p(days-1)/log1p(4) + 0.45 * span/7
        """
        ud = e.get("unique_days") or 0
        if ud == 0:
            return 0.0
        if ud == 1:
            return 0.2

        spacing = min(1.0, math.log1p(ud - 1) / math.log1p(4))

        first = e.get("first_recalled_at")
        last = e.get("last_recalled_at")
        try:
            first_dt = datetime.fromisoformat(first.replace("Z", "+00:00").split("+")[0])
            last_dt = datetime.fromisoformat(last.replace("Z", "+00:00").split("+")[0])
            span_days = (last_dt - first_dt).days
        except (ValueError, TypeError, AttributeError):
            span_days = 0
        span = min(1.0, span_days / 7.0)

        return 0.55 * spacing + 0.45 * span

    def _conceptual(self, e: dict) -> float:
        """Entity/tag count from content / 6, clamped [0,1]."""
        tags_raw = e.get("concept_tags") or "[]"
        try:
            tags = json.loads(tags_raw) if isinstance(tags_raw, str) else (tags_raw or [])
        except (json.JSONDecodeError, TypeError):
            tags = []
        return min(1.0, len(tags) / 6.0)

    def _phase_boost(self, e: dict) -> float:
        """
        Consolidation encounter boost (0 to 0.08).
        Memories found during SANCHO dream get a multiplicative boost.
        """
        hits = e.get("consolidation_hits") or 0
        if hits == 0:
            return 0.0
        strength = min(1.0, math.log1p(hits) / math.log1p(6))
        recency = self._recency(e)
        return min(self.PHASE_BOOST_MAX, self.PHASE_BOOST_MAX * strength * recency)

    # ═══════════════════════════════════════════════════════════════
    #  RANKING
    # ═══════════════════════════════════════════════════════════════

    def rank_candidates(self) -> List[dict]:
        """
        Rebuild stats, score all entries, filter by gates, return ranked candidates.
        """
        # Rebuild stats from raw recall_log
        from core.memory.recall_tracker import RecallTracker
        tracker = RecallTracker(self.db_path)
        tracker.rebuild_stats()

        entries = self._load_stats()
        candidates = []

        for entry in entries:
            # Skip already promoted
            if entry.get("promoted_at"):
                continue

            # Gate: minimum recall count
            rc = entry.get("recall_count") or 0
            if rc < self.MIN_RECALL_COUNT:
                continue

            # Gate: minimum query diversity
            uq = entry.get("unique_queries") or 0
            if uq < self.MIN_UNIQUE_QUERIES:
                continue

            # Gate: max age
            age = self._age_days(entry)
            if age is not None and age > self.MAX_AGE_DAYS:
                continue

            score, components = self.score(entry)

            # Gate: minimum composite score
            if score < self.MIN_SCORE:
                continue

            candidates.append({
                **entry,
                "promotion_score": score,
                "components": components,
            })

        candidates.sort(key=lambda x: x["promotion_score"], reverse=True)
        return candidates[:self.MAX_PROMOTIONS_PER_RUN]

    # ═══════════════════════════════════════════════════════════════
    #  PROMOTION
    # ═══════════════════════════════════════════════════════════════

    def promote(self, dry_run: bool = False) -> dict:
        """
        Full promotion run.

        Args:
            dry_run: If True, score and rank but don't write anything.

        Returns:
            Report dict with candidates count, promoted count, and entries.
        """
        candidates = self.rank_candidates()

        if dry_run:
            return {
                "candidates": len(candidates),
                "promoted": 0,
                "entries": candidates,
                "dry_run": True,
            }

        promoted = []
        for c in candidates:
            success = self._apply_promotion(c)
            if success:
                promoted.append(c)

        # Log promotion to today's journal
        if promoted:
            self._log_promotions_to_journal(promoted)

        return {
            "candidates": len(candidates),
            "promoted": len(promoted),
            "entries": promoted,
            "dry_run": False,
        }

    def _apply_promotion(self, candidate: dict) -> bool:
        """
        Write promotion to Brain + mark as promoted in recall_stats.

        Strategy:
        - If source is a Brain page → append promotion marker
        - If source is a journal → extract snippet to promoted-memories page
        - Always: mark promoted_at in recall_stats, emit event
        """
        content_hash = candidate.get("content_hash", "")
        doc_path = candidate.get("doc_path", "")
        snippet = candidate.get("snippet", "")
        score = candidate.get("promotion_score", 0.0)
        components = candidate.get("components", {})

        try:
            # Write to promoted-memories Brain page
            promoted_page = Path(PAGES_DIR) / "promoted-memories.md"
            today = datetime.now().strftime("%Y-%m-%d")
            score_str = f"{score:.2f}"

            entry_line = (
                f"- [{today}] (score: {score_str}) "
                f"from `{doc_path}`: {snippet[:200]}\n"
            )

            # Append to page (create if needed)
            if promoted_page.exists():
                existing = promoted_page.read_text(encoding="utf-8")
            else:
                existing = (
                    "title:: promoted-memories\n"
                    "type:: system\n"
                    "description:: Automatically promoted memories from recall tracking\n\n"
                    "- # Promoted Memories\n"
                )

            # Check for duplicate (content_hash already promoted)
            if content_hash in existing:
                log.info("skipping already-promoted content_hash %s", content_hash)
                return False

            # Append with hash marker for dedup
            promoted_page.write_text(
                existing + f"  - <!-- hash:{content_hash} -->\n" + f"  {entry_line}",
                encoding="utf-8",
            )

            # Mark as promoted in recall_stats
            with sqlite3.connect(self.db_path, check_same_thread=False) as conn:
                conn.execute("""
                    UPDATE recall_stats
                    SET promoted_at = datetime('now')
                    WHERE content_hash = ?
                """, (content_hash,))

            log.info(
                "promoted memory: %s (score=%.2f, recalls=%d, days=%d)",
                content_hash, score,
                candidate.get("recall_count", 0),
                candidate.get("unique_days", 0),
            )

            # Emit event via EventBus
            try:
                from core.orchestration.event_bus import EventBus
                EventBus.instance().publish("memory.promoted", {
                    "content_hash": content_hash,
                    "doc_path": doc_path,
                    "score": score,
                    "components": components,
                    "snippet": snippet[:200],
                })
            except Exception:
                pass  # EventBus may not be available

            return True

        except Exception as e:
            log.error("promotion failed for %s: %s", content_hash, e)
            return False

    def _log_promotions_to_journal(self, promoted: List[dict]) -> None:
        """Log promotion summary to today's Brain journal."""
        try:
            today = datetime.now().strftime("%Y_%m_%d")
            journal_path = Path(JOURNALS_DIR) / f"{today}.md"

            lines = [f"- [memory-promotion] Promoted {len(promoted)} memories:"]
            for p in promoted:
                snippet = (p.get("snippet") or "")[:100]
                score = p.get("promotion_score", 0.0)
                recalls = p.get("recall_count", 0)
                days = p.get("unique_days", 0)
                lines.append(
                    f"  - (score: {score:.2f}, recalled {recalls}x across {days} days) "
                    f"{snippet}"
                )

            entry = "\n".join(lines) + "\n"

            if journal_path.exists():
                existing = journal_path.read_text(encoding="utf-8")
                if "memory-promotion" not in existing:
                    journal_path.write_text(existing + entry, encoding="utf-8")
            else:
                journal_path.write_text(entry, encoding="utf-8")

        except Exception as e:
            log.warning("failed to log promotions to journal: %s", e)

    # ═══════════════════════════════════════════════════════════════
    #  HELPERS
    # ═══════════════════════════════════════════════════════════════

    def _load_stats(self) -> List[dict]:
        """Load all recall_stats entries from superbrain.db."""
        try:
            with sqlite3.connect(self.db_path, check_same_thread=False) as conn:
                conn.row_factory = sqlite3.Row
                rows = conn.execute("""
                    SELECT * FROM recall_stats
                    ORDER BY recall_count DESC
                """).fetchall()
            return [dict(r) for r in rows]
        except Exception as e:
            log.error("failed to load recall_stats: %s", e)
            return []

    def _age_days(self, entry: dict) -> Optional[int]:
        """Days since first recall."""
        first = entry.get("first_recalled_at")
        if not first:
            return None
        try:
            first_dt = datetime.fromisoformat(first.replace("Z", "+00:00").split("+")[0])
            return (datetime.now() - first_dt).days
        except (ValueError, TypeError):
            return None
