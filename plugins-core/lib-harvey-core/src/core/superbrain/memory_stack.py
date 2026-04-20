#!/usr/bin/env python3
"""
Memory Stack — Token-budgeted context injection for LLM queries.

4 layers, strict token budget (~800 tokens for L0+L1):

  L0 Identity   (~40 tokens):  Who Harvey is — one sentence from SOUL.md
  L1 Essential  (~400 tokens): Today's journal + top entities, compressed
  L2 On-demand  (~350 tokens): Vector/FTS5 search for current query
  L3 Deep       (unlimited):   Full search, only fires when explicitly requested

Key insight: L0+L1 is always injected (~440 tokens). L2 fires per-query.
L3 only fires on explicit "search everything" requests.

Usage:
    from core.superbrain.memory_stack import MemoryStack
    ms = MemoryStack()
    context = ms.compact()           # L0+L1 as string (~440 tokens)
    context = ms.for_query("BTC")    # L0+L1+L2 (~800 tokens)
"""

import json
import logging
import os
import re
from datetime import date, datetime
from pathlib import Path
from typing import List, Optional

log = logging.getLogger("superbrain.memory_stack")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
BRAIN_DIR = os.path.join(HARVEY_HOME, "data", "Brain")
SOUL_PATH = os.path.join(HARVEY_HOME, "harvey-os", "SOUL.md")

# Token budget (1 token ≈ 4 chars for English text)
L0_LEAN_CHAR_BUDGET = 160     # ~40 tokens — used when intent is "code"
L0_FULL_CHAR_BUDGET = 1000    # ~250 tokens — used when intent is "creative"
L0_CHAR_BUDGET = L0_LEAN_CHAR_BUDGET  # legacy alias for callers that don't pass intent
L1_CHAR_BUDGET = 1600   # ~400 tokens
L2_CHAR_BUDGET = 1400   # ~350 tokens

# Section header that marks the Core Tone block in SOUL.md
SOUL_CORE_TONE_HEADER = "## Core Tone"
SOUL_NEXT_SECTION_RE = "\n## "

# ── S2: Heuristic intent detection (~20 LOC, ~80% accuracy) ─────
# Goal: tell apart "code" turns (lean L0 OK, model is grounded by code)
# from "creative" turns (full Core Tone needed to prevent generic-AI drift).
# This is keyword/pattern matching, NOT a classifier model. Opencode-debate
# 2026-04-11 verdict: 20-line heuristic at ~80% accuracy beats deferring
# until "drift is measured".

CODE_KEYWORDS = (
    "refactor", "debug", "test", "build", "compile", "stacktrace",
    "import", "function", "class ", "def ", "lint", "typecheck",
    "merge conflict", "rebase", "commit", "pytest", "npm ", "cargo ",
)
CODE_FILE_EXTENSIONS = (
    ".py", ".ts", ".tsx", ".js", ".jsx", ".go", ".rs", ".java",
    ".c", ".cpp", ".h", ".rb", ".php", ".swift", ".kt", ".sh",
    ".sql", ".yaml", ".yml", ".toml", ".json",
)
CODE_FENCE = "```"


def detect_intent(messages: Optional[List[str]] = None,
                  files: Optional[List[str]] = None) -> str:
    """Heuristic intent classification: 'code' or 'creative'.

    Returns 'code' if any of:
      - any file in `files` has a code extension
      - any message contains a code fence (```)
      - any message contains a file path with a code extension
      - the last 3 messages contain a code keyword

    Otherwise returns 'creative' (the safer default for persona-sensitive
    work — better to spend 200 tokens on tone than to drift into generic
    sludge during a draft).

    20-line implementation per opencode-debate 2026-04-11. Trades the
    last 20% of accuracy for zero-cost evaluation.
    """
    files = files or []
    messages = messages or []

    for f in files:
        if any(f.lower().endswith(ext) for ext in CODE_FILE_EXTENSIONS):
            return "code"

    last_three = messages[-3:] if len(messages) >= 3 else messages
    blob = " ".join(last_three).lower()
    if CODE_FENCE in blob:
        return "code"
    for ext in CODE_FILE_EXTENSIONS:
        if ext in blob:
            return "code"
    if any(kw in blob for kw in CODE_KEYWORDS):
        return "code"

    return "creative"


def _estimate_tokens(text: str) -> int:
    """Rough token estimate: 1 token ≈ 4 chars."""
    return len(text) // 4


def _truncate_to_budget(text: str, char_budget: int) -> str:
    """Truncate text to char budget at sentence boundary."""
    if len(text) <= char_budget:
        return text
    # Find last sentence end within budget
    truncated = text[:char_budget]
    for sep in [". ", ".\n", "\n- ", "\n"]:
        idx = truncated.rfind(sep)
        if idx > char_budget * 0.6:
            return truncated[:idx + 1].rstrip()
    return truncated.rstrip()


class MemoryStack:
    """
    Token-budgeted memory context manager.

    Builds context layers from Brain (via SQLite FTS5 store or filesystem fallback).
    Total L0+L1 budget: ~440 tokens. L0+L1+L2: ~790 tokens.
    """

    def __init__(self, brain_dir: str = None):
        self.brain_dir = brain_dir or BRAIN_DIR
        self._store = None
        self._l0_cache: Optional[str] = None        # legacy lean cache (40 tokens)
        self._l0_full_cache: Optional[str] = None   # full Core Tone cache (~250 tokens)
        self._l1_cache: Optional[str] = None
        self._l1_cache_date: Optional[str] = None

    def _get_store(self):
        """Lazy-load store to avoid import cost when not needed."""
        if self._store is None:
            try:
                from core.superbrain.store import SuperbrainStore
                self._store = SuperbrainStore()
            except Exception:
                self._store = False  # Mark as unavailable
        return self._store if self._store is not False else None

    # ── L0: Identity (lean ~40 tokens / full ~250 tokens) ─────────

    def _build_l0(self, intent: str = "code") -> str:
        """Core identity. Two modes:

          - intent="code"     → lean L0 (~40 tokens, one sentence). Used
                                during deep technical work where every
                                token counts and the model is grounded
                                by the code itself.
          - intent="creative" → full Core Tone block (~250 tokens). Used
                                for conversational/drafting/persona-
                                sensitive turns where the model would
                                otherwise revert to "generic AI sludge".

        The full block is the `## Core Tone` section of SOUL.md. If that
        section is missing (older SOUL.md), falls back to the legacy
        first-sentence extraction so existing deployments don't break.
        """
        if intent == "creative":
            return self._build_l0_full()
        return self._build_l0_lean()

    def _build_l0_lean(self) -> str:
        """Legacy ~40-token L0: first sentence of SOUL.md."""
        if self._l0_cache:
            return self._l0_cache

        if os.path.exists(SOUL_PATH):
            try:
                text = Path(SOUL_PATH).read_text(encoding="utf-8")
                # Extract first meaningful line after the heading
                for line in text.split("\n"):
                    line = line.strip()
                    if line.startswith("*") and "Harvey" in line:
                        self._l0_cache = line.strip("*").strip()
                        return _truncate_to_budget(self._l0_cache, L0_LEAN_CHAR_BUDGET)
                    if line.startswith("**The Prime Directive"):
                        self._l0_cache = re.sub(r'\*\*([^*]+)\*\*', r'\1', line)
                        return _truncate_to_budget(self._l0_cache, L0_LEAN_CHAR_BUDGET)
            except Exception:
                pass

        self._l0_cache = "Harvey is Sebastian Schkudlara's autonomous cognitive extension."
        return self._l0_cache

    def _build_l0_full(self) -> str:
        """Full ~250-token Core Tone block from SOUL.md.

        Falls back to lean L0 if SOUL.md doesn't have a `## Core Tone`
        section yet — keeps backward compat with older deployments.
        """
        if self._l0_full_cache:
            return self._l0_full_cache

        if os.path.exists(SOUL_PATH):
            try:
                text = Path(SOUL_PATH).read_text(encoding="utf-8")
                core_tone = self._extract_core_tone_section(text)
                if core_tone:
                    self._l0_full_cache = _truncate_to_budget(
                        core_tone, L0_FULL_CHAR_BUDGET
                    )
                    return self._l0_full_cache
            except Exception:
                pass

        # Fallback: lean L0 (single sentence)
        return self._build_l0_lean()

    @staticmethod
    def _extract_core_tone_section(text: str) -> Optional[str]:
        """Extract the `## Core Tone` section body from SOUL.md.

        Returns the section content (excluding the header line) up to
        the next `## ` header, or None if the section is missing.
        """
        if SOUL_CORE_TONE_HEADER not in text:
            return None
        # Find header position
        header_idx = text.find(SOUL_CORE_TONE_HEADER)
        body_start = text.find("\n", header_idx) + 1
        if body_start <= 0:
            return None
        # Find next ## header (or end of file)
        next_section = text.find(SOUL_NEXT_SECTION_RE, body_start)
        if next_section == -1:
            body = text[body_start:]
        else:
            body = text[body_start:next_section]
        body = body.strip()
        return body if body else None

    # ── L1: Essential context (~400 tokens) ────────────────────────

    def _build_l1(self) -> str:
        """Today's journal + top entities, compressed to budget."""
        today_str = date.today().strftime("%Y_%m_%d")

        # Return cached if same day
        if self._l1_cache and self._l1_cache_date == today_str:
            return self._l1_cache

        pieces = []
        chars_used = 0

        # 1. Today's journal (most important, gets 60% of budget)
        journal_budget = int(L1_CHAR_BUDGET * 0.6)
        today_path = Path(self.brain_dir) / "journals" / f"{today_str}.md"
        if today_path.exists():
            try:
                content = today_path.read_text(encoding="utf-8", errors="replace").strip()
                if len(content) > 30:
                    # Take most recent entries (bottom of file = newest)
                    compressed = self._compress_journal(content, journal_budget)
                    if compressed:
                        pieces.append(f"Today: {compressed}")
                        chars_used += len(pieces[-1])
            except Exception:
                pass

        # 2. Yesterday's journal (if today is thin)
        if chars_used < journal_budget * 0.3:
            from datetime import timedelta
            yesterday = (date.today() - timedelta(days=1)).strftime("%Y_%m_%d")
            yest_path = Path(self.brain_dir) / "journals" / f"{yesterday}.md"
            if yest_path.exists():
                try:
                    content = yest_path.read_text(encoding="utf-8", errors="replace").strip()
                    if len(content) > 30:
                        remaining = journal_budget - chars_used
                        compressed = self._compress_journal(content, remaining)
                        if compressed:
                            pieces.append(f"Yesterday: {compressed}")
                            chars_used += len(pieces[-1])
                except Exception:
                    pass

        # 3. Top entities from graph (remaining budget)
        entity_budget = L1_CHAR_BUDGET - chars_used
        if entity_budget > 100:
            store = self._get_store()
            if store:
                try:
                    gods = store.god_nodes(top_n=8)
                    if gods:
                        entity_str = ", ".join(g["name"] for g in gods)
                        entity_line = _truncate_to_budget(
                            f"Key entities: {entity_str}", entity_budget
                        )
                        pieces.append(entity_line)
                except Exception:
                    pass

        self._l1_cache = "\n".join(pieces) if pieces else ""
        self._l1_cache_date = today_str
        return self._l1_cache

    def _compress_journal(self, content: str, char_budget: int) -> str:
        """
        Compress journal entries to budget.
        Strategy: keep most recent entries, strip formatting noise.
        """
        lines = content.strip().split("\n")

        # Filter out empty lines and pure formatting
        meaningful = []
        for line in lines:
            stripped = line.strip()
            if not stripped or stripped == "-" or stripped.startswith("collapsed::"):
                continue
            # Strip leading "- " for compactness
            if stripped.startswith("- "):
                stripped = stripped[2:]
            # Strip indented sub-bullets to just first level
            if line.startswith("    ") or line.startswith("\t\t"):
                continue
            meaningful.append(stripped)

        if not meaningful:
            return ""

        # Take from the end (most recent) and build up to budget
        result = []
        chars = 0
        for entry in reversed(meaningful):
            if chars + len(entry) + 2 > char_budget:
                break
            result.insert(0, entry)
            chars += len(entry) + 2

        return " | ".join(result)

    # ── L2: On-demand query context (~350 tokens) ─────────────────

    def _build_l2(self, query: str) -> str:
        """Search-based context for a specific query."""
        if not query:
            return ""

        store = self._get_store()
        if not store:
            return self._build_l2_fallback(query)

        results = store.search(query, top_k=5)
        if not results:
            return ""

        parts = []
        chars = 0
        for r in results:
            # Extract relevant snippet
            snippet = self._extract_snippet(r["content"], query, max_len=300)
            entry = f"[{r['name']}] {snippet}"
            if chars + len(entry) > L2_CHAR_BUDGET:
                break
            parts.append(entry)
            chars += len(entry)

        return "\n".join(parts)

    def _build_l2_fallback(self, query: str) -> str:
        """Filesystem grep fallback when store unavailable."""
        import subprocess
        try:
            result = subprocess.run(
                ["grep", "-ril", query.split()[0], self.brain_dir],
                capture_output=True, text=True, timeout=5
            )
            if result.stdout:
                files = result.stdout.strip().split("\n")[:3]
                parts = []
                for f in files:
                    try:
                        content = Path(f).read_text(encoding="utf-8", errors="replace")
                        snippet = self._extract_snippet(content, query, max_len=300)
                        parts.append(f"[{Path(f).stem}] {snippet}")
                    except Exception:
                        continue
                return "\n".join(parts)
        except Exception:
            pass
        return ""

    def _extract_snippet(self, content: str, query: str, max_len: int = 300) -> str:
        """Extract most relevant snippet from content."""
        content_lower = content.lower()
        query_words = query.lower().split()

        # Find best position
        best_pos = 0
        best_score = 0
        window = min(max_len, len(content))

        for start in range(0, max(1, len(content) - window + 1), 100):
            chunk = content_lower[start:start + window]
            score = sum(chunk.count(w) for w in query_words)
            if score > best_score:
                best_score = score
                best_pos = start

        snippet = content[best_pos:best_pos + max_len].strip()
        # Clean: don't start mid-line
        if best_pos > 0:
            nl = snippet.find("\n")
            if 0 < nl < 80:
                snippet = snippet[nl + 1:]

        return snippet.replace("\n", " ").strip()

    # ═══════════════════════════════════════════════════════════════
    #  Public API
    # ═══════════════════════════════════════════════════════════════

    def compact(self, intent: str = "code") -> str:
        """
        L0+L1 context string. Always-on, cheap injection.

        Args:
            intent: "code" for lean L0 (~440 total tokens) or "creative"
                    for full Core Tone L0 (~650 total tokens). Defaults
                    to "code" for backward compatibility.

        Use this as system prompt addon.
        """
        l0 = self._build_l0(intent=intent)
        l1 = self._build_l1()
        parts = [l0]
        if l1:
            parts.append(l1)
        return "\n".join(parts)

    def for_query(self, query: str, intent: str = "code") -> str:
        """
        L0+L1+L2 context for a specific query.
        ~790 tokens (lean) or ~1000 tokens (creative). Use when
        synthesizing answers.
        """
        compact = self.compact(intent=intent)
        l2 = self._build_l2(query)
        if l2:
            return f"{compact}\nRelevant:\n{l2}"
        return compact

    def system_prompt_addon(self, intent: str = "code") -> str:
        """Alias for compact(). Backward compatible."""
        return self.compact(intent=intent)

    def token_usage(self) -> dict:
        """Report current token usage per layer."""
        l0 = self._build_l0()
        l1 = self._build_l1()
        return {
            "l0_tokens": _estimate_tokens(l0),
            "l1_tokens": _estimate_tokens(l1),
            "l0_l1_total": _estimate_tokens(l0) + _estimate_tokens(l1),
            "l0_chars": len(l0),
            "l1_chars": len(l1),
        }


# ─────────────────────────────────────────────────────────────────────────────
#  CLI
# ─────────────────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    import sys
    logging.basicConfig(level=logging.INFO)

    ms = MemoryStack()

    if len(sys.argv) > 1 and sys.argv[1] == "query":
        query = " ".join(sys.argv[2:])
        print(f"\n=== Memory Stack for query: {query} ===\n")
        print(ms.for_query(query))
    else:
        print("\n=== Memory Stack (compact L0+L1) ===\n")
        print(ms.compact())

    print(f"\n--- Token usage: {ms.token_usage()} ---")
