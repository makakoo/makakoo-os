"""
AgentManifest — machine-readable schema for a Harvey plugin agent.

One manifest per agent. Can be loaded from `agents/<name>/agent.yaml` OR
synthesized in-memory from a Subagent class by the loader.

Schema (see templates/agent.yaml.jinja for the full example):

    name: arbitrage-agent
    version: "1.0"
    description: "..."
    type: daemon  # daemon | cron | cli | subagent | body-adapter
    status: active  # active | experimental | disabled
    runtime:
      command: python3
      entrypoint: agent.py
      workdir: agents/arbitrage-agent
      args: []
    schedule: "*/15 * * * *"  # only for type=cron
    tools:
      - name: check_arbitrage
        description: "..."
        schema: null
    comm:
      type: in-process | mailbox | subprocess
      channel: "agents.arbitrage"
    state:
      dir: data/arbitrage-agent
      schema: files | sqlite | none
    requires:
      - polymarket-api
      - skill:arbitrage-research

Validation is strict: unknown fields are rejected, required fields
must be present, enum fields must match known values.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, List, Optional


class ManifestValidationError(ValueError):
    """Raised when an agent.yaml fails validation."""
    pass


class AgentType(str, Enum):
    DAEMON = "daemon"             # long-running process (harveychat, arbitrage bot)
    CRON = "cron"                 # scheduled via crontab / apscheduler
    CLI = "cli"                   # on-demand command (ingest-once scripts)
    SUBAGENT = "subagent"         # in-process, loaded into AgentCoordinator
    BODY_ADAPTER = "body-adapter" # messaging gateway (harveychat-style)


class AgentState(str, Enum):
    ACTIVE = "active"
    EXPERIMENTAL = "experimental"
    DISABLED = "disabled"


@dataclass
class AgentRuntime:
    """How to actually execute the agent (for daemon / cron / cli types)."""
    command: str = "python3"
    entrypoint: str = ""
    workdir: str = ""
    args: List[str] = field(default_factory=list)


@dataclass
class AgentComm:
    """How the agent communicates with the rest of Harvey."""
    type: str = "in-process"        # in-process | mailbox | subprocess
    channel: str = ""                # topic / queue name / mailbox name


@dataclass
class AgentTool:
    """One tool the agent exposes to callers (harvey_agent LLM tool loop)."""
    name: str
    description: str = ""
    schema: Optional[Dict[str, Any]] = None


@dataclass
class AgentManifest:
    """Machine-readable manifest for a Harvey plugin agent."""

    name: str
    description: str = ""
    version: str = "1.0"
    type: AgentType = AgentType.DAEMON
    status: AgentState = AgentState.ACTIVE

    runtime: AgentRuntime = field(default_factory=AgentRuntime)
    comm: AgentComm = field(default_factory=AgentComm)
    tools: List[AgentTool] = field(default_factory=list)
    schedule: str = ""                           # cron syntax for type=cron
    state_dir: str = ""                          # relative to HARVEY_HOME
    state_schema: str = "files"                  # files | sqlite | none
    requires: List[str] = field(default_factory=list)

    # Phase 4 (SPRINT-HARVEY-TICKETING): optional concurrency cap.
    # None = no limit. Positive int = AgentCoordinator wraps agent.handle
    # in a threading.BoundedSemaphore(max_concurrency). Used for agents
    # that call external APIs with per-account rate limits (image_gen
    # hits MiniMax, which does not like concurrent generate calls).
    max_concurrency: Optional[int] = None

    # Where this manifest came from — for debugging / introspection
    source_path: str = ""                        # e.g. "agents/arbitrage-agent/agent.yaml"

    # ─── Factory methods ────────────────────────────────────────

    _KNOWN_TOP_LEVEL_FIELDS = frozenset({
        "name", "description", "version", "type", "status",
        "runtime", "comm", "tools", "schedule", "state", "requires",
        "max_concurrency",
    })

    @classmethod
    def from_dict(cls, data: Dict[str, Any], source_path: str = "") -> "AgentManifest":
        """Build a manifest from a parsed YAML/JSON dict.

        Strict: required fields must be present, enum values must match,
        and any top-level field not in _KNOWN_TOP_LEVEL_FIELDS is rejected
        to catch typos early.
        """
        if not isinstance(data, dict):
            raise ManifestValidationError(
                f"expected mapping at top level, got {type(data).__name__}"
            )

        # Reject unknown top-level fields (catches typos like "typ:" → "type:")
        unknown = set(data.keys()) - cls._KNOWN_TOP_LEVEL_FIELDS
        if unknown:
            raise ManifestValidationError(
                f"unknown field(s): {sorted(unknown)} — "
                f"allowed: {sorted(cls._KNOWN_TOP_LEVEL_FIELDS)}"
            )

        # Required: name
        name = data.get("name")
        if not name or not isinstance(name, str):
            raise ManifestValidationError("manifest missing required field: name")

        # Optional but validated: type + status
        type_raw = data.get("type", "daemon")
        try:
            agent_type = AgentType(type_raw)
        except ValueError:
            raise ManifestValidationError(
                f"invalid type '{type_raw}' — must be one of "
                f"{[t.value for t in AgentType]}"
            )

        status_raw = data.get("status", "active")
        try:
            status = AgentState(status_raw)
        except ValueError:
            raise ManifestValidationError(
                f"invalid status '{status_raw}' — must be one of "
                f"{[s.value for s in AgentState]}"
            )

        # Runtime sub-dict
        runtime_raw = data.get("runtime") or {}
        if not isinstance(runtime_raw, dict):
            raise ManifestValidationError(
                f"'runtime' must be a mapping, got {type(runtime_raw).__name__}"
            )
        runtime = AgentRuntime(
            command=str(runtime_raw.get("command", "python3")),
            entrypoint=str(runtime_raw.get("entrypoint", "")),
            workdir=str(runtime_raw.get("workdir", "")),
            args=list(runtime_raw.get("args", []) or []),
        )

        # Comm sub-dict
        comm_raw = data.get("comm") or {}
        if not isinstance(comm_raw, dict):
            raise ManifestValidationError("'comm' must be a mapping")
        comm = AgentComm(
            type=str(comm_raw.get("type", "in-process")),
            channel=str(comm_raw.get("channel", "")),
        )

        # Tools list
        tools_raw = data.get("tools") or []
        if not isinstance(tools_raw, list):
            raise ManifestValidationError("'tools' must be a list")
        tools: List[AgentTool] = []
        for idx, t in enumerate(tools_raw):
            if not isinstance(t, dict):
                raise ManifestValidationError(
                    f"tools[{idx}] must be a mapping, got {type(t).__name__}"
                )
            if "name" not in t or not t["name"]:
                raise ManifestValidationError(f"tools[{idx}] missing 'name'")
            tools.append(AgentTool(
                name=str(t["name"]),
                description=str(t.get("description", "")),
                schema=t.get("schema"),
            ))

        # State sub-dict
        state_raw = data.get("state") or {}
        if not isinstance(state_raw, dict):
            raise ManifestValidationError("'state' must be a mapping")
        state_dir = str(state_raw.get("dir", ""))
        state_schema = str(state_raw.get("schema", "files"))
        if state_schema not in ("files", "sqlite", "none"):
            raise ManifestValidationError(
                f"state.schema '{state_schema}' must be files|sqlite|none"
            )

        # Requires list
        requires_raw = data.get("requires") or []
        if not isinstance(requires_raw, list):
            raise ManifestValidationError("'requires' must be a list")
        requires = [str(r) for r in requires_raw]

        # Cron schedule (only meaningful for type=cron, but always allowed)
        schedule = str(data.get("schedule", ""))
        if agent_type == AgentType.CRON and not schedule:
            raise ManifestValidationError(
                f"type=cron requires a 'schedule' field (cron syntax)"
            )

        # Phase 4: max_concurrency — optional, None or positive int
        max_concurrency_raw = data.get("max_concurrency", None)
        if max_concurrency_raw is None:
            max_concurrency: Optional[int] = None
        else:
            if not isinstance(max_concurrency_raw, int) or isinstance(max_concurrency_raw, bool):
                raise ManifestValidationError(
                    f"'max_concurrency' must be an integer or null, "
                    f"got {type(max_concurrency_raw).__name__}"
                )
            if max_concurrency_raw <= 0:
                raise ManifestValidationError(
                    f"'max_concurrency' must be positive, got {max_concurrency_raw}"
                )
            max_concurrency = max_concurrency_raw

        return cls(
            name=name,
            description=str(data.get("description", "")),
            version=str(data.get("version", "1.0")),
            type=agent_type,
            status=status,
            runtime=runtime,
            comm=comm,
            tools=tools,
            schedule=schedule,
            state_dir=state_dir,
            state_schema=state_schema,
            requires=requires,
            max_concurrency=max_concurrency,
            source_path=source_path,
        )

    @classmethod
    def from_yaml_text(cls, text: str, source_path: str = "") -> "AgentManifest":
        """Parse YAML source → AgentManifest. Requires PyYAML."""
        try:
            import yaml
        except ImportError as e:
            raise ManifestValidationError(f"PyYAML required for YAML parsing: {e}")
        try:
            data = yaml.safe_load(text)
        except yaml.YAMLError as e:
            raise ManifestValidationError(f"YAML parse error in {source_path}: {e}")
        return cls.from_dict(data or {}, source_path=source_path)

    @classmethod
    def from_subagent_class(cls, sub_cls: type, source_path: str = "") -> "AgentManifest":
        """Synthesize a manifest from a Subagent subclass.

        Reads NAME, ACTIONS, DESCRIPTION class attributes and builds an
        in-process subagent manifest. This is how core/subagents/*.py
        classes get registered without each needing their own YAML file.
        """
        name = getattr(sub_cls, "NAME", None) or sub_cls.__name__.lower().replace("agent", "")
        description = getattr(sub_cls, "DESCRIPTION", "") or sub_cls.__doc__ or ""
        actions = list(getattr(sub_cls, "ACTIONS", []) or [])
        max_concurrency_raw = getattr(sub_cls, "MAX_CONCURRENCY", None)
        max_concurrency: Optional[int] = (
            max_concurrency_raw
            if isinstance(max_concurrency_raw, int)
            and not isinstance(max_concurrency_raw, bool)
            and max_concurrency_raw > 0
            else None
        )

        tools = [
            AgentTool(
                name=action,
                description=f"{name} subagent action: {action}",
                schema=None,
            )
            for action in actions
        ]

        return cls(
            name=name,
            description=description.strip().split("\n")[0][:200],
            type=AgentType.SUBAGENT,
            status=AgentState.ACTIVE,
            runtime=AgentRuntime(command="in-process"),
            comm=AgentComm(type="in-process", channel=f"subagent.{name}"),
            tools=tools,
            max_concurrency=max_concurrency,
            source_path=source_path or f"core/subagents/{sub_cls.__module__.rsplit('.', 1)[-1]}.py",
        )

    # ─── Introspection ──────────────────────────────────────────

    def to_dict(self) -> Dict[str, Any]:
        return {
            "name": self.name,
            "version": self.version,
            "description": self.description,
            "type": self.type.value,
            "status": self.status.value,
            "runtime": {
                "command": self.runtime.command,
                "entrypoint": self.runtime.entrypoint,
                "workdir": self.runtime.workdir,
                "args": list(self.runtime.args),
            },
            "comm": {
                "type": self.comm.type,
                "channel": self.comm.channel,
            },
            "tools": [
                {"name": t.name, "description": t.description, "schema": t.schema}
                for t in self.tools
            ],
            "schedule": self.schedule,
            "state": {"dir": self.state_dir, "schema": self.state_schema},
            "requires": list(self.requires),
            "max_concurrency": self.max_concurrency,
            "source_path": self.source_path,
        }

    def is_active(self) -> bool:
        return self.status == AgentState.ACTIVE
