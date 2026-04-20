"""
PiSubagent — wraps `pi --rpc` as a Makakoo subagent.

v0.2 Phase B.5 deliverable. Lets `AgentCoordinator` route `code_task`
workflow steps to a pi-driven worker. The actual MCP surface for direct
pi invocation lives at `makakoo-mcp/src/handlers/tier_b/pi.rs` (handlers
pi_run, pi_session_*). This class is the orchestration-layer adapter so
the Python swarm path can pick pi the same way it picks a researcher or
synthesizer.

Actions:
  - run:    single-turn pi execution (equivalent of MCP `pi_run`)
  - fork:   branch a session (equivalent of MCP `pi_session_fork`)
  - steer:  mid-run guidance injection

The subagent shells out to `pi --rpc` directly rather than going through
`makakoo-mcp` because this code runs inside the kernel's Python process;
going through MCP would mean self-RPC.

Gate: `PI_AGENT_ENABLED=1` env var. The AgentCoordinator skips registration
if this isn't set so machines without pi installed don't emit "pi missing"
errors at boot.
"""
from __future__ import annotations

import asyncio
import json
import logging
import os
import shutil
import time
from typing import Any, Dict, List, Optional

from core.subagents.subagent import Subagent

log = logging.getLogger("harvey.subagent.pi")

DEFAULT_TIMEOUT_SECS = 300
MAX_TIMEOUT_SECS = 1800


class PiSubagent(Subagent):
    """Route code-task steps through `pi --rpc`.

    Use by setting `PI_AGENT_ENABLED=1` before AgentCoordinator startup;
    otherwise `register_all_default()` skips this agent. The subagent is
    stateless — every call spawns a fresh pi --rpc subprocess.
    """

    NAME = "pi_coder"
    ACTIONS = ["run", "fork", "steer"]
    DESCRIPTION = (
        "Delegates to pi-mono for code-editing tasks. Spawns `pi --rpc` "
        "per action; see PI_AGENT_ENABLED gate."
    )

    def __init__(self, artifact_store=None, event_bus=None):
        super().__init__(artifact_store=artifact_store, event_bus=event_bus)
        self._pi_binary = shutil.which("pi")

    def available(self) -> bool:
        """True iff pi is on PATH AND PI_AGENT_ENABLED is truthy.

        AgentCoordinator.register_all_default() checks this before
        registering — the agent silently opts out if either is false.
        """
        return bool(self._pi_binary) and os.environ.get("PI_AGENT_ENABLED") in (
            "1", "true", "yes",
        )

    def execute(self, step, ctx: Dict) -> Dict[str, Any]:
        if not self._pi_binary:
            return {
                "ok": False,
                "error": "pi binary not on PATH; install pi-mono first",
            }
        action = step.action
        prompt = ctx.get("prompt") or ctx.get("query") or ""
        session_id = ctx.get("session_id")
        timeout_s = min(
            int(ctx.get("timeout_s", DEFAULT_TIMEOUT_SECS)), MAX_TIMEOUT_SECS
        )
        model = ctx.get("model")

        if action == "run":
            if not prompt:
                return {"ok": False, "error": "no prompt provided for pi_coder.run"}
            req = {
                "id": f"subagent-{int(time.time_ns())}",
                "type": "prompt",
                "message": prompt,
            }
            if session_id:
                req["session_id"] = session_id
            if model:
                req["model"] = model
            return self._rpc_turn(req, timeout_s)

        if action == "fork":
            from_msg_id = ctx.get("from_msg_id")
            if not session_id or not from_msg_id:
                return {
                    "ok": False,
                    "error": "fork requires session_id + from_msg_id",
                }
            req = {
                "id": f"subagent-fork-{int(time.time_ns())}",
                "type": "fork",
                "session_id": session_id,
                "from_msg_id": from_msg_id,
            }
            return self._rpc_turn(req, timeout_s)

        if action == "steer":
            message = ctx.get("message") or prompt
            if not session_id or not message:
                return {
                    "ok": False,
                    "error": "steer requires session_id + message",
                }
            req = {
                "id": f"subagent-steer-{int(time.time_ns())}",
                "type": "steer",
                "session_id": session_id,
                "message": message,
            }
            return self._rpc_turn(req, timeout_s)

        return {"ok": False, "error": f"unknown action {action!r}"}

    # ─── Subprocess plumbing ─────────────────────────────────────────

    def _rpc_turn(self, request: Dict[str, Any], timeout_s: int) -> Dict[str, Any]:
        """Spawn pi --rpc, pipe request, drain stdout, return parsed result."""
        try:
            return asyncio.get_event_loop().run_until_complete(
                self._rpc_turn_async(request, timeout_s)
            )
        except RuntimeError:
            # No running loop — create a temp one.
            return asyncio.run(self._rpc_turn_async(request, timeout_s))

    async def _rpc_turn_async(
        self, request: Dict[str, Any], timeout_s: int
    ) -> Dict[str, Any]:
        proc = await asyncio.create_subprocess_exec(
            self._pi_binary,
            "--rpc",
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        try:
            body = (json.dumps(request) + "\n").encode("utf-8")
            try:
                await asyncio.wait_for(
                    self._drive_once(proc, body), timeout=timeout_s
                )
            except asyncio.TimeoutError:
                proc.kill()
                await proc.wait()
                return {
                    "ok": False,
                    "error": f"pi --rpc timed out after {timeout_s}s",
                }
            frames = self._decode_frames(proc)
            return self._summarize(frames)
        finally:
            if proc.returncode is None:
                proc.kill()
                await proc.wait()

    async def _drive_once(
        self, proc: asyncio.subprocess.Process, body: bytes
    ) -> None:
        if proc.stdin is not None:
            proc.stdin.write(body)
            await proc.stdin.drain()
            proc.stdin.close()
        self._captured_stdout = await proc.stdout.read() if proc.stdout else b""
        self._captured_stderr = await proc.stderr.read() if proc.stderr else b""
        await proc.wait()

    def _decode_frames(self, proc: asyncio.subprocess.Process) -> List[Dict[str, Any]]:
        frames: List[Dict[str, Any]] = []
        for line in (self._captured_stdout or b"").splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                frames.append(json.loads(line))
            except json.JSONDecodeError as e:
                log.debug("[pi_subagent] non-JSON stdout line: %s (%s)", line, e)
        if proc.returncode and proc.returncode != 0:
            log.debug(
                "[pi_subagent] pi exited %s stderr=%r",
                proc.returncode,
                self._captured_stderr,
            )
        return frames

    def _summarize(self, frames: List[Dict[str, Any]]) -> Dict[str, Any]:
        """Pick out the assistant text + usage for the caller."""
        text: Optional[str] = None
        usage: Optional[Dict[str, Any]] = None
        for frame in reversed(frames):
            if text is None and frame.get("type") == "event":
                event = frame.get("event") or {}
                if (
                    event.get("type") == "message_end"
                    and event.get("role") == "assistant"
                ):
                    text = event.get("text") or (
                        (event.get("message") or {}).get("content")
                    )
            if usage is None:
                if isinstance(frame.get("usage"), dict):
                    usage = frame["usage"]
                elif isinstance((frame.get("event") or {}).get("usage"), dict):
                    usage = frame["event"]["usage"]
            if text and usage:
                break
        return {
            "ok": True,
            "text": text or "",
            "usage": usage or {},
            "frames": len(frames),
        }
