#!/usr/bin/env python3
"""
Log Analyzer — Sprint 5

Analyzes activity logs for patterns, generates summaries,
and surfaces insights to the user.
"""

from __future__ import annotations

import json
import re
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

import os

HARVEY_ROOT = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")))
LOG_DIR = HARVEY_ROOT / "data" / "logs"
ACTIVITY_LOG_DIR = HARVEY_ROOT / "data" / "logs" / "activity"


# ---------------------------------------------------------------------------
# LogReader — reads JSONL activity log files
# ---------------------------------------------------------------------------

@dataclass
class LogEntry:
    """Parsed activity log entry."""
    timestamp: datetime
    actor_type: str
    actor_id: str
    action: str
    entity_type: str
    entity_id: str
    session_id: Optional[str] = None
    details: Optional[Dict[str, Any]] = None
    raw: Dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> LogEntry:
        ts = d.get("timestamp") or d.get("occurredAt") or d.get("@timestamp")
        if isinstance(ts, str):
            ts = datetime.fromisoformat(ts.replace("Z", "+00:00"))
        elif ts is None:
            ts = datetime.now(timezone.utc)
        return cls(
            timestamp=ts,
            actor_type=d.get("actorType", "unknown"),
            actor_id=d.get("actorId", "unknown"),
            action=d.get("action", "unknown"),
            entity_type=d.get("entityType", "unknown"),
            entity_id=d.get("entityId", "unknown"),
            session_id=d.get("sessionId") or d.get("runId"),
            details=d.get("details"),
            raw=d,
        )


class LogReader:
    """
    Reads Harvey activity logs from JSONL files.

    Log files live in data/logs/activity/ (or data/logs/ for legacy files).
    Each line is a JSON object with fields like:
        timestamp, actorType, actorId, action, entityType, entityId,
        sessionId, runId, details

    Falls back to scanning other *.log files for structured JSON lines.
    """

    def __init__(self, log_dir: Path = LOG_DIR):
        self.log_dir = log_dir

    def get_log_files(self) -> List[Path]:
        """Return all candidate log files, newest first."""
        activity = self.log_dir / "activity"
        candidates: List[Path] = []
        if activity.is_dir():
            candidates.extend(sorted(activity.glob("*.jsonl"), reverse=True))
        candidates.extend(sorted(self.log_dir.glob("*.log"), reverse=True))
        return candidates

    def read_events(
        self,
        days: Optional[int] = None,
        actions: Optional[List[str]] = None,
        entity_ids: Optional[List[str]] = None,
        session_id: Optional[str] = None,
        limit: Optional[int] = None,
    ) -> List[LogEntry]:
        """
        Read and filter log entries.

        Args:
            days: Only entries from the last N days (None = no filter)
            actions: Only entries with these actions (None = all)
            entity_ids: Only entries with these entity_ids (None = all)
            session_id: Only entries for this session (None = all)
            limit: Maximum number of entries to return (None = all)

        Returns:
            List of LogEntry objects, oldest first.
        """
        cutoff: Optional[datetime] = None
        if days is not None:
            cutoff = datetime.now(timezone.utc) - timedelta(days=days)

        actions_set = set(actions) if actions else None
        entity_ids_set = set(entity_ids) if entity_ids else None

        entries: List[LogEntry] = []

        for path in self.get_log_files():
            try:
                for line in _read_lines(path):
                    if not line.strip():
                        continue
                    try:
                        d = json.loads(line)
                    except (json.JSONDecodeError, ValueError):
                        # Try extracting JSON from structured log lines
                        d = _extract_json(line)
                    if not d:
                        continue

                    entry = LogEntry.from_dict(d)

                    if cutoff and entry.timestamp < cutoff:
                        continue
                    if actions_set and entry.action not in actions_set:
                        continue
                    if entity_ids_set and entry.entity_id not in entity_ids_set:
                        continue
                    if session_id and entry.session_id != session_id:
                        continue

                    entries.append(entry)
            except Exception:
                continue

        # Sort oldest first
        entries.sort(key=lambda e: e.timestamp)

        if limit:
            entries = entries[-limit:]

        return entries


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _read_lines(path: Path) -> List[str]:
    try:
        return path.read_text().splitlines()
    except Exception:
        return []


_JSON_RE = re.compile(r'\{[^{}]*(?:\{[^{}]*\}[^{}]*)*\}', re.DOTALL)


def _extract_json(line: str) -> Optional[Dict[str, Any]]:
    """Try to extract a JSON object from a log line."""
    m = _JSON_RE.search(line)
    if m:
        try:
            return json.loads(m.group())
        except Exception:
            pass
    return None


def _format_ts(ts: datetime) -> str:
    """Format a datetime as YYYY-MM-DD HH:MM UTC."""
    return ts.strftime("%Y-%m-%d %H:%M UTC")


def _format_date(ts: datetime) -> str:
    """Format a datetime as YYYY-MM-DD."""
    return ts.strftime("%Y-%m-%d")


# ---------------------------------------------------------------------------
# LogAnalyzer
# ---------------------------------------------------------------------------

class LogAnalyzer:
    """Analyze activity logs and generate summaries."""

    def __init__(self, log_dir: Path = LOG_DIR):
        self.reader = LogReader(log_dir)

    # -------------------------------------------------------------------------
    # Core aggregation
    # -------------------------------------------------------------------------

    def get_summary(self, days: int = 7) -> Dict[str, Any]:
        """Get a summary of activity over the last N days.

        Returns:
            {
                "total_events": int,
                "by_action": Counter,
                "by_actor": Counter,
                "sessions": int,
                "skills_created": int,
                "skills_updated": int,
                "errors": int,
                "budget_warnings": int,
                "date_range": (start_date, end_date),
            }
        """
        events = self.reader.read_events(days=days)
        if not events:
            return {
                "total_events": 0,
                "by_action": {},
                "by_actor": {},
                "sessions": set(),
                "skills_created": 0,
                "skills_updated": 0,
                "errors": 0,
                "budget_warnings": 0,
                "date_range": (None, None),
            }

        by_action: Counter[str] = Counter()
        by_actor: Counter[str] = Counter()
        sessions: set[str] = set()
        skills_created = 0
        skills_updated = 0
        errors = 0
        budget_warnings = 0

        for e in events:
            by_action[e.action] += 1
            by_actor[f"{e.actor_type}:{e.actor_id}"] += 1
            if e.session_id:
                sessions.add(e.session_id)

            action = e.action.lower()
            entity = e.entity_type.lower()

            if action == "skill.created" or (entity == "skill" and "created" in action):
                skills_created += 1
            elif action == "skill.updated" or (entity == "skill" and "updated" in action):
                skills_updated += 1
            elif "error" in action or "failed" in action or "failure" in action:
                errors += 1
            elif "budget.warning" in action or "budget.exceeded" in action:
                budget_warnings += 1

        dates = [e.timestamp for e in events]
        date_range = (
            _format_date(min(dates)) if dates else None,
            _format_date(max(dates)) if dates else None,
        )

        return {
            "total_events": len(events),
            "by_action": dict(by_action),
            "by_actor": dict(by_actor),
            "sessions": sessions,
            "sessions_count": len(sessions),
            "skills_created": skills_created,
            "skills_updated": skills_updated,
            "errors": errors,
            "budget_warnings": budget_warnings,
            "date_range": date_range,
        }

    def get_skill_report(self, days: int = 30) -> str:
        """Generate a human-readable skill activity report."""
        events = self.reader.read_events(days=days)
        skill_events = [
            e for e in events
            if "skill" in e.entity_type.lower() or "skill" in e.action.lower()
        ]

        if not skill_events:
            return f"No skill activity in the last {days} days."

        created: List[LogEntry] = []
        updated: List[LogEntry] = []
        deleted: List[LogEntry] = []

        for e in skill_events:
            a = e.action.lower()
            if "created" in a or "create" in a:
                created.append(e)
            elif "updated" in a or "patched" in a or "edit" in a:
                updated.append(e)
            elif "deleted" in a or "removed" in a:
                deleted.append(e)

        # Count by skill name
        skill_counts: Counter[str] = Counter()
        for e in skill_events:
            skill_counts[e.entity_id] += 1

        lines = [
            f"# Skill Activity Report ({days} days)",
            "",
            f"**Total skill events:** {len(skill_events)}",
            f"**Skills created:** {len(created)}",
            f"**Skills updated:** {len(updated)}",
            f"**Skills deleted:** {len(deleted)}",
            "",
        ]

        if skill_counts:
            lines.append("## Most Active Skills")
            lines.append("")
            for skill, count in skill_counts.most_common(10):
                lines.append(f"- `{skill}` — {count} event{'s' if count != 1 else ''}")
            lines.append("")

        if created:
            lines.append("## Recently Created Skills")
            lines.append("")
            for e in sorted(created, key=lambda x: x.timestamp, reverse=True)[:10]:
                lines.append(f"- **{e.entity_id}** — {_format_ts(e.timestamp)}")
                if e.details:
                    desc = e.details.get("description") or e.details.get("name", "")
                    if desc:
                        lines.append(f"  {desc[:120]}")
            lines.append("")

        if updated:
            lines.append("## Recently Updated Skills")
            lines.append("")
            for e in sorted(updated, key=lambda x: x.timestamp, reverse=True)[:10]:
                lines.append(f"- **{e.entity_id}** — {_format_ts(e.timestamp)}")
            lines.append("")

        return "\n".join(lines)

    def get_error_report(self, days: int = 7) -> str:
        """Generate a report of errors and failures."""
        events = self.reader.read_events(days=days)
        error_events = [
            e for e in events
            if "error" in e.action.lower()
            or "failed" in e.action.lower()
            or "failure" in e.action.lower()
            or (e.details and "error" in str(e.details).lower())
        ]

        if not error_events:
            return f"No errors in the last {days} days."

        lines = [
            f"# Error Report ({days} days)",
            "",
            f"**Total errors:** {len(error_events)}",
            "",
            "## Error Events (newest first)",
            "",
        ]

        for e in sorted(error_events, key=lambda x: x.timestamp, reverse=True)[:50]:
            lines.append(f"### {e.action} — {_format_ts(e.timestamp)}")
            lines.append(f"**Actor:** {e.actor_type}:{e.actor_id}")
            lines.append(f"**Entity:** {e.entity_type}:{e.entity_id}")
            if e.session_id:
                lines.append(f"**Session:** `{e.session_id}`")
            if e.details:
                for k, v in e.details.items():
                    if k.lower() not in ("password", "token", "secret", "key"):
                        lines.append(f"**{k}:** {v}")
            lines.append("")

        return "\n".join(lines)

    def get_budget_report(self, days: int = 7) -> str:
        """Generate a budget usage report."""
        events = self.reader.read_events(days=days)
        budget_events = [
            e for e in events
            if "budget" in e.action.lower()
            or (e.details and "budget" in str(e.details).lower())
        ]

        if not budget_events:
            return f"No budget events in the last {days} days."

        warnings: List[LogEntry] = []
        exceeded: List[LogEntry] = []

        for e in budget_events:
            a = e.action.lower()
            if "exceeded" in a or "stop" in a or "hard" in a:
                exceeded.append(e)
            else:
                warnings.append(e)

        lines = [
            f"# Budget Report ({days} days)",
            "",
            f"**Total budget events:** {len(budget_events)}",
            f"**Warnings:** {len(warnings)}",
            f"**Exceeded events:** {len(exceeded)}",
            "",
        ]

        if warnings:
            lines.append("## Warnings (newest first)")
            lines.append("")
            for e in sorted(warnings, key=lambda x: x.timestamp, reverse=True)[:20]:
                lines.append(f"- **{e.action}** — {_format_ts(e.timestamp)}")
                if e.session_id:
                    lines.append(f"  Session: `{e.session_id}`")
                if e.details:
                    pct = e.details.get("pct") or e.details.get("percentage", "")
                    if pct:
                        lines.append(f"  {pct}%")
            lines.append("")

        if exceeded:
            lines.append("## Exceeded Events")
            lines.append("")
            for e in sorted(exceeded, key=lambda x: x.timestamp, reverse=True)[:20]:
                lines.append(f"- **{e.action}** — {_format_ts(e.timestamp)}")
                if e.session_id:
                    lines.append(f"  Session: `{e.session_id}`")
                if e.details:
                    reason = e.details.get("reason") or e.details.get("message", "")
                    if reason:
                        lines.append(f"  {reason}")
            lines.append("")

        return "\n".join(lines)

    def get_improvement_report(self, days: int = 30) -> str:
        """Generate a self-improvement activity report."""
        events = self.reader.read_events(days=days)
        improvement_events = [
            e for e in events
            if any(
                prefix in e.action.lower()
                for prefix in ("improvement", "memory", "review", "nudge")
            )
            or e.entity_type.lower() in ("memory", "brain", "improvement")
        ]

        if not improvement_events:
            return f"No self-improvement activity in the last {days} days."

        memory_events = [e for e in improvement_events if "memory" in e.action.lower()]
        review_events = [e for e in improvement_events if "review" in e.action.lower()]
        nudge_events = [e for e in improvement_events if "nudge" in e.action.lower()]

        lines = [
            f"# Self-Improvement Report ({days} days)",
            "",
            f"**Total improvement events:** {len(improvement_events)}",
            f"**Memory events:** {len(memory_events)}",
            f"**Review events:** {len(review_events)}",
            f"**Nudge events:** {len(nudge_events)}",
            "",
        ]

        if memory_events:
            lines.append("## Memory Events (newest first)")
            lines.append("")
            for e in sorted(memory_events, key=lambda x: x.timestamp, reverse=True)[:15]:
                lines.append(f"- **{e.action}** — {_format_ts(e.timestamp)}")
                if e.entity_id and e.entity_id != "unknown":
                    lines.append(f"  Entity: `{e.entity_id}`")
                if e.details:
                    summary = str(e.details)[:150]
                    lines.append(f"  {summary}")
            lines.append("")

        if review_events:
            lines.append("## Review Events (newest first)")
            lines.append("")
            for e in sorted(review_events, key=lambda x: x.timestamp, reverse=True)[:15]:
                lines.append(f"- **{e.action}** — {_format_ts(e.timestamp)}")
                if e.session_id:
                    lines.append(f"  Session: `{e.session_id}`")
            lines.append("")

        if nudge_events:
            lines.append("## Nudge Events (newest first)")
            lines.append("")
            for e in sorted(nudge_events, key=lambda x: x.timestamp, reverse=True)[:15]:
                lines.append(f"- **{e.action}** — {_format_ts(e.timestamp)}")
            lines.append("")

        return "\n".join(lines)

    def get_session_summary(self, session_id: str) -> Dict[str, Any]:
        """Get a summary of one specific session."""
        events = self.reader.read_events(session_id=session_id)
        if not events:
            return {
                "session_id": session_id,
                "event_count": 0,
                "duration_minutes": None,
                "first_event": None,
                "last_event": None,
                "actions": {},
                "actors": {},
            }

        events.sort(key=lambda e: e.timestamp)
        first = events[0]
        last = events[-1]
        duration = (last.timestamp - first.timestamp).total_seconds() / 60

        action_counts: Counter[str] = Counter(e.action for e in events)
        actor_counts: Counter[str] = Counter(f"{e.actor_type}:{e.actor_id}" for e in events)

        return {
            "session_id": session_id,
            "event_count": len(events),
            "duration_minutes": round(duration, 1),
            "first_event": _format_ts(first.timestamp),
            "last_event": _format_ts(last.timestamp),
            "actions": dict(action_counts),
            "actors": dict(actor_counts),
        }

    def get_most_active_skills(self, days: int = 30, limit: int = 10) -> List[tuple]:
        """Get most frequently used/created skills.

        Returns:
            List of (skill_name, event_count) tuples, sorted descending.
        """
        events = self.reader.read_events(days=days)
        skill_events = [
            e for e in events
            if e.entity_type.lower() == "skill"
            or "skill" in e.action.lower()
        ]

        counter: Counter[str] = Counter()
        for e in skill_events:
            if e.entity_id and e.entity_id not in ("unknown", ""):
                counter[e.entity_id] += 1

        return counter.most_common(limit)

    def get_daily_activity(self, days: int = 7) -> Dict[str, int]:
        """Get event count per day.

        Returns:
            Dict of "YYYY-MM-DD": event_count, for the last N days.
        """
        events = self.reader.read_events(days=days)
        counts: Counter[str] = Counter()

        for e in events:
            counts[_format_date(e.timestamp)] += 1

        # Fill in zeros for days with no events
        result: Dict[str, int] = {}
        if events:
            dates = sorted({_format_date(e.timestamp) for e in events})
            if dates:
                start = datetime.fromisoformat(dates[0])
                end = datetime.fromisoformat(dates[-1])
                current = start
                while current <= end:
                    key = _format_date(current)
                    result[key] = counts.get(key, 0)
                    current += timedelta(days=1)

        return result

    def format_markdown(
        self,
        days: int = 7,
        include_errors: bool = True,
        include_skills: bool = True,
        include_budget: bool = True,
    ) -> str:
        """Format a full activity report as markdown."""
        summary = self.get_summary(days=days)
        daily = self.get_daily_activity(days=days)
        top_skills = self.get_most_active_skills(days=days, limit=5)

        parts = []

        # Header
        date_range = summary.get("date_range") or (None, None)
        parts.append("# Harvey OS Activity Report")
        parts.append("")
        parts.append(f"**Period:** {date_range[0]} → {date_range[1]} ({days} days)")
        parts.append("")

        # Summary section
        parts.append("## Summary")
        parts.append("")
        parts.append(f"- **Total events:** {summary['total_events']:,}")
        parts.append(f"- **Unique sessions:** {summary.get('sessions_count', len(summary.get('sessions', set())))}")
        parts.append(f"- **Skills created:** {summary['skills_created']}")
        parts.append(f"- **Skills updated:** {summary['skills_updated']}")
        parts.append(f"- **Errors:** {summary['errors']}")
        parts.append(f"- **Budget warnings:** {summary['budget_warnings']}")
        parts.append("")

        # Daily activity
        if daily:
            parts.append("## Daily Activity")
            parts.append("")
            parts.append("| Date | Events |")
            parts.append("|------|--------|")
            for date, count in sorted(daily.items()):
                parts.append(f"| {date} | {count} |")
            parts.append("")

        # Top actions
        by_action = summary.get("by_action", {})
        if by_action:
            parts.append("## Top Actions")
            parts.append("")
            for action, count in sorted(by_action.items(), key=lambda x: x[1], reverse=True)[:10]:
                parts.append(f"- `{action}` — {count}")
            parts.append("")

        # Top actors
        by_actor = summary.get("by_actor", {})
        if by_actor:
            parts.append("## Top Actors")
            parts.append("")
            for actor, count in sorted(by_actor.items(), key=lambda x: x[1], reverse=True)[:10]:
                parts.append(f"- `{actor}` — {count}")
            parts.append("")

        # Top skills
        if include_skills and top_skills:
            parts.append("## Most Active Skills")
            parts.append("")
            for skill, count in top_skills:
                parts.append(f"- `{skill}` — {count} event{'s' if count != 1 else ''}")
            parts.append("")

        # Skill report
        if include_skills:
            skill_report = self.get_skill_report(days=min(days, 30))
            # Avoid repeating header
            skill_lines = skill_report.split("\n")
            if skill_lines and skill_lines[0].startswith("# "):
                skill_lines = skill_lines[1:]  # drop title
            if "\n".join(skill_lines).strip():
                parts.append("## Skill Activity")
                parts.append("")
                parts.append("\n".join(skill_lines).strip())
                parts.append("")

        # Error report
        if include_errors and summary["errors"] > 0:
            error_report = self.get_error_report(days=days)
            error_lines = error_report.split("\n")
            if error_lines and error_lines[0].startswith("# "):
                error_lines = error_lines[1:]
            if "\n".join(error_lines).strip():
                parts.append("## Errors")
                parts.append("")
                parts.append("\n".join(error_lines).strip())
                parts.append("")

        # Budget report
        if include_budget and summary["budget_warnings"] > 0:
            budget_report = self.get_budget_report(days=days)
            budget_lines = budget_report.split("\n")
            if budget_lines and budget_lines[0].startswith("# "):
                budget_lines = budget_lines[1:]
            if "\n".join(budget_lines).strip():
                parts.append("## Budget")
                parts.append("")
                parts.append("\n".join(budget_lines).strip())
                parts.append("")

        return "\n".join(parts).strip()


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Harvey OS Activity Log Analyzer")
    parser.add_argument("--days", type=int, default=7, help="Number of days to analyze")
    parser.add_argument("--output", choices=("markdown", "json", "summary"),
                        default="markdown", help="Output format")
    parser.add_argument("--session", type=str, help="Session ID to summarize")
    args = parser.parse_args()

    analyzer = LogAnalyzer()

    if args.session:
        result = analyzer.get_session_summary(args.session)
        print(json.dumps(result, indent=2, default=str))
    elif args.output == "markdown":
        print(analyzer.format_markdown(days=args.days))
    elif args.output == "json":
        summary = analyzer.get_summary(days=args.days)
        print(json.dumps(summary, indent=2, default=str))
    else:
        summary = analyzer.get_summary(days=args.days)
        print(json.dumps(summary, indent=2, default=str))
