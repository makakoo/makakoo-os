"""Conservative memory extraction from chat turns."""

from __future__ import annotations

import re
from typing import List
from .models import MemoryCandidate

_REMEMBER_RE = re.compile(r"(?is)^\s*(remember|log that|note that)\s*[:,-]?\s*(.+)$")
_PREFER_RE = re.compile(r"(?i)\b(i|sebastian)\s+(prefer|prefers|like|likes|love|loves)\b")
_DECISION_RE = re.compile(r"(?i)\b(chose|choose|decided|decision|go with|picked|use native|use sqlite)\b")
_IDENTITY_RE = re.compile(r"(?i)^\s*my\s+(name|timezone|location|preferred\s+language)\s+is\s+(.+)$")
_TRANSIENT_RE = re.compile(r"(?i)^(thanks|thank you|ok|okay|yes|no|sure|go|done|thinking)\W*$")


def _clean(text: str) -> str:
    text = re.sub(r"\s+", " ", text or "").strip()
    return text.strip('"')


def extract_memory_candidates(user_text: str, assistant_text: str = "") -> List[MemoryCandidate]:
    """Return conservative memory candidates.

    Never extracts from assistant text alone. The assistant text is accepted only for
    future context-aware improvements; MVP rules key off user text.
    """
    text = _clean(user_text)
    if not text or _TRANSIENT_RE.match(text):
        return []

    candidates: List[MemoryCandidate] = []
    explicit = _REMEMBER_RE.match(text)
    content = _clean(explicit.group(2) if explicit else text)
    if not content:
        return []

    if explicit:
        memory_type = "preference" if _PREFER_RE.search(content) else "decision" if _DECISION_RE.search(content) else "fact"
        importance = 0.78 if memory_type in {"preference", "decision"} else 0.65
        candidates.append(MemoryCandidate(content=content, memory_type=memory_type, confidence=0.92, importance_score=importance))
        return candidates

    identity = _IDENTITY_RE.match(text)
    if identity:
        candidates.append(MemoryCandidate(content=text, memory_type="identity", confidence=0.86, importance_score=0.80))
        return candidates

    if _PREFER_RE.search(text):
        candidates.append(MemoryCandidate(content=text, memory_type="preference", confidence=0.82, importance_score=0.72))
    elif _DECISION_RE.search(text):
        candidates.append(MemoryCandidate(content=text, memory_type="decision", confidence=0.80, importance_score=0.74))

    return candidates
