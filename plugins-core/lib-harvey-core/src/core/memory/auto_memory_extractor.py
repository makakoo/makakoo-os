"""
Auto-Memory Extractor — Claude Code Pattern Implemented in Harvey OS

Extracts memories from session transcripts post-session using MiniMax-M2.7 via switchAILocal.
Writes memories to data/Brain/auto-memory/ with Brain-compatible frontmatter.

Key insight from Claude Code audit:
- Session transcript → LLM → auto-memory file (no embeddings)
- Memory types: user, feedback, project, reference
- Format: Markdown with [[entity]] links
"""

from __future__ import annotations

import os
import json
import time
import httpx
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

EXTRACT_SYSTEM_PROMPT = """You are a memory extraction agent. After each session, you extract key information from the transcript.

Extract and format as:

## Decisions
- [decision] — reason if obvious

## Facts Learned
- [fact] — source if available

## Mistakes Made
- [mistake] — what should have been done differently

## Action Items
- [ ] [action] — owner if specified

## Entity Updates
- Update [[Entity]] page with new information discovered

Format: Markdown with [[entity]] wikilinks where appropriate.

IMPORTANT RULES:
- Extract at least 2-3 items across categories if present in the transcript
- Be specific: include actual values, names, numbers mentioned
- If a fact is uncertain, flag it with "(uncertain)"
- Link to entities mentioned using [[Entity Name]] syntax
- Never fabricate — only extract what's actually in the transcript
"""


# ---------------------------------------------------------------------------
# Types
# ---------------------------------------------------------------------------


@dataclass
class MemoryHeader:
    """Lightweight header extracted from auto-memory frontmatter."""

    filename: str
    path: Path
    mtime: float
    description: str
    mem_type: str  # user | feedback | project | reference


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _get_default_brain_dir() -> Path:
    """Resolve HARVEY_HOME and return data/Brain path."""
    harvey_home = os.environ.get("HARVEY_HOME", str(Path.home() / "HARVEY"))
    return Path(harvey_home) / "data" / "Brain"


def _get_auto_memory_dir(brain_dir: Path | None = None) -> Path:
    """Get or create the auto-memory directory."""
    auto_dir = (
        _get_default_brain_dir() if brain_dir is None else brain_dir
    ) / "auto-memory"
    auto_dir.mkdir(parents=True, exist_ok=True)
    return auto_dir


def _call_llm(messages: list[dict], temperature: float = 0.3) -> str:
    """
    Call MiniMax-M2.7 via switchAILocal (localhost:18080).
    Falls back to a clear error if the service is unavailable.
    """
    base_url = os.environ.get("LLM_BASE_URL", "http://localhost:18080/v1").rstrip("/")
    api_key = os.environ.get("SWITCHAI_KEY", "")
    model = os.environ.get("LLM_MODEL", "auto")

    payload = {
        "model": model,
        "messages": messages,
        "temperature": temperature,
    }

    # Retry up to 2 times on transient failures
    last_error: RuntimeError = RuntimeError("LLM call failed after 3 attempts")
    for attempt in range(3):
        try:
            with httpx.Client(timeout=90.0) as client:
                resp = client.post(
                    f"{base_url}/chat/completions",
                    json=payload,
                    headers={
                        "Authorization": f"Bearer {api_key}",
                        "Content-Type": "application/json",
                    },
                )
            resp.raise_for_status()
            result = resp.json()
            if result.get("choices") and len(result["choices"]) > 0:
                return result["choices"][0]["message"]["content"]
            # choices null — treat as transient failure, retry
            last_error = RuntimeError(
                f"LLM returned null choices (attempt {attempt + 1}/3)"
            )
        except (httpx.ConnectError, httpx.TimeoutException) as e:
            last_error = RuntimeError(f"switchAILocal unavailable or timeout: {e}")
        except Exception as e:
            last_error = RuntimeError(f"LLM call failed: {e}")

        if attempt < 2:
            import time

            time.sleep(1 * (attempt + 1))  # Brief backoff

    raise last_error


def _parse_frontmatter(content: str) -> dict:
    """Parse YAML-ish frontmatter from a memory file."""
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


def _truncate_for_llm(text: str, max_chars: int = 3000) -> str:
    """Safely truncate transcript text to avoid token limits."""
    if len(text) <= max_chars:
        return text
    return text[:max_chars].rsplit("\n", 1)[0] + "\n[transcript truncated]"


# ---------------------------------------------------------------------------
# Core API
# ---------------------------------------------------------------------------


def extract_memories(
    transcript: str,
    output_dir: Path | None = None,
    session_name: str | None = None,
) -> Path:
    """
    Extract memories from a session transcript and write to auto-memory dir.

    Args:
        transcript:     The full session transcript text.
        output_dir:      Directory to write memory file to.
                        Defaults to data/Brain/auto-memory/.
        session_name:    Optional descriptive name for the session.
                        Defaults to "session_{timestamp}".

    Returns:
        Path to the created memory file.

    Raises:
        RuntimeError:   If switchAILocal is unavailable or LLM call fails.
    """
    if output_dir is None:
        output_dir = _get_auto_memory_dir()
    else:
        output_dir.mkdir(parents=True, exist_ok=True)

    # Build LLM prompt
    truncated = _truncate_for_llm(transcript)
    messages = [
        {"role": "system", "content": EXTRACT_SYSTEM_PROMPT},
        {"role": "user", "content": f"Session transcript:\n\n{truncated}"},
    ]

    # Call LLM
    raw_output = _call_llm(messages)

    # Validate — ensure it has frontmatter markers
    if "---" not in raw_output:
        # Inject minimal frontmatter if LLM omitted it
        timestamp = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        name = session_name or f"session-{int(time.time())}"
        raw_output = (
            f"---\n"
            f"name: {name}\n"
            f"description: Auto-extracted memories\n"
            f"type: user\n"
            f"created: {timestamp}\n"
            f"---\n\n"
            f"{raw_output}"
        )

    # Parse frontmatter to fill in missing fields
    fm = _parse_frontmatter(raw_output)
    timestamp = fm.get(
        "created", datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    )
    name = fm.get("name", session_name or f"session-{int(time.time())}")
    mem_type = fm.get("type", "user")

    # Ensure type is valid
    valid_types = {"user", "feedback", "project", "reference"}
    if mem_type not in valid_types:
        mem_type = "user"

    # Build safe filename
    safe_name = name.lower().replace(" ", "-").replace("/", "-").replace("\\", "-")[:60]
    date_str = timestamp[:10]  # YYYY-MM-DD
    filename = f"{date_str}_{safe_name}.md"
    output_path = output_dir / filename

    # Avoid overwriting — append counter if exists
    counter = 1
    while output_path.exists():
        filename = f"{date_str}_{safe_name}_{counter}.md"
        output_path = output_dir / filename
        counter += 1

    output_path.write_text(raw_output)

    # Emit telemetry event (best-effort — does not block on missing telemetry)
    _emit_memory_event(
        "MEMORY_AUTO_EXTRACT", {"path": str(output_path), "type": mem_type}
    )

    return output_path


def _emit_memory_event(event_type: str, payload: dict) -> None:
    """
    Emit a telemetry event when telemetry module is available.
    This is a no-op if telemetry isn't loaded yet.
    """
    try:
        # Deferred import — avoids circular dependency
        from harvey_os.core.telemetry.events import TelemetryEmitter

        emitter = TelemetryEmitter()
        emitter.emit(event_type, payload)
    except Exception:
        # Telemetry not yet available — skip silently
        pass


def scan_memory_files(
    brain_dir: Path | None = None,
    max_files: int = 200,
    mem_types: list[str] | None = None,
) -> list[MemoryHeader]:
    """
    Scan auto-memory directory for memory files and extract headers.

    Returns memories sorted by mtime (most recent first).

    Args:
        brain_dir:   Brain directory to scan. Defaults to data/Brain/.
        max_files:   Maximum number of files to scan.
        mem_types:   Filter by memory types (e.g. ["project", "user"]).

    Returns:
        List of MemoryHeader objects.
    """
    # Scan both auto-memory subdir AND root of brain_dir
    # (extract_memories writes to output_dir directly, not to auto-memory/ subdir)
    brain_base = _get_default_brain_dir() if brain_dir is None else brain_dir
    auto_dir = brain_base / "auto-memory"

    scan_dirs: list[Path] = []
    if auto_dir.exists():
        scan_dirs.append(auto_dir)
    if brain_base.exists() and brain_base != auto_dir:
        scan_dirs.append(brain_base)

    headers: list[MemoryHeader] = []

    # Walk all scan dirs, newest first
    all_files: list[Path] = []
    for scan_dir in scan_dirs:
        all_files.extend(scan_dir.glob("*.md"))
    all_files = sorted(all_files, key=lambda p: p.stat().st_mtime, reverse=True)

    for path in all_files[:max_files]:
        try:
            text = path.read_text()
        except OSError:
            continue

        fm = _parse_frontmatter(text)
        if not fm:
            # No frontmatter — skip
            continue

        mem_type = fm.get("type", "unknown")
        if mem_types and mem_type not in mem_types:
            continue

        headers.append(
            MemoryHeader(
                filename=path.name,
                path=path,
                mtime=path.stat().st_mtime,
                description=fm.get("description", ""),
                mem_type=mem_type,
            )
        )

    return headers


# ---------------------------------------------------------------------------
# CLI entry point (for testing)
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    import argparse, tempfile

    parser = argparse.ArgumentParser(description="Auto-memory extraction")
    parser.add_argument("transcript", help="Path to transcript file or '-' for stdin")
    parser.add_argument("--output", "-o", type=Path, help="Output directory")
    parser.add_argument("--name", "-n", help="Session name")
    args = parser.parse_args()

    if args.transcript == "-":
        transcript = "\n".join(__import__("sys").stdin.readlines())
    else:
        transcript = Path(args.transcript).read_text()

    result = extract_memories(transcript, args.output, args.name)
    print(f"Memory file written to: {result}")

    # Scan and print
    headers = scan_memory_files(result.parent)
    print(f"\nScanned {len(headers)} memory file(s) in {result.parent}:")
    for h in headers:
        print(f"  [{h.mem_type}] {h.filename} — {h.description[:50]}")
