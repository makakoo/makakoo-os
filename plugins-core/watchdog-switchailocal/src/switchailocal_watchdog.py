#!/usr/local/opt/python@3.11/bin/python3.11
"""
switchAILocal Watchdog — health check + best-effort restart.

Fresh Makakoo installs run on macOS, Linux VPSes, and occasionally Windows.
The old watchdog always called `launchctl list` after checking health, which
made every Linux SANCHO tick fail even when switchAILocal was healthy. Keep the
manager-specific bits behind platform checks and treat process introspection as
diagnostic, not fatal.
"""

import os
import subprocess
import sys
import time
import platform
import shutil
from pathlib import Path

import requests

GATEWAY = "http://localhost:18080"
HEALTH_ENDPOINT = f"{GATEWAY}/health"
AIL_SH = os.environ.get(
    "SWITCHAI_SCRIPT",
    os.path.expanduser("~/projects/makakoo/agents/switchAILocal/ail.sh"),
)
LOG_FILE = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))) / "data" / "logs" / "switchailocal-watchdog.log"


def log(msg: str):
    ts = time.strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{ts}] {msg}"
    print(line)
    LOG_FILE.parent.mkdir(parents=True, exist_ok=True)
    LOG_FILE.open("a").write(line + "\n")


def health_ok() -> bool:
    try:
        r = requests.get(HEALTH_ENDPOINT, timeout=5)
        if r.status_code == 200 and r.json().get("status") == "ok":
            return True
    except Exception:
        pass
    return False


def run(cmd: list[str], check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, check=check, capture_output=True, text=True)


def _cmd_exists(name: str) -> bool:
    return shutil.which(name) is not None


def _systemctl_scope() -> list[list[str]]:
    return [
        ["systemctl", "--user"],
        ["systemctl"],
    ]


def _service_active(base: list[str], unit: str) -> bool:
    result = run([*base, "is-active", "--quiet", unit], check=False)
    return result.returncode == 0


def _restart_with_manager() -> bool:
    system = platform.system()
    if system == "Darwin" and Path(AIL_SH).exists():
        run([AIL_SH, "stop"], check=False)
        time.sleep(5)
        result = run([AIL_SH, "start"], check=False)
        if result.returncode != 0:
            log(f"START failed: {result.stderr.strip()}")
            return False
        log("Restart initiated via ail.sh.")
        return True

    if system == "Linux" and _cmd_exists("systemctl"):
        for base in _systemctl_scope():
            if _service_active(base, "switchailocal.service"):
                result = run([*base, "restart", "switchailocal.service"], check=False)
                if result.returncode == 0:
                    log(f"Restart initiated via {' '.join(base)} restart switchailocal.service.")
                    return True
                log(f"systemctl restart failed: {result.stderr.strip()}")

    log("No managed restart path found; switchAILocal needs manual attention.")
    return False


def restart():
    log("Health check FAILED — restarting switchAILocal...")
    if not _restart_with_manager():
        return
    # Give it a moment then verify
    time.sleep(8)
    if health_ok():
        log("Health check PASSED after restart.")
    else:
        log("Health check STILL FAILING after restart — needs manual attention.")


def log_manager_state():
    system = platform.system()
    if system == "Darwin" and _cmd_exists("launchctl"):
        state = run(["launchctl", "list"], check=False)
        for line in state.stdout.splitlines():
            if "switchailocal" in line.lower():
                log(f"launchctl: {line.strip()}")
        return

    if system == "Linux":
        if _cmd_exists("systemctl"):
            for base in _systemctl_scope():
                result = run([*base, "is-active", "switchailocal.service"], check=False)
                status = result.stdout.strip() or result.stderr.strip()
                if status:
                    log(f"{' '.join(base)} switchailocal.service: {status}")
        if _cmd_exists("pgrep"):
            result = run(["pgrep", "-af", "switchAILocal|switchailocal"], check=False)
            for line in result.stdout.splitlines():
                if "watchdog" not in line:
                    log(f"process: {line.strip()}")


def main():
    if health_ok():
        log("Health check OK — no action needed.")
    else:
        restart()
    # Always verify current state, but never fail the tick on diagnostics.
    try:
        log_manager_state()
    except Exception as exc:
        log(f"state diagnostic skipped: {exc}")


if __name__ == "__main__":
    main()
