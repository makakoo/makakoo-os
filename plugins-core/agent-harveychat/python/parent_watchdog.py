"""Terminate the legacy Python gateway when its Rust supervisor disappears."""

from __future__ import annotations

import os
import signal
import threading
from collections.abc import Callable


def parent_is_alive(
    expected_pid: int,
    *,
    getppid: Callable[[], int] = os.getppid,
    probe: Callable[[int, int], None] = os.kill,
) -> bool:
    if getppid() != expected_pid:
        return False
    try:
        probe(expected_pid, 0)
    except OSError:
        return False
    return True


class ParentWatchdog:
    def __init__(self, stop: threading.Event, thread: threading.Thread):
        self._stop = stop
        self._thread = thread

    def close(self) -> None:
        self._stop.set()
        self._thread.join(timeout=2)


def start_parent_watchdog(
    *,
    interval: float = 1.0,
    getppid: Callable[[], int] = os.getppid,
    probe: Callable[[int, int], None] = os.kill,
    terminate: Callable[[], None] | None = None,
) -> ParentWatchdog:
    """Start a daemon watchdog pinned to the gateway's initial parent."""
    if os.name == "nt":
        # On Windows, os.kill(pid, 0) maps to TerminateProcess and would kill
        # the Rust supervisor; parent-death cleanup is handled Rust-side there.
        stop = threading.Event()
        thread = threading.Thread(target=lambda: None, daemon=True)
        thread.start()
        return ParentWatchdog(stop, thread)
    expected_pid = getppid()
    stop = threading.Event()
    if terminate is None:
        terminate = lambda: os.kill(os.getpid(), signal.SIGTERM)

    def watch() -> None:
        while not stop.wait(interval):
            if not parent_is_alive(expected_pid, getppid=getppid, probe=probe):
                terminate()
                return

    thread = threading.Thread(target=watch, name="makakoo-parent-watchdog", daemon=True)
    thread.start()
    return ParentWatchdog(stop, thread)
