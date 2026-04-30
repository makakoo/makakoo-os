"""Typed records for native Cortex memory."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, Optional


@dataclass
class MemoryCandidate:
    content: str
    memory_type: str
    confidence: float
    importance_score: float
    metadata: Dict[str, Any] = field(default_factory=dict)


@dataclass
class MemorySource:
    channel: str
    channel_user_id: str
    session_id: Optional[str] = None
    source_message_id: Optional[int] = None


@dataclass
class ScrubResult:
    text: str
    ok: bool = True
    changed: bool = False
    reason: str = ""
