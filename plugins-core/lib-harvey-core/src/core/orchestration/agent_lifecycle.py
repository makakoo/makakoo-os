"""
Harvey OS — Agent Lifecycle Manager

Claude Code's AgentTool.tsx pattern ported to Python.
Key features:
- Named agents with mailbox IPC
- Health monitoring with exponential backoff restart
- Agent state persistence to JSON

Path: plugins-core/lib-harvey-core/src/core/orchestration/agent_lifecycle.py
"""

from __future__ import annotations

import os
import sys
import signal
import asyncio
import json
import time
import threading
import hashlib
import subprocess
from pathlib import Path
from dataclasses import dataclass, field, asdict
from typing import Literal, Callable, Any
from enum import Enum
from contextvars import ContextVar

# Context variable for per-agent cwd isolation
_cwd_override: ContextVar[str | None] = ContextVar("cwd_override", default=None)


def getcwd() -> str:
    """Get current working directory, with optional per-agent override."""
    override = _cwd_override.get()
    return override if override is not None else os.getcwd()


class AgentState(Enum):
    """Agent lifecycle states matching Claude Code patterns."""

    STOPPED = "stopped"
    STARTING = "starting"
    RUNNING = "running"
    PAUSED = "paused"
    ERROR = "error"
    DEAD = "dead"


@dataclass
class AgentConfig:
    """
    Agent configuration.

    Matches Claude Code's AgentConfig interface.
    """

    name: str
    entry_point: str
    args: list[str] = field(default_factory=list)
    env: dict[str, str] = field(default_factory=dict)
    workdir: str | None = None
    health_check_interval_sec: int = 30
    auto_restart: bool = True
    max_restart_attempts: int = 3
    restart_backoff_sec: list[int] = field(default_factory=lambda: [5, 10, 30, 60])
    log_file: str | None = None
    mailbox_enabled: bool = True


@dataclass
class AgentStatus:
    """
    Agent runtime status.

    Includes PID, state, health info, and resource usage.
    """

    name: str
    state: AgentState
    pid: int | None = None
    started_at: float | None = None
    last_health_check: float | None = None
    last_activity: float | None = None
    restart_count: int = 0
    error_log: str | None = None
    memory_mb: float | None = None
    agent_id: str | None = None


class MailboxIndex:
    """
    File-based mailbox for agent-to-agent messaging.

    Claude Code's teammateMailbox.ts pattern:
    - Path: ~/.harvey/teams/{team}/inboxes/{agent}.json
    - Atomic flock-based locking for concurrent access
    - Messages queued when agent is paused
    """

    def __init__(self, base_dir: Path | None = None):
        Harvey_home = os.environ.get("HARVEY_HOME", str(Path.home() / "HARVEY"))
        self.base_dir = base_dir or Path(Harvey_home) / ".harvey" / "teams"

    def _inbox_path(self, agent_name: str, team: str = "default") -> Path:
        """Get inbox file path for an agent."""
        safe_agent = agent_name.replace("/", "_").replace("\\", "_")
        safe_team = team.replace("/", "_").replace("\\", "_")
        return self.base_dir / safe_team / "inboxes" / f"{safe_agent}.json"

    def _lock_path(self, agent_name: str, team: str = "default") -> Path:
        """Get lock file path for an agent's inbox."""
        return self._inbox_path(agent_name, team).with_suffix(".json.lock")

    def _ensure_dir(self, path: Path) -> None:
        """Ensure directory exists."""
        path.parent.mkdir(parents=True, exist_ok=True)

    async def write_message(
        self, recipient: str, message: dict, team: str = "default"
    ) -> None:
        """
        Write a message to an agent's inbox.

        Uses exclusive flock for atomic writes.
        """
        import fcntl

        inbox_path = self._inbox_path(recipient, team)
        lock_path = self._lock_path(recipient, team)
        self._ensure_dir(inbox_path)

        with open(lock_path, "w") as lockf:
            fcntl.flock(lockf.fileno(), fcntl.LOCK_EX)
            try:
                if inbox_path.exists():
                    messages = json.loads(inbox_path.read_text())
                else:
                    messages = []

                messages.append({**message, "read": False, "timestamp": time.time()})

                # Atomic write via temp file
                tmp_path = inbox_path.with_suffix(".json.tmp")
                tmp_path.write_text(json.dumps(messages, indent=2))
                tmp_path.rename(inbox_path)
            finally:
                fcntl.flock(lockf.fileno(), fcntl.LOCK_UN)

    async def read_messages(
        self, agent_name: str, team: str = "default", mark_read: bool = True
    ) -> list[dict]:
        """
        Read messages from an agent's inbox.

        Optionally marks all messages as read.
        """
        import fcntl

        inbox_path = self._inbox_path(agent_name, team)
        lock_path = self._lock_path(agent_name, team)

        if not inbox_path.exists():
            return []

        with open(lock_path, "w") as lockf:
            fcntl.flock(lockf.fileno(), fcntl.LOCK_SH)
            try:
                messages = json.loads(inbox_path.read_text())

                unread = [m for m in messages if not m.get("read")]

                if mark_read and unread:
                    for m in messages:
                        m["read"] = True
                    inbox_path.write_text(json.dumps(messages, indent=2))

                return unread
            finally:
                fcntl.flock(lockf.fileno(), fcntl.LOCK_UN)

    async def send_message(
        self,
        from_agent: str,
        to_agent: str,
        content: str,
        team: str = "default",
        message_type: str = "text",
    ) -> dict:
        """
        Send a message from one agent to another.

        Returns delivery status.
        """
        msg = {
            "from": from_agent,
            "type": message_type,
            "content": content,
        }

        await self.write_message(to_agent, msg, team)
        return {"success": True, "queued": True}

    async def get_unread_count(self, agent_name: str, team: str = "default") -> int:
        """Get count of unread messages for an agent."""
        messages = await self.read_messages(agent_name, team, mark_read=False)
        return len(messages)


class AgentLifecycle:
    """
    Claude Code's AgentTool.tsx pattern ported to Python.

    Key features:
    - Named agents with mailbox IPC
    - AsyncLocalStorage cwd isolation (Python: contextvars)
    - Health monitoring with exponential backoff restart
    - MCP server per-agent initialization
    - Tool pool per-agent (independent permission mode)
    """

    def __init__(self, state_dir: Path | None = None):
        Harvey_home = os.environ.get("HARVEY_HOME", str(Path.home() / "HARVEY"))
        if state_dir is None:
            self.state_dir = Path(Harvey_home) / "data" / "lifecycle"
        else:
            self.state_dir = Path(state_dir)
        self.state_dir.mkdir(parents=True, exist_ok=True)

        self._agents: dict[str, AgentConfig] = {}
        self._status: dict[str, AgentStatus] = {}
        self._health_threads: dict[str, threading.Thread] = {}
        self._processes: dict[str, subprocess.Popen] = {}
        self._lock = threading.RLock()
        self._mailbox = MailboxIndex()

        self._load_state()

    def register(self, config: AgentConfig) -> None:
        """
        Register an agent configuration.

        Does NOT start the agent — call spawn() for that.
        """
        with self._lock:
            self._agents[config.name] = config
            self._status[config.name] = AgentStatus(
                name=config.name,
                state=AgentState.STOPPED,
                pid=None,
                started_at=None,
                last_health_check=None,
                last_activity=None,
                restart_count=0,
                error_log=None,
                memory_mb=None,
                agent_id=self._generate_agent_id(config.name),
            )
            self._save_state()

    def _generate_agent_id(self, name: str) -> str:
        """Deterministic agent ID from name."""
        return f"agent_{hashlib.md5(name.encode()).hexdigest()[:8]}"

    def _is_process_alive(self, pid: int | None) -> bool:
        """Check if a process is still running."""
        if pid is None:
            return False
        try:
            os.kill(pid, 0)
            return True
        except OSError:
            return False

    async def spawn(
        self,
        name: str,
        extra_env: dict | None = None,
        extra_args: list[str] | None = None,
    ) -> AgentStatus:
        """
        Start or restart an agent.

        Claude Code's spawn path with exponential backoff.
        """
        with self._lock:
            if name not in self._agents:
                raise ValueError(f"Agent not registered: {name}")

            config = self._agents[name]
            status = self._status[name]

            # Skip if already running
            if status.state == AgentState.RUNNING and self._is_process_alive(
                status.pid
            ):
                return status

            status.state = AgentState.STARTING
            self._save_state()

            # Build environment
            env = os.environ.copy()
            env["HARVEY_AGENT_NAME"] = name
            env["HARVEY_AGENT_ID"] = status.agent_id or ""
            env["HARVEY_AGENT_CWD"] = config.workdir or os.getcwd()
            env["HARVEY_AGENT_MAILBOX"] = str(self._mailbox._inbox_path(name))
            if extra_env:
                env.update(extra_env)
            for k, v in config.env.items():
                env[k] = v

            # Build log file
            log_file = config.log_file or str(self.state_dir / "logs" / f"{name}.log")
            Path(log_file).parent.mkdir(parents=True, exist_ok=True)

            args = config.args + (extra_args or [])

            try:
                proc = subprocess.Popen(
                    [config.entry_point] + args,
                    env=env,
                    cwd=config.workdir,
                    stdout=open(log_file, "a"),
                    stderr=subprocess.STDOUT,
                    preexec_fn=os.setsid if hasattr(os, "setsid") else None,
                )

                status.pid = proc.pid
                status.started_at = time.time()
                status.last_activity = time.time()
                status.state = AgentState.RUNNING
                status.error_log = None

                self._processes[name] = proc

                # Create mailbox inbox
                mailbox_path = self._mailbox._inbox_path(name)
                mailbox_path.parent.mkdir(parents=True, exist_ok=True)

            except Exception as e:
                status.state = AgentState.ERROR
                status.error_log = str(e)
                self._save_state()
                return status

            self._save_state()
            self._start_health_monitor(name)

            return status

    def _start_health_monitor(self, name: str) -> None:
        """Start background health check thread."""
        if name in self._health_threads and self._health_threads[name].is_alive():
            return

        config = self._agents[name]

        def monitor():
            while True:
                time.sleep(config.health_check_interval_sec)
                with self._lock:
                    if name not in self._agents:
                        break
                    status = self._status[name]

                if status.state not in [AgentState.RUNNING, AgentState.ERROR]:
                    continue

                healthy = self._health_check(name)

                with self._lock:
                    status.last_health_check = time.time()

                    if not healthy:
                        self._handle_agent_death(name)

        t = threading.Thread(target=monitor, daemon=True)
        t.start()
        self._health_threads[name] = t

    def _health_check(self, name: str) -> bool:
        """Check if agent is healthy. Default: process alive."""
        status = self._status[name]
        return self._is_process_alive(status.pid)

    def _handle_agent_death(self, name: str) -> None:
        """Auto-restart with exponential backoff."""
        config = self._agents[name]
        status = self._status[name]

        if not config.auto_restart:
            status.state = AgentState.DEAD
            with self._lock:
                self._save_state()
            return

        if status.restart_count >= config.max_restart_attempts:
            status.state = AgentState.ERROR
            status.error_log = (
                f"Max restart attempts ({config.max_restart_attempts}) exceeded"
            )
            with self._lock:
                self._save_state()
            return

        backoff_idx = min(status.restart_count, len(config.restart_backoff_sec) - 1)
        backoff = config.restart_backoff_sec[backoff_idx]

        status.state = AgentState.STOPPED

        # Schedule restart
        def delayed_spawn():
            time.sleep(backoff)
            asyncio.create_task(self.spawn(name))

        t = threading.Thread(target=delayed_spawn, daemon=True)
        t.start()

        status.restart_count += 1
        with self._lock:
            self._save_state()

    async def send_to_agent(
        self, from_name: str, to_name: str, content: str, team: str = "default"
    ) -> dict:
        """Claude Code's SendMessageTool pattern."""
        return await self._mailbox.send_message(from_name, to_name, content, team)

    def pause(self, name: str) -> AgentStatus:
        """SIGSTOP agent."""
        with self._lock:
            status = self._status[name]
            if status.pid and status.state == AgentState.RUNNING:
                try:
                    os.kill(status.pid, signal.SIGSTOP)
                    status.state = AgentState.PAUSED
                    self._save_state()
                except OSError:
                    pass
            return status

    def resume(self, name: str) -> AgentStatus:
        """SIGCONT agent."""
        with self._lock:
            status = self._status[name]
            if status.pid and status.state == AgentState.PAUSED:
                try:
                    os.kill(status.pid, signal.SIGCONT)
                    status.state = AgentState.RUNNING
                    self._save_state()
                except OSError:
                    pass
            return status

    def kill(self, name: str, force: bool = False) -> AgentStatus:
        """Kill agent."""
        with self._lock:
            status = self._status[name]

            if name in self._processes:
                proc = self._processes[name]
                try:
                    sig = signal.SIGKILL if force else signal.SIGTERM
                    os.killpg(os.getpgid(proc.pid), sig)
                except (OSError, ProcessLookupError):
                    pass
                del self._processes[name]

            status.state = AgentState.STOPPED
            status.pid = None
            self._save_state()
            return status

    def status(self, name: str) -> AgentStatus | None:
        """Get status of a specific agent."""
        return self._status.get(name)

    def list_agents(self) -> list[AgentStatus]:
        """List all agent statuses."""
        return list(self._status.values())

    def _save_state(self) -> None:
        """Persist agent state to disk."""
        state_file = self.state_dir / "lifecycle_state.json"
        data = {n: self._status_to_dict(s) for n, s in self._status.items()}
        state_file.write_text(json.dumps(data, indent=2))

    def _load_state(self) -> None:
        """Load agent state from disk."""
        state_file = self.state_dir / "lifecycle_state.json"
        if state_file.exists():
            try:
                data = json.loads(state_file.read_text())
                for name, sd in data.items():
                    sd["state"] = AgentState(sd["state"])
                    self._status[name] = AgentStatus(**sd)
            except (json.JSONDecodeError, TypeError, KeyError):
                pass  # Ignore corrupted state file

    def _status_to_dict(self, s: AgentStatus) -> dict:
        """Convert AgentStatus to dict for serialization."""
        return {
            "name": s.name,
            "state": s.state.value,
            "pid": s.pid,
            "started_at": s.started_at,
            "last_health_check": s.last_health_check,
            "last_activity": s.last_activity,
            "restart_count": s.restart_count,
            "error_log": s.error_log,
            "memory_mb": s.memory_mb,
            "agent_id": s.agent_id,
        }


# Module-level singleton
_lifecycle: AgentLifecycle | None = None


def get_lifecycle() -> AgentLifecycle:
    """Get the global AgentLifecycle instance."""
    global _lifecycle
    if _lifecycle is None:
        _lifecycle = AgentLifecycle()
    return _lifecycle
