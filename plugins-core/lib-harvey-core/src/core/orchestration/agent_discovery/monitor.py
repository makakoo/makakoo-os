"""
Agent Discovery Monitor — Background heartbeat checker
"""

import threading
import time
import logging
from typing import Optional

logger = logging.getLogger("agent_discovery.monitor")


class HeartbeatMonitor:
    """Background thread that removes stale agents from the registry."""

    def __init__(self, store, interval_seconds: int = 30):
        self.store = store
        self.interval = interval_seconds
        self._stop = threading.Event()
        self._thread: Optional[threading.Thread] = None

    def start(self) -> None:
        self._stop.clear()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()
        logger.info(f"Heartbeat monitor started (interval={self.interval}s)")

    def stop(self) -> None:
        self._stop.set()
        if self._thread:
            self._thread.join(timeout=5)
        logger.info("Heartbeat monitor stopped")

    def _run(self) -> None:
        while not self._stop.wait(self.interval):
            try:
                stale = self.store.list_stale()
                if stale:
                    count = self.store.delete_stale()
                    for agent in stale:
                        logger.info(f"Agent expired: {agent.agent_id}")
                    logger.info(f"Removed {count} stale agent(s)")
            except Exception as e:
                logger.error(f"Monitor error: {e}")

    def trigger_check(self) -> int:
        """Manually trigger a stale check. Returns count of removed agents."""
        stale = self.store.list_stale()
        return self.store.delete_stale()
