"""
resumer — Cron-driven sweeper that resumes stale Harvey tasks.

Runs periodically (every 2 minutes via cron). Finds tasks in RUNNING state
whose heartbeat is older than STALE_HEARTBEAT_SECONDS, atomically claims
them via TaskStore.claim_stale(), and spawns a subprocess to resume each
via `python -m core.agent.run_task --task-id <id>`.

Why a subprocess instead of in-process?
  - The live daemon may not be running (daemon crash → cron still sweeps)
  - Subprocess gets a fresh Python VM — no lingering state from a bad task
  - If the resumer itself crashes, it doesn't take down the daemon
  - Each resumed task is isolated from its peers

CLI:
    python -m core.agent.resumer --sweep      # one sweep, exit
    python -m core.agent.resumer --list       # print stale tasks, don't touch

Cron:
    */2 * * * * cd /Users/sebastian/MAKAKOO && \\
        MAKAKOO_HOME=/Users/sebastian/MAKAKOO \\
        HARVEY_HOME=/Users/sebastian/MAKAKOO \\
        /usr/local/Cellar/python@3.11/3.11.10/bin/python3.11 \\
        -m core.agent.resumer --sweep >> data/logs/resumer.log 2>&1
"""

from __future__ import annotations

import argparse
import logging
import os
import subprocess
import sys
from typing import Callable, List, Optional

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))

log = logging.getLogger("harvey.resumer")


def default_spawner(task_id: str, python: Optional[str] = None) -> subprocess.Popen:
    """Spawn `python -m core.agent.run_task --task-id X` as a detached subprocess.

    The child writes to the same TaskStore DB (via WAL concurrency) and
    updates state + entries + artifacts as it runs. Parent doesn't wait
    for completion — it logs the PID and returns.
    """
    python_bin = python or os.environ.get("HARVEY_PYTHON") or sys.executable
    env = os.environ.copy()
    env.setdefault("HARVEY_HOME", HARVEY_HOME)
    env.setdefault("PYTHONPATH", HARVEY_HOME)
    proc = subprocess.Popen(
        [python_bin, "-m", "core.agent.run_task", "--task-id", task_id],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        cwd=HARVEY_HOME,
    )
    log.info(f"[resumer] spawned pid={proc.pid} for task {task_id[:8]}")
    return proc


def sweep(
    store=None,
    spawner: Callable = default_spawner,
    max_claims: int = 10,
) -> List[str]:
    """One sweep pass.

    0. Reset any tasks stuck in RESUMING back to RUNNING (kill-9 recovery)
    1. Query TaskStore.stale_running() for candidates
    2. For each, atomically claim via claim_stale()
    3. For successful claims, call spawner(task_id)
    4. Return the list of claimed task_ids

    `max_claims` caps how many tasks a single sweep can fire off — prevents
    a catastrophic backlog from spawning a subprocess storm.
    """
    from core.tasks import TaskStore
    if store is None:
        store = TaskStore()

    # Step 0: recover subprocesses that died between claim_stale and
    # run_task's set_state(RUNNING). Tasks stuck in RESUMING longer than
    # 2× the stale threshold are reset to RUNNING so the normal claim
    # path picks them up on THIS sweep cycle.
    reset_count = store.reset_stuck_resuming()
    if reset_count:
        log.warning(f"[resumer] sweep: reset {reset_count} task(s) stuck in RESUMING")

    stale = store.stale_running()
    if not stale:
        log.debug("[resumer] sweep: no stale tasks")
        return []

    log.info(f"[resumer] sweep: {len(stale)} stale candidate(s)")

    claimed: List[str] = []
    for task in stale:
        if len(claimed) >= max_claims:
            log.warning(
                f"[resumer] max_claims={max_claims} reached — remaining "
                f"{len(stale) - len(claimed)} task(s) will be swept next cycle"
            )
            break
        if not store.claim_stale(task.id):
            log.debug(f"[resumer] lost race to claim {task.id[:8]}")
            continue
        try:
            spawner(task.id)
            claimed.append(task.id)
        except Exception as e:
            log.error(
                f"[resumer] spawner crashed for {task.id[:8]}: {e}",
                exc_info=True,
            )
            # Mark the task FAILED so it isn't immediately re-swept
            try:
                from core.tasks import TaskState
                store.set_state(
                    task.id,
                    TaskState.FAILED,
                    error=f"resumer: spawner crash: {e}"[:500],
                )
            except Exception:
                pass

    log.info(f"[resumer] sweep complete: claimed {len(claimed)}/{len(stale)}")
    return claimed


def list_stale(store=None) -> List:
    """Return the list of stale tasks without touching them."""
    from core.tasks import TaskStore
    if store is None:
        store = TaskStore()
    return store.stale_running()


def _cli() -> int:
    parser = argparse.ArgumentParser(description="Harvey cognitive core stale-task sweeper")
    parser.add_argument("--sweep", action="store_true", help="Run one sweep and exit")
    parser.add_argument("--list", action="store_true", help="List stale tasks without claiming")
    parser.add_argument("--max-claims", type=int, default=10, help="Max tasks to claim per sweep")
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s [%(name)s] %(levelname)s: %(message)s",
    )

    if args.list:
        tasks = list_stale()
        if not tasks:
            print("No stale tasks.")
            return 0
        for t in tasks:
            age = ""
            if t.heartbeat:
                import time as _t
                age = f"{int(_t.time() - t.heartbeat)}s stale"
            print(f"  {t.id[:8]} {t.channel}:{t.user_id} {age} — {t.goal[:80]}")
        return 0

    if args.sweep:
        claimed = sweep(max_claims=args.max_claims)
        print(f"Swept: claimed {len(claimed)} task(s)")
        return 0

    parser.print_help()
    return 1


if __name__ == "__main__":
    sys.exit(_cli())
