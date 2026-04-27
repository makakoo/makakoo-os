"""
Data models for the Harvey task tree.

A Task is a durable record of cognitive work. Tasks form a tree via parent_id
(root=user goal, children=plan steps, grandchildren=subagent invocations).
Every interesting thing that happens during execution is appended as a
TaskEntry. Files and URLs produced are recorded as TaskArtifacts.
"""

from __future__ import annotations

import json
import time
import uuid
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, List, Optional


def new_id() -> str:
    return uuid.uuid4().hex


class TaskKind(str, Enum):
    ROOT = "root"            # user-facing goal
    PLAN = "plan"            # planner decomposition node
    STEP = "step"            # plan step (executed by executor or subagent)
    SUBAGENT = "subagent"    # isolated subprocess agent


class TaskState(str, Enum):
    QUEUED = "queued"
    PLANNING = "planning"
    RUNNING = "running"
    AWAITING_INPUT = "awaiting_input"
    RESUMING = "resuming"
    PAUSED = "paused"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"

    @classmethod
    def active_states(cls) -> tuple["TaskState", ...]:
        return (cls.QUEUED, cls.PLANNING, cls.RUNNING, cls.AWAITING_INPUT, cls.RESUMING)

    @classmethod
    def terminal_states(cls) -> tuple["TaskState", ...]:
        return (cls.COMPLETED, cls.FAILED, cls.CANCELLED)


class EntryType(str, Enum):
    MESSAGE = "message"          # role=user|assistant|system
    TOOL_CALL = "tool_call"
    TOOL_RESULT = "tool_result"
    THOUGHT = "thought"          # assistant's reasoning if exposed
    CHECKPOINT = "checkpoint"    # loop-level checkpoint marker
    COMPACTION = "compaction"    # summary replacing older entries
    ERROR = "error"              # hard failure
    PROGRESS = "progress"        # user-facing "still working..." messages


@dataclass
class Task:
    id: str
    root_id: str
    parent_id: Optional[str]
    depth: int

    channel: str
    user_id: str

    kind: TaskKind
    goal: str
    state: TaskState
    plan_json: Optional[str] = None

    created_at: float = field(default_factory=time.time)
    started_at: Optional[float] = None
    completed_at: Optional[float] = None
    heartbeat: Optional[float] = None

    result: str = ""
    error: str = ""
    metadata: Dict[str, Any] = field(default_factory=dict)

    assignee: Optional[str] = None

    @classmethod
    def new_root(cls, *, channel: str, user_id: str, goal: str) -> "Task":
        tid = new_id()
        return cls(
            id=tid,
            root_id=tid,
            parent_id=None,
            depth=0,
            channel=channel,
            user_id=user_id,
            kind=TaskKind.ROOT,
            goal=goal,
            state=TaskState.QUEUED,
        )

    @classmethod
    def new_child(
        cls,
        parent: "Task",
        *,
        kind: TaskKind,
        goal: str,
    ) -> "Task":
        return cls(
            id=new_id(),
            root_id=parent.root_id,
            parent_id=parent.id,
            depth=parent.depth + 1,
            channel=parent.channel,
            user_id=parent.user_id,
            kind=kind,
            goal=goal,
            state=TaskState.QUEUED,
        )

    def is_active(self) -> bool:
        return self.state in TaskState.active_states()

    def is_terminal(self) -> bool:
        return self.state in TaskState.terminal_states()


@dataclass
class TaskEntry:
    id: str
    task_id: str
    parent_entry_id: Optional[str]
    entry_type: EntryType
    content: str
    role: Optional[str] = None
    tool_name: Optional[str] = None
    is_error: bool = False
    tokens_in: Optional[int] = None
    tokens_out: Optional[int] = None
    created_at: float = field(default_factory=time.time)
    actor: Optional[str] = None

    @classmethod
    def message(
        cls,
        task_id: str,
        role: str,
        content: str,
        actor: Optional[str] = None,
    ) -> "TaskEntry":
        return cls(
            id=new_id(),
            task_id=task_id,
            parent_entry_id=None,
            entry_type=EntryType.MESSAGE,
            role=role,
            content=content,
            actor=actor,
        )

    @classmethod
    def tool_call(
        cls,
        task_id: str,
        tool_name: str,
        args: Dict[str, Any],
        parent_entry_id: Optional[str] = None,
        actor: Optional[str] = None,
    ) -> "TaskEntry":
        return cls(
            id=new_id(),
            task_id=task_id,
            parent_entry_id=parent_entry_id,
            entry_type=EntryType.TOOL_CALL,
            tool_name=tool_name,
            content=json.dumps(args, default=str),
            actor=actor,
        )

    @classmethod
    def tool_result(
        cls,
        task_id: str,
        tool_name: str,
        result: str,
        parent_entry_id: str,
        is_error: bool = False,
        actor: Optional[str] = None,
    ) -> "TaskEntry":
        return cls(
            id=new_id(),
            task_id=task_id,
            parent_entry_id=parent_entry_id,
            entry_type=EntryType.TOOL_RESULT,
            tool_name=tool_name,
            content=result,
            is_error=is_error,
            actor=actor,
        )


@dataclass
class TaskArtifact:
    id: str
    task_id: str
    kind: str                       # 'file'|'url'|'image'|'pdf'|'data'
    path: Optional[str]
    url: Optional[str]
    mime: Optional[str]
    size_bytes: Optional[int]
    description: str = ""
    sent_to_user: bool = False
    created_at: float = field(default_factory=time.time)

    @classmethod
    def file(
        cls,
        task_id: str,
        path: str,
        mime: str = "",
        size_bytes: Optional[int] = None,
        description: str = "",
    ) -> "TaskArtifact":
        return cls(
            id=new_id(),
            task_id=task_id,
            kind="file",
            path=path,
            url=None,
            mime=mime,
            size_bytes=size_bytes,
            description=description,
        )

    @classmethod
    def pdf(cls, task_id: str, path: str, size_bytes: Optional[int] = None) -> "TaskArtifact":
        return cls(
            id=new_id(),
            task_id=task_id,
            kind="pdf",
            path=path,
            url=None,
            mime="application/pdf",
            size_bytes=size_bytes,
            description="",
        )
