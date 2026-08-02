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

import os
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

    def classify_and_route(
        self,
        request: str,
        *,
        mode: Optional[str] = None,
    ) -> tuple[IntentClassification, TeamRoster]:
        """Return both the classification AND the team, in one call.

        `mode` picks the classifier:
          - "keyword" (default): the deterministic keyword table above.
          - "llm": one switchAILocal chat completion. Falls back to keyword
            if the LLM call raises (network, timeout, unparseable reply).

        `mode=None` honors the `router.llm_mode=on` env toggle
        (set via `MAKAKOO_ROUTER_LLM_MODE=1`). Default: keyword.
        """
        if mode is None:
            env = os.environ.get("MAKAKOO_ROUTER_LLM_MODE", "").lower()
            mode = "llm" if env in ("1", "on", "true", "yes") else "keyword"

        if mode == "llm":
            cls = self._classify_llm(request) or self.classify(request)
        else:
            cls = self.classify(request)
        parallelism = self._scale_parallelism(request, cls)
        team = TeamComposition.for_intent(cls.intent, parallelism=parallelism)
        return cls, team

    # ── LLM classifier (D.5, flag-gated) ──

    _LLM_PROMPT = (
        "Classify the user request into exactly one of: "
        "research | image | archive | minimal | unknown. "
        "Return JSON {\"intent\": str, \"confidence\": float 0..1, "
        "\"rationale\": str}. No prose outside the JSON.\n\nRequest: "
    )

    # Default LLM budget. The original 0.3s was below the round-trip time of
    # any real model, so the LLM path could never return and every request
    # silently fell back to keywords — the classifier was dead code.
    DEFAULT_LLM_TIMEOUT = 10.0

    @staticmethod
    def _compiled_prompt_enabled() -> bool:
        """The DSPy-compiled prompt is the default since its held-out win
        (decision 97b4c1d5…, 2026-08-02: 80% vs 36% keyword, CI excludes
        zero). Set ``MAKAKOO_ROUTER_COMPILED_PROMPT=off`` to roll back to the
        legacy hand prompt without a deploy."""
        raw = os.environ.get("MAKAKOO_ROUTER_COMPILED_PROMPT", "").strip().lower()
        return raw not in ("off", "0", "false", "no")

    def _llm_request_body(self, request: str, model: str) -> dict:
        """Chat-completion body for the LLM classifier — pure, testable."""
        if self._compiled_prompt_enabled():
            try:
                from . import router_compiled_prompt as _rcp
            except ImportError:
                _rcp = None
            if _rcp is not None:
                return {
                    "model": model,
                    "messages": _rcp.render(request),
                    "temperature": _rcp.TEMPERATURE,
                    "max_tokens": _rcp.MAX_TOKENS,
                }
        return {
            "model": model,
            "messages": [
                {"role": "system", "content": "You are a strict JSON classifier."},
                {"role": "user", "content": self._LLM_PROMPT + request},
            ],
            "temperature": 0.0,
            "max_tokens": 200,
        }

    def _classify_llm(self, request: str) -> Optional[IntentClassification]:
        """One switchAILocal call. Returns None on any error (caller falls back)."""
        try:
            import json as _json
            import os as _os
            import urllib.request as _urllib_request
            import urllib.error as _urllib_error
        except ImportError:
            return None

        base = _os.environ.get("LLM_BASE_URL", "http://localhost:18080/v1")
        model = self._llm_model()
        key = _os.environ.get("LLM_API_KEY") or _os.environ.get("SWITCHAI_KEY", "")
        timeout = self._llm_timeout()

        body = self._llm_request_body(request, model)
        try:
            req = _urllib_request.Request(
                f"{base.rstrip('/')}/chat/completions",
                data=_json.dumps(body).encode("utf-8"),
                headers={
                    "Content-Type": "application/json",
                    **({"Authorization": f"Bearer {key}"} if key else {}),
                },
                method="POST",
            )
            with _urllib_request.urlopen(req, timeout=timeout) as resp:
                payload = _json.loads(resp.read().decode("utf-8", "replace"))
        except (_urllib_error.URLError, TimeoutError, OSError, ValueError):
            return None
        except Exception:
            return None

        try:
            content = payload["choices"][0]["message"]["content"]
            parsed = _json.loads(content)
            intent = str(parsed.get("intent", "unknown")).lower()
            if intent not in {"research", "image", "archive", "minimal", "unknown"}:
                intent = "unknown"
            return IntentClassification(
                intent=intent,
                # The compiled prompt answers with intent only. Defaulting a
                # missing confidence to 0.0 would flunk every is_confident()
                # gate downstream; 0.8 is the candidate's measured held-out
                # accuracy (decision 97b4c1d5…), an uncalibrated but honest
                # stand-in.
                confidence=float(parsed.get("confidence", 0.8)),
                rationale=str(parsed.get("rationale", "llm classifier"))[:200],
                keywords_hit=[],
            )
        except (KeyError, ValueError, TypeError):
            return None

    @staticmethod
    def _llm_model() -> str:
        """Which model the classifier calls — entirely the operator's choice.

        Resolution order, most specific first:

          1. ``MAKAKOO_ROUTER_LLM_MODEL`` — pin the router to one model
             without disturbing anything else.
          2. ``LLM_MODEL`` — the deployment-wide default.
          3. ``"auto"`` — let the endpoint decide.

        Nothing here is provider-specific. The endpoint comes from
        ``LLM_BASE_URL``, so any OpenAI-compatible backend works: a hosted
        API, a local Ollama, a self-hosted vLLM, or a gateway.

        Note for anyone running evaluations: some gateways expose *pool*
        aliases that load-balance across different backing models, so two
        identical requests can be answered by different models. That is fine
        for everyday routing and fatal for measurement. The eval harness
        therefore records the resolved model reported by each response and
        refuses to promote a run whose identity was not stable — it measures
        determinism rather than mandating a particular model here.
        """
        for var in ("MAKAKOO_ROUTER_LLM_MODEL", "LLM_MODEL"):
            value = (os.environ.get(var) or "").strip()
            if value:
                return value
        return "auto"

    @classmethod
    def _llm_timeout(cls) -> float:
        """LLM budget in seconds, from ``MAKAKOO_ROUTER_LLM_TIMEOUT``.

        Falls back to the default on anything unparseable or non-positive —
        a typo in an env var must not silently disable the classifier again.
        """
        raw = os.environ.get("MAKAKOO_ROUTER_LLM_TIMEOUT")
        if raw is None:
            return cls.DEFAULT_LLM_TIMEOUT
        try:
            value = float(raw)
        except (TypeError, ValueError):
            return cls.DEFAULT_LLM_TIMEOUT
        return value if value > 0 else cls.DEFAULT_LLM_TIMEOUT

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
