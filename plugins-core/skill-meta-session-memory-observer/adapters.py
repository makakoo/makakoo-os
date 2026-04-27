"""Session-source adapters.

Each adapter knows where to find session JSONL files for one CLI host and how to
pull human-readable text out of a line. Adding a CLI = writing one adapter class
and adding it to `ALL_ADAPTERS`. The observer doesn't care about per-CLI shape.

Supported today:
- Claude Code — canonical target
- Qwen Code — same shape as Claude Code (type: user|assistant, message.content)
- Vibe — flat {role, content} lines

Not yet supported (format is different enough to need its own parsing):
- Codex (session_meta + event stream, content buried in events)
- pi (session header + events, different role encoding)
- Gemini (not JSONL in the dirs I've seen)
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable


@dataclass
class NormalizedMessage:
    """Unified message shape the observer understands regardless of source CLI."""

    role: str  # "user" | "assistant"
    text: str


@dataclass
class Adapter:
    """Session-source adapter: knows where sessions live and how to decode one line."""

    name: str
    glob_fn: Callable[[], Iterable[Path]]
    parse_line: Callable[[dict], NormalizedMessage | None]

    def sessions(self) -> Iterable[Path]:
        return self.glob_fn()


def _text_from_blocks(content) -> str:
    """Flatten a content value (str or list of blocks) into a text string.

    Preserves tool_use / tool_result markers for context but drops their raw JSON.
    """
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts = []
        for block in content:
            if not isinstance(block, dict):
                continue
            btype = block.get("type")
            if btype == "text":
                parts.append(block.get("text", ""))
            elif btype == "tool_use":
                parts.append(f"[tool_use: {block.get('name', 'tool')}]")
            elif btype == "tool_result":
                parts.append("[tool_result]")
        return "\n".join(p for p in parts if p)
    return ""


def _claude_code_parse(line: dict) -> NormalizedMessage | None:
    t = line.get("type")
    if t not in ("user", "assistant"):
        return None
    text = _text_from_blocks(line.get("message", {}).get("content", ""))
    if not text.strip():
        return None
    return NormalizedMessage(role=t, text=text)


def _claude_code_glob() -> Iterable[Path]:
    base = Path.home() / ".claude" / "projects" / "-Users-sebastian-MAKAKOO"
    if not base.exists():
        return []
    return sorted(base.glob("*.jsonl"))


def _qwen_parse(line: dict) -> NormalizedMessage | None:
    """Qwen Code top-level shape matches Claude Code, but content lives in
    `message.parts[]` instead of `message.content`. Parts can be text blocks
    with an optional `thought: true` flag, or functionCall/functionResponse.
    """
    t = line.get("type")
    if t not in ("user", "assistant"):
        return None
    parts = line.get("message", {}).get("parts", [])
    if not isinstance(parts, list):
        return None
    chunks = []
    for part in parts:
        if not isinstance(part, dict):
            continue
        if "text" in part:
            prefix = "[thinking] " if part.get("thought") else ""
            chunks.append(f"{prefix}{part['text']}")
        elif "functionCall" in part:
            fc = part.get("functionCall", {})
            name = fc.get("name", "tool")
            chunks.append(f"[tool_use: {name}]")
        elif "functionResponse" in part:
            chunks.append("[tool_result]")
    text = "\n".join(c for c in chunks if c)
    if not text.strip():
        return None
    return NormalizedMessage(role=t, text=text)


def _qwen_glob() -> Iterable[Path]:
    base = Path.home() / ".qwen" / "projects"
    if not base.exists():
        return []
    return sorted(base.glob("*/chats/*.jsonl"))


def _vibe_parse(line: dict) -> NormalizedMessage | None:
    role = line.get("role")
    if role not in ("user", "assistant"):
        return None
    content = line.get("content", "")
    text = _text_from_blocks(content)
    if not text.strip():
        return None
    return NormalizedMessage(role=role, text=text)


def _vibe_glob() -> Iterable[Path]:
    base = Path.home() / ".vibe" / "logs" / "session"
    if not base.exists():
        return []
    return sorted(base.glob("*/messages.jsonl"))


ALL_ADAPTERS: list[Adapter] = [
    Adapter(name="claude", glob_fn=_claude_code_glob, parse_line=_claude_code_parse),
    Adapter(name="qwen", glob_fn=_qwen_glob, parse_line=_qwen_parse),
    Adapter(name="vibe", glob_fn=_vibe_glob, parse_line=_vibe_parse),
]
