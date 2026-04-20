"""
intelligent_router.py — Phase 3 deliverable

IntelligentRouter: classifies a free-text user request into one of the
known intents (research / image / archive / minimal / unknown), computes a
confidence score from keyword hits, and returns the corresponding
TeamRoster ready for `build_workflow_from_team()`.

This is Harvey's "dispatcher on the front door": a request arrives via
HarveyChat, the router decides which team should handle it, and the
coordinator spins up that team. The classifier is intentionally simple
(keyword-based heuristics) — Phase 4 can swap in an LLM classifier
without touching callers.

Exposed:
  - IntentClassification (dataclass)
  - IntelligentRouter (classify / route methods)
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Dict, List, Optional

from core.orchestration.agent_team import TeamComposition, TeamRoster


# ─── Classification data ────────────────────────────────────────────


@dataclass
class IntentClassification:
    """Result of classifying a request. Always returned, even for unknowns."""

    intent: str                           # "research" | "image" | "archive" | "minimal" | "unknown"
    confidence: float                     # [0.0, 1.0]
    keywords_hit: List[str] = field(default_factory=list)
    rationale: str = ""

    def is_confident(self, threshold: float = 0.3) -> bool:
        return self.confidence >= threshold


# ─── Router ─────────────────────────────────────────────────────────


class IntelligentRouter:
    """
    Keyword-weighted heuristic classifier. Deliberately simple and
    deterministic so tests are stable; the interface is what matters.

    Phase 4 will either:
      (a) replace `classify()` with an LLM call, or
      (b) keep this as a cheap pre-filter and only invoke the LLM when
          confidence < threshold.
    """

    # Each bucket has a weight (importance) and a keyword list
    INTENT_KEYWORDS: Dict[str, List[str]] = {
        "research": [
            "research", "find", "search", "investigate", "literature",
            "compare", "sources", "papers", "study", "lookup", "explore",
            "what is", "who is", "how does", "analyze", "summarize sources",
        ],
        "image": [
            "image", "picture", "photo", "draw", "illustration", "render",
            "generate image", "create image", "logo", "icon", "artwork",
            "visualize", "painting", "sketch",
        ],
        "archive": [
            "save", "archive", "remember", "store", "persist", "log this",
            "record", "bookmark", "keep", "write to brain",
        ],
        "minimal": [
            "quick", "briefly", "tl;dr", "one-liner", "short answer",
        ],
    }

    # When multiple intents tie, this order breaks the tie (more specific first)
    PRIORITY_ORDER: List[str] = ["image", "research", "archive", "minimal"]

    def __init__(
        self,
        default_parallelism: int = 2,
        research_scale_hint_words: Optional[List[str]] = None,
    ):
        """
        Args:
          default_parallelism: how many researchers to spawn on a plain
            research request. Scaled up by `research_scale_hint_words` hits.
          research_scale_hint_words: request words that bump parallelism
            (e.g. "thorough", "deep", "comprehensive" → +1 researcher each).
        """
        self.default_parallelism = max(1, int(default_parallelism))
        self.research_scale_hint_words = research_scale_hint_words or [
            "thorough", "deep", "comprehensive", "exhaustive",
            "extensive", "full", "in-depth", "complete",
        ]

    # ── Core API ──

    def classify(self, request: str) -> IntentClassification:
        """
        Keyword-match the request against each intent bucket. The intent
        with the most hits wins; confidence is `hits / max(3, total_words)`
        clamped to [0, 1]. Ties are broken by PRIORITY_ORDER.
        """
        if not request or not request.strip():
            return IntentClassification(
                intent="unknown",
                confidence=0.0,
                rationale="empty request",
            )

        normalized = request.lower()
        total_words = max(3, len(re.findall(r"\w+", normalized)))

        hits_by_intent: Dict[str, List[str]] = {k: [] for k in self.INTENT_KEYWORDS}
        for intent, keywords in self.INTENT_KEYWORDS.items():
            for kw in keywords:
                if kw in normalized:
                    hits_by_intent[intent].append(kw)

        max_hits = max(len(v) for v in hits_by_intent.values())
        if max_hits == 0:
            return IntentClassification(
                intent="unknown",
                confidence=0.0,
                rationale="no keyword matches",
            )

        # Tie-break using PRIORITY_ORDER
        winner = None
        for intent in self.PRIORITY_ORDER:
            if len(hits_by_intent[intent]) == max_hits:
                winner = intent
                break

        assert winner is not None  # max_hits > 0 guarantees one

        confidence = min(1.0, max_hits / total_words * 3.0)  # scale up: 1 hit in 3 words = 1.0
        return IntentClassification(
            intent=winner,
            confidence=round(confidence, 3),
            keywords_hit=hits_by_intent[winner],
            rationale=(
                f"{max_hits} keyword hit(s) for '{winner}' "
                f"(tied: {[k for k,v in hits_by_intent.items() if len(v)==max_hits]})"
            ),
        )

    def route(self, request: str) -> TeamRoster:
        """
        Classify the request and return the matching TeamRoster.

        For research intent, scale parallelism based on scale-hint words.
        """
        cls = self.classify(request)
        parallelism = self._scale_parallelism(request, cls)
        return TeamComposition.for_intent(cls.intent, parallelism=parallelism)

    def classify_and_route(self, request: str) -> tuple[IntentClassification, TeamRoster]:
        """Return both the classification AND the team, in one call."""
        cls = self.classify(request)
        parallelism = self._scale_parallelism(request, cls)
        team = TeamComposition.for_intent(cls.intent, parallelism=parallelism)
        return cls, team

    # ── Helpers ──

    def _scale_parallelism(
        self, request: str, cls: IntentClassification
    ) -> int:
        """
        Bump parallelism for research requests that contain scale-hint
        words. Returns default_parallelism for non-research intents (it's
        ignored there anyway).
        """
        if cls.intent != "research":
            return self.default_parallelism

        normalized = request.lower()
        bumps = sum(1 for w in self.research_scale_hint_words if w in normalized)
        return min(self.default_parallelism + bumps, 8)  # hard ceiling


__all__ = ["IntentClassification", "IntelligentRouter"]
