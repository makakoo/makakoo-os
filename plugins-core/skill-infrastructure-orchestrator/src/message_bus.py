"""
Message bus for inter-agent communication.
File-based message passing with inbox/outbox semantics.
"""

import json
import os
import time
import uuid
from pathlib import Path
from typing import Optional


def _get_current_agent_id() -> str:
    """Get current agent ID from environment."""
    return os.environ.get("HARVEY_AGENT_ID", "orchestrator")


def _iso_now() -> str:
    """Return current UTC time in ISO8601 format."""
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


class MessageBus:
    """
    File-based message passing system.
    Each agent has an inbox and outbox directory.
    """

    def __init__(self, base_path: str = None):
        if base_path is None:
            import os
            _harvey_home = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
            base_path = os.path.join(_harvey_home, "data", "orchestrator", "messages")
        self.base = Path(base_path)

    def _ensure_agent_dirs(self, agent_id: str) -> tuple[Path, Path]:
        """Ensure agent inbox/outbox exist and return paths."""
        inbox = self.base / agent_id / "inbox"
        outbox = self.base / agent_id / "outbox"
        inbox.mkdir(parents=True, exist_ok=True)
        outbox.mkdir(parents=True, exist_ok=True)
        return inbox, outbox

    def _atomic_write(self, path: Path, data: dict) -> None:
        """Atomic write: temp file + rename."""
        tmp = path.with_suffix(".tmp")
        tmp.write_text(json.dumps(data, indent=2))
        tmp.rename(path)

    def send(
        self,
        to_agent: str,
        msg_type: str,
        payload: dict,
        from_agent: Optional[str] = None,
        reply_to: Optional[str] = None,
    ) -> str:
        """
        Write message to recipient's inbox.
        Returns the msg_id.
        """
        msg_id = str(uuid.uuid4())
        inbox, _ = self._ensure_agent_dirs(to_agent)

        msg = {
            "msg_id": msg_id,
            "from_agent": from_agent or _get_current_agent_id(),
            "to_agent": to_agent,
            "type": msg_type,
            "payload": payload,
            "reply_to": reply_to,
            "created_at": _iso_now(),
        }

        msg_path = inbox / f"{msg_id}.json"
        self._atomic_write(msg_path, msg)
        return msg_id

    def receive(self, agent_id: str, timeout: float = 30) -> Optional[dict]:
        """
        Block until message available or timeout.
        Returns the message dict or None if timeout.
        """
        inbox, _ = self._ensure_agent_dirs(agent_id)
        deadline = time.time() + timeout

        while time.time() < deadline:
            # Sort by modification time to get oldest first
            messages = sorted(
                inbox.glob("*.json"),
                key=lambda p: p.stat().st_mtime,
            )
            for msg_file in messages:
                if msg_file.suffix == ".tmp":
                    continue
                try:
                    msg = json.loads(msg_file.read_text())
                    msg_file.unlink()  # Remove after successful read
                    return msg
                except (json.JSONDecodeError, OSError):
                    continue

            time.sleep(0.5)

        return None

    def peek(self, agent_id: str) -> Optional[dict]:
        """
        Non-blocking: read oldest message without removing it.
        Returns the message dict or None if no messages.
        """
        inbox, _ = self._ensure_agent_dirs(agent_id)
        messages = sorted(inbox.glob("*.json"), key=lambda p: p.stat().st_mtime)

        for msg_file in messages:
            if msg_file.suffix == ".tmp":
                continue
            try:
                return json.loads(msg_file.read_text())
            except (json.JSONDecodeError, OSError):
                continue

        return None

    def broadcast(self, msg_type: str, payload: dict, from_agent: Optional[str] = None) -> list[str]:
        """
        Send message to all registered agents.
        Returns list of agent_ids that received the message.
        """
        msg_ids = []
        for agent_dir in self.base.iterdir():
            if agent_dir.is_dir():
                agent_id = agent_dir.name
                msg_id = self.send(agent_id, msg_type, payload, from_agent)
                msg_ids.append(msg_id)
        return msg_ids

    def send_result(
        self,
        to_agent: str,
        task_id: str,
        result: dict,
        from_agent: Optional[str] = None,
    ) -> str:
        """Convenience: send a result message."""
        return self.send(
            to_agent,
            "result",
            {"task_id": task_id, "result": result},
            from_agent,
        )

    def send_dependency_ready(
        self,
        to_agent: str,
        task_id: str,
        dependency_id: str,
        from_agent: Optional[str] = None,
    ) -> str:
        """Convenience: notify agent that a dependency is ready."""
        return self.send(
            to_agent,
            "dependency_ready",
            {"task_id": task_id, "dependency_id": dependency_id},
            from_agent,
        )

    def send_error(
        self,
        to_agent: str,
        task_id: str,
        error: str,
        from_agent: Optional[str] = None,
    ) -> str:
        """Convenience: send an error message."""
        return self.send(
            to_agent,
            "error",
            {"task_id": task_id, "error": error},
            from_agent,
        )

    def list_agents(self) -> list[str]:
        """List all registered agent IDs."""
        return [d.name for d in self.base.iterdir() if d.is_dir()]

    def pending_count(self, agent_id: str) -> int:
        """Return number of pending messages for an agent."""
        inbox, _ = self._ensure_agent_dirs(agent_id)
        return len(list(inbox.glob("*.json")))
