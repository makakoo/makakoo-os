"""
FileStateCache — LRU Cache for File Contents (Claude Code Pattern)

Thread-safe, LRU cache that stores file content by path with mtime validation.

Key features (from Claude Code's fileStateCache.ts):
- Configurable max entries (default 1000) and max bytes (default 25MB)
- mtime validation — re-reads file if it changed on disk
- Thread-safe via threading.RLock
- Stats tracking: hits, misses, evictions, hit_rate

Integration points:
- plugins-core/lib-harvey-core/src/core/registry/skill_registry.py (SKILL.md reads)
- plugins-core/lib-harvey-core/src/core/memory/memory_loader.py (Brain page reads)
- plugins-core/lib-harvey-core/src/core/memory/logseq_bridge.py (journal reads via filesystem)

Usage:
    from harvey_os.core.indexing.file_state_cache import FileStateCache, get_file_cache

    cache = get_file_cache()
    content = cache.read("/path/to/file.txt")  # cached read
    stats = cache.stats()
"""

from __future__ import annotations

import threading
import time
from collections import OrderedDict
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

# ---------------------------------------------------------------------------
# Types
# ---------------------------------------------------------------------------


@dataclass
class CacheEntry:
    """A cached file entry with metadata."""

    path: str
    content: bytes
    mtime: float
    last_access: float
    size: int


# ---------------------------------------------------------------------------
# Cache Implementation
# ---------------------------------------------------------------------------


class FileStateCache:
    """
    LRU cache for file contents with mtime validation.

    Features:
    - Thread-safe (RLock)
    - mtime validation (re-reads file if disk content changed)
    - Configurable max entries + max bytes
    - Stats tracking: hits, misses, evictions, hit_rate
    - O(1) get/put via OrderedDict
    """

    def __init__(
        self,
        max_entries: int = 1000,
        max_bytes: int = 25 * 1024 * 1024,  # 25MB
    ):
        self.max_entries = max_entries
        self.max_bytes = max_bytes
        self._cache: OrderedDict[str, CacheEntry] = OrderedDict()
        self._lock = threading.RLock()
        self._stats = {"hits": 0, "misses": 0, "evictions": 0}

    # ------------------------------------------------------------------
    # Core API
    # ------------------------------------------------------------------

    def get(self, path: str) -> bytes | None:
        """
        Get cached file content if valid (mtime matches).

        Returns None if:
        - Not in cache
        - File was modified (mtime changed)
        - File was deleted/inaccessible

        On a valid hit, moves entry to end (most recently used).
        """
        with self._lock:
            entry = self._cache.get(path)
            if entry is None:
                self._stats["misses"] += 1
                return None

            # Validate mtime — re-read if file changed
            try:
                current_mtime = Path(path).stat().st_mtime
                if current_mtime != entry.mtime:
                    del self._cache[path]
                    self._stats["misses"] += 1
                    return None
            except OSError:
                # File deleted or inaccessible — invalidate
                self._cache.pop(path, None)
                self._stats["misses"] += 1
                return None

            # Move to end (mark as most recently used)
            self._cache.move_to_end(path)
            entry.last_access = time.time()
            self._stats["hits"] += 1
            return entry.content

    def put(self, path: str, content: bytes) -> None:
        """
        Cache file content with current mtime.

        Evicts LRU entries if over max_entries or max_bytes limits.
        """
        with self._lock:
            # Remove existing entry if present (to update position)
            if path in self._cache:
                del self._cache[path]

            # Get current mtime
            try:
                mtime = Path(path).stat().st_mtime
            except OSError:
                mtime = time.time()

            entry = CacheEntry(
                path=path,
                content=content,
                mtime=mtime,
                last_access=time.time(),
                size=len(content),
            )

            # Evict until within limits
            while (
                len(self._cache) >= self.max_entries
                or self._total_bytes() + entry.size > self.max_bytes
            ):
                if not self._cache:
                    break
                self._cache.popitem(last=False)  # Pop oldest (LRU)
                self._stats["evictions"] += 1

            self._cache[path] = entry

    def read(self, path: str) -> bytes:
        """
        Read file — from cache if valid, else from disk + cache.

        This is the main entry point for callers.
        """
        content = self.get(path)
        if content is None:
            content = Path(path).read_bytes()
            self.put(path, content)
        return content

    def invalidate(self, path: str) -> None:
        """Manually invalidate cache entry for path."""
        with self._lock:
            self._cache.pop(path, None)

    def invalidate_matching(self, predicate: Callable[[str], bool]) -> int:
        """
        Invalidate all entries matching a predicate.

        Useful when a directory changes, e.g.:
            cache.invalidate_matching(lambda p: p.startswith("/skills"))

        Returns number of entries invalidated.
        """
        with self._lock:
            to_remove = [p for p in self._cache if predicate(p)]
            for p in to_remove:
                self._cache.pop(p, None)
            return len(to_remove)

    def stats(self) -> dict:
        """Return cache statistics."""
        with self._lock:
            total = self._stats["hits"] + self._stats["misses"]
            hit_rate = self._stats["hits"] / total if total > 0 else 0.0
            return {
                "hits": self._stats["hits"],
                "misses": self._stats["misses"],
                "evictions": self._stats["evictions"],
                "entries": len(self._cache),
                "bytes": self._total_bytes(),
                "hit_rate": round(hit_rate, 4),
            }

    def clear(self) -> None:
        """Clear all cache entries and reset stats."""
        with self._lock:
            self._cache.clear()
            self._stats = {"hits": 0, "misses": 0, "evictions": 0}

    # ------------------------------------------------------------------
    # Internals
    # ------------------------------------------------------------------

    def _total_bytes(self) -> int:
        return sum(e.size for e in self._cache.values())


# ---------------------------------------------------------------------------
# Global Singleton
# ---------------------------------------------------------------------------

_global_cache: FileStateCache | None = None
_cache_lock = threading.Lock()


def get_file_cache() -> FileStateCache:
    """
    Get the global FileStateCache singleton.

    Lazily initialized with default settings.
    Callers can replace _global_cache with a custom instance if needed.
    """
    global _global_cache
    if _global_cache is None:
        with _cache_lock:
            if _global_cache is None:
                _global_cache = FileStateCache()
    return _global_cache


def cached_read(path: str) -> bytes:
    """
    Convenience function — read a file with LRU caching.

    Equivalent to get_file_cache().read(path).
    """
    return get_file_cache().read(path)


# ---------------------------------------------------------------------------
# Integration: Wire into skill_registry for repeated SKILL.md reads
# ---------------------------------------------------------------------------


def patch_skill_registry() -> None:
    """
    Monkey-patch skill_registry to use FileStateCache for SKILL.md reads.

    Call once at startup to enable caching on skill indexer.
    """
    import harvey_os.core.registry.skill_registry as sr

    _cache = get_file_cache()

    _orig_get_skills_mtime = sr.SkillRegistry._get_skills_mtime

    def _patched_get_skills_mtime(self: sr.SkillRegistry) -> float:
        """Use cached stat for all SKILL.md files."""
        max_mtime = 0.0
        for skill_path in self.skills_dir.rglob("SKILL.md"):
            if "_registry" in skill_path.parts:
                continue
            try:
                # Use cached read to avoid repeated stat calls
                p_str = str(skill_path)
                if _cache.get(p_str) is None:
                    # Cache miss — stat and cache
                    mtime = skill_path.stat().st_mtime
                    _cache.put(p_str, str(mtime).encode())
                else:
                    # Cache hit — decode cached mtime
                    cached_val = _cache.get(p_str)
                    if cached_val:
                        mtime = float(cached_val.decode())
                    else:
                        mtime = 0.0
                if mtime > max_mtime:
                    max_mtime = mtime
            except OSError:
                pass
        return max_mtime

    sr.SkillRegistry._get_skills_mtime = _patched_get_skills_mtime


# ---------------------------------------------------------------------------
# CLI / Tests
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    import tempfile

    # ---- Test 1: LRU eviction ----
    print("Test 1: LRU eviction")
    cache = FileStateCache(max_entries=3, max_bytes=1000)
    cache.put("a", b"a" * 100)
    cache.put("b", b"b" * 100)
    cache.put("c", b"c" * 100)
    assert len(cache._cache) == 3, f"Expected 3, got {len(cache._cache)}"
    cache.put("d", b"d" * 100)  # Should evict 'a'
    assert "a" not in cache._cache, "LRU eviction failed — 'a' should be evicted"
    print(f"  PASS: cache has {len(cache._cache)} entries (a evicted)")

    # ---- Test 2: mtime invalidation ----
    print("\nTest 2: mtime invalidation")
    with tempfile.NamedTemporaryFile(delete=False) as f:
        f.write(b"original")
        path = f.name

    cache2 = FileStateCache()
    content = cache2.read(path)
    assert content == b"original", "Initial read failed"
    assert cache2.get(path) == b"original", "Cache get after read failed"

    # Modify file
    Path(path).write_bytes(b"modified")
    assert cache2.get(path) is None, "mtime invalidation failed — should return None"

    Path(path).unlink(missing_ok=True)
    print("  PASS: mtime invalidation works")

    # ---- Test 3: Stats tracking ----
    print("\nTest 3: Stats tracking")
    with tempfile.NamedTemporaryFile(delete=False) as f:
        f.write(b"stats-test")
        path2 = f.name

    cache3 = FileStateCache()
    cache3.read(path2)  # miss + put
    cache3.read(path2)  # hit
    cache3.read(path2)  # hit
    stats = cache3.stats()
    assert stats["misses"] == 1, f"Expected 1 miss, got {stats['misses']}"
    assert stats["hits"] == 2, f"Expected 2 hits, got {stats['hits']}"
    print(
        f"  PASS: hit_rate={stats['hit_rate']}, hits={stats['hits']}, misses={stats['misses']}"
    )

    Path(path2).unlink(missing_ok=True)

    # ---- Test 4: Global singleton ----
    print("\nTest 4: Global singleton")
    c1 = get_file_cache()
    c2 = get_file_cache()
    assert c1 is c2, "get_file_cache should return singleton"
    print("  PASS: singleton works")

    print("\n✅ All FileStateCache tests passed")
