"""
Layer 5: Session / Process Context

Per-agent session state management, save, and resume.
"""
import json
import time
import uuid
from pathlib import Path
from dataclasses import dataclass, field, asdict
from typing import Optional
from .layer4_ipc import IPCLayer


SESSION_DIR = Path("data/Sessions")


@dataclass
class SessionState:
    session_id: str
    agent_id: str
    created_at: float = field(default_factory=time.time)
    last_active: float = field(default_factory=time.time)
    memory_regions: list = field(default_factory=list)
    task_graph_refs: list = field(default_factory=list)
    artifacts: list = field(default_factory=list)
    pinned: bool = False


class SessionLayer:
    """Layer 5: Per-agent session state and working memory."""

    def __init__(self, ipc: IPCLayer):
        self.ipc = ipc
        self.active_sessions: dict[str, SessionState] = {}
        SESSION_DIR.mkdir(parents=True, exist_ok=True)

    def create_session(self, agent_id: str) -> SessionState:
        """Create a new session for an agent."""
        session_id = f"{agent_id}_{int(time.time() * 1000)}"
        state = SessionState(session_id=session_id, agent_id=agent_id)
        self.active_sessions[session_id] = state
        return state

    def save_session(self, session_id: str) -> bool:
        """Persist session state to disk."""
        state = self.active_sessions.get(session_id)
        if not state:
            return False
        path = SESSION_DIR / f"{session_id}.json"
        path.write_text(json.dumps(asdict(state)))
        return True

    def resume_session(self, session_id: str) -> Optional[SessionState]:
        """Load session state from disk."""
        path = SESSION_DIR / f"{session_id}.json"
        if not path.exists():
            return None
        data = json.loads(path.read_text())
        state = SessionState(**data)
        self.active_sessions[session_id] = state
        return state

    def list_sessions(self, agent_id: str = "") -> list[SessionState]:
        """List all sessions, optionally filtered by agent."""
        results = []
        for state in self.active_sessions.values():
            if not agent_id or state.agent_id == agent_id:
                results.append(state)
        return results

    def add_region_to_session(self, session_id: str, region_id: str):
        """Track a memory region as belonging to a session."""
        state = self.active_sessions.get(session_id)
        if state and region_id not in state.memory_regions:
            state.memory_regions.append(region_id)

    def add_artifact_to_session(self, session_id: str, artifact_id: str):
        """Track an artifact as produced by a session."""
        state = self.active_sessions.get(session_id)
        if state and artifact_id not in state.artifacts:
            state.artifacts.append(artifact_id)
