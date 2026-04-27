"""
Session State Manager - Cross-Session State Persistence

Manages session boundaries, in-progress work, and key decisions across sessions.
Session states stored as JSON files in data/Brain/sessions/.
"""

import json
import os
import subprocess
from datetime import datetime
from typing import Optional, Dict, Any, List

_HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
BRAIN_DIR = os.path.join(_HARVEY_HOME, "data", "Brain")
SESSIONS_DIR = os.path.join(BRAIN_DIR, "sessions")


class SessionStateManager:
    """
    Persists session state between Harvey sessions.

    Session state schema:
    {
        "session_id": "session_2026_03_27_1432",
        "started_at": "2026-03-27T14:32:00Z",
        "ended_at": null,
        "directory": "~/MAKAKOO",
        "context_summary": "Working on 6-layer memory substrate design",
        "active_files": [...],
        "pending_tasks": [...],
        "key_decisions": [...],
        "git_branch": "memory/substrate",
        "uncommitted_changes": [...],
        "last_memory_load": "2026-03-27T14:32:05Z",
        "memory_layers_loaded": ["identity", "today", "active_projects"]
    }
    """

    def __init__(self, sessions_dir: str = SESSIONS_DIR):
        self.sessions_dir = sessions_dir
        os.makedirs(self.sessions_dir, exist_ok=True)

    def start_session(self, session_id: Optional[str] = None) -> Dict[str, Any]:
        """
        Create a new session state file.

        Args:
            session_id: Optional custom session ID. If None, auto-generates.

        Returns:
            The created session state dict
        """
        if session_id is None:
            ts = datetime.now().strftime("%Y%m%d_%H%M")
            session_id = f"session_{ts}"

        state = {
            "session_id": session_id,
            "started_at": datetime.now().isoformat(),
            "ended_at": None,
            "directory": os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")),
            "context_summary": "",
            "active_files": [],
            "pending_tasks": [],
            "key_decisions": [],
            "git_branch": self._get_git_branch(),
            "uncommitted_changes": self._get_uncommitted_changes(),
            "last_memory_load": None,
            "memory_layers_loaded": []
        }

        self._save_state(state)
        return state

    def end_session(self, session_id: str) -> Optional[Dict[str, Any]]:
        """
        Mark a session as ended.

        Args:
            session_id: Session ID to end

        Returns:
            Updated session state or None if not found
        """
        state = self._load_state(session_id)
        if state:
            state["ended_at"] = datetime.now().isoformat()
            self._save_state(state)
        return state

    def get_last_session(self) -> Optional[Dict[str, Any]]:
        """
        Load the most recent session state.

        Returns:
            Most recent session state or None if no sessions exist
        """
        sessions = sorted(self._list_sessions())
        if sessions:
            return self._load_state(sessions[-1])
        return None

    def get_or_create_session(self) -> Dict[str, Any]:
        """
        Get existing session or create a new one.

        If last session has no ended_at, returns it (resume scenario).
        Otherwise creates a new session.

        Returns:
            Session state dict
        """
        last = self.get_last_session()
        if last and last.get("ended_at") is None:
            return last  # Resume existing session
        return self.start_session()

    def update_state(
        self,
        session_id: str,
        updates: Dict[str, Any]
    ) -> Optional[Dict[str, Any]]:
        """
        Merge updates into session state.

        Args:
            session_id: Session to update
            updates: Dict of fields to update

        Returns:
            Updated session state or None if not found
        """
        state = self._load_state(session_id)
        if state:
            # Merge updates (except session_id, started_at)
            for key, value in updates.items():
                if key not in ("session_id", "started_at"):
                    state[key] = value
            self._save_state(state)
        return state

    def list_sessions(self) -> List[Dict[str, Any]]:
        """List all sessions with their metadata."""
        sessions = []
        for session_file in self._list_sessions():
            state = self._load_state(session_file)
            if state:
                sessions.append(state)
        return sorted(sessions, key=lambda x: x.get("started_at", ""), reverse=True)

    def _session_path(self, session_id: str) -> str:
        """Get path for session file."""
        return os.path.join(self.sessions_dir, f"{session_id}.json")

    def _list_sessions(self) -> List[str]:
        """List all session IDs (filenames without .json)."""
        sessions = []
        if os.path.exists(self.sessions_dir):
            for fname in os.listdir(self.sessions_dir):
                if fname.endswith(".json"):
                    sessions.append(fname[:-5])  # Remove .json
        return sorted(sessions)

    def _save_state(self, state: Dict[str, Any]) -> None:
        """Write session state to disk."""
        path = self._session_path(state["session_id"])
        with open(path, "w") as f:
            json.dump(state, f, indent=2)

    def _load_state(self, session_id: str) -> Optional[Dict[str, Any]]:
        """Load session state from disk."""
        # Handle both with and without .json extension
        if not session_id.endswith(".json"):
            session_id = session_id + ".json"
        path = os.path.join(self.sessions_dir, session_id)
        if os.path.exists(path):
            with open(path) as f:
                return json.load(f)
        return None

    def _get_git_branch(self) -> str:
        """Get current git branch."""
        try:
            result = subprocess.run(
                ["git", "rev-parse", "--abbrev-ref", "HEAD"],
                cwd=os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")),
                capture_output=True,
                text=True,
                timeout=5
            )
            if result.returncode == 0:
                return result.stdout.strip()
        except Exception:
            pass
        return "unknown"

    def _get_uncommitted_changes(self) -> List[str]:
        """Get list of uncommitted changed files."""
        try:
            result = subprocess.run(
                ["git", "diff", "--name-only"],
                cwd=os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")),
                capture_output=True,
                text=True,
                timeout=5
            )
            if result.returncode == 0:
                return [f.strip() for f in result.stdout.strip().split("\n") if f.strip()]
        except Exception:
            pass
        return []


if __name__ == "__main__":
    mgr = SessionStateManager()
    session = mgr.get_or_create_session()
    print(f"Session: {session['session_id']}")
    print(f"Started: {session['started_at']}")
    print(f"Branch: {session['git_branch']}")
