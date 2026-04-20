"""
agent_team.py — Phase 3 deliverable

TeamComposition: pre-configured agent rosters that compile into concrete
WorkflowStep lists for the AsyncDAGExecutor. A team is a reusable recipe:
"for a research job, run N researchers in parallel, feed their outputs into
a synthesizer, then persist the synthesis via storage."

Teams are pure data — they have no knowledge of artifact stores, event buses,
or the executor. `build_workflow_from_team()` is the adapter that turns a
TeamRoster into a real Workflow on a WorkflowEngine.

Exposed:
  - TeamMember, TeamRoster (dataclasses)
  - TeamComposition (factory with built-in teams + for_request dispatch)
  - build_workflow_from_team(engine, team, context) -> Workflow
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Optional

from core.workflow.engine import Workflow, WorkflowEngine, WorkflowStep


# ─── Data model ─────────────────────────────────────────────────────


@dataclass
class TeamMember:
    """A single role on a team. `count > 1` means spawn N parallel instances."""

    agent: str                       # subagent NAME (must match Subagent.NAME)
    action: str                      # one of that agent's ACTIONS
    role: str = ""                   # human label, e.g. "parallel_researcher"
    count: int = 1                   # parallelism for this role
    depends_on_roles: List[str] = field(default_factory=list)
    # per-step input context template; formatted with runtime context
    input_template: Dict[str, Any] = field(default_factory=dict)


@dataclass
class TeamRoster:
    """A compile-time team definition. Immutable once constructed."""

    name: str
    description: str
    members: List[TeamMember]
    default_parallelism: int = 1     # only used by research_team for the scaling knob

    def role_ids(self) -> List[str]:
        return [m.role or m.agent for m in self.members]

    def total_steps(self) -> int:
        return sum(m.count for m in self.members)


# ─── Team factory ───────────────────────────────────────────────────


class TeamComposition:
    """
    Factory for the built-in teams. Phase 3 ships four canonical rosters that
    cover every swarm pattern in the current subagent catalog:

      research_team    — N researchers in parallel → synthesizer → storage
      creative_team    — image_gen → storage
      archive_team     — researcher → storage (fast path, no synthesis)
      minimal_team     — single researcher, no downstream (for smoke tests)

    Extend this class with new @staticmethods as more subagent types land.
    """

    # Mapping of intent → team factory. Keeps IntelligentRouter decoupled
    # from the actual team definitions.
    _DISPATCH: Dict[str, Callable[..., "TeamRoster"]] = {}

    # ── Built-in teams ──

    @staticmethod
    def research_team(parallelism: int = 2) -> TeamRoster:
        """
        N parallel researchers feed a synthesizer that hands off to storage.

        The canonical swarm pattern: the reason Phase 2's end-to-end test
        measured 0.63s vs 1.2s sequential. Increase `parallelism` to scale
        horizontally — every researcher gets its own step and runs
        concurrently under AsyncDAGExecutor.
        """
        parallelism = max(1, int(parallelism))
        members = [
            TeamMember(
                agent="researcher",
                action="search_all",
                role="parallel_researcher",
                count=parallelism,
                input_template={"query": "{query}"},
            ),
            TeamMember(
                agent="synthesizer",
                action="combine",
                role="synthesizer",
                count=1,
                depends_on_roles=["parallel_researcher"],
            ),
            TeamMember(
                agent="storage",
                action="save_to_brain",
                role="storage",
                count=1,
                depends_on_roles=["synthesizer"],
            ),
        ]
        return TeamRoster(
            name="research_team",
            description=f"{parallelism}× researcher → synthesizer → storage",
            members=members,
            default_parallelism=parallelism,
        )

    @staticmethod
    def creative_team() -> TeamRoster:
        """Image generation → storage. Used for visual asset creation."""
        members = [
            TeamMember(
                agent="image_gen",
                action="generate",
                role="image",
                count=1,
                input_template={
                    "prompt": "{prompt}",
                    "aspect_ratio": "{aspect_ratio}",
                },
            ),
            TeamMember(
                agent="storage",
                action="save_to_brain",
                role="storage",
                count=1,
                depends_on_roles=["image"],
            ),
        ]
        return TeamRoster(
            name="creative_team",
            description="image_gen → storage",
            members=members,
        )

    @staticmethod
    def archive_team() -> TeamRoster:
        """
        Fast research-to-archive path. One researcher, straight to storage,
        no synthesis. For "just save this" type requests.
        """
        members = [
            TeamMember(
                agent="researcher",
                action="search_all",
                role="researcher",
                count=1,
                input_template={"query": "{query}"},
            ),
            TeamMember(
                agent="storage",
                action="save_to_brain",
                role="storage",
                count=1,
                depends_on_roles=["researcher"],
            ),
        ]
        return TeamRoster(
            name="archive_team",
            description="researcher → storage (no synthesis)",
            members=members,
        )

    @staticmethod
    def minimal_team() -> TeamRoster:
        """Single researcher. For trivial queries or smoke tests."""
        members = [
            TeamMember(
                agent="researcher",
                action="search_all",
                role="researcher",
                count=1,
                input_template={"query": "{query}"},
            ),
        ]
        return TeamRoster(
            name="minimal_team",
            description="single researcher only",
            members=members,
        )

    # ── Intent dispatch ──

    @classmethod
    def for_intent(
        cls,
        intent: str,
        parallelism: int = 2,
    ) -> TeamRoster:
        """
        Return the team that best matches an intent label.

        Called by IntelligentRouter once it has classified a request. Falls
        back to `minimal_team` for unknown intents rather than crashing —
        the caller can decide whether that's acceptable.
        """
        intent = (intent or "").strip().lower()
        if intent == "research":
            return cls.research_team(parallelism=parallelism)
        if intent == "image":
            return cls.creative_team()
        if intent == "archive":
            return cls.archive_team()
        if intent == "minimal":
            return cls.minimal_team()
        return cls.minimal_team()

    @classmethod
    def available_teams(cls) -> List[str]:
        return ["research_team", "creative_team", "archive_team", "minimal_team"]


# ─── Workflow builder (adapter: TeamRoster → Workflow) ──────────────


def _render_template(template: Dict[str, Any], ctx: Dict[str, Any]) -> Dict[str, Any]:
    """
    Render a per-step input_template by substituting '{key}' placeholders
    with values from ctx. Missing keys are left as literal strings so the
    subagent can decide how to handle them.
    """
    out: Dict[str, Any] = {}
    for k, v in template.items():
        if isinstance(v, str) and v.startswith("{") and v.endswith("}"):
            key = v[1:-1]
            if key in ctx:
                out[k] = ctx[key]
            else:
                out[k] = v  # leave placeholder — lets subagent error clearly
        else:
            out[k] = v
    return out


def build_workflow_from_team(
    engine: WorkflowEngine,
    team: TeamRoster,
    context: Dict[str, Any],
    workflow_name: Optional[str] = None,
    description: Optional[str] = None,
) -> Workflow:
    """
    Compile a TeamRoster into a Workflow with concrete WorkflowSteps.

    Expansion rules:
      - A member with `count=N` becomes N steps with ids like
        `{role}_1`, `{role}_2`, …, `{role}_N` (or `{role}` if count=1).
      - Members with `depends_on_roles=["parallel_researcher"]` depend on
        ALL step ids produced by that role.
      - Steps that depend on an upstream multi-role get their
        `reads_artifacts` populated so the synthesizer can pull both
        researchers' outputs through AsyncDAGExecutor's resolved_artifacts.

    The returned Workflow is saved to the engine but NOT started; the
    caller kicks it off with `await executor.run_workflow(wf)`.
    """
    wf = engine.create_workflow(
        workflow_name or team.name,
        description=description or team.description,
    )

    # Track role → list of concrete step ids (for dependency wiring)
    role_steps: Dict[str, List[str]] = {}
    all_steps: List[WorkflowStep] = []

    for member in team.members:
        role_key = member.role or member.agent
        role_steps[role_key] = []

        for i in range(1, member.count + 1):
            step_id = f"{role_key}" if member.count == 1 else f"{role_key}_{i}"
            role_steps[role_key].append(step_id)

            # Resolve dependencies from upstream roles
            deps: List[str] = []
            reads_artifacts: List[str] = []
            for dep_role in member.depends_on_roles:
                upstream_ids = role_steps.get(dep_role, [])
                deps.extend(upstream_ids)
                for upstream_id in upstream_ids:
                    reads_artifacts.append(f"{wf.id}:{upstream_id}")

            # Render per-step input_context
            input_ctx = _render_template(member.input_template, context)
            # Pass through any extra global context keys that match the
            # member's placeholders (idempotent).
            if reads_artifacts:
                input_ctx["reads_artifacts"] = reads_artifacts

            step = WorkflowStep(
                id=step_id,
                name=f"{role_key}".replace("_", " ").title(),
                agent=member.agent,
                action=member.action,
                input_context=input_ctx,
                depends_on=deps,
            )
            all_steps.append(step)

    wf.steps = all_steps
    engine.save_workflow(wf)
    return wf


__all__ = [
    "TeamMember",
    "TeamRoster",
    "TeamComposition",
    "build_workflow_from_team",
]
