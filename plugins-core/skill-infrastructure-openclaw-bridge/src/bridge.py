#!/usr/bin/env python3
"""
OpenClaw Bridge — Harvey OS Delegation Layer

Usage:
    python3 bridge.py status
    python3 bridge.py delegate --prompt "..."
    python3 bridge.py sessions list
    python3 bridge.py sessions kill --session-id <id>
    python3 bridge.py sessions reset
    python3 bridge.py capabilities
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import uuid
from dataclasses import dataclass, field, asdict
from datetime import datetime, timedelta
from pathlib import Path
from typing import Optional

BASE_DIR = Path.home() / "HARVEY" / "data" / "openclaw-bridge"
STATE_FILE = BASE_DIR / "state.json"
SESSIONS_DIR = BASE_DIR / "sessions"
LOGS_DIR = BASE_DIR / "logs"
CAPABILITIES_FILE = BASE_DIR / "capabilities.json"


# ──────────────────────────────────────────────────────────────
# Data Models
# ──────────────────────────────────────────────────────────────

@dataclass
class DelegationResult:
    task: str
    session_id: str
    trigger: str
    routing_reason: str
    duration_ms: int
    result_type: str  # text | error | artifact
    text: str
    error: Optional[str] = None
    tool_calls: list[str] = field(default_factory=list)

    def to_log_entry(self) -> dict:
        return {
            "timestamp": datetime.utcnow().isoformat() + "Z",
            "task": self.task,
            "session_id": self.session_id,
            "trigger": self.trigger,
            "routing_reason": self.routing_reason,
            "duration_ms": self.duration_ms,
            "result_type": self.result_type,
            "text": self.text[:500],  # Truncate for log
            "error": self.error,
            "tool_calls": self.tool_calls,
        }


@dataclass
class BridgeState:
    session_id: str = ""
    last_used: str = ""  # ISO8601
    openclaw_path: str = "openclaw"
    default_thinking: str = "medium"
    default_timeout: int = 300
    session_idle_ttl: int = 1800  # 30 minutes
    max_retries: int = 2
    gateway_url: str = "http://localhost:8080"
    preferred_mode: str = "cli"  # cli | mcp | gateway
    capabilities_cached: bool = False

    @classmethod
    def load(cls) -> "BridgeState":
        if STATE_FILE.exists():
            data = json.loads(STATE_FILE.read_text())
            return cls(**{k: v for k, v in data.items() if hasattr(cls, k)})
        return cls()

    def save(self) -> None:
        BASE_DIR.mkdir(parents=True, exist_ok=True)
        STATE_FILE.write_text(json.dumps(asdict(self), indent=2))


# ──────────────────────────────────────────────────────────────
# OpenClaw CLI Executor
# ──────────────────────────────────────────────────────────────

class OpenClawNotFoundError(Exception):
    """OpenClaw CLI not found in PATH."""
    pass


class OpenClawNotConfiguredError(Exception):
    """OpenClaw is installed but not configured."""
    pass


class OpenClawExecutor:
    """Executes OpenClaw agent commands via CLI subprocess."""

    def __init__(self, openclaw_path: str = "openclaw", timeout: int = 300):
        self.openclaw_path = openclaw_path
        self.timeout = timeout

    def _check_installed(self) -> str:
        """Verify OpenClaw is installed and return version."""
        try:
            result = subprocess.run(
                [self.openclaw_path, "--version"],
                capture_output=True, text=True, timeout=10
            )
            if result.returncode != 0:
                raise OpenClawNotConfiguredError(result.stderr.strip())
            return result.stdout.strip()
        except FileNotFoundError:
            raise OpenClawNotFoundError(
                f"OpenClaw not found at '{self.openclaw_path}'. "
                "Install: curl -s https://openclaw.ai/install.sh | sh"
            )

    def status(self) -> dict:
        """Run openclaw status --json --deep and return parsed output."""
        self._check_installed()
        try:
            result = subprocess.run(
                [self.openclaw_path, "status", "--json", "--deep"],
                capture_output=True, text=True, timeout=30
            )
            if result.returncode != 0:
                return {"error": result.stderr.strip()}
            return json.loads(result.stdout)
        except json.JSONDecodeError:
            return {"raw": result.stdout}

    def list_agents(self) -> list[dict]:
        """List configured OpenClaw agents."""
        self._check_installed()
        try:
            result = subprocess.run(
                [self.openclaw_path, "agents", "list", "--json"],
                capture_output=True, text=True, timeout=15
            )
            if result.returncode != 0:
                return []
            data = json.loads(result.stdout)
            return data if isinstance(data, list) else []
        except (json.JSONDecodeError, FileNotFoundError):
            return []

    def execute(
        self,
        prompt: str,
        session_id: Optional[str] = None,
        agent_name: Optional[str] = None,
        thinking: str = "medium",
        timeout: Optional[int] = None,
    ) -> DelegationResult:
        """
        Execute a delegation to OpenClaw agent.

        Returns DelegationResult with response text or error.
        """
        start = datetime.utcnow()
        timeout = timeout or self.timeout

        # Build command
        cmd = [
            self.openclaw_path, "agent",
            "--message", prompt,
            "--thinking", thinking,
        ]
        if session_id:
            cmd.extend(["--session-id", session_id])
        if agent_name:
            cmd.extend(["--agent", agent_name])

        try:
            result = subprocess.run(
                cmd,
                capture_output=True, text=True, timeout=timeout
            )
            duration_ms = int((datetime.utcnow() - start).total_seconds() * 1000)

            if result.returncode != 0:
                return DelegationResult(
                    task=prompt,
                    session_id=session_id or "",
                    trigger="unknown",
                    routing_reason="subprocess_error",
                    duration_ms=duration_ms,
                    result_type="error",
                    text="",
                    error=result.stderr.strip() or f"Exit code {result.returncode}",
                )

            return DelegationResult(
                task=prompt,
                session_id=session_id or "",
                trigger="unknown",
                routing_reason="success",
                duration_ms=duration_ms,
                result_type="text",
                text=result.stdout.strip(),
            )

        except subprocess.TimeoutExpired:
            duration_ms = int((datetime.utcnow() - start).total_seconds() * 1000)
            return DelegationResult(
                task=prompt,
                session_id=session_id or "",
                trigger="unknown",
                routing_reason="timeout",
                duration_ms=duration_ms,
                result_type="error",
                text="",
                error=f"Timeout after {timeout}s",
            )

    def discover_capabilities(self) -> dict:
        """Discover OpenClaw capabilities by probing various subcommands."""
        caps = {
            "version": None,
            "agents_available": [],
            "channels": [],
            "browser_available": False,
            "mcp_available": False,
            "gateway_available": False,
            "skills_installed": [],
            "gateway_rpc": False,
        }

        try:
            caps["version"] = self._check_installed()
        except OpenClawNotFoundError:
            caps["error"] = "not_installed"
            return caps

        # Probe agents
        agents = self.list_agents()
        caps["agents_available"] = [a.get("name", a.get("id", "?")) for a in agents]

        # Probe channels
        for channel in ["discord", "telegram", "slack", "whatsapp", "signal"]:
            try:
                r = subprocess.run(
                    [self.openclaw_path, "channels", "status", channel, "--json"],
                    capture_output=True, text=True, timeout=5
                )
                if r.returncode == 0:
                    caps["channels"].append(channel)
            except Exception:
                pass

        # Probe browser
        try:
            r = subprocess.run(
                [self.openclaw_path, "browser", "--version"],
                capture_output=True, text=True, timeout=5
            )
            caps["browser_available"] = r.returncode == 0
        except Exception:
            pass

        # Probe MCP
        try:
            r = subprocess.run(
                [self.openclaw_path, "mcp", "--help"],
                capture_output=True, text=True, timeout=5
            )
            caps["mcp_available"] = r.returncode == 0 and "mcp" in r.stdout.lower()
        except Exception:
            pass

        # Probe gateway
        try:
            r = subprocess.run(
                [self.openclaw_path, "gateway", "status", "--json"],
                capture_output=True, text=True, timeout=5
            )
            caps["gateway_available"] = r.returncode == 0
            if r.returncode == 0:
                try:
                    caps["gateway_rpc"] = "rpc" in json.loads(r.stdout).get("mode", "").lower()
                except Exception:
                    caps["gateway_rpc"] = True  # Assumed if status succeeds
        except Exception:
            pass

        # Probe skills
        try:
            r = subprocess.run(
                [self.openclaw_path, "skills", "list", "--json"],
                capture_output=True, text=True, timeout=10
            )
            if r.returncode == 0:
                data = json.loads(r.stdout)
                caps["skills_installed"] = [
                    s.get("name", "?" ) for s in (data if isinstance(data, list) else [])
                ]
        except Exception:
            pass

        return caps


# ──────────────────────────────────────────────────────────────
# Session Manager
# ──────────────────────────────────────────────────────────────

class SessionManager:
    """Manages OpenClaw session lifecycle."""

    def __init__(self, sessions_dir: Path = SESSIONS_DIR):
        self.sessions_dir = sessions_dir
        self.sessions_dir.mkdir(parents=True, exist_ok=True)

    def create_session(self, context: str = "") -> str:
        """Create a new session, return session ID."""
        session_id = f"oc-sess-{uuid.uuid4().hex[:12]}"
        session_file = self.sessions_dir / f"{session_id}.json"
        session_file.write_text(json.dumps({
            "id": session_id,
            "created_at": datetime.utcnow().isoformat() + "Z",
            "context": context,
            "turns": [],
        }, indent=2))
        return session_id

    def get_session(self, session_id: str) -> Optional[dict]:
        """Load session data."""
        session_file = self.sessions_dir / f"{session_id}.json"
        if not session_file.exists():
            return None
        return json.loads(session_file.read_text())

    def append_turn(self, session_id: str, role: str, content: str) -> None:
        """Append a turn to session history."""
        session = self.get_session(session_id)
        if not session:
            return
        session["turns"].append({
            "role": role,
            "content": content,
            "timestamp": datetime.utcnow().isoformat() + "Z",
        })
        session_file = self.sessions_dir / f"{session_id}.json"
        session_file.write_text(json.dumps(session, indent=2))

    def list_sessions(self) -> list[dict]:
        """List all sessions."""
        sessions = []
        for f in self.sessions_dir.glob("*.json"):
            try:
                data = json.loads(f.read_text())
                sessions.append(data)
            except Exception:
                pass
        return sorted(sessions, key=lambda s: s.get("created_at", ""), reverse=True)

    def kill_session(self, session_id: str) -> bool:
        """Delete a session."""
        session_file = self.sessions_dir / f"{session_id}.json"
        if session_file.exists():
            session_file.unlink()
            return True
        return False

    def reset_all(self) -> int:
        """Delete all sessions. Returns count deleted."""
        count = 0
        for f in self.sessions_dir.glob("*.json"):
            f.unlink()
            count += 1
        return count


# ──────────────────────────────────────────────────────────────
# Result Synthesizer
# ──────────────────────────────────────────────────────────────

class ResultSynthesizer:
    """Synthesizes OpenClaw output into Harvey's response format."""

    @staticmethod
    def synthesize(result: DelegationResult) -> str:
        """Convert DelegationResult into natural language summary for Harvey."""
        if result.result_type == "error":
            return f"OpenClaw returned an error: {result.error}"

        text = result.text.strip()
        if not text:
            return "OpenClaw returned an empty response."

        # If response is short, return it directly
        if len(text) < 200:
            return text

        # Summarize long responses
        lines = text.split("\n")
        if len(lines) > 20:
            summary = f"OpenClaw's response ({len(lines)} lines, {len(text)} chars):\n\n"
            summary += "\n".join(lines[:10])
            summary += f"\n\n... [{len(lines) - 10} more lines]"
            return summary

        return text


# ──────────────────────────────────────────────────────────────
# Audit Logger
# ──────────────────────────────────────────────────────────────

class AuditLogger:
    """Logs delegations to file and optionally to Brain."""

    def __init__(self, logs_dir: Path = LOGS_DIR):
        self.logs_dir = logs_dir
        self.logs_dir.mkdir(parents=True, exist_ok=True)

    def log(self, entry: DelegationResult) -> None:
        """Write delegation to today's log file (JSONL)."""
        today = datetime.utcnow().strftime("%Y_%m_%d")
        log_file = self.logs_dir / f"delegation_{today}.jsonl"
        log_file.open("a").write(json.dumps(entry.to_log_entry()) + "\n")

    def log_to_brain(self, entry: DelegationResult) -> None:
        """Append delegation summary to today's Brain journal."""
        try:
            from logseq_bridge import LogseqBrain
            brain = LogseqBrain()
            today = datetime.utcnow().strftime("%Y_%m_%d")
            brain.journal(
                today,
                f"OpenClaw delegation: {entry.task[:100]} — "
                f"{entry.result_type} in {entry.duration_ms}ms"
            )
        except Exception as e:
            # Don't fail if Brain logging fails
            print(f"[WARN] Could not log to Brain: {e}", file=sys.stderr)


# ──────────────────────────────────────────────────────────────
# Main Bridge
# ──────────────────────────────────────────────────────────────

class OpenClawBridge:
    """Main bridge orchestrator."""

    def __init__(self, state: Optional[BridgeState] = None):
        self.state = state or BridgeState.load()
        self.executor = OpenClawExecutor(
            openclaw_path=self.state.openclaw_path,
            timeout=self.state.default_timeout,
        )
        self.session_manager = SessionManager()
        self.synthesizer = ResultSynthesizer()
        self.audit = AuditLogger()

    # ── Public API ──────────────────────────────────────────────

    def delegate(
        self,
        prompt: str,
        trigger: str = "explicit",
        routing_reason: str = "",
        agent_name: Optional[str] = None,
        thinking: str = "medium",
        timeout: Optional[int] = None,
        log_to_brain: bool = True,
    ) -> DelegationResult:
        """
        Route a task to OpenClaw.

        Returns DelegationResult with synthesized response.
        """
        # Get or create session
        session_id = self._get_active_session()

        # Execute
        result = self.executor.execute(
            prompt=prompt,
            session_id=session_id,
            agent_name=agent_name,
            thinking=thinking,
            timeout=timeout,
        )
        result.trigger = trigger
        result.routing_reason = routing_reason or trigger

        # Persist session
        if session_id:
            self.session_manager.append_turn(session_id, "user", prompt)
            self.session_manager.append_turn(session_id, "assistant", result.text)
            self.state.session_id = session_id
            self.state.last_used = datetime.utcnow().isoformat() + "Z"
            self.state.save()

        # Audit
        self.audit.log(result)
        if log_to_brain:
            self.audit.log_to_brain(result)

        return result

    def status(self) -> dict:
        """Return bridge + OpenClaw status."""
        oc_status = self.executor.status()
        oc_caps = self.executor.discover_capabilities()
        return {
            "bridge": {
                "state_file": str(STATE_FILE),
                "active_session": self.state.session_id,
                "preferred_mode": self.state.preferred_mode,
            },
            "openclaw": oc_status,
            "capabilities": oc_caps,
        }

    def capabilities(self) -> dict:
        """Return and cache OpenClaw capabilities."""
        caps = self.executor.discover_capabilities()
        CAPABILITIES_FILE.write_text(json.dumps(caps, indent=2))
        self.state.capabilities_cached = True
        self.state.save()
        return caps

    def sessions_list(self) -> list[dict]:
        """List all sessions."""
        return self.session_manager.list_sessions()

    def sessions_kill(self, session_id: str) -> bool:
        """Kill a session."""
        return self.session_manager.kill_session(session_id)

    def sessions_reset(self) -> int:
        """Reset all sessions."""
        count = self.session_manager.reset_all()
        self.state.session_id = ""
        self.state.save()
        return count

    # ── Internal ────────────────────────────────────────────────

    def _get_active_session(self) -> Optional[str]:
        """Get current session, checking TTL."""
        if not self.state.session_id:
            return None

        session = self.session_manager.get_session(self.state.session_id)
        if not session:
            return None

        # Check TTL
        last_used = datetime.fromisoformat(self.state.last_used.replace("Z", "+00:00"))
        ttl = timedelta(seconds=self.state.session_idle_ttl)
        if datetime.utcnow().replace(tzinfo=last_used.tzinfo) - last_used > ttl:
            # Session expired, create new
            return self.session_manager.create_session()

        return self.state.session_id


# ──────────────────────────────────────────────────────────────
# CLI Entry Point
# ──────────────────────────────────────────────────────────────

def main() -> int:
    parser = argparse.ArgumentParser(description="OpenClaw Bridge — Harvey OS Delegation Layer")
    sub = parser.add_subparsers(dest="command")

    # status
    sub.add_parser("status", help="Check OpenClaw and bridge status")

    # delegate
    delegate_parser = sub.add_parser("delegate", help="Delegate a task to OpenClaw")
    delegate_parser.add_argument("--prompt", required=True, help="Task prompt")
    delegate_parser.add_argument("--agent", help="OpenClaw agent name")
    delegate_parser.add_argument("--thinking", default="medium", choices=["low", "medium", "high"])
    delegate_parser.add_argument("--timeout", type=int, default=300)
    delegate_parser.add_argument("--trigger", default="explicit")
    delegate_parser.add_argument("--routing-reason", default="")

    # sessions
    sessions_parser = sub.add_parser("sessions", help="Manage OpenClaw sessions")
    sessions_sub = sessions_parser.add_subparsers(dest="sessions_command")
    sessions_sub.add_parser("list", help="List all sessions")
    kill_parser = sessions_sub.add_parser("kill", help="Kill a session")
    kill_parser.add_argument("--session-id", required=True)
    sessions_sub.add_parser("reset", help="Reset all sessions")

    # capabilities
    sub.add_parser("capabilities", help="Discover and cache OpenClaw capabilities")

    args = parser.parse_args()

    if not args.command:
        parser.print_help()
        return 0

    bridge = OpenClawBridge()

    if args.command == "status":
        status = bridge.status()
        print(json.dumps(status, indent=2, default=str))
        return 0

    if args.command == "delegate":
        result = bridge.delegate(
            prompt=args.prompt,
            agent_name=args.agent,
            thinking=args.thinking,
            timeout=args.timeout,
            trigger=args.trigger,
            routing_reason=args.routing_reason,
        )
        print(f"[OpenClaw] Session: {result.session_id}")
        print(f"[OpenClaw] Duration: {result.duration_ms}ms")
        print(f"[OpenClaw] Type: {result.result_type}")
        if result.error:
            print(f"[ERROR] {result.error}")
        else:
            print(f"[RESULT]\n{bridge.synthesizer.synthesize(result)}")
        return 0

    if args.command == "sessions":
        if args.sessions_command == "list":
            sessions = bridge.sessions_list()
            print(json.dumps(sessions, indent=2, default=str))
        elif args.sessions_command == "kill":
            ok = bridge.sessions_kill(args.session_id)
            print(f"Killed: {ok}")
        elif args.sessions_command == "reset":
            count = bridge.sessions_reset()
            print(f"Reset {count} sessions")
        return 0

    if args.command == "capabilities":
        caps = bridge.capabilities()
        print(json.dumps(caps, indent=2, default=str))
        return 0

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
