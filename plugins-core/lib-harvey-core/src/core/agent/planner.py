"""
Planner — decomposes a complex user goal into a DAG of subagent steps.

Calls an LLM with a structured-output prompt that returns JSON of the shape:

    {
      "steps": [
        {"index": 1, "role": "researcher", "task": "find 3 DiT papers",
         "depends_on": []},
        {"index": 2, "role": "researcher", "task": "find 2 survey posts",
         "depends_on": []},
        {"index": 3, "role": "synthesizer", "task": "write a 1500-word summary",
         "depends_on": [1, 2]}
      ]
    }

The planner does NOT execute the plan — that's PlanExecutor's job (next file).
This module only produces + validates the plan.

Design notes (from sprint § C, Phase 7):
  - `role` must be a manifest name discovered by AgentRegistry (Phase 5)
  - Plans are capped at 6 steps to keep the DAG simple and the cost bounded
  - Depth is capped at 2 (step can only depend on earlier steps) — no loops
  - Validation errors are wrapped in PlannerError with a helpful message
  - Retry once on JSON parse failure with an explicit "output valid JSON"
    nudge before giving up

For testing, the LLM call is injected via `llm_call` so tests can substitute
a stub that returns a canned JSON string without hitting a real model.
"""

from __future__ import annotations

import json
import logging
import re
from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Optional, TYPE_CHECKING

if TYPE_CHECKING:
    from core.agents.capability_index import CapabilityIndex

log = logging.getLogger("harvey.planner")

MAX_PLAN_STEPS = 6
MAX_PLAN_RETRIES = 1
PLAN_SCHEMA_VERSION = 2


class PlannerError(ValueError):
    """Raised when plan generation fails after retries."""
    pass


@dataclass
class PlanStep:
    index: int
    role: str                                # subagent manifest name
    task: str                                # task text passed as ctx
    depends_on: List[int] = field(default_factory=list)
    action: str = ""                         # optional subagent action name

    def to_dict(self) -> Dict[str, Any]:
        return {
            "index": self.index,
            "role": self.role,
            "task": self.task,
            "depends_on": list(self.depends_on),
            "action": self.action,
        }


@dataclass
class Plan:
    goal: str
    steps: List[PlanStep] = field(default_factory=list)
    rationale: str = ""                      # LLM's 1-sentence why, for logs
    version: int = PLAN_SCHEMA_VERSION       # SPRINT-HARVEY-TICKETING Phase 3

    def to_dict(self) -> Dict[str, Any]:
        return {
            "version": self.version,
            "goal": self.goal,
            "rationale": self.rationale,
            "steps": [s.to_dict() for s in self.steps],
        }

    @property
    def is_parallelizable(self) -> bool:
        """True if any two steps have no dependency between them."""
        return any(not s.depends_on for s in self.steps[1:])

    def roots(self) -> List[PlanStep]:
        return [s for s in self.steps if not s.depends_on]

    def step_by_index(self, index: int) -> Optional[PlanStep]:
        for s in self.steps:
            if s.index == index:
                return s
        return None


# ─── Prompt ───────────────────────────────────────────────────


PLANNER_SYSTEM_PROMPT = """You are Harvey's task planner. Given a user goal, \
decompose it into up to {max_steps} concrete steps that specialized \
in-process subagents can execute.

## Available subagents (role values):
{role_list}

## Output format — MUST be valid JSON, nothing else:
{{
  "rationale": "one sentence explaining the plan",
  "steps": [
    {{
      "index": 1,
      "role": "<one of the roles above>",
      "task": "a concrete, self-contained task description",
      "depends_on": []
    }}
  ]
}}

## Rules (enforced by the validator — violating any of these means plan rejection):
1. `role` MUST be one of the listed subagent names (case-sensitive).
2. `index` starts at 1 and is monotonically increasing.
3. `depends_on` is a list of earlier `index` values (never the step's own \
index, never a later index).
4. Maximum {max_steps} steps — prefer fewer.
5. Independent steps (no depends_on) can run in parallel.
6. Each `task` must be self-contained — the subagent receives only that \
string and the results of its declared dependencies.
7. Last step is usually a synthesis/writer step that depends on the \
earlier data-gathering steps.

## Example:
User goal: "Research diffusion transformers and give me a PDF report"
Plan:
{{
  "rationale": "Research in parallel, synthesize to markdown, convert to PDF",
  "steps": [
    {{"index": 1, "role": "researcher", "task": "find the DiT paper and summarize its key claims", "depends_on": []}},
    {{"index": 2, "role": "researcher", "task": "find 2 recent survey posts on diffusion transformers", "depends_on": []}},
    {{"index": 3, "role": "synthesizer", "task": "write a 1500-word markdown report combining the above findings", "depends_on": [1, 2]}}
  ]
}}

Now produce a plan for the user goal. Output ONLY the JSON object.
"""


PLANNER_SYSTEM_PROMPT_V2 = """You are Harvey's task planner. Given a user goal, \
decompose it into up to {max_steps} concrete steps that Harvey's in-process \
subagents can execute. Instead of picking agents by name, you pick them by \
the *action* they need to perform — Harvey's CapabilityIndex resolves the \
action to the right agent automatically.

## Available actions (each maps to one agent):
{action_list}

## Output format — MUST be valid JSON, nothing else:
{{
  "version": 2,
  "rationale": "one sentence explaining the plan",
  "steps": [
    {{
      "index": 1,
      "action": "<one of the actions above>",
      "task": "a concrete, self-contained task description",
      "depends_on": []
    }}
  ]
}}

## Rules (enforced by the validator — violating any of these means plan rejection):
1. `action` MUST be one of the listed actions (case-sensitive).
2. `index` starts at 1 and is monotonically increasing.
3. `depends_on` is a list of earlier `index` values (never the step's own \
index, never a later index).
4. Maximum {max_steps} steps — prefer fewer.
5. Independent steps (no depends_on) can run in parallel.
6. Each `task` must be self-contained — the subagent receives only that \
string and the results of its declared dependencies.
7. `version` MUST be 2 — this tells the validator to route by action.

## Example:
User goal: "Research diffusion transformers and give me a PDF report"
Plan:
{{
  "version": 2,
  "rationale": "Search the Brain in parallel, synthesize to markdown",
  "steps": [
    {{"index": 1, "action": "search_all", "task": "find the DiT paper and summarize its key claims", "depends_on": []}},
    {{"index": 2, "action": "search_all", "task": "find 2 recent survey posts on diffusion transformers", "depends_on": []}},
    {{"index": 3, "action": "summarize", "task": "write a 1500-word markdown report combining the above findings", "depends_on": [1, 2]}}
  ]
}}

Now produce a plan for the user goal. Output ONLY the JSON object.
"""


# ─── Planner ──────────────────────────────────────────────────


LLMCall = Callable[[str, str], str]
"""Callable contract: (system_prompt, user_prompt) -> raw LLM text output."""


class Planner:
    """Generates a Plan from a user goal by calling an LLM.

    Two modes:

    - **Legacy (v1):** pass only `available_roles` — the LLM emits
      `role: <name>` and the validator accepts any listed role.
      Produces plans with `version: 1` in their JSON dump. Existing
      tests and callers continue to work unchanged.

    - **Capability-routed (v2):** pass a `capability_index` — the prompt
      lists available *actions* instead, the LLM emits `action: <name>`,
      and the validator resolves the action to an agent name via the
      index. Produces plans with `version: 2`.

    When both are provided, capability_index wins (v2 behavior).
    """

    def __init__(
        self,
        available_roles: Optional[List[str]] = None,
        llm_call: Optional[LLMCall] = None,
        max_steps: int = MAX_PLAN_STEPS,
        capability_index: Optional["CapabilityIndex"] = None,
    ):
        if llm_call is None:
            raise ValueError("Planner needs llm_call")
        # Must have at least one routing source
        if capability_index is None and not available_roles:
            raise ValueError(
                "Planner needs either available_roles or capability_index"
            )
        self.available_roles = list(available_roles or [])
        self.llm_call = llm_call
        self.max_steps = max_steps
        self.capability_index = capability_index
        self._deprecation_warned = False

    def plan(self, goal: str) -> Plan:
        """Produce a validated Plan for the given goal.

        Retries once on JSON parse failure with a stricter nudge. Raises
        PlannerError if both attempts fail.
        """
        if not goal or not goal.strip():
            raise PlannerError("planner: empty goal")

        system_prompt = self._build_system_prompt()

        last_error: Optional[Exception] = None
        for attempt in range(MAX_PLAN_RETRIES + 1):
            suffix = ""
            if attempt > 0 and last_error is not None:
                suffix = (
                    f"\n\nYOUR PREVIOUS OUTPUT WAS INVALID: {last_error}\n"
                    f"Output ONLY a valid JSON object with the required shape. "
                    f"No prose, no markdown fences, no explanation."
                )
            try:
                raw = self.llm_call(system_prompt + suffix, goal)
                plan = self._parse_and_validate(raw, goal)
                log.info(
                    f"[planner] plan produced ({len(plan.steps)} steps, "
                    f"parallelizable={plan.is_parallelizable}) for goal: {goal[:80]!r}"
                )
                return plan
            except PlannerError as e:
                last_error = e
                log.warning(f"[planner] attempt {attempt + 1} failed: {e}")

        raise PlannerError(
            f"planner: all {MAX_PLAN_RETRIES + 1} attempts failed. "
            f"Last error: {last_error}"
        )

    def _build_system_prompt(self) -> str:
        if self.capability_index is not None:
            # v2: list (action, agent) pairs so the LLM knows what's available
            lines = []
            for action in self.capability_index.all_actions():
                agent = self.capability_index.route(action)
                lines.append(f"  - {action}  (handled by {agent})")
            action_block = "\n".join(lines) if lines else "  (none)"
            return PLANNER_SYSTEM_PROMPT_V2.format(
                max_steps=self.max_steps,
                action_list=action_block,
            )
        roles_block = "\n".join(f"  - {r}" for r in self.available_roles)
        return PLANNER_SYSTEM_PROMPT.format(
            max_steps=self.max_steps,
            role_list=roles_block,
        )

    def _parse_and_validate(self, raw: str, goal: str) -> Plan:
        """Parse the LLM output + run strict validation.

        Dual-mode:
          - version == 2 OR step has `action` field → resolve action via
            CapabilityIndex (requires self.capability_index is not None)
          - version == 1 (default / absent) → classic `role`-based
            validation against available_roles. Emits a one-time
            deprecation warning per Planner instance.
        """
        text = self._strip_fences(raw)
        try:
            data = json.loads(text)
        except json.JSONDecodeError as e:
            raise PlannerError(f"invalid JSON: {e}")

        if not isinstance(data, dict):
            raise PlannerError(f"expected JSON object, got {type(data).__name__}")

        if "steps" not in data:
            raise PlannerError("missing 'steps' field")
        if not isinstance(data["steps"], list):
            raise PlannerError("'steps' must be a list")
        if not data["steps"]:
            raise PlannerError("'steps' is empty — planner must produce at least one step")
        if len(data["steps"]) > self.max_steps:
            raise PlannerError(
                f"plan has {len(data['steps'])} steps > max {self.max_steps}"
            )

        version_raw = data.get("version", 1)
        if not isinstance(version_raw, int) or version_raw not in (1, 2):
            raise PlannerError(
                f"invalid version {version_raw!r} — must be 1 (legacy role) or 2 (action routing)"
            )

        # Decide routing mode. A plan can explicitly state version=2 OR the
        # planner instance can coerce v2 because it was built with a
        # CapabilityIndex. In either case, all steps must use `action:`.
        use_action_routing = (
            version_raw == 2 or self.capability_index is not None
        )
        if use_action_routing and self.capability_index is None:
            raise PlannerError(
                "plan declares version=2 (action routing) but Planner has "
                "no capability_index — cannot resolve actions"
            )

        if not use_action_routing and not self._deprecation_warned:
            log.warning(
                "[planner] plan uses legacy role-based routing (version=1). "
                "Migrate to version=2 with action fields for capability routing."
            )
            self._deprecation_warned = True

        rationale = str(data.get("rationale", ""))
        steps: List[PlanStep] = []
        seen_indices: set = set()

        for i, raw_step in enumerate(data["steps"]):
            if not isinstance(raw_step, dict):
                raise PlannerError(f"step {i} must be a mapping")

            index = raw_step.get("index")
            if not isinstance(index, int):
                raise PlannerError(f"step {i}: 'index' must be an integer")
            if index in seen_indices:
                raise PlannerError(f"step {i}: duplicate index {index}")
            seen_indices.add(index)

            # ─── Routing ───
            if use_action_routing:
                action = raw_step.get("action", "")
                if not isinstance(action, str) or not action:
                    raise PlannerError(
                        f"step {i}: 'action' is required in version=2 plans "
                        f"and must be a non-empty string"
                    )
                resolved_agent = self.capability_index.route(action)
                if resolved_agent is None:
                    raise PlannerError(
                        f"step {i}: action '{action}' not in CapabilityIndex. "
                        f"Known actions: {self.capability_index.all_actions()}"
                    )
                role = resolved_agent
            else:
                role = raw_step.get("role", "")
                if not isinstance(role, str) or not role:
                    raise PlannerError(
                        f"step {i}: 'role' is required and must be a string"
                    )
                if role not in self.available_roles:
                    raise PlannerError(
                        f"step {i}: role '{role}' not in available roles: "
                        f"{self.available_roles}"
                    )
                action = str(raw_step.get("action", ""))

            task = raw_step.get("task", "")
            if not isinstance(task, str) or not task.strip():
                raise PlannerError(f"step {i}: 'task' is required and must be non-empty")

            depends_on_raw = raw_step.get("depends_on", [])
            if not isinstance(depends_on_raw, list):
                raise PlannerError(f"step {i}: 'depends_on' must be a list")
            depends_on: List[int] = []
            for dep in depends_on_raw:
                if not isinstance(dep, int):
                    raise PlannerError(
                        f"step {i}: 'depends_on' entry {dep!r} must be an integer"
                    )
                if dep == index:
                    raise PlannerError(f"step {i}: cannot depend on itself (index={index})")
                if dep >= index:
                    raise PlannerError(
                        f"step {i}: forward dependency index={dep} >= {index}"
                    )
                if dep not in seen_indices:
                    raise PlannerError(
                        f"step {i}: depends_on {dep} which has not been declared yet"
                    )
                depends_on.append(dep)

            steps.append(PlanStep(
                index=index,
                role=role,
                task=task.strip(),
                depends_on=depends_on,
                action=action,
            ))

        plan_version = 2 if use_action_routing else 1
        return Plan(goal=goal, steps=steps, rationale=rationale, version=plan_version)

    @staticmethod
    def _strip_fences(raw: str) -> str:
        """Strip ```json ... ``` fences that LLMs sometimes add despite instructions."""
        s = raw.strip()
        # Leading fence
        m = re.match(r"^```(?:json)?\s*\n?", s)
        if m:
            s = s[m.end():]
        # Trailing fence
        if s.rstrip().endswith("```"):
            s = s.rstrip()[:-3].rstrip()
        return s.strip()
