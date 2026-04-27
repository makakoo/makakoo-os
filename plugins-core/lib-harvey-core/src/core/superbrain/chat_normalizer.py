#!/usr/bin/env python3
"""
Chat Normalizer — Multi-format chat transcript → standard format.

Supports:
- Claude.ai JSON (export from claude.ai)
- ChatGPT conversations.json (export from chat.openai.com)
- Claude Code JSONL (~/.claude/projects/*/sessions/*.jsonl)
- OpenAI Codex JSONL
- Slack JSON exports
- Plain text transcripts

Normalizes to a standard transcript format:
  [{"role": "human", "content": "...", "ts": "..."}, {"role": "assistant", "content": "...", "ts": "..."}]

Usage:
    from core.superbrain.chat_normalizer import ChatNormalizer
    normalizer = ChatNormalizer()
    transcript = normalizer.normalize_file("/path/to/export.json")
    for msg in transcript:
        print(f"{msg['role']}: {msg['content'][:80]}")
"""

import json
import logging
import re
import sys
from datetime import datetime
from pathlib import Path
from typing import List, Dict, Optional, Any, Union

log = logging.getLogger("superbrain.chat_normalizer")


# ─────────────────────────────────────────────────────────────────────────────
#  Transcript format (our canonical internal representation)
# ─────────────────────────────────────────────────────────────────────────────

Transcript = List[Dict[str, str]]
"""
Each message:
    role:      "human" | "assistant" | "system"
    content:   message text (str)
    ts:        ISO timestamp string or "" if unknown
"""


# ─────────────────────────────────────────────────────────────────────────────
#  Format detectors
# ─────────────────────────────────────────────────────────────────────────────


class FormatDetector:
    """Auto-detect chat format from file content."""

    @staticmethod
    def detect(raw: Any) -> str:
        if isinstance(raw, list):
            if any(
                "message_type" in item or "type" in item
                for item in raw[:3]
                if isinstance(item, dict)
            ):
                return "claude_code"
            if any(
                "role" in item and "content" in item
                for item in raw[:3]
                if isinstance(item, dict)
            ):
                return "openai_canonical"
            return "generic_list"
        if isinstance(raw, dict):
            if "conversations" in raw:
                return "chatgpt"
            if "items" in raw and "SlackExport" in str(raw):
                return "slack"
            if "chatflow" in raw:
                return "generic_dict"
        return "unknown"


# ─────────────────────────────────────────────────────────────────────────────
#  Claude.ai JSON
# ─────────────────────────────────────────────────────────────────────────────


def normalize_claude_ai(data: Dict) -> Transcript:
    """
    Claude.ai conversation export JSON.

    Expected structure:
        {"messages": [...]} or top-level list of messages

    Message structure (varies):
        {"type": "human|assistant", "content": "...", "create_time": 1234567890}
    """
    messages = data.get("messages", data) if isinstance(data, dict) else data
    result = []

    for msg in messages:
        if not isinstance(msg, dict):
            continue
        content = msg.get("content", "")
        if isinstance(content, list):
            content = " ".join(
                c.get("text", "") for c in content if isinstance(c, dict)
            )
        content = str(content).strip()
        if not content:
            continue

        msg_type = msg.get("type", "")
        if "assistant" in msg_type.lower():
            role = "assistant"
        elif "human" in msg_type.lower():
            role = "human"
        elif msg.get("role") in ("assistant", "user"):
            role = "assistant" if msg["role"] == "assistant" else "human"
        else:
            continue

        ts = ""
        if "create_time" in msg:
            ts = datetime.fromtimestamp(msg["create_time"]).isoformat()
        elif "timestamp" in msg:
            ts = datetime.fromtimestamp(msg["timestamp"]).isoformat()

        result.append({"role": role, "content": content, "ts": ts})

    return result


# ─────────────────────────────────────────────────────────────────────────────
#  ChatGPT conversations.json
# ─────────────────────────────────────────────────────────────────────────────


def normalize_chatgpt(data: Dict) -> Transcript:
    """
    ChatGPT conversation export JSON.

    Expected structure:
        {"conversations": [{"messages": [...]}]}
    """
    result = []
    conversations = data.get("conversations", [])
    if isinstance(data.get("conversations"), list) and len(data["conversations"]) == 0:
        conversations = [data]

    for conversation in conversations:
        messages = (
            conversation.get("messages", [])
            if isinstance(conversation, dict)
            else conversation
        )
        for msg in messages:
            if not isinstance(msg, dict):
                continue
            role = msg.get("role", "")
            if role == "assistant":
                role = "assistant"
            elif role == "user":
                role = "human"
            else:
                continue

            content = msg.get("content", "")
            if isinstance(content, list):
                content = " ".join(
                    c.get("text", "") for c in content if isinstance(c, dict)
                )
            content = str(content).strip()
            if not content:
                continue

            ts = ""
            if "create_time" in msg:
                ts = datetime.fromtimestamp(msg["create_time"]).isoformat()

            result.append({"role": role, "content": content, "ts": ts})

    return result


# ─────────────────────────────────────────────────────────────────────────────
#  Claude Code JSONL
# ─────────────────────────────────────────────────────────────────────────────


def normalize_claude_code_jsonl(file_path: Path) -> Transcript:
    """
    Claude Code session JSONL — one JSON object per line.

    Each line has: type, message {role, content}, ts
    Only include user<>assistant exchanges (not system/mcp/printer).
    """
    result = []
    try:
        lines = file_path.read_text(encoding="utf-8", errors="replace").splitlines()
    except Exception:
        return []

    for line in lines:
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except Exception:
            continue

        msg_type = obj.get("type", "")
        msg = obj.get("message", {})
        if not isinstance(msg, dict):
            continue

        role = msg.get("role", "")
        content = msg.get("content", "")
        if isinstance(content, list):
            content = " ".join(
                c.get("text", "") for c in content if isinstance(c, dict)
            )
        content = str(content).strip()

        if not content:
            continue

        if role == "user":
            role = "human"
        elif role != "assistant":
            continue

        ts = obj.get("ts", "")
        if ts:
            try:
                ts = datetime.fromisoformat(ts.replace("Z", "+00:00")).isoformat()
            except Exception:
                ts = ""

        result.append({"role": role, "content": content, "ts": ts})

    return result


# ─────────────────────────────────────────────────────────────────────────────
#  OpenAI Codex JSONL (same structure as Claude Code)
# ─────────────────────────────────────────────────────────────────────────────


def normalize_codex_jsonl(file_path: Path) -> Transcript:
    """Codex CLI session JSONL — same structure as Claude Code."""
    return normalize_claude_code_jsonl(file_path)


# ─────────────────────────────────────────────────────────────────────────────
#  Slack JSON export
# ─────────────────────────────────────────────────────────────────────────────


def normalize_slack(data: Dict) -> Transcript:
    """
    Slack JSON export format.

    Expected: {"channels": [...], "messages": [...]]
    or flat list of messages with "user", "text", "ts" fields.
    """
    result = []
    messages = data.get("messages", []) if isinstance(data, dict) else data

    for msg in messages:
        if not isinstance(msg, dict):
            continue

        content = msg.get("text", "")
        content = str(content).strip()
        if not content or content in ("", "None"):
            continue

        if msg.get("subtype") in ("bot_message", "channel_join"):
            continue

        user = msg.get("user", "")
        ts = msg.get("ts", "")
        if ts:
            try:
                ts = datetime.fromtimestamp(float(ts)).isoformat()
            except Exception:
                ts = ""

        role = "human"  # all Slack export messages are from humans
        result.append({"role": role, "content": f"[{user}]: {content}", "ts": ts})

    return result


# ─────────────────────────────────────────────────────────────────────────────
#  Plain text transcript
# ─────────────────────────────────────────────────────────────────────────────


def normalize_plaintext(raw: str) -> Transcript:
    """
    Plain text transcript — detect speaker changes by lines starting with
    "Human:", "Assistant:", "User:", "Bot:", or blank-line-separated blocks.
    """
    result = []
    lines = raw.splitlines()

    current_role = "human"
    buffer: List[str] = []

    def flush():
        if buffer:
            result.append({"role": current_role, "content": " ".join(buffer), "ts": ""})
        return []

    for line in lines:
        stripped = line.strip()
        lower = stripped.lower()

        if lower.startswith("human:") or lower.startswith("user:"):
            buffer = flush()
            current_role = "human"
            stripped = stripped.split(":", 1)[1].strip()
        elif (
            lower.startswith("assistant:")
            or lower.startswith("bot:")
            or lower.startswith("ai:")
        ):
            buffer = flush()
            current_role = "assistant"
            stripped = stripped.split(":", 1)[1].strip()
        elif not stripped:
            if buffer:
                buffer.append(" ")
            continue

        if stripped:
            buffer.append(stripped)

    flush()
    return result


# ─────────────────────────────────────────────────────────────────────────────
#  Main normalizer
# ─────────────────────────────────────────────────────────────────────────────


class ChatNormalizer:
    """
    Unified chat transcript normalizer.

    Usage:
        normalizer = ChatNormalizer()
        transcript = normalizer.normalize_file("/path/to/export.json")
        transcript = normalizer.normalize_raw(data, format_hint="claude_code")
    """

    def __init__(self):
        self.handlers = {
            "claude_ai": normalize_claude_ai,
            "chatgpt": normalize_chatgpt,
            "claude_code": lambda p: (
                normalize_claude_code_jsonl(Path(p))
                if isinstance(p, Path)
                else normalize_claude_code_jsonl(p)
            ),
            "codex": lambda p: (
                normalize_codex_jsonl(Path(p))
                if isinstance(p, Path)
                else normalize_codex_jsonl(p)
            ),
            "slack": normalize_slack,
            "plaintext": lambda raw: normalize_plaintext(str(raw)),
        }

    def normalize_file(self, file_path: Union[str, Path]) -> Transcript:
        """Load and normalize a chat export file."""
        path = Path(file_path)

        # Try JSONL first (Claude Code, Codex)
        if path.suffix == ".jsonl":
            raw = path.read_text(encoding="utf-8", errors="replace")
            if raw.strip().startswith("{"):
                return self.normalize_raw(raw, format_hint="claude_code")

        # Try as JSON
        try:
            raw = path.read_text(encoding="utf-8", errors="replace")
            data = json.loads(raw)
        except (json.JSONDecodeError, UnicodeDecodeError):
            # Fall back to plaintext
            raw = path.read_text(encoding="utf-8", errors="replace")
            return self.normalize_raw(raw, format_hint="plaintext")

        fmt = FormatDetector.detect(data)
        return self.normalize_raw(data, format_hint=fmt)

    def normalize_raw(self, data: Any, format_hint: str = "auto") -> Transcript:
        """
        Normalize raw data (dict, list, or str) to transcript format.

        Args:
            data: Raw loaded JSON, list, or string
            format_hint: "auto", "claude_ai", "chatgpt", "claude_code",
                         "codex", "slack", "plaintext"
        """
        if format_hint == "auto":
            fmt = FormatDetector.detect(data)
        else:
            fmt = format_hint

        handler = self.handlers.get(fmt)
        if handler:
            result = handler(data)
            log.info("Normalized %s → %d messages", fmt, len(result))
            return result

        log.warning("Unknown format '%s', trying plain text", fmt)
        return normalize_plaintext(str(data)[:50000])

    def chunk_by_exchanges(
        self, transcript: Transcript, chunk_size: int = 20
    ) -> List[Transcript]:
        """
        Split a transcript into chunk_size-exchange chunks.

        Each chunk has chunk_size human+assistant pairs.
        """
        chunks = []
        exchange_count = 0
        current: Transcript = []

        for msg in transcript:
            current.append(msg)
            if msg["role"] == "assistant":
                exchange_count += 1
                if exchange_count >= chunk_size:
                    chunks.append(current)
                    current = []
                    exchange_count = 0

        if current:
            chunks.append(current)
        return chunks

    def extract_topics(self, transcript: Transcript) -> List[str]:
        """Very lightweight topic extraction — first line keywords."""
        topics = []
        for msg in transcript[:10]:
            words = msg["content"].split()[:30]
            topics.extend([w for w in words if len(w) > 4][:5])
        return list(set(topics))[:20]


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python3 chat_normalizer.py <export_file.json|jsonl>")
        sys.exit(1)

    normalizer = ChatNormalizer()
    transcript = normalizer.normalize_file(sys.argv[1])

    print(f"\n{len(transcript)} messages:\n")
    for msg in transcript[:20]:
        role = msg["role"][:3].upper()
        content = msg["content"][:100].replace("\n", " ")
        ts = msg["ts"][:10] if msg["ts"] else "--------"
        print(f"[{ts}] {role}: {content}")
