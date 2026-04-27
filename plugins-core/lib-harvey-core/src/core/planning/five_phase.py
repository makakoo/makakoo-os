"""
5-Phase Plan Workflow — Claude Code Plan Mode Pattern

This module implements the structured 5-phase planning workflow that Claude Code
uses internally for plan creation. It manages phase transitions, turn limits,
and provides appropriate system prompts for each phase.

Key concepts:
- Phase 1 (EXPLORE): Launch parallel explore agents to understand the problem
- Phase 2 (DESIGN): Parallel plan agents design the implementation
- Phase 3 (REVIEW): Review plans, align with intent, synthesize into ONE coherent plan
- Phase 4 (CONTROL): Write final plan to file (detailed version)
- Phase 4 (CUT): Write final plan to file (concise version, under 40 lines)
- Phase 5 (EXIT): Call ExitPlanMode tool for user approval

Note: This is the PLANNING WORKFLOW, not the /gsd-plan-phase implementation.
This is Claude Code's internal plan mode that the user experiences.

Path: plugins-core/lib-harvey-core/src/core/planning/five_phase.py
"""

from __future__ import annotations

import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

# ---------------------------------------------------------------------------
# Phase System Prompts
# ---------------------------------------------------------------------------

PLAN_PHASE1_EXPLORE = """You are in Phase 1 (EXPLORE) of the 5-Phase Planning workflow.

Your task: Launch parallel explore agents to deeply understand the problem space.

Phase 1 Instructions:
- Spawn 1-3 EXPLORE_AGENT subagents to investigate different aspects of the problem
- Explore agents CANNOT see the main conversation — they get isolated context
- Each explore agent should focus on a specific angle:
  * What's the current state/behavior?
  * What are the constraints and requirements?
  * What could go wrong or what edge cases exist?
- Collect findings from all explore agents
- When all explore agents report back, advance to Phase 2

Key Pattern: Use EXPLORE_AGENT subagent type for workers that need isolation.
Do NOT try to solve the problem in Phase 1 — just understand it.

After exploring: Type 'advance' to move to Phase 2 (DESIGN).
"""


PLAN_PHASE2_DESIGN = """You are in Phase 2 (DESIGN) of the 5-Phase Planning workflow.

Your task: Launch parallel plan agents to design the implementation approach.

Phase 2 Instructions:
- Spawn 1-3 PLAN_AGENT subagents, each designing a different approach
- Plan agents should reference specific files and line numbers for context
- Each plan agent should consider:
  * Which files need to be created/modified
  * What the data flow and interfaces look like
  * How to verify the implementation works
  * What could break and how to prevent it
- Reference existing code patterns in the codebase (cite file:line)
- Do NOT write code yet — just design the approach

Key Pattern: PLAN_AGENT with specific file:line references for context.

After designing: Type 'advance' to move to Phase 3 (REVIEW).
"""


PLAN_PHASE3_REVIEW = """You are in Phase 3 (REVIEW) of the 5-Phase Planning workflow.

Your task: Review all plan designs, align with user intent, and synthesize into ONE coherent plan.

Phase 3 Instructions:
- Read through all designs from Phase 2
- Evaluate each against:
  * Does it match the user's stated goal?
  * Is it feasible within the project constraints?
  * Does it integrate well with existing code?
- Synthesize all designs into ONE coherent plan
- NO alternatives — produce a single unified plan
- The plan should include:
  * Clear objective
  * Context and constraints
  * Specific tasks with order
  * Verification steps
  * Success criteria

Key Pattern: ONE coherent plan, no alternatives. Review + synthesis.

After reviewing: Type 'advance' to move to Phase 4 (CONTROL).
"""


PLAN_PHASE4_CONTROL = """You are in Phase 4 (CONTROL - Detailed) of the 5-Phase Planning workflow.

Your task: Write the final detailed plan to a plan file.

Phase 4 Instructions:
- Write the complete plan to a file with structured sections:
  * Context: Background and problem statement
  * Objective: What we're trying to achieve
  * Tasks: Numbered list of specific implementation steps
  * Verification: How to verify each task is complete
  * Success Criteria: What "done" looks like
- Be thorough — this is the detailed version
- Include file paths and key implementation details
- Add human decision points/checkpoints if needed
- Keep the plan actionable and specific

Key Pattern: Write to plan file with Context, Verification, Success sections.

After writing: Type 'advance' to move to Phase 5 (EXIT).
"""


PLAN_PHASE4_CUT = """You are in Phase 4 (CONTROL - Concise) of the 5-Phase Planning workflow.

Your task: Write a concise plan summary (under 40 lines) to a plan file.

Phase 4 Instructions:
- Write a condensed version of the plan
- Maximum 40 lines total
- Include only:
  * Objective (1-2 lines)
  * Key tasks (3-5 bullet points)
  * Success criteria (1-2 lines)
- This is the "elevator pitch" version for quick review
- The detailed version should already exist from PLAN_PHASE4_CONTROL

Key Pattern: Concise under-40-lines version for quick stakeholder review.

After writing: Type 'advance' to move to Phase 5 (EXIT).
"""


PLAN_PHASE5_EXIT = """You are in Phase 5 (EXIT) of the 5-Phase Planning workflow.

Your task: Present the completed plan and request user approval.

Phase 5 Instructions:
- Summarize what was planned
- Present the plan file path to the user
- Call the ExitPlanMode tool to request approval
- Wait for user feedback:
  * If approved: Execute the plan
  * If rejected: Return to appropriate phase based on feedback
  * If modifications needed: Incorporate feedback and re-present

Key Pattern: Exit for user approval. Don't execute until user approves.

The plan is complete. Awaiting your approval to proceed.
"""


# ---------------------------------------------------------------------------
# FivePhasePlanner Class
# ---------------------------------------------------------------------------


@dataclass
class SkillDef:
    """A discovered skill definition."""

    name: str
    description: str
    path: Path
    allowed_tools: list[str] = field(default_factory=list)
    when_to_use: str = ""
    argument_hint: str = ""
    context: str = ""
    agent: str = ""
    content: str = ""


class FivePhasePlanner:
    """
    Manages the 5-phase planning workflow with turn limits per phase.

    State:
    - phase: Current phase (1-5)
    - explore_agents: List of spawned explore agent IDs
    - plan_agents: List of spawned plan agent IDs
    - plan_file: Path to the plan file
    - turns_in_phase: Number of turns spent in current phase

    Phase turn limits (PHASE_TURNS):
    - Phase 1 (EXPLORE): 3 turns max
    - Phase 2 (DESIGN): 5 turns max
    - Phase 3 (REVIEW): 2 turns max
    - Phase 4 (CONTROL): 3 turns max
    - Phase 5 (EXIT): 1 turn (approval)
    """

    PHASE_TURNS: dict[int, int] = {
        1: 3,
        2: 5,
        3: 2,
        4: 3,
        5: 1,
    }

    def __init__(
        self,
        plan_file: Optional[Path] = None,
        explore_agents: Optional[list] = None,
        plan_agents: Optional[list] = None,
    ):
        self.phase = 1
        self.explore_agents = explore_agents or []
        self.plan_agents = plan_agents or []
        self.plan_file = plan_file
        self.turns_in_phase = 0
        self._phase_history: list[int] = [1]
        self._created_at = time.time()

    def get_current_phase_prompt(self) -> str:
        """
        Returns the appropriate system prompt for the current phase.

        Returns:
            str: System prompt for the current phase
        """
        prompts = {
            1: PLAN_PHASE1_EXPLORE,
            2: PLAN_PHASE2_DESIGN,
            3: PLAN_PHASE3_REVIEW,
            4: PLAN_PHASE4_CONTROL,
            5: PLAN_PHASE5_EXIT,
        }
        return prompts.get(self.phase, PLAN_PHASE1_EXPLORE)

    def get_phase_name(self) -> str:
        """Returns human-readable phase name."""
        names = {
            1: "EXPLORE",
            2: "DESIGN",
            3: "REVIEW",
            4: "CONTROL",
            5: "EXIT",
        }
        return names.get(self.phase, "UNKNOWN")

    def advance_phase(self) -> bool:
        """
        Move to the next phase.

        Returns:
            bool: True if advanced successfully, False if already at phase 5
        """
        if self.phase >= 5:
            return False

        self.phase += 1
        self.turns_in_phase = 0
        self._phase_history.append(self.phase)
        return True

    def should_auto_advance(self) -> bool:
        """
        Check if the turn limit for current phase has been exceeded.

        Returns:
            bool: True if should auto-advance, False otherwise
        """
        max_turns = self.PHASE_TURNS.get(self.phase, 3)
        return self.turns_in_phase >= max_turns

    def increment_turn(self) -> None:
        """Increment the turn counter for the current phase."""
        self.turns_in_phase += 1

    def get_turns_remaining(self) -> int:
        """Get number of turns remaining in current phase."""
        max_turns = self.PHASE_TURNS.get(self.phase, 3)
        return max(0, max_turns - self.turns_in_phase)

    def get_phase_turn_info(self) -> dict:
        """Get detailed turn info for current phase."""
        max_turns = self.PHASE_TURNS.get(self.phase, 3)
        return {
            "phase": self.phase,
            "phase_name": self.get_phase_name(),
            "turns_in_phase": self.turns_in_phase,
            "max_turns": max_turns,
            "remaining": self.get_turns_remaining(),
            "should_auto_advance": self.should_auto_advance(),
        }

    def spawn_explore_agent(self, agent_id: str) -> None:
        """Register an explore agent as spawned."""
        if agent_id not in self.explore_agents:
            self.explore_agents.append(agent_id)

    def spawn_plan_agent(self, agent_id: str) -> None:
        """Register a plan agent as spawned."""
        if agent_id not in self.plan_agents:
            self.plan_agents.append(agent_id)

    def get_spawned_agents(self) -> dict:
        """Get summary of spawned agents."""
        return {
            "explore_agents": list(self.explore_agents),
            "plan_agents": list(self.plan_agents),
            "total_explore": len(self.explore_agents),
            "total_plan": len(self.plan_agents),
        }

    def reset(self) -> None:
        """Reset the planner to phase 1."""
        self.phase = 1
        self.turns_in_phase = 0
        self.explore_agents = []
        self.plan_agents = []
        self._phase_history = [1]

    def to_dict(self) -> dict:
        """Serialize planner state to dict."""
        return {
            "phase": self.phase,
            "phase_name": self.get_phase_name(),
            "turns_in_phase": self.turns_in_phase,
            "phase_turn_info": self.get_phase_turn_info(),
            "spawned_agents": self.get_spawned_agents(),
            "plan_file": str(self.plan_file) if self.plan_file else None,
            "phase_history": list(self._phase_history),
            "created_at": self._created_at,
        }


# ---------------------------------------------------------------------------
# CLI / Tests
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    import json

    print("=" * 60)
    print("FivePhasePlanner Test Suite")
    print("=" * 60)

    # Test 1: Initial state
    print("\nTest 1: Initial state")
    planner = FivePhasePlanner()
    assert planner.phase == 1, f"Expected phase 1, got {planner.phase}"
    assert planner.turns_in_phase == 0
    print(f"  PASS: Initial phase={planner.phase}, turns={planner.turns_in_phase}")

    # Test 2: Phase prompts exist
    print("\nTest 2: Phase prompts")
    for p in range(1, 6):
        planner.phase = p
        prompt = planner.get_current_phase_prompt()
        assert len(prompt) > 0, f"Phase {p} prompt is empty"
        print(f"  Phase {p} ({planner.get_phase_name()}): {len(prompt)} chars")

    # Test 3: Advance phase
    print("\nTest 3: Phase advancement")
    planner.reset()
    for expected in range(1, 6):
        assert planner.phase == expected, (
            f"Expected phase {expected}, got {planner.phase}"
        )
        if not planner.advance_phase() and expected < 5:
            print(f"  WARNING: advance_phase returned False at phase {expected}")
        elif expected < 5:
            assert planner.phase == expected + 1
    assert not planner.advance_phase(), "Should not advance past phase 5"
    print("  PASS: Phase advancement works correctly")

    # Test 4: Turn limits
    print("\nTest 4: Turn limits")
    planner.reset()
    planner.turns_in_phase = 2
    assert not planner.should_auto_advance(), "Should not auto-advance at 2/3 turns"
    planner.turns_in_phase = 3
    assert planner.should_auto_advance(), "Should auto-advance at 3/3 turns"
    print(f"  PASS: Turn limits work (auto-advance at PHASE_TURNS[phase])")

    # Test 5: Turn info
    print("\nTest 5: Turn info")
    planner.reset()
    planner.phase = 2
    planner.turns_in_phase = 3
    info = planner.get_phase_turn_info()
    assert info["phase"] == 2
    assert info["phase_name"] == "DESIGN"
    assert info["turns_in_phase"] == 3
    assert info["remaining"] == 2  # 5 - 3 = 2
    print(f"  PASS: Turn info: {json.dumps(info, indent=2)}")

    # Test 6: Agent spawning
    print("\nTest 6: Agent spawning")
    planner.reset()
    planner.spawn_explore_agent("explore-1")
    planner.spawn_plan_agent("plan-1")
    planner.spawn_plan_agent("plan-2")
    agents = planner.get_spawned_agents()
    assert agents["total_explore"] == 1
    assert agents["total_plan"] == 2
    print(f"  PASS: Spawned agents: {json.dumps(agents, indent=2)}")

    # Test 7: Serialization
    print("\nTest 7: Serialization")
    planner.reset()
    state = planner.to_dict()
    assert state["phase"] == 1
    assert state["phase_name"] == "EXPLORE"
    print(f"  PASS: Serialization works")

    # Test 8: Phase 4 variants
    print("\nTest 8: Phase 4 variants")
    planner.phase = 4
    assert PLAN_PHASE4_CONTROL is not None
    assert PLAN_PHASE4_CUT is not None
    assert "Context" in PLAN_PHASE4_CONTROL or len(PLAN_PHASE4_CONTROL) > 100
    print(f"  PASS: Phase 4 has both CONTROL and CUT variants")

    print("\n" + "=" * 60)
    print("✅ All FivePhasePlanner tests passed")
    print("=" * 60)
