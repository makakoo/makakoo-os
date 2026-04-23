"""Heuristic memory-type classifier.

The upstream `auto_memory_extractor.extract_memories` hardcodes `type: user` for
every draft. That's a lie for most sessions — a sprint log is a `project`, a
correction is `feedback`, a Linear link is `reference`. This module re-classifies
a draft based on its content and rewrites the frontmatter `type` field.

Design:
- Pure regex/string scoring, no LLM call. Fast, deterministic, free.
- Score each type on signal indicators. Tie → default to `project` for
  substantive sessions, `user` for sparse ones.
- If classification changes, rewrite only the `type:` line; preserve everything
  else in the file exactly.
"""

from __future__ import annotations

import re
from pathlib import Path


USER_PREFERENCE_PATTERNS = [
    r"\b(?:sebastian|he|user) (?:prefers?|likes?|wants?|hates?|dislikes?)\b",
    r"\bmy (?:rate|floor|preference|style)\b",
    r"\b(?:his|her|their) (?:rate|floor|preference|style|availability|location)\b",
    r"\b(?:based in|located in|lives in)\b",
    r"\bI (?:prefer|like|want|hate|dislike)\b",
]

FEEDBACK_PATTERNS = [
    r"##\s*Mistakes?\s*Made",
    r"\b(?:caught|burned|bit me|poisoned|regressed|wrong|broke|fail)\b",
    r"\b(?:should (?:have|not)|don't|never|always)\b",
    r"\b(?:lesson|gotcha|pitfall|caveat)\b",
    r"\bcorrect(?:ed|ion)\b",
]

PROJECT_PATTERNS = [
    r"\bSPRINT-[A-Z0-9-]+\b",
    r"\b(?:phase|milestone|deliverable|roadmap|release|tag|commit)\b",
    r"\bshipped\b",
    r"\[\[[^\]]+\]\]",  # wikilinks to projects/entities
    r"\b(?:v\d+\.\d+(?:\.\d+)?)\b",
    r"^##\s*Action\s*Items",
]

REFERENCE_PATTERNS = [
    r"https?://(?!(?:github\.com/traylinx|jevvellabs\.com))[^\s)]+",
    r"\b(?:Grafana|Linear|Notion|Slack|Jira|Confluence)\b",
    r"\bchannel\b",
    r"\bdashboard\b",
    r"\bruntimes?\b",
]


def _score_patterns(text: str, patterns: list[str]) -> int:
    total = 0
    for p in patterns:
        total += len(re.findall(p, text, re.IGNORECASE | re.MULTILINE))
    return total


def classify(body: str) -> str:
    """Return one of user / feedback / project / reference based on content scoring."""
    scores = {
        "user": _score_patterns(body, USER_PREFERENCE_PATTERNS),
        "feedback": _score_patterns(body, FEEDBACK_PATTERNS),
        "project": _score_patterns(body, PROJECT_PATTERNS),
        "reference": _score_patterns(body, REFERENCE_PATTERNS),
    }

    # Short sessions with no clear signal → user (catch-all, low stakes)
    total = sum(scores.values())
    if total < 3:
        return "user"

    # Pick max; tie-break order: project > feedback > user > reference
    priority = ["project", "feedback", "user", "reference"]
    best = max(priority, key=lambda t: (scores[t], -priority.index(t)))
    return best


def reclassify_draft(path: Path) -> tuple[str, str] | None:
    """Rewrite the `type:` line in a draft file if classification differs.

    Returns (old_type, new_type) if a change was made, None otherwise.
    """
    try:
        content = path.read_text()
    except OSError:
        return None

    fm_match = re.match(r"^(---\n)(.*?)(\n---\n)(.*)$", content, re.DOTALL)
    if not fm_match:
        return None

    fm_text = fm_match.group(2)
    body = fm_match.group(4)

    type_match = re.search(r"^type:\s*(\S+)\s*$", fm_text, re.MULTILINE)
    if not type_match:
        return None

    old_type = type_match.group(1)
    new_type = classify(body)
    if old_type == new_type:
        return None

    new_fm_text = re.sub(
        r"^type:\s*\S+\s*$",
        f"type: {new_type}",
        fm_text,
        count=1,
        flags=re.MULTILINE,
    )
    new_content = f"---\n{new_fm_text}\n---\n{body}"
    path.write_text(new_content)
    return old_type, new_type


def reclassify_all(drafts_dir: Path) -> dict[str, int]:
    """Walk drafts_dir and reclassify every .md file. Returns tally of changes by new type."""
    tally: dict[str, int] = {}
    for p in drafts_dir.glob("*.md"):
        result = reclassify_draft(p)
        if result:
            _, new_type = result
            tally[new_type] = tally.get(new_type, 0) + 1
    return tally


if __name__ == "__main__":
    import sys

    if len(sys.argv) != 2:
        print("Usage: classifier.py <drafts_dir>", file=sys.stderr)
        sys.exit(1)
    drafts = Path(sys.argv[1])
    if not drafts.is_dir():
        print(f"Not a directory: {drafts}", file=sys.stderr)
        sys.exit(1)
    tally = reclassify_all(drafts)
    print(f"Reclassified {sum(tally.values())} drafts: {tally}")
