#!/usr/local/opt/python@3.11/bin/python3.11
"""
switchAILocal Watchdog — Health check + auto-restart
Fails if: health endpoint returns non-ok
Action:    stop + start via ail.sh
Schedule:  every 5 min via crontab
Logs:      ~/MAKAKOO/data/logs/switchailocal-watchdog.log
"""

import os
import subprocess
import sys
import time
from pathlib import Path

import requests

GATEWAY = "http://localhost:18080"
HEALTH_ENDPOINT = f"{GATEWAY}/health"
AIL_SH = os.environ.get("SWITCHAI_SCRIPT", os.path.expanduser("~/projects/makakoo/agents/switchAILocal/ail.sh"))
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


def run(*cmd, check=True) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, check=check, capture_output=True, text=True)


def restart():
    log("Health check FAILED — restarting switchAILocal...")
    # Stop
    run([AIL_SH, "stop"], check=False)
    time.sleep(5)
    # Start
    result = run([AIL_SH, "start"], check=False)
    if result.returncode != 0:
        log(f"START failed: {result.stderr}")
    else:
        log("Restart initiated successfully.")
    # Give it a moment then verify
    time.sleep(8)
    if health_ok():
        log("Health check PASSED after restart.")
    else:
        log("Health check STILL FAILING after restart — needs manual attention.")


def main():
    if health_ok():
        log("Health check OK — no action needed.")
    else:
        restart()
    # Always verify current state
    state = run("launchctl", "list", check=False)
    for line in state.stdout.splitlines():
        if "switchailocal" in line.lower():
            log(f"launchctl: {line.strip()}")


if __name__ == "__main__":
    main()
