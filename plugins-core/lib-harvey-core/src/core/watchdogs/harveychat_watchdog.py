#!/usr/local/opt/python@3.11/bin/python3.11
"""
HarveyChat Watchdog — Health check + auto-restart
Fails if: PID file missing or process dead
Action:    restart via python3 -m core.chat start --daemon
Schedule:  every 5 min via crontab
Logs:      ~/MAKAKOO/data/logs/harveychat-watchdog.log
"""

import os
import subprocess
import sys
import time
from pathlib import Path

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
PID_FILE = Path(HARVEY_HOME) / "data" / "chat" / "harveychat.pid"
LOG_FILE = Path(HARVEY_HOME) / "data" / "logs" / "harveychat-watchdog.log"
HARVEY_OS = Path(HARVEY_HOME) / "plugins-core" / "lib-harvey-core" / "src"


def log(msg: str):
    ts = time.strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{ts}] {msg}"
    print(line)
    LOG_FILE.parent.mkdir(parents=True, exist_ok=True)
    with LOG_FILE.open("a") as f:
        f.write(line + "\n")


def is_running() -> bool:
    """Check if HarveyChat process is alive."""
    if not PID_FILE.exists():
        return False
    try:
        pid = int(PID_FILE.read_text().strip())
        os.kill(pid, 0)
        return True
    except (OSError, ValueError):
        return False


def restart():
    """Restart HarveyChat daemon."""
    log("HarveyChat not running — restarting...")

    # Clean stale PID
    if PID_FILE.exists():
        PID_FILE.unlink()

    result = subprocess.run(
        [sys.executable, "-m", "core.chat", "start", "--daemon"],
        cwd=str(HARVEY_OS),
        capture_output=True,
        text=True,
        env={**os.environ, "HARVEY_HOME": HARVEY_HOME},
    )

    if result.returncode == 0:
        log("HarveyChat restart initiated.")
    else:
        log(f"Restart failed: {result.stderr[:200]}")

    # Verify after a few seconds
    time.sleep(5)
    if is_running():
        log("HarveyChat is running after restart.")
    else:
        log("WARNING: HarveyChat still not running after restart attempt.")


def main():
    if is_running():
        log("HarveyChat health: OK")
    else:
        # Check if Telegram token is configured before trying restart
        config_path = Path(HARVEY_HOME) / "data" / "chat" / "config.json"
        token_env = os.environ.get("TELEGRAM_BOT_TOKEN", "")
        has_config = config_path.exists()

        if not token_env and not has_config:
            log("HarveyChat not configured (no token). Skipping restart.")
            return

        restart()


if __name__ == "__main__":
    main()
