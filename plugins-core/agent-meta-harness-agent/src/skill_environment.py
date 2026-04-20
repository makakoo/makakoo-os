#!/usr/bin/env python3
"""
Skill Environment — Sandbox setup for skill compliance evaluation.

Wraps AgentHarness environment bootstrapping with Harvey SKILL.md injection.

Usage:
    env = SkillEnvironment(
        session_name="mh_eval_001",
        skill_path=Path("harvey-os/skills/dev/writing-skills/SKILL.md"),
    )
    await env.setup()
    result = await agent.run(instruction=task, env=env)
"""

from __future__ import annotations

import asyncio
import subprocess
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

from agent import AgentHarness, SkillEnv


DEFAULT_TASK = (
    "You are a software engineer. Complete the following task:\n"
    "Create a Python file called fibonacci.py with a function fib(n) that returns "
    "the nth Fibonacci number using iterative approach. "
    "Add a main block that prints fib(10). "
    "Run the file to verify it works."
)


@dataclass
class SkillEnvironment(SkillEnv):
    """Skill environment with optional SKILL.md injection."""

    skill_path: Path | None = None
    skill_content: str | None = None
    task_instruction: str = DEFAULT_TASK
    setup_commands: list[str] = field(default_factory=list)
    working_dir: str = "/app"

    @classmethod
    def from_skill_path(
        cls,
        session_name: str,
        skill_path: Path,
        task_instruction: str | None = None,
    ) -> "SkillEnvironment":
        """Create environment from a SKILL.md file path."""
        skill_content = skill_path.read_text() if skill_path.exists() else None
        return cls(
            session_name=session_name,
            skill_path=skill_path,
            skill_content=skill_content,
            task_instruction=task_instruction or DEFAULT_TASK,
        )

    @classmethod
    def from_skill_content(
        cls,
        session_name: str,
        skill_content: str,
        task_instruction: str | None = None,
    ) -> "SkillEnvironment":
        """Create environment from raw skill content (for improved skill testing)."""
        return cls(
            session_name=session_name,
            skill_content=skill_content,
            task_instruction=task_instruction or DEFAULT_TASK,
        )

    @classmethod
    def no_skill(
        cls,
        session_name: str,
        task_instruction: str | None = None,
    ) -> "SkillEnvironment":
        """Baseline: run without any skill loaded."""
        return cls(
            session_name=session_name,
            skill_content=None,
            task_instruction=task_instruction or DEFAULT_TASK,
        )

    async def setup(self) -> None:
        """Set up the tmux sandbox environment."""
        from agent import TmuxSession

        session = TmuxSession(self.session_name)
        session.start()

        session.send_keys(f"mkdir -p {self.working_dir} && cd {self.working_dir}")
        await asyncio.sleep(0.5)

        for cmd in self.setup_commands:
            session.send_keys(cmd)
            await asyncio.sleep(1.0)

        session.kill()
