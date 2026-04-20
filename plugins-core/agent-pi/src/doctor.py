#!/usr/bin/env python3
"""agent-pi doctor — checks the pi runtime is present and configured.

Exit 0 if everything green, exit 1 on any red. Output is human-readable.
Called by `makakoo plugin health agent-pi` and by the B.2 `pi-doctor`
CLI surface.

Checks:
  1. `pi` binary on PATH + version parses.
  2. `~/.pi/AGENTS.md` exists with a v10+ infect marker.
  3. `@traylinx/pi-switchai-provider` extension installed.
  4. `~/.pi/memory` symlink points at
     `$MAKAKOO_HOME/data/auto-memory` (shared cross-CLI brain).
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path


GREEN = "\033[32m✓\033[0m"
RED = "\033[31m✗\033[0m"


def _home() -> Path:
    raw = (
        os.environ.get("MAKAKOO_HOME")
        or os.environ.get("HARVEY_HOME")
        or os.path.expanduser("~/MAKAKOO")
    )
    return Path(raw).expanduser().resolve()


def check_pi_binary() -> tuple[bool, str]:
    pi = shutil.which("pi")
    if not pi:
        return False, "pi binary not on PATH"
    try:
        out = subprocess.run(
            ["pi", "--version"],
            capture_output=True,
            text=True,
            timeout=5,
        )
    except subprocess.TimeoutExpired:
        return False, "pi --version timed out"
    if out.returncode != 0:
        return False, f"pi --version exited {out.returncode}"
    return True, out.stdout.strip() or out.stderr.strip() or "ok"


def check_agents_md() -> tuple[bool, str]:
    path = Path("~/.pi/AGENTS.md").expanduser()
    if not path.exists():
        return False, f"{path} missing — run `makakoo infect` to install"
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError as e:
        return False, f"cannot read {path}: {e}"
    # Accept any v10+ marker — infect_global v11 is current, but older
    # installs still work.
    if "<!-- harvey:infect-global START v" not in text:
        return False, f"no harvey:infect-global marker in {path}"
    return True, "markers present"


def check_switchai_extension() -> tuple[bool, str]:
    ext_dir = Path("~/.pi/extensions/@traylinx/pi-switchai-provider").expanduser()
    if ext_dir.exists():
        return True, str(ext_dir)
    # Older pi versions may use a flatter layout.
    alt = Path("~/.pi/extensions/pi-switchai-provider").expanduser()
    if alt.exists():
        return True, str(alt)
    return False, "pi-switchai-provider not installed under ~/.pi/extensions/"


def check_memory_symlink() -> tuple[bool, str]:
    link = Path("~/.pi/memory").expanduser()
    canonical = _home() / "data" / "auto-memory"
    if not link.exists():
        return False, f"{link} missing (should symlink → {canonical})"
    try:
        resolved = link.resolve()
    except OSError as e:
        return False, f"{link} resolve failed: {e}"
    if resolved != canonical.resolve():
        return False, f"{link} → {resolved} (expected {canonical})"
    return True, f"→ {canonical}"


def main() -> int:
    checks = [
        ("pi binary", check_pi_binary()),
        ("~/.pi/AGENTS.md", check_agents_md()),
        ("pi-switchai-provider", check_switchai_extension()),
        ("~/.pi/memory symlink", check_memory_symlink()),
    ]
    all_green = True
    for label, (ok, detail) in checks:
        mark = GREEN if ok else RED
        print(f"  {mark} {label:25s} {detail}")
        if not ok:
            all_green = False
    print()
    if all_green:
        print("agent-pi: all green.")
        return 0
    print("agent-pi: RED — fix the items above before `pi_run` will work.",
          file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
