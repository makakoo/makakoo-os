"""
Freshness Validator - Stale Memory Detection

Detects stale memories that may need updating or reconfirmation.
Freshness levels: FRESH (<7d), OK (7-30d), STALE (30-90d), ARCHIVE (>90d).
"""

import os
import json
from datetime import datetime, timedelta
from enum import Enum
from typing import List, Dict, Any, Optional

BRAIN_DIR = os.path.join(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")), "data", "Brain")


class FreshnessLevel(Enum):
    """Memory freshness classification."""
    FRESH = "fresh"      # < 7 days
    OK = "ok"            # 7-30 days
    STALE = "stale"      # 30-90 days
    ARCHIVE = "archive"  # > 90 days

    def __str__(self):
        return self.value


class MemoryFreshnessValidator:
    """
    Validates memory freshness and flags stale entries.

    Freshness thresholds:
    - FRESH: < 7 days
    - OK: 7-30 days
    - STALE: 30-90 days
    - ARCHIVE: > 90 days
    """

    FRESHNESS_THRESHOLDS = {
        FreshnessLevel.FRESH: timedelta(days=7),
        FreshnessLevel.OK: timedelta(days=30),
        FreshnessLevel.STALE: timedelta(days=90),
        FreshnessLevel.ARCHIVE: timedelta(days=365),
    }

    def __init__(self, brain_dir: str = BRAIN_DIR):
        self.brain_dir = brain_dir

    def check_freshness(self, memory: Dict[str, Any]) -> FreshnessLevel:
        """
        Determine freshness level for a single memory.

        Args:
            memory: Memory dict with last_interaction or updated_at field

        Returns:
            FreshnessLevel enum value
        """
        last_update = (
            memory.get("last_interaction")
            or memory.get("updated_at")
            or memory.get("modified_at")
        )

        if not last_update:
            return FreshnessLevel.STALE  # Default for undated

        try:
            if isinstance(last_update, str):
                last_date_raw = last_update.replace("Z", "+00:00")
                # Handle timezone-aware datetimes
                if "+" in last_date_raw or last_date_raw.endswith("+00:00"):
                    last_date = datetime.fromisoformat(last_date_raw).replace(tzinfo=None)
                else:
                    last_date = datetime.fromisoformat(last_date_raw)
            else:
                last_date = last_update
        except (ValueError, TypeError):
            return FreshnessLevel.STALE

        days_old = (datetime.now() - last_date).days
        if days_old < 0:
            days_old = 0

        if days_old < 7:
            return FreshnessLevel.FRESH
        elif days_old < 30:
            return FreshnessLevel.OK
        elif days_old < 90:
            return FreshnessLevel.STALE
        else:
            return FreshnessLevel.ARCHIVE

    def flag_stale_leads(self, threshold_days: int = 30) -> List[Dict[str, Any]]:
        """
        Find leads that haven't been touched in > threshold_days.

        Args:
            threshold_days: Days threshold for stale flagging (default 30)

        Returns:
            List of stale lead dicts with page, name, days_stale
        """
        stale = []
        pages_dir = os.path.join(self.brain_dir, "pages")

        if not os.path.exists(pages_dir):
            return stale

        for fname in os.listdir(pages_dir):
            if not fname.endswith(".md"):
                continue

            fpath = os.path.join(pages_dir, fname)
            with open(fpath, "r") as f:
                content = f.read()

            # Check if it's a career-lead
            if "type:: career-lead" not in content.lower():
                continue

            # Extract last_interaction
            last_interaction = self._extract_property(content, "last_interaction")
            if not last_interaction:
                stale.append({
                    "page": fname[:-3],
                    "name": self._extract_title(content, fname),
                    "days_stale": None,
                    "action": "reconfirm_or_archive",
                    "reason": "no_last_interaction"
                })
                continue

            try:
                last_date_raw = last_interaction.replace("Z", "+00:00")
                if "+" in last_date_raw or last_date_raw.endswith("+00:00"):
                    last_date = datetime.fromisoformat(last_date_raw).replace(tzinfo=None)
                else:
                    last_date = datetime.fromisoformat(last_date_raw)
                days_old = (datetime.now() - last_date).days
                if days_old > threshold_days:
                    stale.append({
                        "page": fname[:-3],
                        "name": self._extract_title(content, fname),
                        "days_stale": days_old,
                        "action": "reconfirm_or_archive",
                        "reason": f"no_contact_{days_old}_days"
                    })
            except (ValueError, TypeError):
                stale.append({
                    "page": fname[:-3],
                    "name": self._extract_title(content, fname),
                    "days_stale": None,
                    "action": "reconfirm_or_archive",
                    "reason": "invalid_date_format"
                })

        return stale

    def auto_archive_stale(
        self,
        threshold_days: int = 180,
        dry_run: bool = True
    ) -> List[Dict[str, Any]]:
        """
        Archive memories older than threshold_days.

        Args:
            threshold_days: Days threshold for archival (default 180)
            dry_run: If True, return what would be archived without doing it

        Returns:
            List of memories that were/would be archived
        """
        archives = []
        pages_dir = os.path.join(self.brain_dir, "pages")

        if not os.path.exists(pages_dir):
            return archives

        for fname in os.listdir(pages_dir):
            if not fname.endswith(".md"):
                continue

            fpath = os.path.join(pages_dir, fname)
            with open(fpath, "r") as f:
                content = f.read()

            # Check for last_interaction or updated_at
            last_update = (
                self._extract_property(content, "last_interaction")
                or self._extract_property(content, "updated_at")
                or self._extract_property(content, "modified_at")
            )

            if not last_update:
                continue

            try:
                last_date_raw = last_update.replace("Z", "+00:00")
                if "+" in last_date_raw or last_date_raw.endswith("+00:00"):
                    last_date = datetime.fromisoformat(last_date_raw).replace(tzinfo=None)
                else:
                    last_date = datetime.fromisoformat(last_date_raw)
                days_old = (datetime.now() - last_date).days
                if days_old > threshold_days:
                    archive_entry = {
                        "page": fname[:-3],
                        "path": fpath,
                        "days_old": days_old,
                        "archived_at": datetime.now().isoformat() if not dry_run else None
                    }
                    archives.append(archive_entry)

                    if not dry_run:
                        # Add archive tag
                        with open(fpath, "a") as f:
                            f.write(f"\n\n- archived:: true\n- archived_at:: {datetime.now().isoformat()}\n")
            except (ValueError, TypeError):
                continue

        return archives

    def get_freshness_report(self) -> Dict[str, Any]:
        """
        Generate a freshness report for all memories.

        Returns:
            Dict with counts by freshness level
        """
        pages_dir = os.path.join(self.brain_dir, "pages")
        counts = {
            "fresh": 0,
            "ok": 0,
            "stale": 0,
            "archive": 0,
            "unknown": 0,
            "total": 0
        }

        if not os.path.exists(pages_dir):
            return counts

        for fname in os.listdir(pages_dir):
            if not fname.endswith(".md"):
                continue

            fpath = os.path.join(pages_dir, fname)
            with open(fpath, "r") as f:
                content = f.read()

            last_update = (
                self._extract_property(content, "last_interaction")
                or self._extract_property(content, "updated_at")
                or self._extract_property(content, "modified_at")
            )

            memory = {"last_interaction": last_update}
            level = self.check_freshness(memory)

            counts["total"] += 1
            counts[level.value] = counts.get(level.value, 0) + 1

        return counts

    def _extract_property(self, content: str, prop_name: str) -> Optional[str]:
        """Extract a property value from page content."""
        import re
        pattern = rf'{prop_name}::\s*(.+)'
        match = re.search(pattern, content, re.IGNORECASE)
        if match:
            return match.group(1).strip()
        return None

    def _extract_title(self, content: str, default: str) -> str:
        """Extract title from page content."""
        import re
        match = re.search(r'^#\s+(.+)$', content, re.MULTILINE)
        if match:
            return match.group(1).strip()
        return default[:-3]  # Remove .md


if __name__ == "__main__":
    validator = MemoryFreshnessValidator()
    report = validator.get_freshness_report()
    print(f"Freshness Report: {report}")

    stale_leads = validator.flag_stale_leads()
    print(f"Stale leads: {len(stale_leads)}")
    for lead in stale_leads[:5]:
        print(f"  - {lead['name']}: {lead['days_stale']} days")
