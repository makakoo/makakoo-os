"""
HarveyChat Brain Sync — Logs significant chat interactions to Brain journals.

Runs periodically to extract notable conversations and persist them
in Brain outliner format.
"""

import logging
import os
import time
from pathlib import Path
from typing import Dict, List

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
BRAIN_JOURNALS = Path(HARVEY_HOME) / "data" / "Brain" / "journals"

log = logging.getLogger("harveychat.brain_sync")


def log_to_journal(channel: str, summary: str, details: str = ""):
    """
    Log a chat interaction to today's Brain journal.

    Args:
        channel: Source channel (telegram, whatsapp, etc.)
        summary: One-line summary of the interaction
        details: Optional multi-line details (each line will be indented as sub-bullet)
    """
    today = time.strftime("%Y_%m_%d")
    journal_path = BRAIN_JOURNALS / f"{today}.md"
    BRAIN_JOURNALS.mkdir(parents=True, exist_ok=True)

    lines = [f"- [[HarveyChat]] via {channel}: {summary}"]
    if details:
        for line in details.strip().split("\n"):
            lines.append(f"  - {line}")

    entry = "\n".join(lines) + "\n"

    # Append to journal
    if journal_path.exists():
        content = journal_path.read_text()
        if entry.strip() not in content:  # Avoid duplicates
            journal_path.write_text(content.rstrip() + "\n" + entry)
    else:
        journal_path.write_text(entry)

    log.info(f"Brain journal updated: {summary}")


def log_session_summary(channel: str, channel_user: str,
                        messages: List[Dict], duration_sec: float):
    """
    Log a session summary when conversation goes idle (>30min gap).

    Only logs if the conversation had 3+ exchanges.
    """
    if len(messages) < 6:  # 3 exchanges = 6 messages minimum
        return

    user_msgs = [m["content"] for m in messages if m["role"] == "user"]
    topics = _extract_topics(user_msgs)

    summary = f"Session with {channel_user} ({len(messages)} msgs, {int(duration_sec/60)}min)"
    details = f"Topics: {', '.join(topics)}" if topics else ""

    log_to_journal(channel, summary, details)


def _extract_topics(messages: List[str], max_topics: int = 5) -> List[str]:
    """Extract rough topic keywords from user messages. Simple heuristic."""
    # Take first 3 words of each message as rough topic indicators
    topics = set()
    for msg in messages[:10]:
        words = msg.strip().split()[:4]
        if len(words) >= 2:
            topics.add(" ".join(words[:3]).rstrip("?.,!"))
    return list(topics)[:max_topics]
