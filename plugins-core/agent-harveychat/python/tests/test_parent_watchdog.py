"""Tests for legacy Python gateway parent-death handling."""

from __future__ import annotations

import os
import threading

from plugins_core.agent_harveychat.python.parent_watchdog import (
    parent_is_alive,
    start_parent_watchdog,
)


def test_parent_identity_and_liveness_are_both_required():
    assert parent_is_alive(42, getppid=lambda: 42, probe=lambda _pid, _sig: None)
    assert not parent_is_alive(42, getppid=lambda: 99, probe=lambda _pid, _sig: None)

    def missing(_pid: int, _sig: int) -> None:
        raise ProcessLookupError

    assert not parent_is_alive(42, getppid=lambda: 42, probe=missing)


def test_watchdog_terminates_after_reparent():
    parent = [42]
    terminated = threading.Event()
    watchdog = start_parent_watchdog(
        interval=0.001,
        getppid=lambda: parent[0],
        probe=lambda _pid, _sig: None,
        terminate=terminated.set,
    )
    parent[0] = 99
    assert terminated.wait(1)
    watchdog.close()


def test_watchdog_is_disabled_on_windows(monkeypatch):
    # On win32, os.kill(pid, 0) is TerminateProcess, so the watchdog must not run.
    monkeypatch.setattr(os, "name", "nt")
    watchdog = start_parent_watchdog(interval=0.001)
    watchdog.close()
    assert not any(t.name == "makakoo-parent-watchdog" for t in threading.enumerate())
