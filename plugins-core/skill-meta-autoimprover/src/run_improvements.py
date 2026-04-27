#!/usr/bin/env python3
"""
Harvey Auto-Improver: Background Runner

Orchestrates the autonomous skill improvement loop.
Runs on a schedule (via launchd/cron), processes skills, and commits improvements.

Usage:
    python3 run_improvements.py              # Run one cycle
    python3 run_improvements.py --continuous  # Run continuously (for testing)
    python3 run_improvements.py --daemon      # Run as daemon (production)
"""

import os
import sys
import subprocess
import re
import argparse
from datetime import datetime
from pathlib import Path
from dataclasses import dataclass

# Base paths
HARVEY_ROOT = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")))
SKILLS_ROOT = HARVEY_ROOT / "plugins-core"
AUTOIMPROVER_ROOT = SKILLS_ROOT / "meta" / "autoimprover"
SKILLS_TO_IMPROVE = AUTOIMPROVER_ROOT / "skills_to_improve.md"
PROGRAM_MD = AUTOIMPROVER_ROOT / "PROGRAM.md"
RESULTS_TSV = AUTOIMPROVER_ROOT / "results.tsv"

# Git
HARVEY_GIT_DIR = HARVEY_ROOT


@dataclass
class SkillImprovement:
    skill_path: str
    priority: int
    status: str
    last_improved: str
    notes: str


def run_git(cmd: list, cwd: Path = HARVEY_GIT_DIR) -> subprocess.CompletedProcess:
    """Run git command and return result"""
    return subprocess.run(["git"] + cmd, cwd=cwd, capture_output=True, text=True)


def get_next_skill() -> SkillImprovement:
    """Parse skills_to_improve.md and return highest priority pending skill"""
    content = SKILLS_TO_IMPROVE.read_text()

    skills = []
    # Parse table rows (skip header and separator)
    lines = content.split("\n")
    in_table = False
    for line in lines:
        if "| skill" in line and "-" in line:
            in_table = True
            continue
        if in_table and line.startswith("|"):
            parts = [p.strip() for p in line.split("|")[1:-1]]
            if len(parts) >= 5:
                skill_path = parts[0].strip().strip("`")
                priority = parts[1]
                status = parts[2]
                last_improved = parts[3]
                notes = parts[4] if len(parts) > 4 else ""

                # Parse priority
                p_num = 999
                if priority.startswith("P"):
                    try:
                        p_num = int(priority[1:])
                    except ValueError:
                        pass

                # Check if frozen
                if "frozen" in notes.lower() or status == "frozen":
                    continue

                # Only pending skills
                if status == "pending":
                    skills.append(
                        SkillImprovement(
                            skill_path=skill_path,
                            priority=p_num,
                            status=status,
                            last_improved=last_improved,
                            notes=notes,
                        )
                    )

    # Sort by priority (lower = higher priority)
    skills.sort(key=lambda s: s.priority)

    if skills:
        return skills[0]
    return None


def _skill_pattern(skill_path: str) -> str:
    """Build regex pattern that matches skill path with or without backticks."""
    return rf"(\| \`?{re.escape(skill_path)}\`? \| P\d+ \|)"


def mark_skill_in_progress(skill_path: str):
    """Update skills_to_improve.md to mark skill as in-progress"""
    content = SKILLS_TO_IMPROVE.read_text()

    new_content = re.sub(
        rf"{_skill_pattern(skill_path)} pending (\|)",
        r"\1 in-progress \2",
        content,
    )

    if new_content != content:
        SKILLS_TO_IMPROVE.write_text(new_content)
        print(f"Marked {skill_path} as in-progress")


def mark_skill_completed(skill_path: str, improved: bool):
    """Update skills_to_improve.md after processing"""
    content = SKILLS_TO_IMPROVE.read_text()
    today = datetime.now().strftime("%Y-%m-%d")

    if improved:
        new_content = re.sub(
            rf"{_skill_pattern(skill_path)} in-progress \| never (\|)",
            rf"\1 done | {today} \2",
            content,
        )
    else:
        new_content = re.sub(
            rf"{_skill_pattern(skill_path)} in-progress (\|)",
            r"\1 pending \2",
            content,
        )

    if new_content != content:
        SKILLS_TO_IMPROVE.write_text(new_content)
        print(
            f"Updated {skill_path} - last improved: {today if improved else 'not improved'}"
        )


def improve_skill(skill: SkillImprovement) -> bool:
    """
    Run the improvement cycle for a single skill.
    Returns True if any improvement was made.
    """
    skill_md_path = SKILLS_ROOT / skill.skill_path / "SKILL.md"

    if not skill_md_path.exists():
        print(f"ERROR: Skill file not found: {skill_md_path}")
        return False

    print(f"\n{'=' * 60}")
    print(f"IMPROVING: {skill.skill_path}")
    print(f"Priority: P{skill.priority}")
    print(f"Notes: {skill.notes}")
    print(f"{'=' * 60}")

    # Read current skill content
    original_content = skill_md_path.read_text()

    # Run evaluation
    eval_script = AUTOIMPROVER_ROOT / "evaluate_skill.py"
    result = subprocess.run(
        [sys.executable, str(eval_script), skill.skill_path],
        capture_output=True,
        text=True,
    )

    print(result.stdout)
    if result.stderr:
        print(f"STDERR: {result.stderr}")

    # Check if any improvements were made
    improved = False

    if RESULTS_TSV.exists():
        # Check recent results for this skill
        content = RESULTS_TSV.read_text()
        lines = content.strip().split("\n")
        if len(lines) > 1:
            last_line = lines[-1]
            if skill.skill_path in last_line and "improved" in last_line:
                improved = True

    # If evaluation made changes, git commit
    current_content = skill_md_path.read_text()
    if current_content != original_content:
        print("\nChanges detected - committing...")

        # Git add and commit
        run_git(["add", str(skill_md_path.relative_to(HARVEY_GIT_DIR))])
        commit_msg = f"improve({skill.skill_path}): auto-improvement from autoimprover"
        run_git(["commit", "-m", commit_msg])
        print(f"Committed: {commit_msg}")
        improved = True

        # Push (if remote exists)
        push_result = run_git(["push", "origin", "HEAD"])
        if push_result.returncode != 0:
            print(f"Push failed (may be normal if no remote): {push_result.stderr}")
    else:
        print("\nNo changes made to skill.")

    return improved


def log_to_brain(message: str):
    """Log a message to today's Brain journal"""
    today = datetime.now().strftime("%Y_%m_%d")
    journal_path = HARVEY_ROOT / "data" / "Brain" / "journals" / f"{today}.md"

    if journal_path.exists():
        content = journal_path.read_text()
        # Ensure proper bullet point format
        if not content.endswith("\n"):
            content += "\n"
    else:
        content = ""

    timestamp = datetime.now().strftime("%H:%M")
    new_entry = f"- [{timestamp}] AUTOIMPROVER: {message}\n"

    journal_path.write_text(content + new_entry)
    print(f"Logged to Brain: {message}")


def run_improvement_cycle(max_skills: int = 3) -> int:
    """
    Run one improvement cycle.
    Returns number of skills improved.
    """
    print(f"\n{'#' * 60}")
    print(f"HARVEY AUTO-IMPROVER - Starting Cycle")
    print(f"Time: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    print(f"{'#' * 60}")

    improved_count = 0

    for i in range(max_skills):
        skill = get_next_skill()
        if not skill:
            print("\nNo more pending skills in queue.")
            break

        mark_skill_in_progress(skill.skill_path)
        improved = improve_skill(skill)
        mark_skill_completed(skill.skill_path, improved)

        if improved:
            improved_count += 1
            log_to_brain(f"Improved skill: {skill.skill_path}")

    print(f"\n{'=' * 60}")
    print(f"CYCLE COMPLETE")
    print(f"Skills processed: {min(max_skills, i + 1)}")
    print(f"Improvements made: {improved_count}")
    print(f"{'=' * 60}")

    return improved_count


def main():
    parser = argparse.ArgumentParser(description="Harvey Auto-Improver")
    parser.add_argument(
        "--continuous", action="store_true", help="Run continuously (for testing)"
    )
    parser.add_argument(
        "--daemon",
        action="store_true",
        help="Run as daemon (production - runs once and exits)",
    )
    parser.add_argument(
        "--max-skills",
        type=int,
        default=3,
        help="Max skills to process per cycle (default: 3)",
    )
    args = parser.parse_args()

    if args.continuous:
        # Run continuously for testing
        print("Running in CONTINUOUS mode (for testing)...")
        while True:
            run_improvement_cycle(max_skills=args.max_skills)
            print("\nSleeping 60 seconds before next cycle...")
            import time

            time.sleep(60)
    else:
        # Single run (default, for cron/launchd)
        run_improvement_cycle(max_skills=args.max_skills)

    return 0


if __name__ == "__main__":
    sys.exit(main())
