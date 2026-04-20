"""Mascot patrol functions — one useful chore per nursery creature.

Each function is standalone, safe to call every SANCHO tick, and returns
a JSON-serializable dict. No I/O outside the platform home; no network.

Python 3.9 compatible (no PEP 604 unions, no `match` statements).
"""

from __future__ import annotations

import ast
import os
import re
import subprocess
import sys
from dataclasses import asdict, dataclass, field
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Tuple

# ─── Config ─────────────────────────────────────────────────────

# Look-back window for "recent" changes. Kept conservative so every
# tick has something to work with without thrashing when Harvey is idle.
_RECENT_WINDOW_HOURS = 24
_STALE_TODO_AGE_DAYS = 14

# Max number of findings reported per mascot. Prevents a bad day from
# producing a 10k-line SANCHO result dict.
_MAX_FINDINGS_PER_MASCOT = 50

# Patterns that identify error-ish lines in daemon logs. Kept broad
# enough to catch pytest-style tracebacks, httpx errors, and plain
# ERROR lines from the stdlib logger.
_LOG_ERROR_PATTERNS = [
    re.compile(r"Traceback \(most recent call last\)"),
    re.compile(r"^\s*\w+(?:\.\w+)*Error: "),
    re.compile(r"\bERROR\b[^A-Z]"),
    re.compile(r"^\s*raise \w+"),
    re.compile(r"^\s*FAIL\b"),
    re.compile(r"401 Unauthorized"),
]

_TODO_MARKER_RE = re.compile(r"\b(TODO|FIXME|XXX|\?\?\?)\b")

# ─── Model ──────────────────────────────────────────────────────


@dataclass
class MascotFinding:
    """A single finding from one mascot patrol.

    `severity` is "info" | "warn" | "error", chosen so daily_briefing
    can surface error-level findings without parsing everything.
    """

    mascot: str
    kind: str
    severity: str
    path: str = ""
    line: int = 0
    message: str = ""
    extra: Dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        """Return a JSON-serializable dict representation of this finding."""
        return asdict(self)


# ─── Path helpers ───────────────────────────────────────────────


def _platform_home() -> Path:
    """Resolve the Makakoo home directory, tolerating legacy env."""
    for env_var in ("MAKAKOO_HOME", "HARVEY_HOME"):
        val = os.environ.get(env_var)
        if val:
            return Path(os.path.expanduser(val))
    return Path.home() / "MAKAKOO"


def _recent_cutoff() -> datetime:
    return datetime.now() - timedelta(hours=_RECENT_WINDOW_HOURS)


def _stale_cutoff() -> datetime:
    return datetime.now() - timedelta(days=_STALE_TODO_AGE_DAYS)


def _iter_py_files(root: Path) -> Iterable[Path]:
    """Yield every .py file under `root`, skipping caches, vendored libs, and venvs."""
    skip_parts = {
        "__pycache__",
        ".git",
        "node_modules",
        ".venv",
        "venv",
        ".pytest_cache",
        "site-packages",
        "build",
        "dist",
    }
    if not root.exists():
        return
    for dirpath, dirnames, filenames in os.walk(root):
        # Mutate dirnames in-place to prune traversal
        dirnames[:] = [d for d in dirnames if d not in skip_parts]
        for fn in filenames:
            if fn.endswith(".py"):
                yield Path(dirpath) / fn


def _recently_modified(root: Path) -> List[Path]:
    """Return .py files under root whose mtime is within the recent window."""
    cutoff = _recent_cutoff().timestamp()
    out: List[Path] = []
    for p in _iter_py_files(root):
        try:
            if p.stat().st_mtime >= cutoff:
                out.append(p)
        except OSError:
            continue
    return out


# ─── Pixel — daemon log error scan ──────────────────────────────


def pixel_daemon_scan(home: Optional[Path] = None, tail_lines: int = 300) -> Dict[str, Any]:
    """Tail every known daemon log and flag fresh error lines.

    Pixel hums while Harvey debugs, so this is her chore. She only looks at
    the last `tail_lines` lines of each log (fast, bounded), greps them for
    error patterns, and returns hits grouped by daemon.
    """
    home = home or _platform_home()

    # Known daemon log locations. Missing logs are skipped without raising.
    log_paths = [
        ("btc-sniper", home / "data" / "logs" / "btc_5m_sniper.log"),
        ("sancho", home / "tmp" / "sancho.log"),
        ("harveychat", home / "data" / "chat" / "harveychat.log"),
    ]

    findings: List[MascotFinding] = []
    summary: Dict[str, Dict[str, Any]] = {}

    for name, path in log_paths:
        entry: Dict[str, Any] = {"log_path": str(path), "present": False, "error_hits": 0}
        summary[name] = entry
        if not path.is_file():
            continue
        entry["present"] = True
        try:
            # Read the last `tail_lines` lines without loading the whole file.
            with path.open("rb") as f:
                f.seek(0, os.SEEK_END)
                size = f.tell()
                read_bytes = min(size, max(tail_lines, 1) * 400)
                f.seek(size - read_bytes)
                tail = f.read().decode("utf-8", errors="replace").splitlines()[-tail_lines:]
        except OSError as exc:
            entry["error"] = f"cannot read: {exc}"
            continue

        hits = 0
        for i, line in enumerate(tail):
            for pat in _LOG_ERROR_PATTERNS:
                if pat.search(line):
                    hits += 1
                    if len(findings) < _MAX_FINDINGS_PER_MASCOT:
                        findings.append(
                            MascotFinding(
                                mascot="Pixel",
                                kind="daemon_error",
                                severity="warn" if hits < 10 else "error",
                                path=str(path),
                                line=i + 1,
                                message=line.strip()[:200],
                                extra={"daemon": name},
                            )
                        )
                    break
        entry["error_hits"] = hits

    total_hits = sum(s["error_hits"] for s in summary.values())
    flavor = (
        "Pixel hums ominously: no error music today."
        if total_hits == 0
        else f"Pixel stops humming mid-note — {total_hits} error lines in daemon tails."
    )

    return {
        "mascot": "Pixel",
        "kind": "daemon_error_scan",
        "total_hits": total_hits,
        "summary": summary,
        "findings": [f.to_dict() for f in findings],
        "flavor": flavor,
    }


# ─── Cinder — python syntax check ───────────────────────────────


def cinder_syntax_check(home: Optional[Path] = None) -> Dict[str, Any]:
    """Byte-compile every .py file changed in the last 24h.

    Cinder is relentlessly optimistic and afraid of semicolons, so she runs
    py_compile — the cheapest way to catch a typo before it lands in a live
    daemon. Only scans the makakoo-owned tree to avoid compiling vendored
    site-packages.
    """
    home = home or _platform_home()
    scan_roots = [
        home / "plugins-core" / "lib-harvey-core" / "src" / "core",
        home / "plugins-core",
        home / "bin",
        home / "agents",
        home / "tests",
    ]

    candidates: List[Path] = []
    for root in scan_roots:
        candidates.extend(_recently_modified(root))

    findings: List[MascotFinding] = []
    ok = 0
    failed = 0
    for path in candidates:
        try:
            source = path.read_text(encoding="utf-8")
        except OSError as exc:
            findings.append(
                MascotFinding(
                    mascot="Cinder",
                    kind="read_error",
                    severity="warn",
                    path=str(path),
                    message=str(exc),
                )
            )
            failed += 1
            continue
        try:
            compile(source, str(path), "exec")
            ok += 1
        except SyntaxError as exc:
            failed += 1
            if len(findings) < _MAX_FINDINGS_PER_MASCOT:
                findings.append(
                    MascotFinding(
                        mascot="Cinder",
                        kind="syntax_error",
                        severity="error",
                        path=str(path),
                        line=exc.lineno or 0,
                        message=f"{exc.msg} (offset {exc.offset or 0})",
                    )
                )

    flavor = (
        "Cinder hops gleefully — every .py file compiles. No semicolons in sight."
        if failed == 0
        else f"Cinder recoils — {failed} file(s) won't compile. She'll wait until we fix them."
    )

    return {
        "mascot": "Cinder",
        "kind": "syntax_check",
        "files_checked": len(candidates),
        "ok": ok,
        "failed": failed,
        "findings": [f.to_dict() for f in findings],
        "flavor": flavor,
    }


# ─── Ziggy — docstring coverage patrol ──────────────────────────


def ziggy_doc_patrol(home: Optional[Path] = None) -> Dict[str, Any]:
    """Find public defs/classes without docstrings in recently-modified files.

    Ziggy hoards documentation, so undocumented public code offends him
    personally. Uses ast so it's fast and never runs arbitrary imports.
    """
    home = home or _platform_home()
    scan_roots = [
        home / "plugins-core" / "lib-harvey-core" / "src" / "core",
        home / "plugins-core",
        home / "agents",
    ]

    candidates: List[Path] = []
    for root in scan_roots:
        candidates.extend(_recently_modified(root))

    findings: List[MascotFinding] = []
    files_scanned = 0
    symbols_checked = 0
    missing = 0

    for path in candidates:
        try:
            source = path.read_text(encoding="utf-8")
            tree = ast.parse(source, filename=str(path))
        except (OSError, SyntaxError):
            continue
        files_scanned += 1

        for node in ast.walk(tree):
            if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                continue
            # Private names are excused — Ziggy only judges public API.
            if node.name.startswith("_"):
                continue
            symbols_checked += 1
            if ast.get_docstring(node):
                continue
            missing += 1
            if len(findings) < _MAX_FINDINGS_PER_MASCOT:
                kind_label = (
                    "class" if isinstance(node, ast.ClassDef) else "function"
                )
                findings.append(
                    MascotFinding(
                        mascot="Ziggy",
                        kind="missing_docstring",
                        severity="info",
                        path=str(path),
                        line=node.lineno,
                        message=f"{kind_label} `{node.name}` has no docstring",
                        extra={"symbol": node.name, "symbol_kind": kind_label},
                    )
                )

    if symbols_checked == 0:
        coverage_pct = 100.0
    else:
        coverage_pct = round((symbols_checked - missing) / symbols_checked * 100.0, 1)

    if missing == 0 and symbols_checked > 0:
        flavor = f"Ziggy purrs. {symbols_checked} public symbols, all documented. Rare."
    elif missing == 0:
        flavor = "Ziggy shrugs — nothing recent to judge."
    else:
        flavor = (
            f"Ziggy sighs loudly: {missing} of {symbols_checked} public symbols "
            f"have no docstring ({coverage_pct}% covered)."
        )

    return {
        "mascot": "Ziggy",
        "kind": "doc_patrol",
        "files_scanned": files_scanned,
        "symbols_checked": symbols_checked,
        "missing_docstrings": missing,
        "coverage_pct": coverage_pct,
        "findings": [f.to_dict() for f in findings],
        "flavor": flavor,
    }


# ─── Glimmer — stale TODO / FIXME patrol ────────────────────────


def glimmer_stale_todos(home: Optional[Path] = None) -> Dict[str, Any]:
    """Find Brain-page TODO / FIXME / ??? / XXX markers older than N days.

    Glimmer is patient and curious, so she's the one who notices when a
    note has been sitting unfinished for two weeks. She only looks at
    `data/Brain/pages/` — journals are historical and get a pass.
    """
    home = home or _platform_home()
    pages_dir = home / "data" / "Brain" / "pages"

    findings: List[MascotFinding] = []
    files_with_stale = 0
    total_stale = 0
    files_scanned = 0

    if not pages_dir.is_dir():
        return {
            "mascot": "Glimmer",
            "kind": "stale_todo_patrol",
            "pages_dir": str(pages_dir),
            "files_scanned": 0,
            "files_with_stale": 0,
            "total_stale": 0,
            "findings": [],
            "flavor": "Glimmer blinks. No Brain pages directory to explore.",
        }

    cutoff_ts = _stale_cutoff().timestamp()

    for path in sorted(pages_dir.glob("*.md")):
        files_scanned += 1
        try:
            stat = path.stat()
            # Only flag a page as stale if the file's mtime is older than the
            # cutoff — a fresh edit means the TODO was just written.
            if stat.st_mtime > cutoff_ts:
                continue
            content = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue

        file_had_stale = False
        for line_idx, line in enumerate(content.splitlines(), 1):
            m = _TODO_MARKER_RE.search(line)
            if not m:
                continue
            file_had_stale = True
            total_stale += 1
            if len(findings) < _MAX_FINDINGS_PER_MASCOT:
                age_days = int((datetime.now().timestamp() - stat.st_mtime) / 86400)
                findings.append(
                    MascotFinding(
                        mascot="Glimmer",
                        kind="stale_todo",
                        severity="info",
                        path=str(path),
                        line=line_idx,
                        message=line.strip()[:200],
                        extra={"marker": m.group(1), "age_days": age_days},
                    )
                )
        if file_had_stale:
            files_with_stale += 1

    if total_stale == 0:
        flavor = "Glimmer beams — no stale notes. Her patience is rewarded."
    else:
        flavor = (
            f"Glimmer hums with interest: {total_stale} stale TODO markers "
            f"across {files_with_stale} Brain page(s). The oldest deserve a second look."
        )

    return {
        "mascot": "Glimmer",
        "kind": "stale_todo_patrol",
        "pages_dir": str(pages_dir),
        "files_scanned": files_scanned,
        "files_with_stale": files_with_stale,
        "total_stale": total_stale,
        "findings": [f.to_dict() for f in findings],
        "flavor": flavor,
    }


# ─── All four — unified entry point ─────────────────────────────


def patrol_all(home: Optional[Path] = None) -> Dict[str, Any]:
    """Run every mascot patrol and return a combined report.

    Order is deterministic: Pixel, Cinder, Ziggy, Glimmer. Each patrol is
    independent; one mascot failing must not block the others. A catch-all
    captures any patrol-level exception and stores it under
    `errors[<mascot>]` so SANCHO's tick report still lands cleanly.
    """
    home = home or _platform_home()
    patrols = [
        ("pixel", pixel_daemon_scan),
        ("cinder", cinder_syntax_check),
        ("ziggy", ziggy_doc_patrol),
        ("glimmer", glimmer_stale_todos),
    ]

    results: Dict[str, Any] = {}
    errors: Dict[str, str] = {}
    for key, fn in patrols:
        try:
            results[key] = fn(home=home)
        except Exception as exc:  # noqa: BLE001 — patrol must not crash SANCHO
            errors[key] = f"{type(exc).__name__}: {exc}"

    # Aggregate critical-signal counters so daily_briefing can surface them
    # without walking the whole findings list.
    aggregate = {
        "daemon_error_hits": results.get("pixel", {}).get("total_hits", 0),
        "syntax_failures": results.get("cinder", {}).get("failed", 0),
        "missing_docstrings": results.get("ziggy", {}).get("missing_docstrings", 0),
        "stale_todos": results.get("glimmer", {}).get("total_stale", 0),
    }
    aggregate["critical"] = bool(
        aggregate["daemon_error_hits"] > 0 or aggregate["syntax_failures"] > 0
    )

    return {
        "mascot_patrol": True,
        "results": results,
        "aggregate": aggregate,
        "errors": errors,
    }
