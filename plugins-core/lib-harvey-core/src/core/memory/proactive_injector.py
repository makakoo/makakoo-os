"""
Proactive Context Injector - Anticipatory Context Injection

Injects relevant memories BEFORE Sebastian asks, based on:
- Time patterns (morning, Monday, etc.)
- Active projects (Harvey OS, career, etc.)
"""

import os
import subprocess
from datetime import datetime, time
from typing import List, Dict, Any, Optional

BRAIN_DIR = os.path.join(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")), "data", "Brain")


class ProactiveContextInjector:
    """
    Injects relevant memories BEFORE Sebastian asks.

    Time-based triggers:
    - Morning (8am weekdays): career pipeline status
    - Monday 9am: week ahead summary

    Project-based triggers:
    - Harvey OS: recent commits, active tasks
    - Career: pending outreach, application status
    """

    def __init__(self, brain_dir: str = BRAIN_DIR):
        self.brain_dir = brain_dir

    def get_proactive_injections(self) -> List[Dict[str, Any]]:
        """
        Get all applicable proactive injections for current time/context.

        Returns:
            List of injection dicts with type, priority, content
        """
        injections = []
        now = datetime.now()

        # Morning career check (8am weekdays)
        if now.hour == 8 and now.weekday() < 5:
            injections.extend(self._morning_career_check())

        # Monday morning: Week ahead
        if now.weekday() == 0 and now.hour == 9:
            injections.extend(self._monday_week_ahead())

        # Check for Harvey OS context
        injections.extend(self._harvey_os_status())

        # Check for career context
        injections.extend(self._career_pipeline_status())

        return injections

    def _morning_career_check(self) -> List[Dict[str, Any]]:
        """Morning: Quick pipeline health check."""
        # Load career lead count
        lead_count = self._count_career_leads()
        pending_outreach = self._count_pending_outreach()

        return [{
            "type": "reminder",
            "priority": "medium",
            "trigger": "morning_weekday",
            "content": f"Career Pipeline: {lead_count} total leads, {pending_outreach} pending outreach"
        }]

    def _monday_week_ahead(self) -> List[Dict[str, Any]]:
        """Monday: Week's key dates and deadlines."""
        # Load upcoming items from journal/brain
        upcoming = self._get_upcoming_items()

        return [{
            "type": "summary",
            "priority": "high",
            "trigger": "monday_morning",
            "content": f"This week ahead: {upcoming}"
        }]

    def _harvey_os_status(self) -> List[Dict[str, Any]]:
        """When working on Harvey OS: Current state."""
        # Check if we're in Harvey OS directory
        harvey_home = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
        git_branch = self._get_git_branch(harvey_home)

        if git_branch and git_branch != "main" and git_branch != "HEAD":
            recent_commits = self._get_recent_commits(harvey_home, count=3)
            return [{
                "type": "context",
                "priority": "high",
                "trigger": "active_harvey_os",
                "content": f"Harvey OS: On branch '{git_branch}'. Recent commits: {recent_commits}"
            }]

        return []

    def _career_pipeline_status(self) -> List[Dict[str, Any]]:
        """Check career pipeline status."""
        injections = []

        # Pending outreach
        pending = self._count_pending_outreach()
        if pending > 0:
            injections.append({
                "type": "action",
                "priority": "medium",
                "trigger": "pending_outreach",
                "content": f"Autopilot has {pending} pending connection requests queued"
            })

        # Interview scheduled
        interviews = self._get_upcoming_interviews()
        if interviews:
            injections.append({
                "type": "reminder",
                "priority": "high",
                "trigger": "scheduled_interviews",
                "content": f"Upcoming interviews: {interviews}"
            })

        return injections

    def _count_career_leads(self) -> int:
        """Count total career leads in Brain."""
        pages_dir = os.path.join(self.brain_dir, "pages")
        if not os.path.exists(pages_dir):
            return 0

        count = 0
        for fname in os.listdir(pages_dir):
            if fname.endswith(".md"):
                fpath = os.path.join(pages_dir, fname)
                with open(fpath, "r") as f:
                    content = f.read()
                if "type:: career-lead" in content.lower():
                    count += 1
        return count

    def _count_pending_outreach(self) -> int:
        """Count leads pending outreach."""
        pages_dir = os.path.join(self.brain_dir, "pages")
        if not os.path.exists(pages_dir):
            return 0

        count = 0
        for fname in os.listdir(pages_dir):
            if fname.endswith(".md"):
                fpath = os.path.join(pages_dir, fname)
                with open(fpath, "r") as f:
                    content = f.read()
                if "type:: career-lead" in content.lower():
                    if "status:: pending" in content.lower() or "#pending" in content.lower():
                        count += 1
        return count

    def _get_upcoming_items(self) -> str:
        """Get upcoming items from recent journals."""
        # This would ideally query the Brain API
        # For now, return placeholder
        return "Check today's journal for details"

    def _get_upcoming_interviews(self) -> str:
        """Get upcoming interview count."""
        pages_dir = os.path.join(self.brain_dir, "pages")
        if not os.path.exists(pages_dir):
            return ""

        interviews = []
        for fname in os.listdir(pages_dir):
            if fname.endswith(".md"):
                fpath = os.path.join(pages_dir, fname)
                with open(fpath, "r") as f:
                    content = f.read()
                if "interview" in content.lower() and "type:: career-lead" in content.lower():
                    # Extract company name
                    import re
                    match = re.search(r'company::\s*(.+)', content, re.IGNORECASE)
                    if match:
                        interviews.append(match.group(1).strip())

        if interviews:
            return ", ".join(interviews[:3])
        return ""

    def _get_git_branch(self, repo_path: str) -> Optional[str]:
        """Get current git branch."""
        try:
            result = subprocess.run(
                ["git", "rev-parse", "--abbrev-ref", "HEAD"],
                cwd=repo_path,
                capture_output=True,
                text=True,
                timeout=5
            )
            if result.returncode == 0:
                return result.stdout.strip()
        except Exception:
            pass
        return None

    def _get_recent_commits(self, repo_path: str, count: int = 3) -> str:
        """Get recent commit messages."""
        try:
            result = subprocess.run(
                ["git", "log", f"--oneline", f"-{count}"],
                cwd=repo_path,
                capture_output=True,
                text=True,
                timeout=5
            )
            if result.returncode == 0:
                commits = result.stdout.strip().split("\n")
                return "; ".join(c for c in commits if c)
        except Exception:
            pass
        return "none"


if __name__ == "__main__":
    injector = ProactiveContextInjector()
    injections = injector.get_proactive_injections()
    print(f"Proactive injections ({len(injections)}):")
    for inj in injections:
        print(f"  [{inj['priority']}] {inj['type']}: {inj['content']}")
