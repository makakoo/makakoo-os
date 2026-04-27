"""
Harvey OS Coordinator — Multi-agent swarm orchestration.

4-phase pipeline: Research -> Synthesis -> Implementation -> Verification.
Anti-lazy delegation ensures structured handoffs between workers.

Usage:
    from core.coordinator import Coordinator

    coord = Coordinator()
    task = coord.execute("Build a REST API for user management")
    print(task.status, task.workers)
"""

from core.coordinator.coordinator import Coordinator, SwarmTask
from core.coordinator.scratchpad import Scratchpad
from core.coordinator.worker import Worker, WorkerRole

__all__ = [
    "Coordinator",
    "SwarmTask",
    "Worker",
    "WorkerRole",
    "Scratchpad",
]
