"""
Layer 2: Virtual Memory / Address Space

Provides process isolation and address translation via mmap/munmap.
"""
import uuid
import time
from dataclasses import dataclass, field
from typing import Optional
from .layer1_physical import PhysicalMemoryLayer, MemoryRegion


@dataclass
class VirtRegion:
    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    layer: int = 2
    phys_id: str = ""
    size: int = 0
    owner: str = ""  # pid or agent_id
    created_at: float = field(default_factory=time.time)
    ttl_seconds: int = 0
    pinned: bool = False
    depends_on: list = field(default_factory=list)
    consumed_by: list = field(default_factory=list)


class VirtualMemoryLayer:
    """Layer 2: Address space isolation and page tables."""

    def __init__(self, physical: PhysicalMemoryLayer):
        self.physical = physical
        self.page_size = 4096
        self.page_tables: dict[str, dict[str, VirtRegion]] = {}  # pid -> {region_id -> VirtRegion}

    def mmap(self, size: int, pid: str) -> VirtRegion:
        """Map a memory region into an address space."""
        # Allocate from physical layer
        phys_region = self.physical.allocate(size, owner=f"virt:{pid}")
        # Create virtual region
        virt_region = VirtRegion(
            phys_id=phys_region.id,
            size=size,
            owner=pid,
        )
        # Add to page table
        self.page_tables.setdefault(pid, {})[virt_region.id] = virt_region
        return virt_region

    def munmap(self, region_id: str, pid: str) -> bool:
        """Unmap a memory region from an address space."""
        pt = self.page_tables.get(pid, {})
        virt_region = pt.get(region_id)
        if not virt_region:
            return False
        # Free physical region
        self.physical.deallocate(virt_region.phys_id)
        del pt[region_id]
        return True

    def get_virt_regions(self, pid: str) -> list[VirtRegion]:
        """Get all virtual regions for a process."""
        return list(self.page_tables.get(pid, {}).values())

    def get_region(self, region_id: str, pid: str) -> Optional[VirtRegion]:
        return self.page_tables.get(pid, {}).get(region_id)
