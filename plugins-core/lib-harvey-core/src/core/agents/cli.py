"""
`harvey agents` CLI — bridges the Phase 5 AgentRegistry (manifest discovery)
to the existing AgentLifecycle (Claude Code's subprocess supervisor).

Commands:
    harvey agents list              — show all registered manifests
    harvey agents info <name>       — full manifest for one agent
    harvey agents start <name>      — spawn via AgentLifecycle
    harvey agents stop <name>       — terminate the spawned process
    harvey agents status <name>     — runtime status (pid, state, restarts)
    harvey agents boot              — auto-start every active spawnable agent

Spawnable types (managed by this CLI):
    daemon, cron, cli, body-adapter

Non-spawnable types (in-process only, rejected with a clear error):
    subagent — these live inside the parent Harvey process and are
               loaded by AgentCoordinator.register_all_default() rather
               than spawned as OS subprocesses.

Design notes:
  - AgentLifecycle.spawn() is async. CLI invocations run the coroutine
    via asyncio.run() for each call (short-lived — the lifecycle state
    is persisted to disk so subsequent CLI calls pick it up).
  - The CLI does NOT touch harveychat.py or auto_pilot.py directly.
    An auto_pilot that wants to start all active agents on boot can
    import and call `boot_active_agents()` from this module.
  - Registration with AgentLifecycle is idempotent — safe to re-register
    the same manifest across multiple CLI calls.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import os
import sys
from typing import List, Optional

from .loader import AgentRegistry
from .manifest import AgentManifest, AgentType

log = logging.getLogger("harvey.agents.cli")

HARVEY_HOME = os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO"))

# Types this CLI can spawn as subprocesses (everything but pure in-process subagents)
_SPAWNABLE_TYPES = {
    AgentType.DAEMON,
    AgentType.CRON,
    AgentType.CLI,
    AgentType.BODY_ADAPTER,
}


# ─── Manifest → AgentConfig conversion ────────────────────────


def manifest_to_agent_config(manifest: AgentManifest):
    """Convert an AgentManifest into the AgentConfig dataclass that
    AgentLifecycle.register() expects.

    Handles workdir path resolution (relative to HARVEY_HOME) and
    default env/args population from the manifest runtime block.
    """
    # Lazy import so modules that don't need lifecycle can import cli without
    # loading the orchestration package
    from core.orchestration.agent_lifecycle import AgentConfig

    workdir = manifest.runtime.workdir
    if workdir and not os.path.isabs(workdir):
        workdir = os.path.join(HARVEY_HOME, workdir)

    # Resolve entrypoint relative to workdir if it's a local filename
    entrypoint = manifest.runtime.entrypoint
    if workdir and entrypoint and not entrypoint.startswith("-") and not os.path.isabs(entrypoint):
        entrypoint_abs = os.path.join(workdir, entrypoint)
        if os.path.exists(entrypoint_abs):
            entrypoint = entrypoint_abs

    # AgentLifecycle expects `entry_point` as the command, so prefix with
    # the interpreter if the manifest specifies one
    command = manifest.runtime.command or "python3"
    if command not in ("", "in-process"):
        args = [entrypoint] + list(manifest.runtime.args)
        entry_point = command
    else:
        # In-process or bare entry — shouldn't happen for spawnable types
        entry_point = entrypoint
        args = list(manifest.runtime.args)

    return AgentConfig(
        name=manifest.name,
        entry_point=entry_point,
        args=args,
        env={},
        workdir=workdir,
    )


# ─── Lifecycle accessor ───────────────────────────────────────


_LIFECYCLE_SINGLETON = None


def _get_lifecycle():
    """Return the module-level singleton AgentLifecycle, creating it lazily.

    Separate from the coordinator singleton in harvey_agent.py — this one
    is for subprocess-level management, not in-process subagents.
    """
    global _LIFECYCLE_SINGLETON
    if _LIFECYCLE_SINGLETON is None:
        from core.orchestration.agent_lifecycle import AgentLifecycle
        _LIFECYCLE_SINGLETON = AgentLifecycle()
    return _LIFECYCLE_SINGLETON


def reset_lifecycle():
    """Test hook — invalidate the cached lifecycle singleton."""
    global _LIFECYCLE_SINGLETON
    _LIFECYCLE_SINGLETON = None


# ─── Registration helper ─────────────────────────────────────


def _ensure_registered(manifest: AgentManifest, lifecycle=None) -> None:
    """Register the manifest with AgentLifecycle if not already. Idempotent."""
    lc = lifecycle or _get_lifecycle()
    # AgentLifecycle.register is idempotent-ish: re-registering the same
    # name just overwrites the config
    config = manifest_to_agent_config(manifest)
    lc.register(config)


def _ensure_spawnable(manifest: AgentManifest) -> Optional[str]:
    """Return an error string if the manifest is not spawnable, else None."""
    if manifest.type not in _SPAWNABLE_TYPES:
        return (
            f"agent '{manifest.name}' has type={manifest.type.value} "
            f"which is in-process only — not spawnable via CLI. "
            f"Spawnable types: {sorted(t.value for t in _SPAWNABLE_TYPES)}"
        )
    return None


# ─── Command handlers ────────────────────────────────────────


def cmd_list(registry: Optional[AgentRegistry] = None) -> int:
    """Print all discovered manifests in a compact table."""
    reg = registry or AgentRegistry()
    manifests = reg.list_all()
    if not manifests:
        print("No agents discovered.")
        return 0
    print(f"{'NAME':26s} {'TYPE':14s} {'STATUS':14s} {'TOOLS':6s} SOURCE")
    print("-" * 90)
    for m in manifests:
        print(
            f"{m.name:26s} {m.type.value:14s} {m.status.value:14s} "
            f"{len(m.tools):<6d} {m.source_path}"
        )
    print(f"\nTotal: {len(manifests)} ({len(reg.list_active())} active)")
    return 0


def cmd_info(name: str, registry: Optional[AgentRegistry] = None) -> int:
    """Print the full manifest for one agent as JSON."""
    reg = registry or AgentRegistry()
    m = reg.get(name)
    if m is None:
        print(f"Unknown agent: {name}", file=sys.stderr)
        print(f"Available: {', '.join(reg.names())}", file=sys.stderr)
        return 2
    print(json.dumps(m.to_dict(), indent=2))
    return 0


def cmd_start(name: str, registry: Optional[AgentRegistry] = None, lifecycle=None) -> int:
    """Spawn the agent via AgentLifecycle."""
    reg = registry or AgentRegistry()
    m = reg.get(name)
    if m is None:
        print(f"Unknown agent: {name}", file=sys.stderr)
        return 2

    err = _ensure_spawnable(m)
    if err:
        print(err, file=sys.stderr)
        return 3

    if m.status.value == "disabled":
        print(f"agent '{name}' is disabled in the manifest — refusing to start")
        return 4

    lc = lifecycle or _get_lifecycle()
    try:
        _ensure_registered(m, lc)
    except Exception as e:
        print(f"registration failed: {e}", file=sys.stderr)
        return 5

    try:
        status = asyncio.run(lc.spawn(name))
    except Exception as e:
        print(f"spawn failed: {e}", file=sys.stderr)
        return 6

    state = getattr(status, "state", None)
    state_val = getattr(state, "value", state) if state is not None else "?"
    pid = getattr(status, "pid", None)
    print(f"started {name}: state={state_val} pid={pid}")
    return 0 if state_val in ("running", "starting") else 1


def cmd_stop(name: str, lifecycle=None, force: bool = False) -> int:
    """Stop a running agent.

    AgentLifecycle's public API uses `kill(name, force=False)` (SIGTERM
    by default, SIGKILL if force=True) — there is no `stop()` method.
    We call kill() directly.
    """
    lc = lifecycle or _get_lifecycle()
    if not hasattr(lc, "kill"):
        print("AgentLifecycle.kill not available", file=sys.stderr)
        return 1
    try:
        result = lc.kill(name, force=force)
    except Exception as e:
        print(f"stop failed: {e}", file=sys.stderr)
        return 1
    state = getattr(getattr(result, "state", None), "value", "?")
    print(f"stopped {name}: state={state}")
    return 0


def cmd_status(name: str, registry: Optional[AgentRegistry] = None, lifecycle=None) -> int:
    """Show runtime status for one agent."""
    reg = registry or AgentRegistry()
    m = reg.get(name)
    if m is None:
        print(f"Unknown agent: {name}", file=sys.stderr)
        return 2

    lc = lifecycle or _get_lifecycle()
    status = lc.status(name)
    if status is None:
        print(f"{name}: not registered with lifecycle (never started)")
        return 0

    state = getattr(status.state, "value", status.state)
    print(f"{name}:")
    print(f"  manifest type: {m.type.value}")
    print(f"  manifest status: {m.status.value}")
    print(f"  lifecycle state: {state}")
    print(f"  pid: {status.pid}")
    print(f"  started_at: {status.started_at}")
    print(f"  restart_count: {status.restart_count}")
    if status.error_log:
        print(f"  last error: {status.error_log[:200]}")
    return 0


def boot_active_agents(
    registry: Optional[AgentRegistry] = None,
    lifecycle=None,
    dry_run: bool = False,
) -> List[str]:
    """Register + spawn every active spawnable agent manifest.

    Called by auto_pilot.py at daemon boot (once it's wired in a later
    phase). Returns the list of agent names that were started.

    Skips: subagent-type manifests, experimental/disabled statuses,
    and meta-harness-agent (explicitly excluded from auto-boot because
    it's a manual-trigger tool).

    `dry_run=True` lists what WOULD be started without actually calling
    spawn — useful for boot-time diagnostics + CI checks.
    """
    reg = registry or AgentRegistry()
    lc = lifecycle or _get_lifecycle() if not dry_run else None

    started: List[str] = []
    for m in reg.list_active():
        if m.type not in _SPAWNABLE_TYPES:
            continue
        if m.name == "meta-harness-agent":
            continue  # experimental, manual-trigger only
        if dry_run:
            started.append(m.name)
            continue
        # Registration AND spawn are both inside the try/except so neither
        # a registration failure nor a spawn crash takes down the boot loop.
        try:
            _ensure_registered(m, lc)
            asyncio.run(lc.spawn(m.name))
            started.append(m.name)
            log.info(f"[boot] started {m.name} ({m.type.value})")
        except Exception as e:
            log.error(f"[boot] failed to start {m.name}: {e}", exc_info=True)

    return started


def cmd_boot(registry: Optional[AgentRegistry] = None, dry_run: bool = True) -> int:
    """CLI wrapper around boot_active_agents — defaults to dry_run."""
    started = boot_active_agents(registry=registry, dry_run=dry_run)
    mode = "would start" if dry_run else "started"
    print(f"{mode} {len(started)} agent(s):")
    for name in started:
        print(f"  - {name}")
    return 0


# ─── argparse entry ──────────────────────────────────────────


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="harvey agents",
        description="Manage Harvey plugin agents via AgentRegistry + AgentLifecycle",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("list", help="List all discovered agent manifests")

    info = sub.add_parser("info", help="Show full manifest for one agent")
    info.add_argument("name")

    start = sub.add_parser("start", help="Spawn an agent via AgentLifecycle")
    start.add_argument("name")

    stop = sub.add_parser("stop", help="Stop a running agent (SIGTERM by default, --force for SIGKILL)")
    stop.add_argument("name")
    stop.add_argument("--force", action="store_true", help="SIGKILL instead of SIGTERM")

    status = sub.add_parser("status", help="Show runtime status for one agent")
    status.add_argument("name")

    boot = sub.add_parser(
        "boot",
        help="Auto-start every active spawnable agent (auto_pilot entry point)",
    )
    boot.add_argument(
        "--live", action="store_true",
        help="Actually spawn subprocesses (default: dry-run list only)",
    )

    return parser


def main(argv: Optional[List[str]] = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(name)s] %(levelname)s: %(message)s",
    )

    if args.command == "list":
        return cmd_list()
    if args.command == "info":
        return cmd_info(args.name)
    if args.command == "start":
        return cmd_start(args.name)
    if args.command == "stop":
        return cmd_stop(args.name, force=getattr(args, "force", False))
    if args.command == "status":
        return cmd_status(args.name)
    if args.command == "boot":
        return cmd_boot(dry_run=not args.live)

    parser.print_help()
    return 1


if __name__ == "__main__":
    sys.exit(main())
