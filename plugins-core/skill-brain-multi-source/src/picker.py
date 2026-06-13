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


def _prompt(label: str, default: str = "") -> str:
    suffix = f" [{default}]" if default else ""
    try:
        answer = input(f"{label}{suffix}: ").strip()
    except (EOFError, KeyboardInterrupt):
        print()  # newline after ^C
        return default
    return answer or default


def _yes_no(label: str, default: bool = False) -> bool:
    d = "Y/n" if default else "y/N"
    try:
        raw = input(f"{label} [{d}]: ").strip().lower()
    except (EOFError, KeyboardInterrupt):
        print()
        return default
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


def _normalize_optional_path(raw: str) -> str:
    value = raw.strip()
    if value.lower() in NO_PATH_SENTINELS:
        return ""
    return value


def _prompt_obsidian_path(default: str = "") -> str:
    path = _normalize_optional_path(
        _prompt("  Obsidian vault path (blank / no = skip)", default=default)
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
    cfg.add_source({
        "name": "default",
        "type": "logseq",
        "path": DEFAULT_LOGSEQ_PATH,
        "writable": True,
    })
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
        default=bool(guess),
    )
    if add_obsidian:
        if not obsidian_installed:
            print("\nObsidian app was not detected on this machine.")
            print("Install Obsidian first if you want the desktop app here:")
            print("  macOS:  brew install --cask obsidian")
            print("  Linux:  install from https://obsidian.md/download, Flatpak, Snap, or AppImage")
            print("You can also skip this and later open the default Brain as an Obsidian vault:")
            print(f"  {DEFAULT_LOGSEQ_PATH}\n")
            add_obsidian = _yes_no("Register an existing Obsidian vault path anyway?", default=False)
        else:
            print(f"Obsidian app detected: {obsidian_where}")

    if add_obsidian:
        path = _prompt_obsidian_path(default=guess or "")
        if path:
            pending_adds.append({
                "name": "obsidian",
                "type": "obsidian",
                "path": str(path),
                "writable": True,
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
        current_default = cfg.load_registry().default_name
        print(f"\nAfter these changes, sources will be: {', '.join(sorted(projected_names))}")
        print(f"Current write-default: {current_default}")
        new_default = _prompt(
            "  Change write-default? (blank to keep current)",
            default="",
        )
        if new_default and new_default in projected_names and new_default != current_default:
            pending_default = new_default

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

    # Commit phase
    committed: list[str] = []
    for entry in pending_adds:
        try:
            cfg.add_source(entry)
            committed.append(entry["name"])
            print(f"  Registered {entry['name']!r}")
        except Exception as e:
            print(f"  Failed to register {entry['name']!r}: {e}", file=sys.stderr)

    if pending_default:
        try:
            cfg.set_default(pending_default)
            print(f"  Write-default → {pending_default}")
        except Exception as e:
            print(f"  Failed to set default: {e}", file=sys.stderr)

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
