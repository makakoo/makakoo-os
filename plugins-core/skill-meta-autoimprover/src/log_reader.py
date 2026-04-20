#!/usr/bin/env python3
"""
Log Reader — Sprint 5

Reads and queries the activity audit log.
Filters by action, entity, actor, date range.
"""

import json
import re
from datetime import datetime, timedelta
from pathlib import Path
from typing import List, Dict, Any, Optional, Iterator

import os

LOG_DIR = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))) / "data" / "logs" / "activity"

# Entity type for skill events
SKILL_ENTITY_TYPE = "skill"


class LogReader:
    """Read and query the activity audit log."""

    def __init__(self, log_dir: Path = LOG_DIR):
        self.log_dir = log_dir

    def iter_events(
        self,
        start_date: Optional[datetime] = None,
        end_date: Optional[datetime] = None,
        action: Optional[str] = None,
        entity_type: Optional[str] = None,
        entity_id: Optional[str] = None,
        actor: Optional[str] = None,
        session_id: Optional[str] = None,
        limit: Optional[int] = None,
    ) -> Iterator[Dict[str, Any]]:
        """Iterate over matching events, newest first.

        Args:
            start_date: Only events after this date
            end_date: Only events before this date
            action: Filter by action (e.g. "skill.created")
            entity_type: Filter by entity type (e.g. "skill", "memory")
            entity_id: Filter by specific entity (e.g. "dev/skill-manager")
            actor: Filter by actor (e.g. "harvey", "review_agent")
            session_id: Filter by session
            limit: Maximum events to return

        Yields:
            ActivityEvent dicts, newest first
        """
        count = 0
        for log_file in self.iter_log_files(start_date=start_date, end_date=end_date):
            for line in _read_lines(log_file):
                event = _parse_line(line)
                if event is None:
                    continue
                if not _matches_filters(
                    event,
                    action=action,
                    entity_type=entity_type,
                    entity_id=entity_id,
                    actor=actor,
                    session_id=session_id,
                ):
                    continue
                yield event
                count += 1
                if limit is not None and count >= limit:
                    return

    def get_events(
        self,
        **kwargs
    ) -> List[Dict[str, Any]]:
        """Same as iter_events but returns a list."""
        return list(self.iter_events(**kwargs))

    def count_events(
        self,
        action: Optional[str] = None,
        entity_type: Optional[str] = None,
        start_date: Optional[datetime] = None,
        end_date: Optional[datetime] = None,
    ) -> int:
        """Count matching events (no limit)."""
        count = 0
        for _ in self.iter_events(
            start_date=start_date,
            end_date=end_date,
            action=action,
            entity_type=entity_type,
        ):
            count += 1
        return count

    def get_recent(
        self,
        days: int = 7,
        action: Optional[str] = None,
        limit: int = 100,
    ) -> List[Dict[str, Any]]:
        """Get recent events from the last N days."""
        start_date = datetime.now() - timedelta(days=days)
        return self.get_events(start_date=start_date, action=action, limit=limit)

    def get_for_session(self, session_id: str) -> List[Dict[str, Any]]:
        """Get all events for a specific session."""
        return self.get_events(session_id=session_id)

    def get_skill_events(
        self,
        skill_name: Optional[str] = None,
        days: int = 30,
    ) -> List[Dict[str, Any]]:
        """Get all skill-related events for a skill or all skills."""
        start_date = datetime.now() - timedelta(days=days)
        entity_id = skill_name if skill_name else None
        return self.get_events(
            start_date=start_date,
            entity_type=SKILL_ENTITY_TYPE,
            entity_id=entity_id,
        )

    def search(self, query: str, days: int = 30) -> List[Dict[str, Any]]:
        """Full-text search across event details."""
        start_date = datetime.now() - timedelta(days=days)
        query_lower = query.lower()
        results = []
        for event in self.iter_events(start_date=start_date):
            # Search in entity_id
            entity_id = event.get("entity_id", "") or ""
            if query_lower in entity_id.lower():
                results.append(event)
                continue
            # Search in details
            details = event.get("details") or {}
            if _search_in_details(query_lower, details):
                results.append(event)
        return results

    def iter_log_files(
        self,
        start_date: Optional[datetime] = None,
        end_date: Optional[datetime] = None,
    ) -> Iterator[Path]:
        """Yield log file paths in date range, newest first."""
        if not self.log_dir.exists():
            return

        now = datetime.now()
        end_date = end_date or now
        start_date = start_date or (now - timedelta(days=365))

        # Walk YYYY/MM/ subdirs
        for year_dir in sorted(self.log_dir.iterdir(), reverse=True):
            if not year_dir.is_dir():
                continue
            year_str = year_dir.name
            if not re.match(r"^\d{4}$", year_str):
                continue
            year = int(year_str)
            if year < start_date.year or year > end_date.year:
                continue

            for month_dir in sorted(year_dir.iterdir(), reverse=True):
                if not month_dir.is_dir():
                    continue
                month_str = month_dir.name
                if not re.match(r"^\d{2}$", month_str):
                    continue
                month = int(month_str)
                if month < 1 or month > 12:
                    continue
                if year == start_date.year and month < start_date.month:
                    continue
                if year == end_date.year and month > end_date.month:
                    continue

                for log_file in sorted(month_dir.iterdir(), reverse=True):
                    if not log_file.is_file():
                        continue
                    if not log_file.suffix == ".jsonl":
                        continue
                    # Parse date from filename (YYYY-MM-DD.jsonl)
                    date_str = log_file.stem  # filename without extension
                    if not re.match(r"^\d{4}-\d{2}-\d{2}$", date_str):
                        continue
                    file_date = datetime.strptime(date_str, "%Y-%m-%d")
                    if file_date < start_date or file_date > end_date:
                        continue
                    yield log_file


def _read_lines(path: Path) -> Iterator[str]:
    """Read a JSONL file line by line."""
    try:
        with open(path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if line:
                    yield line
    except (OSError, UnicodeDecodeError):
        return


def _parse_line(line: str) -> Optional[Dict[str, Any]]:
    """Parse a JSON line, skip malformed."""
    try:
        return json.loads(line)
    except json.JSONDecodeError:
        return None


def _matches_filters(
    event: Dict[str, Any],
    action: Optional[str] = None,
    entity_type: Optional[str] = None,
    entity_id: Optional[str] = None,
    actor: Optional[str] = None,
    session_id: Optional[str] = None,
) -> bool:
    """Check if an event matches all the given filters."""
    if action is not None:
        if event.get("action") != action:
            return False
    if entity_type is not None:
        if event.get("entity_type") != entity_type:
            return False
    if entity_id is not None:
        if event.get("entity_id") != entity_id:
            return False
    if actor is not None:
        if event.get("actor_id") != actor and event.get("actor") != actor:
            return False
    if session_id is not None:
        if event.get("session_id") != session_id:
            return False
    return True


def _search_in_details(query: str, details: Dict[str, Any]) -> bool:
    """Recursively search for query in details values (case-insensitive)."""
    for value in details.values():
        if isinstance(value, str):
            if query in value.lower():
                return True
        elif isinstance(value, dict):
            if _search_in_details(query, value):
                return True
        elif isinstance(value, list):
            for item in value:
                if isinstance(item, str) and query in item.lower():
                    return True
                elif isinstance(item, dict):
                    if _search_in_details(query, item):
                        return True
    return False


if __name__ == "__main__":
    # Quick smoke test
    reader = LogReader()
    events = reader.get_recent(days=7, limit=5)
    print(f"Found {len(events)} recent events")
    for event in events:
        print(f"  {event.get('timestamp', 'no-timestamp')} | {event.get('action')} | {event.get('entity_id')}")
