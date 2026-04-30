"""PII and secret scrubbing for long-term memory candidates."""

from __future__ import annotations

import re
from .models import ScrubResult

_SECRET_PATTERNS = [
    re.compile(r"AKIA[0-9A-Z]{16}"),
    re.compile(r"gh[pousr]_[A-Za-z0-9_]{20,}"),
    re.compile(r"\bsk-[A-Za-z0-9_-]{10,}\b"),
    re.compile(r"(?i)\b(api[_-]?key|token|password|secret)\s*=\s*[^\s,;]+"),
    re.compile(r"\b\d{3}-\d{2}-\d{4}\b"),
    re.compile(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b"),
]


def fallback_scrub(text: str) -> ScrubResult:
    out = text
    for pattern in _SECRET_PATTERNS:
        out = pattern.sub("[REDACTED]", out)
    return ScrubResult(text=out, ok=True, changed=(out != text), reason="fallback")


def scrub_memory_text(text: str, pii_enabled: bool = True) -> ScrubResult:
    """Scrub text before long-term memory write.

    Presidio is optional and lazy. Fallback regex always runs for obvious secrets.
    If Presidio errors while enabled, fallback still protects common secrets.
    """
    if not pii_enabled:
        # Even when PII is disabled, obvious credential-shaped values should not enter LTM.
        return fallback_scrub(text)

    try:
        from presidio_analyzer import AnalyzerEngine  # type: ignore
        from presidio_anonymizer import AnonymizerEngine  # type: ignore

        analyzer = AnalyzerEngine()
        anonymizer = AnonymizerEngine()
        findings = analyzer.analyze(text=text, language="en")
        anonymized = anonymizer.anonymize(text=text, analyzer_results=findings).text if findings else text
        fallback = fallback_scrub(anonymized)
        fallback.reason = "presidio+fallback"
        fallback.changed = fallback.changed or (anonymized != text)
        return fallback
    except Exception:
        # Fail protected, not open: fallback still catches secrets; caller may log if desired.
        result = fallback_scrub(text)
        result.reason = "fallback-presidio-unavailable"
        return result
