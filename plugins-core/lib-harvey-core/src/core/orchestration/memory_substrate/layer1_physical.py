"""
Layer 1: Physical/Hardware Memory Abstraction

Tracks raw memory resources and hardware interfaces.
"""
import uuid
import time
from dataclasses import dataclass, field
from typing import Optional


@dataclass
class MemoryRegion:
    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    layer: int = 1
    base_addr: int = 0
    size: int = 0
    owner: str = "KERNEL"
    created_at: float = field(default_factory=time.time)
    ttl_seconds: int = 0
    pinned: bool = False
    depends_on: list = field(default_factory=list)
    consumed_by: list = field(default_factory=list)


class PhysicalMemoryLayer:
    """Layer 1: Raw memory resource tracking."""

    def __init__(self):
        self.regions: dict[str, MemoryRegion] = {}
        self.total_bytes: int = 0
        self.free_bytes: int = 0

    def allocate(self, size: int, owner: str = "KERNEL") -> MemoryRegion:
        """Allocate a physical memory region."""
        region = MemoryRegion(
            size=size,
            owner=owner,
            base_addr=id(region),  # pseudo-address
        )
        self.regions[region.id] = region
        self.total_bytes += size
        self.free_bytes += size
        return region

    def deallocate(self, region_id: str) -> bool:
        """Free a physical memory region."""
        region = self.regions.get(region_id)
        if not region:
            return False
        self.free_bytes -= region.size
        self.total_bytes -= region.size
        del self.regions[region_id]
        return True

    def get_info(self) -> dict:
        """Return memory usage info."""
        return {
            "total_bytes": self.total_bytes,
            "free_bytes": self.free_bytes,
            "region_count": len(self.regions),
        }

    def get_region(self, region_id: str) -> Optional[MemoryRegion]:
        return self.regions.get(region_id)
