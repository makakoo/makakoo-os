"""daemon_admin.py — thin wrapper exposing upstream admin.py as a CLI.

`makakoo agent start/stop/health agent-browser-harness` calls into this
shim, which in turn drives the upstream browser-harness daemon primitives.
The wrapper lives in our plugins-core dir, NOT in the upstream tree, so
upstream updates never clobber Makakoo integration glue.

Exit codes:
    0  — command succeeded (health: daemon is alive)
    1  — command failed (health: daemon is not alive; doctor: Chrome not reachable)
    2  — bad usage

CLI surface:
    daemon_admin.py start     # ensure daemon + Chrome tab are up
    daemon_admin.py stop      # stop the daemon cleanly
    daemon_admin.py health    # "OK" + exit 0 if up, "DOWN" + exit 1 otherwise
    daemon_admin.py doctor    # verify Chrome is reachable via CDP
"""
from __future__ import annotations

import os
import sys
import urllib.error
import urllib.request
from pathlib import Path


def _locate_upstream() -> Path:
    """Resolve the upstream browser-harness tree cloned by install.sh."""
    plugin_dir = os.environ.get("MAKAKOO_PLUGIN_DIR") or str(Path(__file__).resolve().parent)
    return Path(plugin_dir) / "upstream"


def _load_upstream():
    upstream = _locate_upstream()
    if not upstream.is_dir():
        print(
            f"upstream not cloned at {upstream}. "
            "Re-run `makakoo plugin install agent-browser-harness`.",
            file=sys.stderr,
        )
        sys.exit(1)
    sys.path.insert(0, str(upstream))
    import admin  # noqa: E402  (must happen after sys.path fixup)
    return admin


def cmd_start() -> int:
    admin = _load_upstream()
    # ensure_daemon blocks until the socket is healthy.
    admin.ensure_daemon()
    print("daemon: OK")
    return 0


def cmd_stop() -> int:
    admin = _load_upstream()
    name = os.environ.get("BU_NAME", "default")
    admin.stop_remote_daemon(name=name)
    print("daemon: stopped")
    return 0


def cmd_health() -> int:
    admin = _load_upstream()
    if admin.daemon_alive():
        print("OK")
        return 0
    print("DOWN")
    return 1


def cmd_doctor() -> int:
    url = os.environ.get("BU_CDP_URL", "http://127.0.0.1:9222/json/version")
    try:
        with urllib.request.urlopen(url, timeout=2) as resp:
            body = resp.read().decode("utf-8", errors="replace")
            print(f"chrome: OK ({len(body)} bytes from {url})")
            return 0
    except (urllib.error.URLError, urllib.error.HTTPError, TimeoutError, ConnectionError) as e:
        print(f"chrome: NOT AVAILABLE ({e})", file=sys.stderr)
        return 1


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print("usage: daemon_admin.py {start|stop|health|doctor}", file=sys.stderr)
        return 2
    cmd = argv[1]
    handlers = {
        "start": cmd_start,
        "stop": cmd_stop,
        "health": cmd_health,
        "doctor": cmd_doctor,
    }
    fn = handlers.get(cmd)
    if fn is None:
        print(f"unknown command: {cmd}", file=sys.stderr)
        return 2
    return fn()


if __name__ == "__main__":
    sys.exit(main(sys.argv))
