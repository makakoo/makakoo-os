#!/usr/bin/env python3
"""
SOUL Vocabulary Extractor — Seeds buddy mascot trait pools from SOUL.md.

Per the OpenClaw audit lope-team debate (2026-04-11): Olibia's hard-coded
trait template approach was kept untouched, but the procedural buddy mascot
generator in `core/buddy/nursery.py` is enriched with vocabulary pulled from
the canonical Core Tone section in `harvey-os/SOUL.md`.

This is a *vocabulary seeding* pattern, NOT a voice replacement pattern:
  - Existing whimsical pools stay (preserves the joy of "afraid of semicolons")
  - SOUL adjectives APPEND to the pool (Harvey's voice signature can also surface)
  - Procedural seeded generation in `_generate_mascot_from_seed` is unchanged
  - Same seed → same mascot, given the same SOUL.md state

Public API:
    get_enriched_pools(soul_path=None, base_first=None, base_quirk=None)
        → (enriched_first, enriched_quirk) tuple. Falls back gracefully if
        SOUL.md is missing, unreadable, or has no `## Core Tone` section.
    extract_voice_traits(text) → list[str]   (Core Tone adjectives)
    extract_voice_quirks(text) → list[str]   (Core Tone behavioral phrases)
    reset_cache()                            (test helper)
"""

from __future__ import annotations

import logging
import os
import re
from pathlib import Path
from typing import List, Optional, Tuple

log = logging.getLogger("harvey.buddy.soul_vocab")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
DEFAULT_SOUL_PATH = Path(HARVEY_HOME) / "harvey-os" / "SOUL.md"

CORE_TONE_HEADER = "## Core Tone"
NEXT_SECTION_RE = re.compile(r"\n##\s")

# Adjective extraction: matches the canonical "tone is X, Y, Z, [and] W"
# pattern in harvey-os/SOUL.md. Anchored on "tone is" specifically — broader
# patterns like bare "are" leak unrelated noun phrases ("are Harvey", "are the
# boss of all other agents") that aren't adjective lists. The trailing dash/
# period/newline anchors stop the match at the end of the adjective list so
# prose after the dash is dropped.
_TONE_LIST_RE = re.compile(
    r"\btone\s+is\s+([^\n.—\u2014]+)",
    re.IGNORECASE,
)

# Quirk imperative verbs — matches phrases that capture Harvey's behavioral
# voice ("State errors plainly", "Trust internal code", "Skip the preamble").
# Kept narrow on purpose: better to extract zero quirks than to extract noise.
_QUIRK_VERB_RE = re.compile(
    r"\b(skip|state|trust|return|fix|surgical|tight)\b[^.\n!?—\u2014]*",
    re.IGNORECASE,
)

# Length filters keep extraction outputs in the same shape as the existing
# whimsical TRAIT_FIRST / TRAIT_QUIRK pools (3-50 chars per entry).
_MIN_LEN = 3
_MAX_LEN = 50

# Discard extractions containing these characters — they signal the regex
# pulled in code fragments, quotes, or markup we don't want as trait labels.
_DISALLOWED_CHARS = set('"`[]{}<>|*=#')


# ────────────────────────────────────────────────────────────────────
#  Internal helpers
# ────────────────────────────────────────────────────────────────────


def _extract_core_tone(text: str) -> Optional[str]:
    """Pull the body of the `## Core Tone` section, or None if missing."""
    if CORE_TONE_HEADER not in text:
        return None
    header_idx = text.find(CORE_TONE_HEADER)
    body_start = text.find("\n", header_idx) + 1
    if body_start <= 0:
        return None
    next_match = NEXT_SECTION_RE.search(text, body_start)
    body = text[body_start:next_match.start()] if next_match else text[body_start:]
    body = body.strip()
    return body if body else None


def _clean_phrase(phrase: str) -> str:
    """Strip leading 'and ', trailing punctuation, surrounding whitespace."""
    phrase = phrase.strip()
    phrase = re.sub(r"^and\s+", "", phrase, flags=re.IGNORECASE)
    phrase = phrase.rstrip(",.;:")
    return phrase.strip().lower()


def _is_clean(phrase: str) -> bool:
    """Reject phrases that contain code/markup characters or are out of length."""
    if not (_MIN_LEN <= len(phrase) <= _MAX_LEN):
        return False
    if any(c in _DISALLOWED_CHARS for c in phrase):
        return False
    return True


# ────────────────────────────────────────────────────────────────────
#  Public extraction API
# ────────────────────────────────────────────────────────────────────


def extract_voice_traits(core_tone_text: str) -> List[str]:
    """Extract comma-separated adjective phrases from the Core Tone body.

    Targets the canonical "Your tone is sharp, concise, hyper-competent,
    slightly blunt" pattern. Returns the list of cleaned phrases in source
    order, deduped while preserving first-occurrence position.

    ANCHORING CONTRACT: This regex matches "tone is X, Y, Z" specifically.
    Other voice-signature framings ("tone: sharp", "Harvey's voice: sharp",
    "personality is X") will silently fall back to the base pool. That is
    intentional — a cosmetic system should not crash on vocabulary drift,
    and a broader anchor (e.g. bare "are X") leaks unrelated noun phrases
    like "are Harvey" or "are the boss of all other agents" into the pool.
    If SOUL.md is rewritten with a new framing, update _TONE_LIST_RE here.
    """
    if not core_tone_text:
        return []

    seen = set()
    out: List[str] = []
    for match in _TONE_LIST_RE.finditer(core_tone_text):
        clause = match.group(1)
        for piece in clause.split(","):
            cleaned = _clean_phrase(piece)
            if _is_clean(cleaned) and cleaned not in seen:
                seen.add(cleaned)
                out.append(cleaned)
    return out


def extract_voice_quirks(core_tone_text: str) -> List[str]:
    """Extract behavioral imperative phrases from the Core Tone body.

    Pulls phrases that begin with one of Harvey's signature imperative verbs
    (Skip, State, Trust, Return, Fix, Surgical, Tight). Each phrase is
    truncated at the next sentence boundary.
    """
    if not core_tone_text:
        return []

    seen = set()
    out: List[str] = []
    for match in _QUIRK_VERB_RE.finditer(core_tone_text):
        phrase = _clean_phrase(match.group(0))
        # Drop everything after a colon (often introduces a list)
        phrase = phrase.split(":", 1)[0].strip()
        if _is_clean(phrase) and phrase not in seen:
            seen.add(phrase)
            out.append(phrase)
    return out


# ────────────────────────────────────────────────────────────────────
#  Pool enrichment (cached)
# ────────────────────────────────────────────────────────────────────


_cache: dict = {"first": None, "quirk": None, "soul_path": None}


def reset_cache() -> None:
    """Test helper — clears the enrichment cache so the next call re-reads."""
    _cache["first"] = None
    _cache["quirk"] = None
    _cache["soul_path"] = None


def get_enriched_pools(
    soul_path: Optional[Path] = None,
    base_first: Optional[List[str]] = None,
    base_quirk: Optional[List[str]] = None,
) -> Tuple[List[str], List[str]]:
    """Return (first_traits, quirks) pools enriched with SOUL.md vocabulary.

    Preserves the base whimsical pools — SOUL-derived phrases are APPENDED,
    so the procedural generator can still produce "wildly enthusiastic,
    afraid of semicolons" buddies AND occasionally surface a "sharp, trust
    internal code" buddy that mirrors Harvey's voice.

    Cached: first call reads SOUL.md once; subsequent calls return the cached
    result. `reset_cache()` clears it for tests.

    Fallback: if SOUL.md is missing, unreadable, or has no `## Core Tone`
    section, returns the base pools unchanged.
    """
    soul_path = Path(soul_path) if soul_path else DEFAULT_SOUL_PATH
    base_first = list(base_first or [])
    base_quirk = list(base_quirk or [])

    if (
        _cache["first"] is not None
        and _cache["quirk"] is not None
        and _cache["soul_path"] == str(soul_path)
    ):
        return list(_cache["first"]), list(_cache["quirk"])

    if not soul_path.exists():
        _cache["first"] = base_first
        _cache["quirk"] = base_quirk
        _cache["soul_path"] = str(soul_path)
        return list(base_first), list(base_quirk)

    try:
        text = soul_path.read_text(encoding="utf-8")
    except OSError as e:
        log.debug("soul_vocabulary: failed to read %s (%s)", soul_path, e)
        _cache["first"] = base_first
        _cache["quirk"] = base_quirk
        _cache["soul_path"] = str(soul_path)
        return list(base_first), list(base_quirk)

    core_tone = _extract_core_tone(text)
    if not core_tone:
        _cache["first"] = base_first
        _cache["quirk"] = base_quirk
        _cache["soul_path"] = str(soul_path)
        return list(base_first), list(base_quirk)

    soul_first = extract_voice_traits(core_tone)
    soul_quirk = extract_voice_quirks(core_tone)

    enriched_first = base_first + [t for t in soul_first if t not in base_first]
    enriched_quirk = base_quirk + [q for q in soul_quirk if q not in base_quirk]

    _cache["first"] = enriched_first
    _cache["quirk"] = enriched_quirk
    _cache["soul_path"] = str(soul_path)
    return list(enriched_first), list(enriched_quirk)
