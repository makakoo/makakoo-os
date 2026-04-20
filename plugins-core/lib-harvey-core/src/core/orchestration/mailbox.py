"""
Harvey OS — Mailbox IPC

File-based mailbox for agent-to-agent messaging.
Claude Code's teammateMailbox.ts pattern.

Key features:
- Path: ~/.harvey/teams/{team}/inboxes/{agent}.json
- Atomic flock-based locking for concurrent access
- Message queuing when agent is running, resume when paused

Path: harvey-os/core/orchestration/mailbox.py
"""

from __future__ import annotations

import os
import json
import time
import fcntl
from pathlib import Path
from dataclasses import dataclass, field
from typing import Literal, Any
from enum import Enum


class MailboxIndex:
    """
    Claude Code's teammateMailbox.ts pattern:
    File-based inbox with atomic locking.

    Path: ~/.harvey/teams/{team}/inboxes/{agent}.json
    Lock: {inbox}.lock (flock)

    Features:
    - Atomic writes with flock
    - Concurrent access safe
    - Message queuing
    - Team-based organization
    """

    def __init__(self, base_dir: Path | None = None):
        """
        Initialize MailboxIndex.

        Args:
            base_dir: Base directory for mailboxes.
                     Defaults to ~/.harvey/teams
        """
        Harvey_home = os.environ.get("HARVEY_HOME", str(Path.home() / "HARVEY"))
        self.base_dir = base_dir or Path(Harvey_home) / ".harvey" / "teams"
        self._lock_dir = Path(Harvey_home) / ".harvey" / "teams" / "_locks"
        self._lock_dir.mkdir(parents=True, exist_ok=True)

    def _inbox_path(self, agent_name: str, team: str = "default") -> Path:
        """
        Get inbox file path for an agent.

        Sanitizes agent and team names to be filesystem-safe.
        """
        safe_agent = agent_name.replace("/", "_").replace("\\", "_").replace(":", "_")
        safe_team = team.replace("/", "_").replace("\\", "_").replace(":", "_")
        return self.base_dir / safe_team / "inboxes" / f"{safe_agent}.json"

    def _lock_path(self, agent_name: str, team: str = "default") -> Path:
        """Get lock file path for an agent's inbox."""
        safe_agent = agent_name.replace("/", "_").replace("\\", "_").replace(":", "_")
        safe_team = team.replace("/", "_").replace("\\", "_").replace(":", "_")
        return self._lock_dir / f"{safe_team}_{safe_agent}.lock"

    def _ensure_dir(self, path: Path) -> None:
        """Ensure directory exists."""
        path.parent.mkdir(parents=True, exist_ok=True)

    async def write_message(
        self, recipient: str, message: dict, team: str = "default"
    ) -> None:
        """
        Write a message to an agent's inbox.

        Uses exclusive flock for atomic writes.
        Creates inbox file if it doesn't exist.

        Args:
            recipient: Agent name to deliver to
            message: Message dict (must be JSON-serializable)
            team: Team name (default: "default")
        """
        inbox_path = self._inbox_path(recipient, team)
        lock_path = self._lock_path(recipient, team)
        self._ensure_dir(inbox_path)

        with open(lock_path, "w") as lockf:
            fcntl.flock(lockf.fileno(), fcntl.LOCK_EX)
            try:
                if inbox_path.exists():
                    messages = json.loads(inbox_path.read_text())
                else:
                    messages = []

                messages.append(
                    {
                        **message,
                        "read": False,
                        "timestamp": time.time(),
                        "delivered": False,
                    }
                )

                # Atomic write via rename
                tmp_path = inbox_path.with_suffix(".json.tmp")
                tmp_path.write_text(json.dumps(messages, indent=2))
                tmp_path.rename(inbox_path)
            finally:
                fcntl.flock(lockf.fileno(), fcntl.LOCK_UN)

    async def read_messages(
        self, agent_name: str, team: str = "default", mark_read: bool = True
    ) -> list[dict]:
        """
        Read messages from an agent's inbox.

        Args:
            agent_name: Agent whose inbox to read
            team: Team name (default: "default")
            mark_read: Whether to mark messages as read (default: True)

        Returns:
            List of unread message dicts
        """
        inbox_path = self._inbox_path(agent_name, team)
        lock_path = self._lock_path(agent_name, team)

        if not inbox_path.exists():
            return []

        with open(lock_path, "w") as lockf:
            fcntl.flock(lockf.fileno(), fcntl.LOCK_SH)
            try:
                messages = json.loads(inbox_path.read_text())

                unread = [m for m in messages if not m.get("read")]

                if mark_read and unread:
                    for m in messages:
                        m["read"] = True
                    inbox_path.write_text(json.dumps(messages, indent=2))

                return unread
            finally:
                fcntl.flock(lockf.fileno(), fcntl.LOCK_UN)

    async def send_message(
        self,
        from_agent: str,
        to_agent: str,
        content: str,
        team: str = "default",
        message_type: str = "text",
    ) -> dict:
        """
        Send a message from one agent to another.

        Convenience method that wraps the message and writes it.

        Args:
            from_agent: Sender agent name
            to_agent: Recipient agent name
            content: Message content (string)
            team: Team name (default: "default")
            message_type: Type of message (default: "text")

        Returns:
            Delivery status dict with 'success' boolean
        """
        msg = {
            "from": from_agent,
            "type": message_type,
            "content": content,
        }

        await self.write_message(to_agent, msg, team)
        return {"success": True, "queued": True}

    async def get_unread_count(self, agent_name: str, team: str = "default") -> int:
        """
        Get count of unread messages for an agent.

        Args:
            agent_name: Agent to check
            team: Team name (default: "default")

        Returns:
            Count of unread messages
        """
        messages = await self.read_messages(agent_name, team, mark_read=False)
        return len(messages)

    async def clear_inbox(self, agent_name: str, team: str = "default") -> None:
        """
        Clear all messages from an agent's inbox.

        Args:
            agent_name: Agent whose inbox to clear
            team: Team name (default: "default")
        """
        inbox_path = self._inbox_path(agent_name, team)
        lock_path = self._lock_path(agent_name, team)

        if not inbox_path.exists():
            return

        with open(lock_path, "w") as lockf:
            fcntl.flock(lockf.fileno(), fcntl.LOCK_EX)
            try:
                inbox_path.write_text(json.dumps([], indent=2))
            finally:
                fcntl.flock(lockf.fileno(), fcntl.LOCK_UN)

    def list_teams(self) -> list[str]:
        """List all team directories."""
        if not self.base_dir.exists():
            return []
        return [d.name for d in self.base_dir.iterdir() if d.is_dir()]

    def list_agents(self, team: str = "default") -> list[str]:
        """List all agent inboxes in a team."""
        team_dir = self.base_dir / team / "inboxes"
        if not team_dir.exists():
            return []
        return [f.stem for f in team_dir.glob("*.json")]
