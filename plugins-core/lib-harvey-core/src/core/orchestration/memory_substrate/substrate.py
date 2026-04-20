"""
Memory Substrate — Unified 6-layer facade

Coordinates all 6 memory layers into a single interface.
"""
from .layer1_physical import PhysicalMemoryLayer
from .layer2_virtual import VirtualMemoryLayer
from .layer3_kalloc import KernelAllocatorLayer
from .layer4_ipc import IPCLayer, MessageQueue
from .layer5_session import SessionLayer
from .layer6_artifact import ArtifactLayer, Artifact
from .task_graph import TaskGraphLayer
from .dependency_graph import DependencyGraph


class MemorySubstrate:
    """Unified 6-layer memory substrate."""

    def __init__(self):
        self.phys = PhysicalMemoryLayer()
        self.virt = VirtualMemoryLayer(self.phys)
        self.kalloc = KernelAllocatorLayer(self.virt)
        self.ipc = IPCLayer(self.kalloc)
        self.session = SessionLayer(self.ipc)
        self.artifact = ArtifactLayer(self.session)
        self.task_graph = TaskGraphLayer()
        self.dep_graph = DependencyGraph()

        # Set current session for artifact creation
        self.session.current_session_id = "system"

    def allocate_for_agent(self, agent_id: str, size: int) -> str:
        """Allocate a memory region for an agent. Returns region_id."""
        region = self.kalloc.kmalloc(size, owner=agent_id)
        self.session.add_region_to_session(agent_id, region.id)
        return region.id

    def publish_artifact(
        self,
        name: str,
        content: str,
        agent_id: str,
        depends_on: list[str] = None,
        ttl_seconds: int = 86400,
        pinned: bool = False,
    ) -> Artifact:
        """Publish an artifact to the registry."""
        # Add dependency edges
        art = self.artifact.create_artifact(
            name=name,
            content=content,
            producer=agent_id,
            depends_on=depends_on,
            ttl_seconds=ttl_seconds,
            pinned=pinned,
        )
        if depends_on:
            self.dep_graph.add(art.id, depends_on)
        return art

    def get_artifact(self, artifact_id: str):
        """Retrieve an artifact by ID."""
        return self.artifact.get_artifact(artifact_id)

    def gc(self) -> int:
        """Run garbage collection. Returns count of artifacts removed."""
        return self.artifact.gc_artifacts()

    def get_info(self) -> dict:
        """Get status of all layers."""
        return {
            "layer1_physical": self.phys.get_info(),
            "layer3_kalloc": self.kalloc.get_info(),
            "layer5_sessions": len(self.session.active_sessions),
            "layer6_artifacts": len(self.artifact._cache),
        }
