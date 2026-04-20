#!/usr/bin/env python3
"""
Harvey OS Orchestrator Daemon Launcher

Usage:
    python3 daemon.py start   - Start the orchestrator in background
    python3 daemon.py stop    - Stop the running orchestrator
    python3 daemon.py status  - Check if running
    python3 daemon.py restart - Restart the orchestrator

The orchestrator processes tasks from data/orchestrator/queues/incoming/
and spawns sub-agents to execute them. Results are aggregated and
returned via the message bus.

Requires: Python 3.10+
"""

import os
import sys
import signal
import subprocess
import time
import argparse
from pathlib import Path

HARVEY_HOME = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")))
PID_FILE = HARVEY_HOME / "data" / "orchestrator" / "daemon.pid"
LOG_FILE = HARVEY_HOME / "data" / "logs" / "orchestrator.log"
CONTROLLER_PATH = HARVEY_HOME / "harvey-os" / "skills" / "infrastructure" / "orchestrator" / "controller.py"

PYTHON = sys.executable


def ensure_log_dir():
    LOG_FILE.parent.mkdir(parents=True, exist_ok=True)


def write_pid(pid: int):
    PID_FILE.parent.mkdir(parents=True, exist_ok=True)
    PID_FILE.write_text(str(pid))


def read_pid() -> int | None:
    if PID_FILE.exists():
        return int(PID_FILE.read_text().strip())
    return None


def is_running(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def start():
    ensure_log_dir()
    existing = read_pid()
    if existing and is_running(existing):
        print(f"Orchestrator already running (PID {existing})")
        return

    print("Starting Harvey Orchestrator daemon...")
    log_fd = open(LOG_FILE, "a")

    proc = subprocess.Popen(
        [PYTHON, str(CONTROLLER_PATH)],
        stdout=log_fd,
        stderr=subprocess.STDOUT,
        cwd=str(HARVEY_HOME),
        env={**os.environ, "HARVEY_HOME": str(HARVEY_HOME)},
        preexec_fn=os.setsid,  # New process group
    )

    write_pid(proc.pid)
    print(f"Orchestrator started (PID {proc.pid})")
    print(f"Logs: {LOG_FILE}")


def stop():
    pid = read_pid()
    if not pid:
        print("Orchestrator not running (no PID file)")
        return

    if not is_running(pid):
        print("Orchestrator not running (stale PID file)")
        PID_FILE.unlink()
        return

    print(f"Stopping orchestrator (PID {pid})...")
    try:
        os.killpg(os.getpgid(pid), signal.SIGTERM)
        time.sleep(2)
        if is_running(pid):
            print("Graceful stop failed, forcing...")
            os.killpg(os.getpgid(pid), signal.SIGKILL)
    except OSError as e:
        print(f"Error stopping: {e}")

    if PID_FILE.exists():
        PID_FILE.unlink()
    print("Orchestrator stopped")


def status():
    pid = read_pid()
    if not pid:
        print("Orchestrator: NOT RUNNING (no PID)")
        return

    if is_running(pid):
        print(f"Orchestrator: RUNNING (PID {pid})")
    else:
        print(f"Orchestrator: NOT RUNNING (stale PID {pid})")
        PID_FILE.unlink()


def restart():
    stop()
    time.sleep(1)
    start()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Harvey Orchestrator Daemon")
    parser.add_argument("action", choices=["start", "stop", "status", "restart"])
    args = parser.parse_args()

    if args.action == "start":
        start()
    elif args.action == "stop":
        stop()
    elif args.action == "status":
        status()
    elif args.action == "restart":
        restart()
