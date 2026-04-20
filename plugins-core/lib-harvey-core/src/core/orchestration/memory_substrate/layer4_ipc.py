"""
Layer 4: IPC / Shared Memory

Agent-to-agent communication via shared memory regions and message queues.
"""
import uuid
import time
from collections import deque
from dataclasses import dataclass, field
from typing import Optional
from .layer3_kalloc import KernelAllocatorLayer, KallocRegion


@dataclass
class SharedRegion:
    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    layer: int = 4
    kalloc_id: str = ""
    size: int = 0
    agent_ids: list = field(default_factory=list)
    created_at: float = field(default_factory=time.time)
    ttl_seconds: int = 0
    pinned: bool = False
    depends_on: list = field(default_factory=list)
    consumed_by: list = field(default_factory=list)
    data: bytes = field(default_factory=bytes)


class MessageQueue:
    """Lightweight agent-to-agent messaging."""

    def __init__(self, ipc: "IPCLayer"):
        self.ipc = ipc
        self.queues: dict[str, deque] = {}

    def send(self, to: str, message: dict):
        """Send a message to an agent's queue."""
        self.queues.setdefault(to, deque()).append(message)

    def receive(self, agent_id: str, timeout: float = 0) -> Optional[dict]:
        """Receive a message from an agent's queue."""
        q = self.queues.get(agent_id)
        if q and q:
            return q.popleft()
        return None


class IPCLayer:
    """Layer 4: Agent-to-agent communication via shared memory."""

    def __init__(self, kalloc: KernelAllocatorLayer):
        self.kalloc = kalloc
        self.shared_regions: dict[str, SharedRegion] = {}
        self.message_queue = MessageQueue(self)

    def create_shared_region(self, size: int, agent_ids: list[str]) -> SharedRegion:
        """Create a shared memory region accessible to listed agents."""
        kalloc_region = self.kalloc.kmalloc(size, owner="|".join(agent_ids))
        region = SharedRegion(
            kalloc_id=kalloc_region.id,
            size=size,
            agent_ids=agent_ids,
        )
        self.shared_regions[region.id] = region
        return region

    def attach(self, region_id: str, agent_id: str) -> Optional[SharedRegion]:
        """Check if agent can attach to a shared region."""
        region = self.shared_regions.get(region_id)
        if not region:
            return None
        if agent_id not in region.agent_ids:
            return None
        return region

    def write_region(self, region_id: str, data: bytes):
        """Write data to a shared region."""
        region = self.shared_regions.get(region_id)
        if region:
            region.data = data[: region.size]

    def read_region(self, region_id: str) -> Optional[bytes]:
        """Read data from a shared region."""
        region = self.shared_regions.get(region_id)
        return region.data if region else None
