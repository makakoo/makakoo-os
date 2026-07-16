#!/usr/bin/env python3
"""Interactive first-run picker for brain sources.

Runs once during install (or any time via `brain_cli.py init`). Seeds the
canonical Makakoo Brain folder, checks whether Obsidian is installed, and
registers optional additional sources only when the user asks for them.

Design principles:
- **Optional**, not mandatory. Empty answers accept visible defaults. The
  default Brain is `$MAKAKOO_HOME/data/Brain`. Install flow never blocks.
- **Idempotent**. Running twice just re-presents options and edits the config.
- **Batched + confirmed**. Answers collected in memory, summary shown, nothing
  extra is persisted until user approves at the final prompt. Ctrl-C before
  confirmation leaves pending source registrations untouched.
- **Post-write sync**. After approval, dry-walks each newly-added source so the
  user sees doc counts immediately and knows the registration took.
"""

from __future__ import annotations

import os
import platform
import json
import shutil
import subprocess
import sys
from datetime import datetime
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import config as cfg  # type: ignore


DEFAULT_BRAIN_PATH = "$MAKAKOO_HOME/data/Brain"
# Back-compat name for the source type. The user-facing concept is now the
# Makakoo Brain folder, which can also be opened directly as an Obsidian vault.
DEFAULT_LOGSEQ_PATH = DEFAULT_BRAIN_PATH
PICKER_ABORT_EXIT = 10
DEFAULT_OBSIDIAN_GUESSES = [
    "~/Documents/Obsidian Vault",
    "~/Documents/obsidian",
    "~/Obsidian",
]

NO_PATH_SENTINELS = {"n", "no", "none", "skip", "s"}

OBSIDIAN_CORE_PLUGINS = {
    "file-explorer": True,
    "global-search": True,
    "switcher": True,
    "graph": True,
    "backlink": True,
    "canvas": True,
    "outgoing-link": True,
    "tag-pane": True,
    "properties": True,
    "page-preview": True,
    "daily-notes": True,
    "templates": True,
    "note-composer": True,
    "command-palette": True,
    "outline": True,
    "file-recovery": True,
    "bases": True,
}


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


def _expanded_default_brain_path() -> Path:
    home = cfg._makakoo_home()  # type: ignore[attr-defined]
    return home / "data" / "Brain"


def _display_brain_path() -> str:
    """The default Brain path as the USER should see it: the real folder on
    their disk, not the `$MAKAKOO_HOME` literal. The symbolic form stays in
    the stored config (portability); display always expands it so the path
    can be pasted into Obsidian, a file manager, or a shell as-is."""
    return str(_expanded_default_brain_path())


def _prompt_obsidian_path(default: str = "") -> str:
    label = "  Additional Obsidian vault path (no = skip)"
    if default:
        label = "  Additional Obsidian vault path (Enter = detected vault, no = skip)"
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
    """Baseline guarantee: the Makakoo Brain default source is present on disk.

    This is NOT a user choice — it's the filesystem invariant. Call this only
    after a successful default-only flow, not before interactive prompts, so
    Ctrl-C does not accidentally mark the setup section complete.
    """
    path = cfg.config_path()
    default_entry = {
        "name": "default",
        "role": "canonical",
        "type": "logseq",
        "path": DEFAULT_BRAIN_PATH,
        "writable": True,
    }
    _expanded_default_brain_path().mkdir(parents=True, exist_ok=True)

    if not path.exists():
        cfg.save_registry({"canonical": "default", "default": "default", "sources": [default_entry]})
        return

    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        cfg.save_registry({"canonical": "default", "default": "default", "sources": [default_entry]})
        return

    sources = data.setdefault("sources", [])
    names = {s.get("name") for s in sources if isinstance(s, dict)}
    if "default" not in names:
        sources.append(default_entry)
        names.add("default")

    if data.get("default") not in names:
        data["default"] = "default"
    if data.get("canonical") not in names:
        data["canonical"] = "default"

    cfg.save_registry(data)


def _write_json_if_absent(path: Path, data: dict) -> bool:
    if path.exists():
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    return True


def _ensure_obsidian_profile() -> list[str]:
    """Create safe Obsidian UI defaults for the canonical Brain, never overwrite."""
    brain = _expanded_default_brain_path()
    obsidian = brain / ".obsidian"
    written: list[str] = []
    if _write_json_if_absent(obsidian / "core-plugins.json", OBSIDIAN_CORE_PLUGINS):
        written.append(".obsidian/core-plugins.json")
    if _write_json_if_absent(
        obsidian / "daily-notes.json",
        {
            "format": "YYYY_MM_DD",
            "folder": "journals",
            "template": "",
        },
    ):
        written.append(".obsidian/daily-notes.json")
    if _write_json_if_absent(
        obsidian / "graph.json",
        {
            "showTags": True,
            "showAttachments": False,
            "hideUnresolved": False,
            "showOrphans": True,
            "showArrow": True,
            "collapse-filter": False,
            "search": "",
            "colorGroups": [],
        },
    ):
        written.append(".obsidian/graph.json")
    return written


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
        return PICKER_ABORT_EXIT
    except Exception as e:
        print(f"Brain setup failed: {e}", file=sys.stderr)
        return 1


def _run_interactive_impl(non_interactive: bool = False) -> int:
    print("\nMakakoo brain setup\n" + "=" * 20)
    print("Default Brain folder (always present — created for you):")
    print(f"  {_display_brain_path()}")
    print(f"  (stored in config as {DEFAULT_BRAIN_PATH})")
    print("Makakoo writes journals/pages there by default. Obsidian can use the same")
    print("folder as an editor/vault; a separate Obsidian vault is optional.\n")

    if non_interactive:
        _ensure_default_logseq()
        print("Non-interactive mode: kept default Makakoo Brain source only.")
        return 0

    pending_adds: list[dict] = []
    pending_obsidian_profile = False

    # Prompt 1 — Obsidian app/editor setup for the default Brain folder.
    obsidian_installed, obsidian_where = _detect_obsidian_app()
    if not obsidian_installed:
        print("Obsidian app was not detected on this machine.")
        installer = _obsidian_install_command()
        if installer:
            if _yes_no(f"Install Obsidian now? Runs: {' '.join(installer)}", default=False):
                if _install_obsidian_app():
                    print("Obsidian installer finished.")
                else:
                    print("Obsidian install failed or was cancelled.")
                obsidian_installed, obsidian_where = _detect_obsidian_app()
                if obsidian_installed:
                    print(f"Obsidian app detected: {obsidian_where}")
        else:
            print("No supported package manager was detected for automatic install.")
            print("Install Obsidian manually if you want the desktop app here:")
            for line in _manual_obsidian_install_lines():
                print(f"  {line}")

        if not obsidian_installed:
            print("Obsidian setup skipped. Later, install Obsidian and open this folder as a vault:")
            print(f"  {_display_brain_path()}\n")

    add_obsidian = False
    if obsidian_installed:
        if obsidian_where:
            print(f"Obsidian app detected: {obsidian_where}")
        use_default_brain = _yes_no(
            f"Use {_display_brain_path()} as your Obsidian vault/editor folder?",
            default=True,
        )
        if use_default_brain:
            print("  Good. No extra source registration needed for the default Brain.")
            pending_obsidian_profile = True
            print("  Obsidian UI defaults will be added if missing; existing config stays untouched.")
            print(f"  Open this folder in Obsidian: {_display_brain_path()}\n")
            guess = _guess_obsidian_vault()
            add_obsidian = _yes_no(
                "Register an additional existing Obsidian vault?"
                + (f" (detected vault: {guess})" if guess else ""),
                default=False,
            )
        else:
            print("  Default Obsidian editor setup skipped.")
            guess = _guess_obsidian_vault()
            add_obsidian = _yes_no(
                "Register a separate existing Obsidian vault instead?"
                + (f" (detected vault: {guess})" if guess else ""),
                default=False,
            )
    else:
        guess = None

    if add_obsidian:
        path = _prompt_obsidian_path(default=guess or "")
        if path:
            writable = _yes_no("  Allow Makakoo to write into this Obsidian vault?", default=False)
            pending_adds.append({
                "name": "obsidian",
                "role": "enrichment",
                "type": "obsidian",
                "path": str(path),
                "writable": writable,
            })

    # Prompt 2 — any other plain-markdown folder?
    add_plain = _yes_no("Any other plain markdown folder to connect?", default=False)
    if add_plain:
        path = _prompt("  Folder path (Enter = skip)")
        if path:
            name = _prompt("  Give this source a name", default="notes")
            writable = _yes_no("  Allow Makakoo to write into it?", default=False)
            pending_adds.append({
                "name": name,
                "role": "enrichment",
                "type": "plain",
                "path": path,
                "writable": writable,
            })

    # Summary + final confirmation
    if not pending_adds:
        _ensure_default_logseq()
        if pending_obsidian_profile:
            written = _ensure_obsidian_profile()
            if written:
                print("Added Obsidian UI defaults:")
                for rel in written:
                    print(f"  - {rel}")
            else:
                print("Existing Obsidian config left untouched.")
        print("\nNothing to change. Default Makakoo Brain source is registered and writable:")
        print(f"  {_display_brain_path()}")
        return 0

    print("\nPending changes")
    print("-" * 20)
    for entry in pending_adds:
        flag = "writable" if entry["writable"] else "read-only"
        print(f"  + register {entry['name']!r} ({entry['type']}, enrichment, {flag}) → {entry['path']}")
    if pending_obsidian_profile:
        print("  + add Obsidian UI defaults to canonical Brain if missing")
    print()

    if not _yes_no("Save these changes?", default=True):
        print("Aborted. No changes written.")
        return 0

    # Commit phase — one atomic config write for the whole batch.
    try:
        cfg.apply_changes(pending_adds, None)
    except Exception as e:
        print(f"  Failed to save Brain source changes: {e}", file=sys.stderr)
        print("\nBrain setup finished with errors:", file=sys.stderr)
        print(f"  - {e}", file=sys.stderr)
        return 1

    committed = [entry["name"] for entry in pending_adds]
    for name in committed:
        print(f"  Registered {name!r}")
    print("  Canonical Brain remains 'default'. External sources are enrichment.")
    if pending_obsidian_profile:
        written = _ensure_obsidian_profile()
        if written:
            print("  Added Obsidian UI defaults:")
            for rel in written:
                print(f"    - {rel}")
        else:
            print("  Existing Obsidian config left untouched.")

    # Post-write sync — walk each newly-registered source so the user sees counts
    if committed:
        print("\nInitial sync (dry walk, no DB writes)")
        print("-" * 20)
        for name in committed:
            print(f"  {_dry_sync(name)}")

    print(f"\nDone. Config saved to {cfg.config_path()}")
    print("Change anything later: `makakoo brain list|add|remove`, or rerun this")
    print("section with `makakoo setup brain`.\n")
    return 0


if __name__ == "__main__":
    sys.exit(run_interactive())
