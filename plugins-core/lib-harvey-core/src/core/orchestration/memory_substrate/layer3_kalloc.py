"""
Layer 3: Kernel Allocator

Slab-style allocator for fixed-size kernel objects.
"""
import uuid
import time
from dataclasses import dataclass, field
from typing import Optional
from .layer2_virtual import VirtualMemoryLayer, VirtRegion


@dataclass
class KallocRegion:
    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    layer: int = 3
    virt_id: str = ""
    size: int = 0
    owner: str = ""
    created_at: float = field(default_factory=time.time)
    ttl_seconds: int = 0
    pinned: bool = False
    depends_on: list = field(default_factory=list)
    consumed_by: list = field(default_factory=list)
    in_use: bool = False


class KernelAllocatorLayer:
    """Layer 3: Slab allocator for kernel objects."""

    SIZE_CLASSES = [32, 64, 128, 256, 512, 1024, 4096]

    def __init__(self, virtual: VirtualMemoryLayer):
        self.virtual = virtual
        # slabs[size_class] = list of KallocRegions
        self.slabs: dict[int, list[KallocRegion]] = {sz: [] for sz in self.SIZE_CLASSES}
        self._size_map: dict[str, int] = {}  # region_id -> size_class

    def _nearest_size_class(self, size: int) -> int:
        for sc in self.SIZE_CLASSES:
            if sc >= size:
                return sc
        return self.SIZE_CLASSES[-1]

    def kmalloc(self, size: int, owner: str = "") -> KallocRegion:
        """Allocate a kernel object of given size."""
        sc = self._nearest_size_class(size)
        # Try to reuse from slab
        for region in self.slabs[sc]:
            if not region.in_use:
                region.in_use = True
                region.owner = owner
                self._size_map[region.id] = sc
                return region
        # Allocate new from virtual layer
        virt_region = self.virtual.mmap(sc, pid=f"kalloc:{owner}")
        kalloc_region = KallocRegion(
            virt_id=virt_region.id,
            size=sc,
            owner=owner,
        )
        kalloc_region.in_use = True
        self.slabs[sc].append(kalloc_region)
        self._size_map[kalloc_region.id] = sc
        return kalloc_region

    def kfree(self, region_id: str) -> bool:
        """Free a kernel object (return to slab)."""
        sc = self._size_map.get(region_id)
        if sc is None:
            return False
        for region in self.slabs[sc]:
            if region.id == region_id:
                region.in_use = False
                region.owner = ""
                return True
        return False

    def get_info(self) -> dict:
        """Return allocator stats per size class."""
        return {
            sc: {
                "total": len(self.slabs[sc]),
                "in_use": sum(1 for r in self.slabs[sc] if r.in_use),
            }
            for sc in self.SIZE_CLASSES
        }
