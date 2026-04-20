#!/usr/bin/env python3
"""
HarveyChat plugin agent entry point.

Thin wrapper that lets AgentRunner start HarveyChat the same way it starts
any other plugin agent — by spawning `python agent.py` in the agent's
workdir. Under the hood it delegates to the existing `core.chat` CLI, so
`python agents/harveychat/agent.py start --daemon` behaves identically to
`python -m core.chat start --daemon`.

The Phase 8 manifest at agents/harveychat/agent.yaml declares:
  runtime:
    command: python3
    entrypoint: agent.py
    workdir: agents/harveychat
    args: ["start", "--daemon"]

So `AgentRunner` (Phase 9) can spawn this agent the same way it spawns
any other plugin — `cd agents/harveychat && python3 agent.py start
--daemon`. No special cases, no package-path knowledge required. This
file is the concrete entry point Phase 9's auto_pilot + AgentRunner
reference.

Usage:
    python agents/harveychat/agent.py start           # foreground
    python agents/harveychat/agent.py start --daemon  # background
    python agents/harveychat/agent.py stop
    python agents/harveychat/agent.py status
"""

import os
import sys


def _bootstrap_harvey_home() -> str:
    """Ensure HARVEY_HOME is set + harvey-os is on sys.path before imports."""
    harvey_home = os.environ.get("HARVEY_HOME")
    if not harvey_home:
        # Default: walk up from this file until we find CLAUDE.md at the root
        here = os.path.abspath(os.path.dirname(__file__))
        candidate = here
        for _ in range(6):
            if os.path.exists(os.path.join(candidate, "CLAUDE.md")):
                harvey_home = candidate
                break
            parent = os.path.dirname(candidate)
            if parent == candidate:
                break
            candidate = parent
        if not harvey_home:
            harvey_home = os.path.expanduser("~/MAKAKOO")
        os.environ["HARVEY_HOME"] = harvey_home

    harvey_os = os.path.join(harvey_home, "harvey-os")
    if harvey_os not in sys.path:
        sys.path.insert(0, harvey_os)
    return harvey_home


def main() -> int:
    """Entry point — delegate to core.chat.__main__.main()."""
    _bootstrap_harvey_home()
    try:
        from core.chat.__main__ import main as chat_main
    except Exception as e:
        print(f"harveychat: failed to import core.chat: {e}", file=sys.stderr)
        return 2

    # core.chat.__main__.main() parses sys.argv[1:], so passing through
    # naturally preserves all CLI args
    try:
        return chat_main() or 0
    except SystemExit as e:
        return int(e.code) if e.code is not None else 0
    except Exception as e:
        print(f"harveychat: crashed: {type(e).__name__}: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
