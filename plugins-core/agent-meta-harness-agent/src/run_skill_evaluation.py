#!/usr/bin/env python3
"""
Meta Harness — Skill Compliance Evaluation Runner

Wraps the AgentHarness for use in Harvey's autoresearch loop.
Provides baseline vs. improved evaluation for skill compliance scoring.

Usage:
    # Evaluate a skill (with skill loaded)
    python3 run_skill_evaluation.py --skill dev/writing-skills --with-skill

    # Baseline evaluation (without skill)
    python3 run_skill_evaluation.py --skill dev/writing-skills --baseline

    # Custom scenario
    python3 run_skill_evaluation.py --scenario "Create a Fibonacci implementation in Python"

    # Terminal-Bench task (if data available)
    python3 run_skill_evaluation.py --scenario tbench:medium/12 --with-skill

    # Compare delta (for autoresearch integration)
    python3 run_skill_evaluation.py --skill dev/writing-skills \
        --improved-content "$(cat ../plugins-core/dev/writing-skills/SKILL.md)"
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import subprocess
import sys
import time
import uuid
from datetime import datetime
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from agent import AgentHarness
from config import (
    AI_MODEL,
    DATA_DIR,
    HARVEY_SKILLS_ROOT,
    LOG_DIR,
    RESULTS_TSV,
    STATE_DIR,
)
from skill_environment import DEFAULT_TASK, SkillEnvironment
from tbench_integration import TerminalBenchLoader


def _run_git(cmd: list) -> subprocess.CompletedProcess:
    # Post-harvey-os retirement: all skills are git-tracked under MAKAKOO_HOME
    # (the repo root), not the removed harvey-os/ subtree.
    makakoo_home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME") \
        or os.path.expanduser("~/MAKAKOO")
    return subprocess.run(
        ["git"] + cmd,
        cwd=makakoo_home,
        capture_output=True,
        text=True,
    )


def _log_result(
    skill: str,
    scenario: str,
    score: int,
    turns: int,
    elapsed: float,
    mode: str,
    delta: int | None = None,
    improved_content: str | None = None,
) -> None:
    """Append evaluation result to results.tsv."""
    RESULTS_TSV.parent.mkdir(parents=True, exist_ok=True)
    if not RESULTS_TSV.exists():
        RESULTS_TSV.write_text(
            "date\ttime\tskill\tscenario\tscore\tturns\telapsed_sec\tmode\tdelta\tstatus\n"
        )

    now = datetime.now()
    status = (
        "improved"
        if mode == "improved"
        else ("baseline" if mode == "baseline" else "evaluated")
    )
    line = (
        f"{now.strftime('%Y-%m-%d')}\t{now.strftime('%H:%M:%S')}\t{skill}\t{scenario}\t"
        f"{score}\t{turns}\t{elapsed:.1f}\t{mode}\t{delta or ''}\t{status}\n"
    )
    RESULTS_TSV.write_text(RESULTS_TSV.read_text() + line)


async def run_evaluation(
    scenario: str,
    skill_content: str | None,
    task_instruction: str | None = None,
    model: str = AI_MODEL,
    max_turns: int = 30,
) -> dict:
    """
    Run a single meta-harness evaluation.

    Args:
        scenario: task description or "tbench:<difficulty>/<index>"
        skill_content: SKILL.md content to inject, or None for baseline
        task_instruction: override task instruction
        model: LLM model to use

    Returns:
        dict with score, turns, elapsed, success, completion_message
    """
    session_id = f"mh_{uuid.uuid4().hex[:8]}"
    agent = AgentHarness(
        model=model,
        max_turns=max_turns,
        logging_dir=LOG_DIR / session_id,
    )

    LOG_DIR.mkdir(parents=True, exist_ok=True)

    env = SkillEnvironment(
        session_name=session_id,
        skill_content=skill_content,
        task_instruction=task_instruction or DEFAULT_TASK,
    )

    await env.setup()
    result = await agent.run(instruction=task_instruction or DEFAULT_TASK, env=env)

    return result


def score_from_result(result: dict) -> int:
    """
    Convert agent execution result into a 0-100 compliance score.

    Scoring:
    - Task completed + fast (< 10 turns): 100
    - Task completed + medium turns: 85
    - Max turns reached (partial): 50
    - Early failure: 25
    """
    turns = result.get("turns", 0)
    success = result.get("success", False)
    elapsed = result.get("elapsed_sec", 999)

    if success:
        if turns <= 10:
            return 100
        elif turns <= 20:
            return 90
        else:
            return 80
    else:
        if turns >= 25:
            return 50
        elif turns >= 15:
            return 30
        else:
            return 15


def main():
    parser = argparse.ArgumentParser(
        description="Meta Harness Skill Compliance Evaluator"
    )
    parser.add_argument(
        "--skill",
        help="Skill path relative to plugins-core/, e.g. dev/writing-skills",
    )
    parser.add_argument(
        "--scenario", default=DEFAULT_TASK, help="Task description or tbench:<tier>/<n>"
    )
    parser.add_argument(
        "--with-skill", action="store_true", help="Run with skill loaded"
    )
    parser.add_argument(
        "--baseline",
        action="store_true",
        help="Run baseline (no skill) — alias for --with-skill false",
    )
    parser.add_argument(
        "--improved-content", help="Raw SKILL.md content for improved evaluation"
    )
    parser.add_argument("--model", default=AI_MODEL, help="LLM model")
    parser.add_argument("--max-turns", type=int, default=30, help="Max agent turns")
    parser.add_argument(
        "--max-elapsed", type=int, default=300, help="Max time in seconds"
    )
    args = parser.parse_args()

    is_baseline = args.baseline or not (args.with_skill or args.improved_content)
    mode = "baseline" if is_baseline else "with-skill"

    skill_name = args.skill or "ad-hoc"
    scenario = args.scenario

    print(f"\n{'=' * 60}")
    print(f"META HARNESS — Skill Compliance Evaluation")
    print(f"{'=' * 60}")
    print(f"Skill:       {skill_name}")
    print(f"Mode:        {mode}")
    print(f"Scenario:    {scenario[:80]}...")
    print(f"Model:       {args.model}")
    print(f"{'=' * 60}\n")

    skill_content = None
    if args.improved_content:
        skill_content = args.improved_content
        print("Using improved skill content (--improved-content)")
    elif args.with_skill and args.skill:
        skill_path = HARVEY_SKILLS_ROOT / args.skill / "SKILL.md"
        if skill_path.exists():
            skill_content = skill_path.read_text()
            print(f"Loaded skill: {skill_path}")
        else:
            print(f"WARNING: Skill not found: {skill_path}")
    elif not is_baseline:
        skill_content = None
        print("Running without skill (no content provided)")

    tbench_loader = TerminalBenchLoader()
    task_instruction = None

    if scenario.startswith("tbench:"):
        _, tier_idx = scenario.split(":", 1)
        tier, idx_str = tier_idx.rsplit("/", 1)
        try:
            idx = int(idx_str)
            task = tbench_loader.load_task(tier, idx)
            if task:
                task_instruction = task.to_skill_instruction()
                print(f"Terminal-Bench task: {task.task_id} ({task.difficulty})")
            else:
                print(
                    f"WARNING: Terminal-Bench task not available. Set TBENCH_DATA_DIR or install from https://tbench.ai"
                )
        except (ValueError, IndexError):
            print(
                f"Invalid tbench format: {scenario}. Use tbench:<easy|medium|hard>/<n>"
            )

    start = time.time()
    try:
        result = asyncio.run(
            run_evaluation(
                scenario=scenario,
                skill_content=skill_content,
                task_instruction=task_instruction,
                model=args.model,
                max_turns=args.max_turns,
            )
        )
    except KeyboardInterrupt:
        print("\nInterrupted.")
        return 1

    elapsed = time.time() - start
    score = score_from_result(result)

    print(f"\n{'=' * 60}")
    print(f"RESULT")
    print(f"{'=' * 60}")
    print(f"  Score:         {score}/100")
    print(f"  Turns:          {result.get('turns', 0)}")
    print(f"  Elapsed:       {elapsed:.1f}s")
    print(f"  Time saved:    {result.get('time_saved_sec', 0):.1f}s (early polling)")
    print(f"  Success:       {result.get('success', False)}")
    print(f"  Completion:    {result.get('completion_message', 'N/A')}")
    print(f"  Mode:          {mode}")
    print(f"{'=' * 60}")

    _log_result(
        skill=skill_name,
        scenario=scenario,
        score=score,
        turns=result.get("turns", 0),
        elapsed=elapsed,
        mode=mode,
        improved_content=args.improved_content,
    )

    return 0 if result.get("success") else 1


if __name__ == "__main__":
    sys.exit(main())
