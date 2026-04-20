"""
Shared polling utilities — exponential backoff and stall watchdog.

Ported from openclaw's polling loop. Every channel that polls (Telegram,
Slack RTM, email IMAP) uses these helpers instead of rolling its own.

Parameters tuned in openclaw production:
    initial = 2s     max = 30s     factor = 1.8     jitter = 0.25
    stall threshold = 90s, check every 30s
"""

from __future__ import annotations

import asyncio
import logging
import random
import time
from dataclasses import dataclass
from typing import Callable, Optional

log = logging.getLogger("harvey.chat.polling")


@dataclass
class BackoffConfig:
    initial_seconds: float = 2.0
    max_seconds: float = 30.0
    factor: float = 1.8
    jitter: float = 0.25  # fractional, so ±25% of current delay


class ExponentialBackoff:
    """Stateful exponential backoff with jitter."""

    def __init__(self, config: Optional[BackoffConfig] = None):
        self.config = config or BackoffConfig()
        self._current = self.config.initial_seconds
        self._attempts = 0

    def next_delay(self) -> float:
        """Return the next delay, advance state."""
        delay = self._current
        jitter_amount = delay * self.config.jitter
        jittered = delay + random.uniform(-jitter_amount, jitter_amount)
        self._attempts += 1
        self._current = min(self._current * self.config.factor, self.config.max_seconds)
        return max(0.0, jittered)

    def reset(self) -> None:
        """Call on successful operation."""
        self._current = self.config.initial_seconds
        self._attempts = 0

    @property
    def attempts(self) -> int:
        return self._attempts


class StallWatchdog:
    """Watches a poll loop for silent stalls.

    Channels call `heartbeat()` every time they successfully receive or
    attempt to receive messages. An async task runs in the background and
    fires `on_stall` if no heartbeat arrives within `threshold_seconds`.

    This catches the case where a poll loop is stuck in a weird state
    (e.g. awaiting an HTTP response that will never arrive) without
    raising an exception.
    """

    def __init__(
        self,
        threshold_seconds: float = 90.0,
        check_interval_seconds: float = 30.0,
        on_stall: Optional[Callable[[], None]] = None,
    ):
        self.threshold = threshold_seconds
        self.check_interval = check_interval_seconds
        self.on_stall = on_stall
        self._last_heartbeat: float = time.time()
        self._task: Optional[asyncio.Task] = None
        self._stopped = False

    def heartbeat(self) -> None:
        self._last_heartbeat = time.time()

    async def _watch(self) -> None:
        while not self._stopped:
            await asyncio.sleep(self.check_interval)
            if self._stopped:
                break
            idle = time.time() - self._last_heartbeat
            if idle > self.threshold:
                log.warning(
                    f"StallWatchdog: idle {idle:.0f}s > threshold {self.threshold:.0f}s — firing stall handler"
                )
                if self.on_stall is not None:
                    try:
                        self.on_stall()
                    except Exception as e:
                        log.error(f"on_stall handler crashed: {e}", exc_info=True)
                # Reset so we don't spam
                self._last_heartbeat = time.time()

    def start(self) -> None:
        if self._task is not None:
            return
        self._stopped = False
        self._last_heartbeat = time.time()
        loop = asyncio.get_event_loop()
        self._task = loop.create_task(self._watch())

    def stop(self) -> None:
        self._stopped = True
        if self._task is not None:
            self._task.cancel()
            self._task = None

    @property
    def idle_seconds(self) -> float:
        return time.time() - self._last_heartbeat
