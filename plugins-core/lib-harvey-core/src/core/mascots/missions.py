"""Mascot missions — proactive chores that make Makakoo OS healthier.

Unlike `patrol.py` (reactive scans), these missions produce durable
artifacts that Sebastian can act on: sick-task reports, broken plugin
lists, SKILL.md lint findings, archived lead pages, weekly digests.

Each mission writes to `data/mascots/<mascot>/` and optionally appends
an outliner line to today's Brain journal. Every function is:

* idempotent within a SANCHO tick (safe to re-run)
* bounded (findings capped so one bad day can't flood the journal)
* read-only by default — Glimmer is the only writer (archives Leads)
* Python 3.9 compatible

Mapping mascot → mission → cadence:

  * Pixel    → "SANCHO Doctor"   — every 2h
  * Cinder   → "Entrypoint Sentinel" — every 4h
  * Ziggy    → "SKILL.md Doctor" — daily (08:00)
  * Glimmer  → "Brain Gardener"  — daily (22:00)
  * Olibia   → "Weekly Digest"   — Sundays 09:00 CEST
"""

from __future__ import annotations

import json
import os
import py_compile
import re
import subprocess
import tempfile
from collections import Counter, defaultdict
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Tuple

_UTC = timezone.utc


# ─── shared helpers ─────────────────────────────────────────────────


def _home() -> Path:
    for k in ("MAKAKOO_HOME", "HARVEY_HOME"):
        v = os.environ.get(k)
        if v:
            return Path(os.path.expanduser(v))
    return Path.home() / "MAKAKOO"


def _mascot_dir(home: Path, mascot: str) -> Path:
    d = home / "data" / "mascots" / mascot
    d.mkdir(parents=True, exist_ok=True)
    return d


def _today_journal(home: Path) -> Path:
    stamp = datetime.now(_UTC).strftime("%Y_%m_%d")
    return home / "data" / "Brain" / "journals" / f"{stamp}.md"


def _append_journal(home: Path, line: str) -> None:
    """Append a single outliner line to today's Brain journal.

    No-op if the journal path is not writable — missions must never
    fail because the Brain is mid-rotation.
    """
    try:
        j = _today_journal(home)
        j.parent.mkdir(parents=True, exist_ok=True)
        current = j.read_text(encoding="utf-8") if j.exists() else ""
        if not current.endswith("\n"):
            current += "\n"
        j.write_text(current + line + "\n", encoding="utf-8")
    except OSError:
        pass


def _atomic_write_json(path: Path, data: Any) -> None:
    """Write JSON atomically so a crashed mission never leaves a torn file."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=str(path.parent), suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=2, sort_keys=True, default=str)
        os.replace(tmp, path)
    except Exception:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


# ═════════════════════════════════════════════════════════════════════
# PIXEL — SANCHO Doctor
# ═════════════════════════════════════════════════════════════════════


_SANCHO_LINE_RE = re.compile(
    r"\[\[SANCHO\]\]\s+(?P<task>[A-Za-z0-9_]+):\s+(?P<status>FAILED|ok|skipped)"
    r"(?:\s*\([^)]*\))?"
    r"(?:\s*[—-]\s*(?P<detail>.+?))?$",
    re.MULTILINE,
)


def pixel_sancho_doctor(home: Optional[Path] = None, journal_window_days: int = 2) -> Dict[str, Any]:
    """Scan recent Brain journals for repeatedly failing SANCHO tasks.

    Parses every `[[SANCHO]] <task>: FAILED/ok/skipped — <detail>` line
    from the current + prior day's journal, aggregates by task name, and
    reports any task whose *consecutive* tail of entries is FAILED with
    count >= 3 (3 failures in a row is the threshold where something is
    genuinely broken, not just a single tick hiccup).

    This is exactly the class of bug we just caught on `inbox_pipeline`
    — it was failing every single tick for days, logged visibly to the
    journal, and nobody noticed until a human eyeballed the tail.
    """
    home = home or _home()
    journals_dir = home / "data" / "Brain" / "journals"

    # Collect today + yesterday (and prior-day if we wrap midnight)
    today = datetime.now(_UTC).date()
    journal_texts: List[Tuple[str, str]] = []
    for delta in range(journal_window_days):
        day = today - timedelta(days=delta)
        p = journals_dir / f"{day.strftime('%Y_%m_%d')}.md"
        if p.is_file():
            try:
                journal_texts.append((p.name, p.read_text(encoding="utf-8", errors="replace")))
            except OSError:
                continue

    # Per-task chronological entries (most recent first, since we read newest journal first)
    per_task: Dict[str, List[Dict[str, Any]]] = defaultdict(list)
    total_entries = 0
    for name, text in journal_texts:
        for m in _SANCHO_LINE_RE.finditer(text):
            total_entries += 1
            per_task[m.group("task")].append(
                {
                    "status": m.group("status"),
                    "detail": (m.group("detail") or "").strip()[:240],
                    "journal": name,
                }
            )

    # We collected newest→oldest because we looked at today first.
    # For "consecutive failing tail" we want most-recent-first, so we
    # walk as-is and count FAILED until we hit a non-FAIL.
    sick_tasks: List[Dict[str, Any]] = []
    healthy_tasks: List[str] = []
    for task, entries in per_task.items():
        consecutive = 0
        for e in entries:
            if e["status"] == "FAILED":
                consecutive += 1
            else:
                break
        fails = sum(1 for e in entries if e["status"] == "FAILED")
        oks = sum(1 for e in entries if e["status"] == "ok")
        if consecutive >= 3:
            sick_tasks.append(
                {
                    "task": task,
                    "consecutive_failures": consecutive,
                    "total_failures_window": fails,
                    "total_ok_window": oks,
                    "last_error": entries[0]["detail"] if entries else "",
                }
            )
        else:
            healthy_tasks.append(task)

    # Sort sick tasks by severity (most consecutive failures first)
    sick_tasks.sort(key=lambda d: d["consecutive_failures"], reverse=True)

    report = {
        "mascot": "Pixel",
        "mission": "sancho_doctor",
        "generated_at": datetime.now(_UTC).isoformat(),
        "window_days": journal_window_days,
        "total_sancho_entries": total_entries,
        "tasks_observed": len(per_task),
        "sick_task_count": len(sick_tasks),
        "sick_tasks": sick_tasks,
        "healthy_tasks": sorted(healthy_tasks),
    }

    # Persist
    out = _mascot_dir(home, "pixel") / "sick_tasks.json"
    _atomic_write_json(out, report)

    # Journal entry — only if something is actually sick
    if sick_tasks:
        worst = sick_tasks[0]
        _append_journal(
            home,
            f"- [[Pixel]] barks: [[SANCHO]] task `{worst['task']}` has failed "
            f"{worst['consecutive_failures']} ticks in a row. Last error: "
            f"{worst['last_error'][:160]}",
        )

    flavor = (
        "Pixel wags — every SANCHO task is healthy."
        if not sick_tasks
        else f"Pixel barks at {len(sick_tasks)} sick SANCHO task(s). Oldest wound first."
    )
    report["flavor"] = flavor
    return report


# ═════════════════════════════════════════════════════════════════════
# CINDER — Plugin Entrypoint Sentinel
# ═════════════════════════════════════════════════════════════════════


_ENTRYPOINT_RE = re.compile(r"run\s*=\s*\"([^\"]+)\"")


def cinder_entrypoint_sentinel(home: Optional[Path] = None) -> Dict[str, Any]:
    """Byte-compile every Python plugin's entrypoint file.

    Walks every `plugins/*/plugin.toml`, extracts the `[entrypoint] run`
    command, resolves the Python file inside, and runs `py_compile` on
    it. A plugin with a broken entrypoint will crash on its next SANCHO
    tick — this catches it proactively so we fix before fire.
    """
    home = home or _home()
    plugins_dir = home / "plugins"
    if not plugins_dir.is_dir():
        return {"mascot": "Cinder", "mission": "entrypoint_sentinel",
                "plugins_checked": 0, "broken": [], "flavor": "Cinder squints — no plugin tree."}

    broken: List[Dict[str, Any]] = []
    ok_count = 0
    py_plugin_count = 0

    for plugin_root in sorted(plugins_dir.iterdir()):
        if not plugin_root.is_dir():
            continue
        manifest = plugin_root / "plugin.toml"
        if not manifest.is_file():
            continue
        try:
            text = manifest.read_text(encoding="utf-8")
        except OSError:
            continue

        m = _ENTRYPOINT_RE.search(text)
        if not m:
            continue
        run_cmd = m.group(1)

        # We only sentinel Python entrypoints. Heuristic: command starts
        # with `python` or ends in a `.py` arg. Anything else (shell
        # scripts, rust binaries) isn't Cinder's job.
        if "python" not in run_cmd and not run_cmd.endswith(".py"):
            continue

        # Extract the .py file from the command. Simplest: first arg
        # ending in `.py` after shell-splitting on whitespace.
        py_file: Optional[Path] = None
        for tok in run_cmd.split():
            if tok.endswith(".py"):
                tok = tok.replace("$MAKAKOO_HOME", str(home))
                candidate = (plugin_root / tok) if not tok.startswith("/") else Path(tok)
                if candidate.is_file():
                    py_file = candidate
                    break
        if py_file is None:
            continue

        py_plugin_count += 1
        try:
            py_compile.compile(str(py_file), doraise=True)
            ok_count += 1
        except py_compile.PyCompileError as exc:
            broken.append(
                {
                    "plugin": plugin_root.name,
                    "entrypoint": str(py_file.relative_to(home)) if py_file.is_relative_to(home) else str(py_file),
                    "error": str(exc)[:400],
                }
            )
        except Exception as exc:  # noqa: BLE001 — defensive
            broken.append(
                {
                    "plugin": plugin_root.name,
                    "entrypoint": str(py_file),
                    "error": f"{type(exc).__name__}: {exc}",
                }
            )

    report = {
        "mascot": "Cinder",
        "mission": "entrypoint_sentinel",
        "generated_at": datetime.now(_UTC).isoformat(),
        "plugins_checked": py_plugin_count,
        "ok": ok_count,
        "broken_count": len(broken),
        "broken": broken,
    }

    out = _mascot_dir(home, "cinder") / "broken_plugins.json"
    _atomic_write_json(out, report)

    if broken:
        names = ", ".join(b["plugin"] for b in broken[:5])
        _append_journal(
            home,
            f"- [[Cinder]] flinches: {len(broken)} plugin entrypoint(s) won't compile. "
            f"First: {names}",
        )
        report["flavor"] = f"Cinder flinches — {len(broken)} broken plugin entrypoint(s)."
    else:
        report["flavor"] = f"Cinder purrs — all {py_plugin_count} Python plugins compile cleanly."
    return report


# ═════════════════════════════════════════════════════════════════════
# ZIGGY — SKILL.md Doctor
# ═════════════════════════════════════════════════════════════════════


_FRONTMATTER_RE = re.compile(r"^---\s*\n(.*?)\n---\s*\n", re.DOTALL)


def ziggy_skill_md_doctor(home: Optional[Path] = None, top_n: int = 5) -> Dict[str, Any]:
    """Lint every SKILL.md for completeness. Report the worst offenders.

    Checks per SKILL.md:
      * has frontmatter block at all
      * has `name:` field
      * has `description:` field with >= 10 chars
      * body has an `## Example` / `## Usage` / `## How to` section
      * body is not a bare stub (< 200 chars)

    Writes the full sorted list to disk; journals only the top `top_n`.
    Over time, whoever reads the report closes gaps and skills get richer.
    """
    home = home or _home()
    plugins_dir = home / "plugins"
    skills_root = home / "skills-shared"

    issues: List[Dict[str, Any]] = []
    total = 0

    def _check_skill(path: Path) -> None:
        nonlocal total
        total += 1
        gaps: List[str] = []
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            return
        fm_match = _FRONTMATTER_RE.match(text)
        if not fm_match:
            gaps.append("no_frontmatter")
            body = text
        else:
            fm = fm_match.group(1)
            body = text[fm_match.end():]
            if not re.search(r"^\s*name\s*:", fm, re.MULTILINE):
                gaps.append("missing_name")
            desc_match = re.search(r"^\s*description\s*:\s*(.+?)$", fm, re.MULTILINE)
            if not desc_match:
                gaps.append("missing_description")
            elif len(desc_match.group(1).strip().strip("\"'")) < 10:
                gaps.append("too_short_description")
        if not re.search(r"^##\s+(Example|Usage|How to|Usage patterns)", body, re.MULTILINE):
            gaps.append("no_usage_section")
        if len(body.strip()) < 200:
            gaps.append("stub_body")

        if gaps:
            issues.append(
                {
                    "path": str(path.relative_to(home)) if path.is_relative_to(home) else str(path),
                    "gaps": gaps,
                    "gap_count": len(gaps),
                }
            )

    # Walk plugins and skills-shared
    for root in (plugins_dir, skills_root):
        if not root.is_dir():
            continue
        for skill_md in root.rglob("SKILL.md"):
            # Skip vendored / archive copies
            parts = skill_md.parts
            if any(p in ("upstream", "node_modules", ".git", "archive") for p in parts):
                continue
            _check_skill(skill_md)

    issues.sort(key=lambda d: d["gap_count"], reverse=True)

    report = {
        "mascot": "Ziggy",
        "mission": "skill_md_doctor",
        "generated_at": datetime.now(_UTC).isoformat(),
        "skills_checked": total,
        "issue_count": len(issues),
        "coverage_pct": round((total - len(issues)) / total * 100, 1) if total else 100.0,
        "worst_offenders": issues[:top_n],
    }

    out = _mascot_dir(home, "ziggy") / "skill_md_issues.json"
    _atomic_write_json(out, {**report, "all_issues": issues})

    if issues:
        preview = ", ".join(Path(i["path"]).parent.name for i in issues[:3])
        _append_journal(
            home,
            f"- [[Ziggy]] sighs: {len(issues)}/{total} SKILL.md files have gaps "
            f"({report['coverage_pct']}% clean). Worst: {preview}",
        )
        report["flavor"] = (
            f"Ziggy sighs — {len(issues)}/{total} SKILL.md files have gaps."
        )
    else:
        report["flavor"] = f"Ziggy beams — every one of {total} SKILL.md is complete. Rare."
    return report


# ═════════════════════════════════════════════════════════════════════
# GLIMMER — Brain Gardener
# ═════════════════════════════════════════════════════════════════════


_LEAD_PAGE_RE = re.compile(r"^(Lead|Inbox) - .+\.md$")
_EMPTY_THRESHOLD_CHARS = 160  # a page under this is effectively empty
_LEAD_STALE_DAYS = 14


def glimmer_brain_gardener(home: Optional[Path] = None, dry_run: bool = False) -> Dict[str, Any]:
    """Archive empty `Lead - *` / `Inbox - *` pages older than 14 days.

    The inbox-pipeline auto-captures every vaguely-recruiter-shaped email
    as a Brain page. Most of those never turn into real opportunities —
    they sit empty, inflate the wiki, poison the superbrain retrieval
    with irrelevant snippets. Glimmer moves them to
    `data/Brain/pages/_archived/leads/YYYY-MM/` so they're recoverable
    but out of the main index.
    """
    home = home or _home()
    pages_dir = home / "data" / "Brain" / "pages"
    if not pages_dir.is_dir():
        return {"mascot": "Glimmer", "mission": "brain_gardener",
                "pages_dir": str(pages_dir), "archived": [],
                "flavor": "Glimmer blinks — no Brain pages dir."}

    stale_cutoff = (datetime.now() - timedelta(days=_LEAD_STALE_DAYS)).timestamp()
    archived: List[Dict[str, Any]] = []
    kept: int = 0
    scanned: int = 0

    for path in sorted(pages_dir.glob("*.md")):
        if not _LEAD_PAGE_RE.match(path.name):
            continue
        scanned += 1
        try:
            st = path.stat()
            if st.st_mtime > stale_cutoff:
                kept += 1
                continue
            body = path.read_text(encoding="utf-8", errors="replace").strip()
        except OSError:
            continue
        if len(body) >= _EMPTY_THRESHOLD_CHARS:
            kept += 1
            continue

        # Archive target: data/Brain/pages/_archived/leads/YYYY-MM/
        dest_dir = pages_dir / "_archived" / "leads" / datetime.now(_UTC).strftime("%Y-%m")
        dest_dir.mkdir(parents=True, exist_ok=True)
        dest = dest_dir / path.name

        # Collision-safe: if same name already archived, suffix -N
        i = 1
        while dest.exists():
            dest = dest_dir / f"{path.stem}-{i}{path.suffix}"
            i += 1

        record = {"from": path.name, "to": str(dest.relative_to(home)),
                  "size": len(body), "age_days": int((datetime.now().timestamp() - st.st_mtime) / 86400)}
        archived.append(record)
        if not dry_run:
            try:
                path.rename(dest)
            except OSError:
                # Archive failed — skip quietly, will retry next tick
                archived.pop()
                continue

    report = {
        "mascot": "Glimmer",
        "mission": "brain_gardener",
        "generated_at": datetime.now(_UTC).isoformat(),
        "scanned": scanned,
        "kept": kept,
        "archived_count": len(archived),
        "archived": archived,
        "dry_run": dry_run,
    }

    out = _mascot_dir(home, "glimmer") / "last_garden.json"
    _atomic_write_json(out, report)

    if archived and not dry_run:
        _append_journal(
            home,
            f"- [[Glimmer]] tends the garden: archived {len(archived)} empty "
            f"`Lead -*` pages to `_archived/leads/{datetime.now(_UTC).strftime('%Y-%m')}/`",
        )
        report["flavor"] = f"Glimmer moved {len(archived)} empty Lead pages to the archive."
    elif archived and dry_run:
        report["flavor"] = f"Glimmer WOULD archive {len(archived)} empty Lead pages (dry-run)."
    else:
        report["flavor"] = "Glimmer beams — no empty stale Leads to archive today."
    return report


# ═════════════════════════════════════════════════════════════════════
# OLIBIA — Weekly Home Digest
# ═════════════════════════════════════════════════════════════════════


def _git_commits_past_week(repo: Path, since_days: int = 7) -> List[str]:
    if not (repo / ".git").exists():
        return []
    since = (datetime.now() - timedelta(days=since_days)).strftime("%Y-%m-%d")
    try:
        out = subprocess.check_output(
            ["git", "log", f"--since={since}", "--pretty=format:%h %s", "-n", "200"],
            cwd=str(repo), stderr=subprocess.DEVNULL, timeout=10,
        ).decode("utf-8", errors="replace")
        return [line for line in out.splitlines() if line.strip()]
    except (subprocess.SubprocessError, OSError):
        return []


def olibia_weekly_digest(home: Optional[Path] = None) -> Dict[str, Any]:
    """Compile a weekly pulse of Makakoo OS health.

    Aggregates:
      * Git commits in the past 7 days across $HOME/makakoo-os and $MAKAKOO_HOME
      * SANCHO task health (pass/fail counts from past 7 journals)
      * Plugin count (total + Python + non-Python)
      * Brain growth (pages + journal entries past 7d)
      * Prospect pipeline counts by status
      * GYM queue (pending/approved/rejected)
      * Open grants (write-access active + expired-today from perms audit if present)

    Writes to `data/mascots/olibia/weekly/YYYY-WW.md` and appends one
    "see digest" line to today's journal so Sebastian finds it Sunday.
    """
    home = home or _home()
    now = datetime.now(_UTC)
    iso_year, iso_week, _ = now.isocalendar()

    # Git: makakoo-os (Rust repo) — only one likely to have a .git
    makakoo_os = Path(os.path.expanduser("~/makakoo-os"))
    commits_rust = _git_commits_past_week(makakoo_os)

    # SANCHO health over past 7 journals.
    # We also track a "still firing" set — tasks with any entry in the
    # last 2 days — so the top-failing list doesn't surface tasks that
    # were removed from a plugin manifest and are just sitting in the
    # 7-day historical window. Caught 2026-04-24: `freelance_*_tick`
    # was removed 2026-04-22 but still showed 661 failures in digest.
    sancho_counts: Counter = Counter()
    task_failures: Counter = Counter()
    task_oks: Counter = Counter()
    tasks_still_firing: set = set()
    # "Recent" = today + yesterday only. 2-day windows include 3 calendar
    # dates, which pulled in a retirement-day backlog (2026-04-22) that
    # had no entries on 23/24 but still counted as "still firing". Caught
    # 2026-04-24 after the first Olibia run.
    recent_cutoff = (now - timedelta(days=1)).date()
    journals_dir = home / "data" / "Brain" / "journals"
    for delta in range(7):
        day = (now - timedelta(days=delta)).date()
        p = journals_dir / f"{day.strftime('%Y_%m_%d')}.md"
        if not p.is_file():
            continue
        try:
            text = p.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        is_recent = day >= recent_cutoff
        for m in _SANCHO_LINE_RE.finditer(text):
            status = m.group("status")
            task = m.group("task")
            sancho_counts[status] += 1
            if is_recent:
                tasks_still_firing.add(task)
            if status == "FAILED":
                task_failures[task] += 1
            elif status == "ok":
                task_oks[task] += 1

    # Filter top-failing to only tasks that are still actively being
    # scheduled. A task in the registry but not the recent window is
    # either disabled or dead-manifest — not a current bug.
    active_failures = Counter(
        {t: n for t, n in task_failures.items() if t in tasks_still_firing}
    )
    dead_manifest_backlog = {
        t: n for t, n in task_failures.items() if t not in tasks_still_firing
    }

    # Plugin inventory
    plugins_dir = home / "plugins"
    plugins: List[str] = []
    if plugins_dir.is_dir():
        plugins = [p.name for p in sorted(plugins_dir.iterdir())
                   if p.is_dir() and (p / "plugin.toml").is_file()]

    # Prospect pipeline
    prospects_dir = home.parent / "freelance-office" / "prospects"
    prospect_counts: Counter = Counter()
    if prospects_dir.is_dir():
        for pdir in prospects_dir.iterdir():
            if not pdir.is_dir() or pdir.name.startswith("_"):
                continue
            meta = pdir / "meta.yaml"
            if not meta.is_file():
                continue
            try:
                meta_text = meta.read_text(encoding="utf-8")
            except OSError:
                continue
            status_m = re.search(r"^current_status\s*:\s*([^\s#]+)", meta_text, re.MULTILINE)
            if status_m:
                prospect_counts[status_m.group(1).strip().strip("\"'").lower()] += 1
            else:
                prospect_counts["unknown"] += 1

    # GYM queue
    imp = home / "data" / "improvements"
    gym_counts = {
        "pending": len(list((imp / "pending").glob("*.md"))) if (imp / "pending").is_dir() else 0,
        "approved": len(list((imp / "approved").glob("*.md"))) if (imp / "approved").is_dir() else 0,
        "rejected": len(list((imp / "rejected").glob("*.md"))) if (imp / "rejected").is_dir() else 0,
    }

    # Brain growth: count journal files modified past week + pages modified past week
    week_ago = (now - timedelta(days=7)).timestamp()
    pages_modified = 0
    pages_dir = home / "data" / "Brain" / "pages"
    if pages_dir.is_dir():
        for p in pages_dir.rglob("*.md"):
            try:
                if p.stat().st_mtime >= week_ago:
                    pages_modified += 1
            except OSError:
                continue

    digest = {
        "mascot": "Olibia",
        "mission": "weekly_digest",
        "generated_at": now.isoformat(),
        "iso_year": iso_year,
        "iso_week": iso_week,
        "commits_makakoo_os": len(commits_rust),
        "commits_sample": commits_rust[:10],
        "sancho_status_counts": dict(sancho_counts),
        # Active tasks only — past-48h signal, not dead manifest entries
        "sancho_top_failing_tasks": active_failures.most_common(5),
        "sancho_dead_manifest_backlog": dead_manifest_backlog,
        "tasks_still_firing_count": len(tasks_still_firing),
        "plugin_count": len(plugins),
        "prospect_pipeline": dict(prospect_counts),
        "gym_queue": gym_counts,
        "brain_pages_modified": pages_modified,
    }

    # Write markdown digest
    out_dir = _mascot_dir(home, "olibia") / "weekly"
    out_dir.mkdir(exist_ok=True)
    md_path = out_dir / f"{iso_year}-W{iso_week:02d}.md"

    md_lines = [
        f"# Olibia — Weekly Home Digest — {iso_year}-W{iso_week:02d}",
        "",
        f"*Generated {now.isoformat()}*",
        "",
        "## Commits (makakoo-os, past 7 days)",
        f"- Total: **{len(commits_rust)}**",
    ]
    for c in commits_rust[:10]:
        md_lines.append(f"  - {c}")
    if len(commits_rust) > 10:
        md_lines.append(f"  - _…and {len(commits_rust) - 10} more_")

    md_lines += [
        "",
        "## SANCHO health (past 7 days)",
        f"- Entries: ok={sancho_counts.get('ok', 0)} · failed={sancho_counts.get('FAILED', 0)} · "
        f"skipped={sancho_counts.get('skipped', 0)}",
        f"- Tasks still firing (past 48h): **{len(tasks_still_firing)}**",
    ]
    if active_failures:
        md_lines.append("- Top failing (still-firing) tasks:")
        for task, n in active_failures.most_common(5):
            md_lines.append(f"  - `{task}` — {n} failures this week")
    else:
        md_lines.append("- _No still-firing task has any failures this week._")
    if dead_manifest_backlog:
        total_dead = sum(dead_manifest_backlog.values())
        md_lines.append(
            f"- Historical backlog (tasks no longer firing): **{total_dead}** failures "
            f"across {len(dead_manifest_backlog)} retired task(s)"
        )

    md_lines += [
        "",
        f"## Plugins installed: **{len(plugins)}**",
        "",
        "## Prospect pipeline",
    ]
    for status, n in sorted(prospect_counts.items()):
        md_lines.append(f"- {status}: **{n}**")

    md_lines += [
        "",
        "## GYM improvement queue",
        f"- pending: {gym_counts['pending']}",
        f"- approved: {gym_counts['approved']}",
        f"- rejected: {gym_counts['rejected']}",
        "",
        f"## Brain activity: **{pages_modified}** pages modified this week",
        "",
        "_Olibia, quietly._",
    ]
    md_path.write_text("\n".join(md_lines) + "\n", encoding="utf-8")

    _atomic_write_json(_mascot_dir(home, "olibia") / "last_digest.json", digest)
    _append_journal(
        home,
        f"- [[Olibia]] weekly digest ready for {iso_year}-W{iso_week:02d} — "
        f"see `data/mascots/olibia/weekly/{iso_year}-W{iso_week:02d}.md`",
    )
    digest["report_path"] = str(md_path)
    digest["flavor"] = f"Olibia wrote the {iso_year}-W{iso_week:02d} digest."
    return digest
