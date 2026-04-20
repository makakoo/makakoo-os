"""
LLM-Based Memory Selector — Claude Code Pattern for Harvey OS

Selects relevant memories from Brain using MiniMax-M2.7 (no vector embeddings).

Key insight from Claude Code audit (§11.1 findRelevantMemories):
- NO vector embeddings for Brain text
- LLM does all the ranking — text-match pre-filter + LLM selection
- Hybrid: Brain journals/pages → LLM; Multimodal (PDFs, video) → Qdrant stays

This module replaces the embedding-based selection in superbrain/query.py
for Brain-only queries.
"""

from __future__ import annotations

import json
import os
import time
import httpx
from collections import OrderedDict
from dataclasses import dataclass
from datetime import datetime, timezone, timedelta
from pathlib import Path
from typing import Optional

# ---------------------------------------------------------------------------
# Types
# ---------------------------------------------------------------------------


@dataclass
class MemoryCandidate:
    """A candidate memory from Brain with header + content."""

    index: int
    filename: str
    relative_path: str
    content: str
    mtime: float
    mem_type: str  # user | feedback | project | reference
    description: str


# ---------------------------------------------------------------------------
# Prompt Template
# ---------------------------------------------------------------------------

SELECTION_PROMPT_TEMPLATE = """You are a memory selection agent. Given a query and a list of candidate memories, select the most relevant ones.

Query: "{query}"

Select up to {max_return} memories that are most relevant to the query.

Consider:
- Topic relevance: Does this memory relate to the query subject?
- Recency: More recent memories are generally more valuable
- Specificity: Specific facts and decisions > generic statements
- Type priority: user > feedback > project > reference (when in doubt)
- Completeness: Memories with detailed descriptions are more useful

Return a JSON array of INTEGER INDICES only (e.g. [0, 3, 7]).
Do NOT include any other text in your response. Only valid JSON.

Memories:
{memories}
"""


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _get_default_brain_dir() -> Path:
    """Resolve HARVEY_HOME and return data/Brain path."""
    harvey_home = os.environ.get("HARVEY_HOME", str(Path.home() / "HARVEY"))
    return Path(harvey_home) / "data" / "Brain"


def _call_llm(messages: list[dict], temperature: float = 0.1) -> str:
    """Call MiniMax-M2.7 via switchAILocal."""
    base_url = os.environ.get("LLM_BASE_URL", "http://localhost:18080/v1").rstrip("/")
    api_key = os.environ.get("SWITCHAI_KEY", "")
    model = os.environ.get("LLM_MODEL", "auto")

    payload = {
        "model": model,
        "messages": messages,
        "temperature": temperature,
    }

    try:
        with httpx.Client(timeout=60.0) as client:
            resp = client.post(
                f"{base_url}/chat/completions",
                json=payload,
                headers={
                    "Authorization": f"Bearer {api_key}",
                    "Content-Type": "application/json",
                },
            )
        resp.raise_for_status()
        return resp.json()["choices"][0]["message"]["content"]
    except httpx.ConnectError:
        raise RuntimeError(
            "switchAILocal not available at localhost:18080. "
            "Ensure the local AI gateway is running."
        )
    except Exception as e:
        raise RuntimeError(f"LLM call failed: {e}")


def _parse_frontmatter(content: str) -> dict:
    """Parse YAML-ish frontmatter from a Brain page/memory file."""
    if not content.startswith("---"):
        return {}
    parts = content[3:].split("---", 1)
    if len(parts) < 2:
        return {}
    fm_text = parts[0].strip()
    fm: dict = {}
    for line in fm_text.splitlines():
        if ":" not in line:
            continue
        key, _, value = line.partition(":")
        fm[key.strip()] = value.strip()
    return fm


def _truncate(text: str, max_chars: int = 1500) -> str:
    """Truncate text for LLM context, respecting line boundaries."""
    if len(text) <= max_chars:
        return text
    return text[:max_chars].rsplit("\n", 1)[0] + "\n[truncated]"


def _truncate_for_manifest(text: str, max_chars: int = 400) -> str:
    """Short truncation for the candidate manifest."""
    if len(text) <= max_chars:
        return text
    return text[:max_chars].rsplit("\n", 1)[0] + "..."


def _guess_mem_type(filename: str, content: str) -> str:
    """Infer memory type from filename or content if frontmatter missing."""
    fname_lower = filename.lower()
    if "feedback" in fname_lower:
        return "feedback"
    if "project" in fname_lower:
        return "project"
    if "reference" in fname_lower:
        return "reference"
    fm = _parse_frontmatter(content)
    return fm.get("type", "user")


# ---------------------------------------------------------------------------
# Core API
# ---------------------------------------------------------------------------


def llm_select_memories(
    query: str,
    brain_dir: Path | None = None,
    max_candidates: int = 20,
    max_return: int = 5,
    days_back: int = 30,
) -> list[MemoryCandidate]:
    """
    LLM-based memory selection — no vector embeddings.

    Flow:
    1. Scan recent Brain journals and auto-memory files (last `days_back` days)
    2. Keyword pre-filter to narrow candidates (fast, no LLM needed)
    3. Build manifest: "[index] filename (type): description + content preview"
    4. Send to MiniMax-M2.7 with SELECTION_PROMPT
    5. Parse JSON response, return selected MemoryCandidate list

    Args:
        query:           The query string (e.g. "BTC Polymarket trading")
        brain_dir:       Brain directory. Defaults to data/Brain/.
        max_candidates: Maximum candidates to pass to LLM (default 20)
        max_return:     Maximum memories to return (default 5)
        days_back:      How far back to scan journals (default 30 days)

    Returns:
        List of selected MemoryCandidate objects, ordered by LLM relevance.

    Raises:
        RuntimeError: If switchAILocal is unavailable.
    """
    if brain_dir is None:
        brain_dir = _get_default_brain_dir()

    candidates: list[MemoryCandidate] = []

    # Keywords from query for fast text pre-filter
    query_keywords = [kw.lower() for kw in query.split() if len(kw) > 2][:8]

    cutoff_time = time.time() - (days_back * 86400)

    # --- Scan auto-memory files ---
    # NOTE: Do NOT apply keyword pre-filter to auto-memory files.
    # They are session summaries and should always be considered.
    # Scan both brain_dir/auto-memory/ AND brain_dir/ root
    # (extract_memories writes directly to output_dir, not to auto-memory/ subdir)
    scan_dirs: list[Path] = [brain_dir / "auto-memory", brain_dir]
    for scan_dir in scan_dirs:
        if not scan_dir.exists():
            continue
        for path in sorted(
            scan_dir.glob("*.md"), key=lambda p: p.stat().st_mtime, reverse=True
        ):
            if len(candidates) >= max_candidates:
                break
            try:
                content = path.read_text()
            except OSError:
                continue

            # Time filter only
            if path.stat().st_mtime < cutoff_time:
                continue

            fm = _parse_frontmatter(content)
            description = fm.get("description", "")
            mem_type = fm.get("type", _guess_mem_type(path.name, content))

            candidates.append(
                MemoryCandidate(
                    index=len(candidates),
                    filename=path.name,
                    relative_path=str(path.relative_to(brain_dir)),
                    content=_truncate_for_manifest(content),
                    mtime=path.stat().st_mtime,
                    mem_type=mem_type,
                    description=description,
                )
            )
        if len(candidates) >= max_candidates:
            break

    # --- Scan recent journals ---
    journal_dir = brain_dir / "journals"
    if journal_dir.exists() and len(candidates) < max_candidates:
        now = datetime.now(timezone.utc)
        journal_count = 0
        for i in range(days_back):
            date = now - timedelta(days=i)
            journal_path = journal_dir / f"{date.strftime('%Y_%m_%d')}.md"
            if not journal_path.exists():
                continue
            if len(candidates) >= max_candidates:
                break
            if len(candidates) >= max_candidates:
                break

            try:
                content = journal_path.read_text()
            except OSError:
                continue

            # Keyword pre-filter (loose — include if any keyword matches)
            if query_keywords:
                content_lower = content.lower()
                if not any(kw in content_lower for kw in query_keywords):
                    continue

            fm = _parse_frontmatter(content)
            # Get first meaningful line as description
            description = fm.get("description", "")
            if not description:
                # First non-empty non-frontmatter line
                body = content.split("---", 2)[-1] if "---" in content else content
                description = body.strip().split("\n")[0][:100]

            candidates.append(
                MemoryCandidate(
                    index=len(candidates),
                    filename=journal_path.name,
                    relative_path=str(journal_path.relative_to(brain_dir)),
                    content=_truncate_for_manifest(content),
                    mtime=journal_path.stat().st_mtime,
                    mem_type="user",
                    description=description,
                )
            )
            journal_count += 1

    if not candidates:
        return []

    # --- Build manifest for LLM ---
    manifest_lines = []
    for c in candidates:
        type_tag = f"[{c.mem_type}]"
        desc = c.description[:80] if c.description else ""
        preview = c.content[:200].replace("\n", " ")
        manifest_lines.append(f"[{c.index}] {c.filename} {type_tag}: {desc}\n{preview}")

    manifest_text = "\n\n".join(manifest_lines)

    # --- Call LLM ---
    prompt = SELECTION_PROMPT_TEMPLATE.format(
        query=query,
        max_return=max_return,
        memories=manifest_text,
    )

    # --- Call LLM ---
    try:
        raw_response = _call_llm([{"role": "user", "content": prompt}])
    except RuntimeError:
        # LLM unavailable or failed — return keyword-filtered candidates as fallback
        return candidates[:max_return]

    # --- Parse JSON response ---
    try:
        # Strip markdown code fences if present
        cleaned = raw_response.strip()
        if cleaned.startswith("```"):
            cleaned = cleaned.split("```")[1]
            if cleaned.startswith("json"):
                cleaned = cleaned[4:]
        selected_indices: list[int] = json.loads(cleaned)
    except (json.JSONDecodeError, ValueError):
        # Fallback: return all candidates truncated
        return candidates[:max_return]

    # --- Build result (preserve LLM order, skip out-of-range) ---
    selected: list[MemoryCandidate] = []
    for idx in selected_indices:
        if idx < len(candidates) and candidates[idx] not in selected:
            selected.append(candidates[idx])
        if len(selected) >= max_return:
            break

    return selected


def llm_select_memories_simple(
    query: str,
    brain_dir: Path | None = None,
    max_return: int = 5,
) -> list[str]:
    """
    Simple convenience wrapper — returns just content strings.

    Args:
        query:       Search query
        brain_dir:   Brain directory
        max_return:  Max results

    Returns:
        List of memory content strings (truncated).
    """
    candidates = llm_select_memories(query, brain_dir, max_return=max_return)
    return [c.content for c in candidates]


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="LLM-based memory selection")
    parser.add_argument("query", help="Search query")
    parser.add_argument("--brain-dir", type=Path, help="Brain directory")
    parser.add_argument("--max-candidates", type=int, default=20)
    parser.add_argument("--max-return", type=int, default=5)
    args = parser.parse_args()

    results = llm_select_memories(
        args.query,
        brain_dir=args.brain_dir,
        max_candidates=args.max_candidates,
        max_return=args.max_return,
    )

    print(f"Selected {len(results)} memories:\n")
    for r in results:
        print(f"  [{r.mem_type}] {r.filename}")
        print(f"    {r.description[:80]}")
        print()
