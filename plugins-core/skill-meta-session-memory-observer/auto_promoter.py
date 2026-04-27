"""Auto-promote low-risk draft memories into live memory without manual review.

Not every memory needs a human gate. User preferences, feedback rules, and
external references are well-scoped, high-signal, and safe to index straight
into MEMORY.md. Project-type drafts stay drafts because they summarize sprint
work and can be noisy / duplicative — those benefit from human curation.

Policy (conservative by default):
- `user` type   — auto-promote   (preferences rarely harm if slightly off)
- `feedback`    — auto-promote   (corrections should apply immediately)
- `reference`   — auto-promote   (pointer data, low stakes)
- `project`     — keep as draft  (noise-prone, benefits from review)

Additional guards:
- Body must be >= MIN_BODY_CHARS. Empty or thin drafts get rejected outright.
- Title/name slug must be unique against existing live memories. Duplicate
  slugs get a `-2026_04_23` timestamp suffix so we don't overwrite curated
  memories.
- Live writes touch MEMORY.md atomically (tmp + rename) to stay consistent
  under a concurrent tick.
"""

from __future__ import annotations

import re
import time
from pathlib import Path

DRAFTS_DIR = Path.home() / ".claude" / "projects" / "-Users-sebastian-MAKAKOO" / "memory" / "drafts"
MEMORY_ROOT = DRAFTS_DIR.parent
MEMORY_INDEX = MEMORY_ROOT / "MEMORY.md"

AUTO_PROMOTE_TYPES = {"user", "feedback", "reference"}
MIN_BODY_CHARS = 200

# Markers that indicate the LLM produced structured memory content (not raw
# tool-call leakage or mid-turn scratch). A promotable draft must contain at
# least one of these section headers.
REQUIRED_SECTION_MARKERS = [
    r"^##\s+(Decisions|Facts|Mistakes|Action Items|Entity Updates|Rules|Why|How to apply)",
    r"^\*\*(Why|How to apply|Reason):\*\*",
]

# Hard blockers — if any of these appear, the draft is raw transcript noise
# and should never be auto-promoted even if it passes other checks.
CONTENT_BLOCKERS = [
    r"^\[TOOL_CALL\]",
    r"^\[/TOOL_CALL\]",
    r"^--paths\s+\[",
    r"^\{tool\s*=>",
]


def _parse_frontmatter(content: str) -> tuple[dict, str]:
    m = re.match(r"^---\n(.*?)\n---\n(.*)$", content, re.DOTALL)
    if not m:
        return {}, content
    fm = {}
    for line in m.group(1).splitlines():
        if ":" in line:
            k, _, v = line.partition(":")
            fm[k.strip()] = v.strip()
    return fm, m.group(2)


def _slug(name: str) -> str:
    return re.sub(r"[^a-zA-Z0-9_-]", "_", name).strip("_") or "memory"


def _unique_dest(memory_root: Path, mem_type: str, name: str) -> Path:
    base = f"{mem_type}_{_slug(name)}"
    dest = memory_root / f"{base}.md"
    if not dest.exists():
        return dest
    suffix = time.strftime("%Y_%m_%d_%H%M")
    return memory_root / f"{base}_{suffix}.md"


def _append_to_index(title: str, filename: str, description: str) -> None:
    if not MEMORY_INDEX.exists():
        return
    new_line = f"- [{title}]({filename}) — {description}\n"
    current = MEMORY_INDEX.read_text()
    if filename in current:
        return
    tmp = MEMORY_INDEX.with_suffix(".tmp")
    tmp.write_text(current.rstrip() + "\n" + new_line)
    tmp.replace(MEMORY_INDEX)


def _is_structured_memory(body: str) -> bool:
    """Verify the draft body looks like structured memory content, not raw transcript noise."""
    for blocker in CONTENT_BLOCKERS:
        if re.search(blocker, body, re.MULTILINE):
            return False
    for marker in REQUIRED_SECTION_MARKERS:
        if re.search(marker, body, re.MULTILINE):
            return True
    return False


def auto_promote(drafts_dir: Path | None = None) -> dict[str, int]:
    """Walk drafts_dir and promote every eligible draft. Returns tally by outcome."""
    drafts_dir = drafts_dir or DRAFTS_DIR
    if not drafts_dir.exists():
        return {}

    tally = {
        "promoted": 0,
        "kept_draft": 0,
        "rejected_thin": 0,
        "rejected_unstructured": 0,
        "skipped_marker": 0,
    }

    for path in sorted(drafts_dir.glob("*.md")):
        # Skip already-handled drafts
        if path.with_suffix(".promoted").exists() or path.with_suffix(".rejected").exists():
            tally["skipped_marker"] += 1
            continue

        try:
            content = path.read_text()
        except OSError:
            continue

        fm, body = _parse_frontmatter(content)
        mem_type = fm.get("type", "user")
        name = fm.get("name") or path.stem
        description = fm.get("description", "Auto-extracted memory")

        if mem_type not in AUTO_PROMOTE_TYPES:
            tally["kept_draft"] += 1
            continue

        if len(body.strip()) < MIN_BODY_CHARS:
            path.with_suffix(".rejected").write_text("thin body\n")
            path.unlink(missing_ok=True)
            tally["rejected_thin"] += 1
            continue

        if not _is_structured_memory(body):
            path.with_suffix(".rejected").write_text("unstructured body (no section markers or tool-call noise)\n")
            path.unlink(missing_ok=True)
            tally["rejected_unstructured"] += 1
            continue

        dest = _unique_dest(MEMORY_ROOT, mem_type, name)
        dest.write_text(content)
        _append_to_index(name, dest.name, description)

        # Leave .promoted marker so the observer doesn't re-extract this draft
        path.with_suffix(".promoted").write_text(f"promoted -> {dest.name}\n")
        path.unlink(missing_ok=True)
        tally["promoted"] += 1

    return tally


if __name__ == "__main__":
    import json as _json
    import sys

    if len(sys.argv) > 1 and sys.argv[1] == "--dry-run":
        # Report what would be promoted without writing anything
        if not DRAFTS_DIR.exists():
            print("{}")
            sys.exit(0)
        preview = []
        for path in sorted(DRAFTS_DIR.glob("*.md")):
            try:
                fm, body = _parse_frontmatter(path.read_text())
            except OSError:
                continue
            mem_type = fm.get("type", "user")
            would = (
                mem_type in AUTO_PROMOTE_TYPES
                and len(body.strip()) >= MIN_BODY_CHARS
                and _is_structured_memory(body)
            )
            preview.append({
                "file": path.name,
                "type": mem_type,
                "would_promote": would,
            })
        print(_json.dumps(preview, indent=2))
    else:
        print(_json.dumps(auto_promote()))
