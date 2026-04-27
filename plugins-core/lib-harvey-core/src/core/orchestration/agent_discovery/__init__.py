"""
Agent Discovery — Dynamic agent registration and discovery service.

Provides:
- Registry store (SQLite) for agent records
- HTTP API server for registration and discovery
- Background heartbeat monitor for stale agent cleanup
- Client SDK for agents to integrate

Usage:
    from agent_discovery import DiscoveryClient

    with DiscoveryClient(
        agent_id="harvey-001",
        name="Harvey Primary",
        capabilities=["reasoning", "planning"],
        skills=["/plan", "/investigate"],
        endpoint="http://localhost:8080",
    ) as client:
        agents = client.find("code-review")
"""

from .models import AgentRecord
from .store import RegistryStore
from .client import DiscoveryClient
from .monitor import HeartbeatMonitor

__all__ = ["AgentRecord", "RegistryStore", "DiscoveryClient", "HeartbeatMonitor"]
