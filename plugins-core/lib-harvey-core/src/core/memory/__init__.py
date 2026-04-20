"""Harvey OS Core — Memory subsystem (Brain bridge (filesystem + optional Logseq API) + retrieval + frozen snapshot)

Frozen Snapshot Memory (frozen_memory.py):
    FrozenSnapshot MemoryStore that persists across sessions but keeps the system
    prompt stable by capturing a snapshot at load time. Mid-session writes go to
    disk but do NOT mutate the snapshot.

    - Memory: 2200 char limit (agent's personal notes)
    - User: 1375 char limit (user profile)
    - Injection/exfiltration scanning
    - Atomic writes via temp file + os.replace()

Usage:
    from core.memory.frozen_memory import MemoryStore, _scan_memory_content
    from core.memory.memory_tool import memory_tool, MEMORY_SCHEMA
"""

from .frozen_memory import MemoryStore, _scan_memory_content
from .memory_tool import memory_tool, MEMORY_SCHEMA

__all__ = ["MemoryStore", "_scan_memory_content", "memory_tool", "MEMORY_SCHEMA"]

# Auto-Memory System: Initialize router and indexer on import
# This enables autonomous fact capture to Brain journal + Superbrain + vector indexing
try:
    from .auto_memory_router import AutoMemoryRouter
    _auto_memory_router = AutoMemoryRouter()
    _auto_memory_router.start()
except ImportError as e:
    import logging
    logging.getLogger("harvey.memory").warning(f"Could not import auto-memory router: {e}")
except Exception as e:
    import logging
    logging.getLogger("harvey.memory").warning(f"Could not start auto-memory router: {e}")

try:
    from .auto_memory_indexer import AutoMemoryIndexer
    _auto_memory_indexer = AutoMemoryIndexer()
    _auto_memory_indexer.start()
except ImportError as e:
    import logging
    logging.getLogger("harvey.memory").warning(f"Could not import auto-memory indexer: {e}")
except Exception as e:
    import logging
    logging.getLogger("harvey.memory").warning(f"Could not start auto-memory indexer: {e}")
