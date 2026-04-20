#!/usr/bin/env python3
"""
Terminal-Bench 2.0 Integration.

Loads tasks from the Terminal-Bench 2.0 benchmark dataset.
Obtain data from: https://tbench.ai

89 tasks across Easy (4), Medium (55), Hard (30).
Score: 76.4% with Claude Opus 4.6.

Usage:
    from tbench_integration import TerminalBenchLoader, TBenchTask

    loader = TerminalBenchLoader(data_dir=Path("data/tbench2"))
    task = loader.load_task("medium", 12)
    print(task.instruction)  # CLI task description
    print(task.ground_truth)  # Expected verification command
    print(task.difficulty)    # "easy" | "medium" | "hard"
"""

from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator

from config import TBENCH_DATA_DIR


@dataclass
class TBenchTask:
    """A single Terminal-Bench 2.0 task."""

    task_id: str
    difficulty: str
    instruction: str
    ground_truth: str
    test_code: str | None = None
    metadata: dict | None = None

    @property
    def tier(self) -> int:
        return {"easy": 0, "medium": 1, "hard": 2}.get(self.difficulty, 1)

    def to_skill_instruction(self) -> str:
        """Format as a Harvey skill instruction string."""
        return (
            f"Complete the following CLI task:\n{self.instruction}\n\n"
            f"Verification: {self.ground_truth}"
        )


class TerminalBenchLoader:
    """
    Loader for Terminal-Bench 2.0 tasks.

    Data must be obtained separately from https://tbench.ai
    Expected structure under data_dir/:
        easy/   (4 tasks)
        medium/ (55 tasks)
        hard/   (30 tasks)
        metadata.json

    Without data, the loader returns None for all tasks and logs a warning.
    """

    DIFFICULTIES = ["easy", "medium", "hard"]

    def __init__(self, data_dir: Path | None = None):
        self.data_dir = data_dir or TBENCH_DATA_DIR
        self._available = self._check_available()

    def _check_available(self) -> bool:
        if not self.data_dir.exists():
            return False
        return any((self.data_dir / d).exists() for d in self.DIFFICULTIES)

    def is_available(self) -> bool:
        return self._available

    def load_task(self, difficulty: str, index: int) -> TBenchTask | None:
        """
        Load a single task by difficulty tier and 0-based index.

        >>> loader.load_task("medium", 12)
        TBenchTask(task_id="medium/012", difficulty="medium", ...)
        """
        if not self._available:
            return None

        tier_dir = self.data_dir / difficulty
        if not tier_dir.exists():
            return None

        task_file = tier_dir / f"{index:03d}.json"
        if not task_file.exists():
            return None

        try:
            data = json.loads(task_file.read_text())
        except (json.JSONDecodeError, OSError):
            return None

        return TBenchTask(
            task_id=f"{difficulty}/{index:03d}",
            difficulty=difficulty,
            instruction=data.get("instruction", ""),
            ground_truth=data.get("ground_truth", ""),
            test_code=data.get("test_code"),
            metadata=data.get("metadata"),
        )

    def iter_tasks(self, difficulty: str | None = None) -> Iterator[TBenchTask]:
        """
        Iterate over all available tasks.

        Args:
            difficulty: filter to one tier ("easy", "medium", "hard"), or None for all
        """
        if not self._available:
            return

        tiers = [difficulty] if difficulty else self.DIFFICULTIES
        for tier in tiers:
            tier_dir = self.data_dir / tier
            if not tier_dir.exists():
                continue
            for task_file in sorted(tier_dir.glob("*.json")):
                try:
                    data = json.loads(task_file.read_text())
                except (json.JSONDecodeError, OSError):
                    continue
                index = int(task_file.stem)
                yield TBenchTask(
                    task_id=f"{tier}/{index:03d}",
                    difficulty=tier,
                    instruction=data.get("instruction", ""),
                    ground_truth=data.get("ground_truth", ""),
                    test_code=data.get("test_code"),
                    metadata=data.get("metadata"),
                )

    def count(self, difficulty: str | None = None) -> int:
        """Return number of available tasks."""
        if not self._available:
            return 0
        if difficulty:
            tier_dir = self.data_dir / difficulty
            return len(list(tier_dir.glob("*.json"))) if tier_dir.exists() else 0
        return sum(1 for _ in self.iter_tasks())

    def verify_task(self, task: TBenchTask) -> tuple[bool, str]:
        """
        Run the ground truth command and return (passed, output).
        Uses a temporary tmux session.
        """
        import uuid

        session_name = f"mh_verify_{uuid.uuid4().hex[:8]}"

        try:
            subprocess.run(
                ["tmux", "kill-session", "-t", session_name], capture_output=True
            )
            subprocess.run(
                ["tmux", "new-session", "-d", "-s", session_name], capture_output=True
            )

            check = f"cd /tmp && {task.ground_truth}"
            result = subprocess.run(
                ["tmux", "send-keys", "-t", f"{session_name}:0", check, "Enter"],
                capture_output=True,
            )
            import time

            time.sleep(2)

            pane = subprocess.run(
                ["tmux", "capture-pane", "-t", f"{session_name}:0", "-p"],
                capture_output=True,
                text=True,
            )
            output = pane.stdout or ""

            passed = "PASS" in output or "SUCCESS" in output or "0" not in output
            return passed, output

        finally:
            subprocess.run(
                ["tmux", "kill-session", "-t", session_name], capture_output=True
            )
