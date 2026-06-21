#!/usr/bin/env python3
"""Interactive first-run picker for brain sources.

Runs once during install (or any time via `makakoo brain init`). Asks the user
which knowledge substrate they already use — Logseq, Obsidian, both, neither —
and registers whichever sources they pick.

Design principles:
- **Optional**, not mandatory. Empty answer or Ctrl-C falls back to default
  (Logseq-only at `$MAKAKOO_HOME/data/Brain`). Install flow never blocks.
- **Idempotent**. Running twice just re-presents options and edits the config.
- **Batched + confirmed**. Answers collected in memory, summary shown, nothing
  persisted until user approves at the final prompt. Ctrl-C before confirmation
  leaves config untouched (other than the baseline default seed).
- **Post-write sync**. After approval, dry-walks each newly-added source so the
  user sees doc counts immediately and knows the registration took.
"""

from __future__ import annotations

import os
import platform
import shutil
import subprocess
import sys
from datetime import datetime
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import config as cfg  # type: ignore


DEFAULT_LOGSEQ_PATH = "$MAKAKOO_HOME/data/Brain"
DEFAULT_OBSIDIAN_GUESSES = [
    "~/Documents/Obsidian Vault",
    "~/Documents/obsidian",
    "~/Obsidian",
]

NO_PATH_SENTINELS = {"n", "no", "none", "skip", "s"}


class _InputInterrupted(Exception):
    """User aborted an interactive picker prompt."""


def _prompt(label: str, default: str = "") -> str:
    suffix = f" [{default}]" if default else ""
    try:
        answer = input(f"{label}{suffix}: ").strip()
    except (EOFError, KeyboardInterrupt):
        print()  # newline after ^C
        raise _InputInterrupted
    return answer or default


def _yes_no(label: str, default: bool = False) -> bool:
    d = "Y/n" if default else "y/N"
    try:
        raw = input(f"{label} [{d}]: ").strip().lower()
    except (EOFError, KeyboardInterrupt):
        print()
        raise _InputInterrupted
    if not raw:
        return default
    return raw.startswith("y")


def _guess_obsidian_vault() -> str | None:
    for candidate in DEFAULT_OBSIDIAN_GUESSES:
        expanded = Path(os.path.expanduser(candidate))
        if expanded.exists() and (expanded / ".obsidian").exists():
            return str(candidate)
    return None


def _detect_obsidian_app() -> tuple[bool, str]:
    """Best-effort local Obsidian app detection for setup UX only."""
    if shutil.which("obsidian"):
        return True, "obsidian command on PATH"

    system = platform.system().lower()
    home = Path.home()
    if system == "darwin":
        for candidate in [
            Path("/Applications/Obsidian.app"),
            home / "Applications" / "Obsidian.app",
        ]:
            if candidate.exists():
                return True, str(candidate)
    elif system == "linux":
        for candidate in [
            Path("/usr/share/applications/obsidian.desktop"),
            home / ".local/share/applications/obsidian.desktop",
            Path("/snap/bin/obsidian"),
            Path("/var/lib/flatpak/app/md.obsidian.Obsidian"),
            home / ".local/share/flatpak/app/md.obsidian.Obsidian",
        ]:
            if candidate.exists():
                return True, str(candidate)
    elif system == "windows":
        for env_name in ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"]:
            base = os.environ.get(env_name)
            if not base:
                continue
            for rel in [
                Path("Obsidian") / "Obsidian.exe",
                Path("Programs") / "Obsidian" / "Obsidian.exe",
            ]:
                candidate = Path(base) / rel
                if candidate.exists():
                    return True, str(candidate)

    return False, "not detected"


def _obsidian_install_command() -> list[str] | None:
    """Best-effort desktop Obsidian installer for the current platform."""
    system = platform.system().lower()
    if system == "darwin" and shutil.which("brew"):
        return ["brew", "install", "--cask", "obsidian"]
    if system == "linux":
        if shutil.which("flatpak"):
            return ["flatpak", "install", "-y", "flathub", "md.obsidian.Obsidian"]
    if system == "windows":
        if shutil.which("winget"):
            return ["winget", "install", "-e", "--id", "Obsidian.Obsidian"]
    return None


def _install_obsidian_app() -> bool:
    """Prompt path already consented; run the platform installer with live output."""
    cmd = _obsidian_install_command()
    if not cmd:
        return False
    print(f"Installing Obsidian: {' '.join(cmd)}")
    try:
        result = subprocess.run(cmd, stdin=sys.stdin, stdout=sys.stdout, stderr=sys.stderr)
        return result.returncode == 0
    except KeyboardInterrupt:
        print("Obsidian install cancelled.")
        raise _InputInterrupted
    except OSError as e:
        print(f"Obsidian install failed to start: {e}")
        return False


def _manual_obsidian_install_lines() -> list[str]:
    system = platform.system().lower()
    if system == "darwin":
        if shutil.which("brew"):
            return ["macOS: brew install --cask obsidian"]
        return ["macOS: download Obsidian from https://obsidian.md/download"]
    if system == "linux":
        if shutil.which("flatpak"):
            return ["Linux: flatpak install flathub md.obsidian.Obsidian"]
        return ["Linux: install from https://obsidian.md/download, Flatpak, Snap, or AppImage"]
    if system == "windows":
        if shutil.which("winget"):
            return ["Windows: winget install -e --id Obsidian.Obsidian"]
        return ["Windows: download Obsidian from https://obsidian.md/download"]
    return ["Download Obsidian from https://obsidian.md/download"]


def _normalize_optional_path(raw: str) -> str:
    value = raw.strip()
    if value.lower() in NO_PATH_SENTINELS:
        return ""
    return value


def _prompt_obsidian_path(default: str = "") -> str:
    label = "  Obsidian vault path (no = skip)"
    if default:
        label = "  Obsidian vault path (Enter = detected vault, no = skip)"
    path = _normalize_optional_path(
        _prompt(label, default=default)
    )
    if not path:
        print("  Skipping Obsidian source registration.")
        return ""

    expanded = Path(os.path.expanduser(path))
    if not expanded.exists():
        print(f"  Warning: {expanded} doesn't exist.")
        if not _yes_no("  Register this missing path anyway?", default=False):
            print("  Skipping Obsidian source registration. Create the vault, then rerun: makakoo setup brain")
            return ""
    elif not (expanded / ".obsidian").exists():
        print(f"  Warning: {expanded} does not contain a .obsidian folder.")
        if not _yes_no("  Register it as an Obsidian vault anyway?", default=False):
            print("  Skipping Obsidian source registration.")
            return ""
    return path


def _ensure_default_logseq() -> None:
    """Baseline guarantee: the Logseq default source is always present in config.

    This is NOT a user choice — it's the filesystem invariant. Runs outside the
    batched-confirmation flow so the config file always has a usable default
    even if the user Ctrl-Cs before confirming their additions.
    """
    registry = cfg.load_registry()
    if "default" in registry.names():
        return
    previous_names = set(registry.names())
    previous_default = registry.default_name
    cfg.add_source({
        "name": "default",
        "type": "logseq",
        "path": DEFAULT_LOGSEQ_PATH,
        "writable": True,
    })
    if previous_default not in previous_names:
        cfg.set_default("default")


def _dry_sync(name: str) -> str:
    """Walk one registered source and return a one-line count summary."""
    try:
        registry = cfg.load_registry()
        src = registry.get(name)
        count = 0
        oldest = newest = None
        for doc in src.iter_docs():
            count += 1
            oldest = min(oldest, doc.mtime) if oldest else doc.mtime
            newest = max(newest, doc.mtime) if newest else doc.mtime
        if count == 0:
            return f"{name}: 0 docs (empty or unreadable)"
        rng = f"{datetime.fromtimestamp(oldest):%Y-%m-%d} → {datetime.fromtimestamp(newest):%Y-%m-%d}"
        return f"{name}: {count} docs, {rng}"
    except Exception as e:  # never crash the picker on post-write sync
        return f"{name}: sync skipped ({e})"


def run_interactive(non_interactive: bool = False) -> int:
    try:
        return _run_interactive_impl(non_interactive=non_interactive)
    except _InputInterrupted:
        print("Aborted. No changes written.")
        return 0
    except Exception as e:
        print(f"Brain setup failed: {e}", file=sys.stderr)
        return 1


def _run_interactive_impl(non_interactive: bool = False) -> int:
    print("\nMakakoo brain setup\n" + "=" * 20)
    print("Your knowledge can live in multiple places. The default is a Logseq-style")
    print(f"graph at {DEFAULT_LOGSEQ_PATH}. You can also connect a SEPARATE Obsidian")
    print("vault or a plain markdown folder. You can skip this and add sources later.\n")
    print("Heads-up: if you just want to USE Obsidian as a nicer editor on top of the")
    print(f"existing Brain, no setup is needed — open {DEFAULT_LOGSEQ_PATH} as a vault")
    print("in Obsidian. This picker is for registering ADDITIONAL vaults.\n")

    _ensure_default_logseq()

    if non_interactive:
        print("Non-interactive mode: kept default Logseq source only.")
        return 0

    pending_adds: list[dict] = []
    pending_default: str | None = None

    # Prompt 1 — additional Obsidian vault?
    guess = _guess_obsidian_vault()
    obsidian_installed, obsidian_where = _detect_obsidian_app()
    add_obsidian = _yes_no(
        "Do you already have a separate Obsidian vault folder to register?"
        + (f" (detected vault: {guess})" if guess else ""),
        default=False,
    )
    if add_obsidian:
        if not obsidian_installed:
            print("\nObsidian app was not detected on this machine.")
            installer = _obsidian_install_command()
            installer_finished = False
            if installer:
                if _yes_no(f"Install Obsidian now? Runs: {' '.join(installer)}", default=False):
                    if _install_obsidian_app():
                        print("Obsidian installer finished.")
                        installer_finished = True
                    else:
                        print("Obsidian install failed or was cancelled.")
                    obsidian_installed, obsidian_where = _detect_obsidian_app()
                    if obsidian_installed:
                        print(f"Obsidian app detected: {obsidian_where}")
            else:
                print("No supported package manager was detected for automatic install.")
            if not obsidian_installed:
                if installer_finished:
                    print("Installer finished, but Makakoo could not detect the app yet.")
                else:
                    print("Install Obsidian manually if you want the desktop app here:")
                    for line in _manual_obsidian_install_lines():
                        print(f"  {line}")
            print("You can also skip this and later open the default Brain as an Obsidian vault:")
            print(f"  {DEFAULT_LOGSEQ_PATH}\n")
            if not obsidian_installed:
                add_obsidian = _yes_no("Register an existing Obsidian vault path anyway?", default=True)
        else:
            print(f"Obsidian app detected: {obsidian_where}")

    if add_obsidian:
        path = _prompt_obsidian_path(default=guess or "")
        if path:
            writable = _yes_no("  Allow Harvey to write into this Obsidian vault?", default=False)
            pending_adds.append({
                "name": "obsidian",
                "type": "obsidian",
                "path": str(path),
                "writable": writable,
            })

    # Prompt 2 — any other plain-markdown folder?
    add_plain = _yes_no("Any other plain markdown folder to connect?", default=False)
    if add_plain:
        path = _prompt("  Folder path")
        if path:
            name = _prompt("  Give this source a name", default="notes")
            writable = _yes_no("  Allow Harvey to write into it?", default=False)
            pending_adds.append({
                "name": name,
                "type": "plain",
                "path": path,
                "writable": writable,
            })

    # Prompt 3 — change default?
    existing_names = set(cfg.load_registry().names())
    projected_names = existing_names | {entry["name"] for entry in pending_adds}
    if len(projected_names) > 1:
        registry = cfg.load_registry()
        current_default = registry.default_name
        writable_by_name = {src.name: src.writable for src in registry.sources}
        writable_by_name.update({entry["name"]: bool(entry.get("writable")) for entry in pending_adds})
        print(f"\nAfter these changes, sources will be: {', '.join(sorted(projected_names))}")
        print(f"Current write-default: {current_default}")
        new_default = _prompt(
            "  Change write-default? (blank to keep current)",
            default="",
        )
        if new_default and new_default in projected_names and new_default != current_default:
            if writable_by_name.get(new_default, False):
                pending_default = new_default
            else:
                print(f"  {new_default!r} is read-only; keeping current write-default.")

    # Summary + final confirmation
    if not pending_adds and pending_default is None:
        print("\nNothing to change. Default Logseq source is registered and writable.")
        return 0

    print("\nPending changes")
    print("-" * 20)
    for entry in pending_adds:
        flag = "writable" if entry["writable"] else "read-only"
        print(f"  + register {entry['name']!r} ({entry['type']}, {flag}) → {entry['path']}")
    if pending_default:
        print(f"  + write-default → {pending_default}")
    print()

    if not _yes_no("Save these changes?", default=True):
        print("Aborted. No changes written.")
        return 0

    # Commit phase — one atomic config write for the whole batch.
    try:
        cfg.apply_changes(pending_adds, pending_default)
    except Exception as e:
        print(f"  Failed to save Brain source changes: {e}", file=sys.stderr)
        print("\nBrain setup finished with errors:", file=sys.stderr)
        print(f"  - {e}", file=sys.stderr)
        return 1

    committed = [entry["name"] for entry in pending_adds]
    for name in committed:
        print(f"  Registered {name!r}")
    if pending_default:
        print(f"  Write-default → {pending_default}")

    # Post-write sync — walk each newly-registered source so the user sees counts
    if committed:
        print("\nInitial sync (dry walk, no DB writes)")
        print("-" * 20)
        for name in committed:
            print(f"  {_dry_sync(name)}")

    print(f"\nDone. Config saved to {cfg.config_path()}")
    print("Change anything later with: makakoo brain {list|add|remove|set-default}\n")
    return 0


if __name__ == "__main__":
    sys.exit(run_interactive())
