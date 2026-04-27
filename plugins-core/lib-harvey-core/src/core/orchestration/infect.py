"""
⚠️ DEPRECATED — Use `makakoo infect --global` (Rust) for global bootstrap.
Use `makakoo install` for full distro + daemon + infect orchestration.
Per-project infect is not yet ported to Rust — this file still handles that.

infect — Phase 4 (CLI entry) of harvey:infect.

Glues the three phases (host_detector → context_shadow → mcp_registrar)
into one user-facing command. This is what `harvey:infect` invokes.

Usage:
    python3 -m core.orchestration.infect [--dry-run] [--status] [--revoke] [--refresh]

Default action is the full run: detect, register MCP, drop shadow context.
Each subcommand below short-circuits to a specific phase or operation.

Exit codes:
    0   success (or no-op)
    1   user-resolvable error (host uncertain without --confirm)
    2   internal error
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from dataclasses import asdict
from pathlib import Path

# Local imports — relative when run as a module, absolute when run directly
try:
    from .context_shadow import ContextShadow, detect_project_root
    from .host_detector import HostType, detect_host_strict, HostUncertain
    from .mcp_registrar import MCPRegistrar, RegistrationStatus
    from .infect_global import GlobalInfector, SlotStatus, BLOCK_VERSION
except ImportError:
    HARVEY_HOME = os.environ.get("HARVEY_HOME") or os.path.expanduser("~/MAKAKOO")
    from core.orchestration.context_shadow import ContextShadow, detect_project_root
    from core.orchestration.host_detector import HostType, detect_host_strict, HostUncertain
    from core.orchestration.mcp_registrar import MCPRegistrar, RegistrationStatus
    from core.orchestration.infect_global import GlobalInfector, SlotStatus, BLOCK_VERSION


def _print(msg: str, *, quiet: bool = False, file=None) -> None:
    if not quiet:
        print(msg, file=file or sys.stdout)


def cmd_run(args: argparse.Namespace) -> int:
    """Default action: detect → register → shadow."""
    project = detect_project_root()
    _print(f"harvey:infect → project: {project}", quiet=args.quiet)

    # Phase 1: detect host
    try:
        host = detect_host_strict()
    except HostUncertain as e:
        _print(
            f"ERROR: {e}\n"
            "Re-run with CONFIRM_HOST=1 to accept the suggestion, "
            "or set the host explicitly via --host.",
            file=sys.stderr,
        )
        return 1

    _print(
        f"  host: {host.name.value} (confidence={host.confidence:.2f}, "
        f"signals={','.join(host.signals_matched)})",
        quiet=args.quiet,
    )

    if host.name == HostType.UNKNOWN:
        _print(
            "  no recognized host detected — context shadow only, no MCP register",
            quiet=args.quiet,
        )

    if args.dry_run:
        _print("DRY RUN — no files written", quiet=args.quiet)
        return 0

    # Phase 2: context shadow
    shadow = ContextShadow()
    shadow_result = shadow.refresh(
        project_path=project,
        host_name=host.name.value,
        force=args.refresh,
    )
    if shadow_result.refreshed:
        _print(
            f"  shadow: wrote {shadow_result.data_file.name} "
            f"({shadow_result.entries_included} entries, "
            f"{shadow_result.entries_rejected} rejected)",
            quiet=args.quiet,
        )
        for marker in shadow_result.marker_files:
            _print(f"  marker: {marker.name}", quiet=args.quiet)
        if shadow_result.gitignore_updated:
            _print("  gitignore: added .harvey/", quiet=args.quiet)
    else:
        _print("  shadow: fresh (no refresh needed)", quiet=args.quiet)

    # Phase 3: MCP register (skip if host has no MCP support or unknown)
    if host.name == HostType.UNKNOWN:
        return 0
    if "mcp" not in host.capabilities:
        _print(f"  mcp: unsupported on {host.name.value} — skipping", quiet=args.quiet)
        return 0

    registrar = MCPRegistrar()
    reg_result = registrar.register(host)
    if reg_result.status == RegistrationStatus.REGISTERED:
        _print(f"  mcp: registered → {reg_result.config_path}", quiet=args.quiet)
    elif reg_result.status == RegistrationStatus.ALREADY_REGISTERED:
        _print(f"  mcp: already registered ({reg_result.config_path})", quiet=args.quiet)
    elif reg_result.status == RegistrationStatus.UPDATED:
        _print(f"  mcp: updated → {reg_result.config_path}", quiet=args.quiet)
    elif reg_result.status == RegistrationStatus.UNSUPPORTED:
        _print(f"  mcp: {reg_result.error}", quiet=args.quiet)
    else:
        _print(f"  mcp: ERROR — {reg_result.error}", file=sys.stderr)
        return 2

    return 0


def cmd_status(args: argparse.Namespace) -> int:
    """Show current host detection + registration state + shadow file age."""
    project = detect_project_root()
    try:
        host = detect_host_strict()
    except HostUncertain as e:
        host = e.host_info  # show the suggestion anyway

    registrar = MCPRegistrar()
    is_reg = (
        registrar.is_registered(host)
        if host.name not in (HostType.UNKNOWN, HostType.CODEX)
        else False
    )

    data_file = Path(project) / ".harvey" / "context.md"
    shadow_age = "absent"
    if data_file.exists():
        import time
        age_s = time.time() - data_file.stat().st_mtime
        shadow_age = f"{int(age_s)}s old"

    out = {
        "project": project,
        "host": host.name.value,
        "host_confidence": round(host.confidence, 2),
        "host_signals": host.signals_matched,
        "host_capabilities": host.capabilities,
        "mcp_registered": is_reg,
        "shadow_file": str(data_file),
        "shadow_age": shadow_age,
    }
    print(json.dumps(out, indent=2))
    return 0


def cmd_revoke(args: argparse.Namespace) -> int:
    """Remove shadow + unregister MCP. Exit rights are non-negotiable."""
    project = detect_project_root()
    try:
        host = detect_host_strict()
    except HostUncertain as e:
        host = e.host_info

    shadow = ContextShadow()
    shadow_removed = shadow.revoke(project)
    _print(
        f"shadow: removed data_file={shadow_removed['data_file']}, "
        f"markers={len(shadow_removed['marker_files'])}",
        quiet=args.quiet,
    )

    if host.name not in (HostType.UNKNOWN, HostType.CODEX):
        registrar = MCPRegistrar()
        registrar.unregister(host)
        _print(f"mcp: unregistered from {host.name.value}", quiet=args.quiet)

    return 0


def cmd_global(args: argparse.Namespace) -> int:
    """Install the Harvey Bootstrap Block into every global CLI config slot."""
    infector = GlobalInfector()
    results = infector.install_all(dry_run=args.dry_run)

    _print(f"harvey:infect --global → bootstrap v{BLOCK_VERSION}", quiet=args.quiet)
    any_error = False
    for r in results:
        host = r.label
        if r.status == SlotStatus.INSTALLED:
            _print(f"  {host}: installed → {r.path}", quiet=args.quiet)
        elif r.status == SlotStatus.UPDATED:
            _print(f"  {host}: updated v{r.previous_version} → v{r.version} ({r.path})", quiet=args.quiet)
        elif r.status == SlotStatus.UNCHANGED:
            _print(f"  {host}: already v{r.version} (no change)", quiet=args.quiet)
        elif r.status == SlotStatus.DRY_RUN:
            _print(f"  {host}: would install/update → {r.path}", quiet=args.quiet)
        elif r.status == SlotStatus.SKIPPED:
            _print(f"  {host}: skipped — {r.error}", quiet=args.quiet)
        elif r.status == SlotStatus.ERROR:
            _print(f"  {host}: ERROR — {r.error}", file=sys.stderr)
            any_error = True
        else:
            _print(f"  {host}: {r.status.value}", quiet=args.quiet)
    return 2 if any_error else 0


def cmd_revoke_global(args: argparse.Namespace) -> int:
    """Strip the Harvey Bootstrap Block from every global CLI config slot."""
    infector = GlobalInfector()
    results = infector.revoke_all(dry_run=args.dry_run)

    _print("harvey:infect --revoke-global", quiet=args.quiet)
    any_error = False
    for r in results:
        host = r.label
        if r.status == SlotStatus.REVOKED:
            _print(f"  {host}: revoked (was v{r.previous_version}) → {r.path}", quiet=args.quiet)
        elif r.status == SlotStatus.ABSENT:
            _print(f"  {host}: absent (nothing to revoke)", quiet=args.quiet)
        elif r.status == SlotStatus.DRY_RUN:
            _print(f"  {host}: would revoke v{r.previous_version} → {r.path}", quiet=args.quiet)
        elif r.status == SlotStatus.ERROR:
            _print(f"  {host}: ERROR — {r.error}", file=sys.stderr)
            any_error = True
        else:
            _print(f"  {host}: {r.status.value}", quiet=args.quiet)
    return 2 if any_error else 0


def cmd_status_global(args: argparse.Namespace) -> int:
    """Show which global CLI slots are currently infected + at what version."""
    infector = GlobalInfector()
    results = infector.status_all()
    out = {
        "bootstrap_version": BLOCK_VERSION,
        "slots": [
            {
                "host": r.label,
                "path": str(r.path),
                "status": r.status.value,
                "version": r.version,
                "error": r.error,
            }
            for r in results
        ],
    }
    print(json.dumps(out, indent=2))
    return 0


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        prog="harvey:infect",
        description="Cooperative host enhancement — detect CLI, install MCP, drop context shadow.",
    )
    parser.add_argument("--dry-run", action="store_true", help="Show what would happen, don't write")
    parser.add_argument("--status", action="store_true", help="Show current state and exit")
    parser.add_argument("--revoke", action="store_true", help="Remove all infect artifacts")
    parser.add_argument("--refresh", action="store_true", help="Force-refresh shadow file even if fresh")
    parser.add_argument("--quiet", "-q", action="store_true", help="Suppress non-error output")
    # Global-parasite subcommands (second strain of the infect fungus)
    parser.add_argument("--global", dest="global_install", action="store_true",
                        help="Install Harvey Bootstrap Block into every global CLI config slot")
    parser.add_argument("--revoke-global", dest="revoke_global", action="store_true",
                        help="Strip Harvey Bootstrap Block from every global CLI config slot")
    parser.add_argument("--status-global", dest="status_global", action="store_true",
                        help="Show which global CLI slots are infected and at what version")

    args = parser.parse_args(argv)

    if args.status_global:
        return cmd_status_global(args)
    if args.revoke_global:
        return cmd_revoke_global(args)
    if args.global_install:
        return cmd_global(args)
    if args.status:
        return cmd_status(args)
    if args.revoke:
        return cmd_revoke(args)
    return cmd_run(args)


if __name__ == "__main__":
    raise SystemExit(main())
