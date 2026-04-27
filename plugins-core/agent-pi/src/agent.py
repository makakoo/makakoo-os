#!/usr/bin/env python3
"""agent-pi — lifecycle entry point.

Pi is a per-request worker (spawned via `pi --rpc` inside `pi_run` MCP tool
calls), not a long-lived daemon. This file exists so `AgentRunner` can
treat agent-pi like every other kind=agent plugin: `start` is a no-op that
exits 0, `stop` doesn't need to kill anything, and `health` is handled by
the separate `doctor.py`.
"""
from __future__ import annotations

import sys


def _usage() -> int:
    print("usage: agent.py start|stop|status", file=sys.stderr)
    return 2


def main(argv: list[str]) -> int:
    if not argv or argv[0] in ("-h", "--help"):
        return _usage()
    sub = argv[0]
    if sub == "start":
        # No daemon to run — pi --rpc is invoked per MCP request.
        print("agent-pi: ready (stateless, driven by pi_run MCP tool)")
        return 0
    if sub == "stop":
        return 0
    if sub == "status":
        print("agent-pi: stateless adapter — no long-lived process")
        return 0
    return _usage()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
