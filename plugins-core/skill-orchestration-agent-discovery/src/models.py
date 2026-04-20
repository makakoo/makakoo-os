"""
Agent Discovery Data Models
"""

from dataclasses import dataclass, field, asdict
from datetime import datetime, timezone
from typing import Optional
import json


@dataclass
class AgentRecord:
    """An agent registration record."""

    agent_id: str
    name: str
    capabilities: list[str] = field(default_factory=list)
    skills: list[str] = field(default_factory=list)
    endpoint: str = ""
    metadata: dict = field(default_factory=dict)
    registered_at: str = field(default_factory=lambda: datetime.now(timezone.utc).isoformat())
    lease_expires_at: str = ""
    status: str = "active"

    def to_dict(self) -> dict:
        return asdict(self)

    @classmethod
    def from_dict(cls, data: dict) -> "AgentRecord":
        return cls(**{k: v for k, v in data.items() if k in cls.__dataclass_fields__})

    def to_json(self) -> str:
        return json.dumps(self.to_dict())

    @classmethod
    def from_json(cls, s: str) -> "AgentRecord":
        return cls.from_dict(json.loads(s))

    def is_stale(self) -> bool:
        """Check if the agent's lease has expired."""
        if not self.lease_expires_at:
            return False
        expires = datetime.fromisoformat(self.lease_expires_at.replace("Z", "+00:00"))
        return datetime.now(timezone.utc) > expires

    def refresh(self, ttl_seconds: int = 300) -> None:
        """Refresh the lease."""
        from datetime import timedelta

        self.lease_expires_at = (datetime.now(timezone.utc) + timedelta(seconds=ttl_seconds)).isoformat()
