"""
Agent Discovery Client SDK
"""

import requests
import threading
import time
import atexit
from typing import Optional, List

from .models import AgentRecord


class DiscoveryClient:
    """
    Client SDK for the Agent Discovery service.
    Supports context-manager usage for automatic registration/cleanup.
    """

    def __init__(
        self,
        agent_id: str,
        name: str,
        capabilities: Optional[List[str]] = None,
        skills: Optional[List[str]] = None,
        endpoint: str = "",
        metadata: Optional[dict] = None,
        base_url: str = "http://localhost:18081",
        ttl_seconds: int = 300,
        heartbeat_interval: int = 60,
    ):
        self.agent_id = agent_id
        self.name = name
        self.capabilities = capabilities or []
        self.skills = skills or []
        self.endpoint = endpoint
        self.metadata = metadata or {}
        self.base_url = base_url.rstrip("/")
        self.ttl_seconds = ttl_seconds
        self.heartbeat_interval = heartbeat_interval

        self._session = requests.Session()
        self._heartbeat_thread: Optional[threading.Thread] = None
        self._stop_heartbeat = threading.Event()
        self._registered = False

    def register(self) -> bool:
        """Register this agent with the discovery service."""
        try:
            resp = self._session.post(
                f"{self.base_url}/agents/register",
                json={
                    "agent_id": self.agent_id,
                    "name": self.name,
                    "capabilities": self.capabilities,
                    "skills": self.skills,
                    "endpoint": self.endpoint,
                    "metadata": self.metadata,
                    "ttl_seconds": self.ttl_seconds,
                },
                timeout=5,
            )
            resp.raise_for_status()
            self._registered = True
            return True
        except requests.RequestException as e:
            print(f"Registration failed: {e}")
            return False

    def heartbeat(self, ttl: Optional[int] = None) -> bool:
        """Send a heartbeat to refresh the lease."""
        try:
            resp = self._session.post(
                f"{self.base_url}/agents/heartbeat/{self.agent_id}",
                json={"ttl_seconds": ttl or self.ttl_seconds},
                timeout=5,
            )
            resp.raise_for_status()
            return True
        except requests.RequestException:
            return False

    def deregister(self) -> bool:
        """Deregister this agent."""
        try:
            resp = self._session.delete(
                f"{self.base_url}/agents/{self.agent_id}",
                timeout=5,
            )
            resp.raise_for_status()
            self._registered = False
            return True
        except requests.RequestException:
            return False

    def find(self, capability: str) -> List[AgentRecord]:
        """Find agents with a specific capability."""
        try:
            resp = self._session.get(
                f"{self.base_url}/agents",
                params={"capability": capability},
                timeout=5,
            )
            resp.raise_for_status()
            data = resp.json()
            return [AgentRecord.from_dict(a) for a in data.get("agents", [])]
        except requests.RequestException:
            return []

    def find_by_skill(self, skill: str) -> List[AgentRecord]:
        """Find agents with a specific skill."""
        return self.find(f"skill:{skill}")

    def get_all(self) -> List[AgentRecord]:
        """Get all registered agents."""
        try:
            resp = self._session.get(f"{self.base_url}/agents", timeout=5)
            resp.raise_for_status()
            data = resp.json()
            return [AgentRecord.from_dict(a) for a in data.get("agents", [])]
        except requests.RequestException:
            return []

    def get_one(self, agent_id: str) -> Optional[AgentRecord]:
        """Get a specific agent by ID."""
        try:
            resp = self._session.get(
                f"{self.base_url}/agents/{agent_id}",
                timeout=5,
            )
            resp.raise_for_status()
            return AgentRecord.from_dict(resp.json())
        except requests.RequestException:
            return None

    def health_stats(self) -> dict:
        """Get health statistics."""
        try:
            resp = self._session.get(
                f"{self.base_url}/agents/health",
                timeout=5,
            )
            resp.raise_for_status()
            return resp.json()
        except requests.RequestException:
            return {"error": "unreachable"}

    def _heartbeat_loop(self) -> None:
        while not self._stop_heartbeat.wait(self.heartbeat_interval):
            if not self.heartbeat():
                break

    def start_heartbeat(self) -> None:
        """Start background heartbeat thread."""
        self._stop_heartbeat.clear()
        self._heartbeat_thread = threading.Thread(
            target=self._heartbeat_loop,
            daemon=True,
        )
        self._heartbeat_thread.start()

    def stop_heartbeat(self) -> None:
        """Stop background heartbeat thread."""
        self._stop_heartbeat.set()
        if self._heartbeat_thread:
            self._heartbeat_thread.join(timeout=5)

    def __enter__(self):
        self.register()
        self.start_heartbeat()
        atexit.register(self._cleanup)
        return self

    def __exit__(self, *args):
        self._cleanup()

    def _cleanup(self) -> None:
        self.stop_heartbeat()
        if self._registered:
            self.deregister()
