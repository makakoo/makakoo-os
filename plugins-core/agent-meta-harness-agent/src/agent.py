#!/usr/bin/env python3
"""
Meta Harness Agent — Behavioral Skill Compliance Evaluator

Based on Stanford IRIS Lab's Meta-Harness (Terminal-Bench 2.0: 76.4% on Claude Opus 4.6).
Adapted for Harvey OS — uses tmux + OpenAI SDK (switchAILocal at localhost:18080).

3 native tools: execute_commands, task_complete, image_read.
Environment bootstrapping: snapshots sandbox before the agent loop starts.

Usage:
    from agent import AgentHarness
    harness = AgentHarness(model="minimax:MiniMax-M2.7")
    result = await harness.run(instruction="...", env=SkillEnv(...))
"""

from __future__ import annotations

import asyncio
import base64
import json
import logging
import re
import subprocess
import tempfile
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from openai import OpenAI

from config import (
    AI_KEY,
    AI_MODEL,
    AI_URL,
    AGENT_MAX_TURNS,
    AGENT_TIMEOUT_SEC,
    STATE_DIR,
)

_client: OpenAI | None = None


def _get_client() -> OpenAI:
    global _client
    if _client is None:
        _client = OpenAI(base_url=AI_URL, api_key=AI_KEY)
    return _client


logger = logging.getLogger("meta-harness")

BLOCK_TIMEOUT_SEC = 600
MARKER_PREFIX = "__MHCMD__"


@dataclass
class Command:
    keystrokes: str
    duration_sec: float = 1.0


@dataclass
class ImageReadRequest:
    file_path: str
    image_read_instruction: str


@dataclass
class ToolResult:
    content: str | None = None
    tool_calls: list[dict[str, Any]] = field(default_factory=list)
    reasoning_content: str | None = None
    usage: dict | None = None


_EXECUTE_COMMANDS_DESC = "Execute commands in the terminal with your analysis and plan."
_ANALYSIS_DESC = "Analyze the current state. What has been accomplished? What still needs to be done?"
_PLAN_DESC = "Describe your plan for the next steps. Be specific about what each command should accomplish."
_COMMANDS_DESC = (
    "The commands array can be empty if you want to wait without taking action."
)
_KEYSTROKES_DESC = (
    "Exact keystrokes to send to the terminal. "
    "Most bash commands should end with newline (\\n). "
    "For Ctrl+C use C-c, Ctrl+D use C-d. "
    "Sent exactly as written — no extra whitespace."
)
_DURATION_DESC = (
    "Seconds to wait for command completion (default: 1.0). "
    "On fast commands (cd, ls, echo): 0.1s. "
    "On compilers (gcc, rustc): 1.0s. "
    "On slow commands (make, wget): set longer. "
    "Never wait > 60s — poll for intermediate results instead."
)
_TASK_COMPLETE_DESC = "Call this when the task is complete."
_IMAGE_READ_DESC = (
    "Read and analyze an image file. "
    "Do NOT use for text files — use cat/head instead. "
    "Image sent to model for visual analysis."
)
_FILE_PATH_DESC = (
    "Absolute path to the image file. Supported: PNG, JPG, JPEG, GIF, WEBP."
)
_IMAGE_READ_INSTRUCTION_DESC = "What to extract from the image. Be specific."


TOOLS = [
    {
        "type": "function",
        "function": {
            "name": "execute_commands",
            "description": _EXECUTE_COMMANDS_DESC,
            "parameters": {
                "type": "object",
                "properties": {
                    "analysis": {"type": "string", "description": _ANALYSIS_DESC},
                    "plan": {"type": "string", "description": _PLAN_DESC},
                    "commands": {
                        "type": "array",
                        "description": _COMMANDS_DESC,
                        "items": {
                            "type": "object",
                            "properties": {
                                "keystrokes": {
                                    "type": "string",
                                    "description": _KEYSTROKES_DESC,
                                },
                                "duration": {
                                    "type": "number",
                                    "description": _DURATION_DESC,
                                },
                            },
                            "required": ["keystrokes"],
                        },
                    },
                },
                "required": ["analysis", "plan", "commands"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "task_complete",
            "description": _TASK_COMPLETE_DESC,
            "parameters": {"type": "object", "properties": {}, "required": []},
        },
    },
    {
        "type": "function",
        "function": {
            "name": "image_read",
            "description": _IMAGE_READ_DESC,
            "parameters": {
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": _FILE_PATH_DESC},
                    "image_read_instruction": {
                        "type": "string",
                        "description": _IMAGE_READ_INSTRUCTION_DESC,
                    },
                },
                "required": ["file_path", "image_read_instruction"],
            },
        },
    },
]


class TmuxSession:
    """Manages a tmux session for command execution."""

    def __init__(self, session_name: str):
        self.session_name = session_name
        self._proc: subprocess.Popen | None = None
        self._started = False

    def start(self) -> None:
        if self._started:
            return
        subprocess.run(
            ["tmux", "kill-session", "-t", self.session_name], capture_output=True
        )
        subprocess.run(
            ["tmux", "new-session", "-d", "-s", self.session_name],
            capture_output=True,
        )
        self._started = True

    def send_keys(self, keystrokes: str) -> None:
        subprocess.run(
            ["tmux", "send-keys", "-t", f"{self.session_name}:0", keystrokes, "Enter"],
            capture_output=True,
        )

    def capture_pane(self) -> str:
        result = subprocess.run(
            ["tmux", "capture-pane", "-t", f"{self.session_name}:0", "-p"],
            capture_output=True,
            text=True,
        )
        return result.stdout

    def exec(self, command: str, timeout_sec: int = 15) -> subprocess.CompletedProcess:
        script = f'tmux send-keys -t "{self.session_name}:0" "{command}" Enter && sleep {timeout_sec} && tmux capture-pane -t "{self.session_name}:0" -p'
        return subprocess.run(
            script, shell=True, capture_output=True, text=True, timeout=timeout_sec + 5
        )

    def is_alive(self) -> bool:
        result = subprocess.run(
            ["tmux", "has-session", "-t", self.session_name],
            capture_output=True,
        )
        return result.returncode == 0

    def kill(self) -> None:
        if self._started:
            subprocess.run(
                ["tmux", "kill-session", "-t", self.session_name], capture_output=True
            )
            self._started = False


class AgentHarness:
    """
    Lightweight agent harness using tmux + litellm.

    Based on Stanford IRIS Lab's Meta-Harness agent. Does NOT depend on harbor framework.
    Instead, uses tmux for terminal command execution and litellm for LLM calls.

    Key innovation (from Meta-Harness):
    - Environment bootstrapping: gather sandbox snapshot before the agent loop
      and inject it into the first prompt. Saves 2-5 early exploration turns.
    """

    def __init__(
        self,
        model: str = AI_MODEL,
        api_base: str = AI_URL,
        api_key: str = AI_KEY,
        max_turns: int = AGENT_MAX_TURNS,
        temperature: float = 0.3,
        logging_dir: Path | None = None,
    ):
        self.model = model
        self.api_base = api_base
        self.api_key = api_key
        self.max_turns = max_turns
        self.temperature = temperature
        self.logging_dir = logging_dir or STATE_DIR
        self._marker_seq = 0
        self._total_time_saved = 0.0
        self._pending_completion = False
        self._session: TmuxSession | None = None
        self._original_instruction = ""

    def version(self) -> str:
        return "1.0.0-harvey"

    async def run(self, instruction: str, env: SkillEnv) -> dict[str, Any]:
        """
        Run the agent against the given skill environment.

        Returns a dict with:
            success: bool
            turns: int
            completion_message: str
            trajectory: list of step dicts
        """
        self._original_instruction = instruction
        self._session = TmuxSession(env.session_name)
        self._session.start()

        if self.logging_dir:
            self.logging_dir.mkdir(parents=True, exist_ok=True)

        snapshot = await self._gather_env_snapshot(env)
        if snapshot:
            instruction = f"{instruction}\n\n{snapshot}"

        messages = [{"role": "user", "content": instruction}]
        trajectory = []
        start_time = time.time()
        episode = 0

        for episode in range(self.max_turns):
            if not self._session.is_alive():
                break

            episode_result = await self._run_episode(
                episode, messages, env, instruction
            )

            messages = episode_result["messages"]
            trajectory.append(episode_result["step"])

            if episode_result.get("is_complete"):
                break

        elapsed = time.time() - start_time

        self._session.kill()

        return {
            "success": self._pending_completion,
            "turns": episode + 1,
            "elapsed_sec": round(elapsed, 1),
            "time_saved_sec": round(self._total_time_saved, 1),
            "trajectory": trajectory,
            "completion_message": (
                "Task completed."
                if self._pending_completion
                else f"Max turns ({self.max_turns}) reached without completion."
            ),
        }

    async def _gather_env_snapshot(self, env: SkillEnv) -> str:
        """Gather compact environment snapshot to eliminate early exploration turns."""
        if self._session is None:
            return ""

        bootstrap_cmd = (
            "echo '@@PWD@@' && pwd && "
            "echo '@@LS@@' && ls -la /app/ 2>/dev/null && "
            "echo '@@LANG@@' && "
            "(python3 --version 2>&1 || echo 'python3: not found') && "
            "(gcc --version 2>&1 | head -1 || echo 'gcc: not found') && "
            "(node --version 2>&1 || echo 'node: not found') && "
            "(java -version 2>&1 | head -1 || echo 'java: not found') && "
            "(go version 2>&1 || echo 'go: not found') && "
            "echo '@@PKG@@' && "
            "(pip3 --version 2>&1 || echo 'pip3: not found') && "
            "(apt-get --version 2>&1 | head -1 || echo 'apt-get: not found') && "
            "echo '@@MEM@@' && free -h 2>/dev/null | head -2 || true"
        )

        try:
            result = await asyncio.wait_for(
                asyncio.to_thread(self._session.exec, bootstrap_cmd, 15),
                timeout=20,
            )
            stdout = result.stdout.strip()
        except Exception:
            return ""

        if not stdout:
            return ""

        sections = {}
        current_key = None
        current_lines: list[str] = []
        for line in stdout.split("\n"):
            if line.startswith("@@") and line.endswith("@@"):
                if current_key:
                    sections[current_key] = "\n".join(current_lines)
                current_key = line.strip("@")
                current_lines = []
            else:
                current_lines.append(line)
        if current_key:
            sections[current_key] = "\n".join(current_lines)

        parts = []
        if "PWD" in sections:
            parts.append(f"Working directory: {sections['PWD'].strip()}")
        if "LS" in sections:
            ls_lines = sections["LS"].strip().split("\n")
            if len(ls_lines) <= 1 or (len(ls_lines) == 2 and "total 0" in ls_lines[0]):
                parts.append("/app contents: (empty directory)")
            elif len(ls_lines) > 25:
                parts.append(
                    f"/app contents ({len(ls_lines)} entries):\n"
                    + "\n".join(ls_lines[:20])
                    + f"\n... ({len(ls_lines) - 20} more)"
                )
            else:
                parts.append(f"/app contents:\n{sections['LS'].strip()}")
        if "LANG" in sections:
            langs = [
                l.strip() for l in sections["LANG"].strip().split("\n") if l.strip()
            ]
            parts.append("Available: " + "; ".join(langs))
        if "PKG" in sections:
            pkgs = [l.strip() for l in sections["PKG"].strip().split("\n") if l.strip()]
            parts.append("Package managers: " + "; ".join(pkgs))
        if "MEM" in sections and sections["MEM"].strip():
            parts.append(f"Memory: {sections['MEM'].strip()}")

        if not parts:
            return ""

        return "[Environment Snapshot]\n" + "\n".join(parts)

    async def _run_episode(
        self,
        episode: int,
        messages: list[dict],
        env: SkillEnv,
        original_instruction: str,
    ) -> dict[str, Any]:
        prompt = self._build_prompt(messages, env)
        logging_dir = self.logging_dir

        if logging_dir:
            prompt_path = logging_dir / f"episode_{episode:03d}_prompt.txt"
            prompt_path.write_text(prompt)

        tool_response = await self._call_llm(messages, prompt)

        content = tool_response.get("content", "") or ""
        tool_calls = tool_response.get("tool_calls", [])

        if tool_response.get("usage"):
            usage = tool_response.get("usage", {})

        messages.append({"role": "user", "content": prompt})
        messages.append(
            {
                "role": "assistant",
                "content": content,
                "tool_calls": tool_calls if tool_calls else None,
            }
        )

        if tool_calls:
            for tc in tool_calls:
                messages.append(
                    {
                        "role": "tool",
                        "tool_call_id": tc.get("id", ""),
                        "content": "executed",
                    }
                )

        commands, is_complete, feedback, analysis, plan, image_req = (
            self._parse_tool_calls(tool_calls)
        )

        step = {
            "episode": episode,
            "analysis": analysis,
            "plan": plan,
            "commands": [c.keystrokes for c in commands],
            "is_complete": is_complete,
            "feedback": feedback,
        }

        if image_req:
            image_result = await self._execute_image_read(image_req, env)
            step["image_read"] = image_req.file_path
            step["image_result"] = image_result[:500]
            messages.append({"role": "user", "content": image_result})
        elif commands:
            exec_result = await self._execute_commands(commands, env)
            step["execution_output"] = exec_result[:300]
            messages.append({"role": "user", "content": exec_result})
        elif is_complete:
            messages.append(
                {"role": "user", "content": "Please confirm task completion."}
            )

        return {
            "messages": messages,
            "step": step,
            "is_complete": is_complete and self._pending_completion,
        }

    def _build_prompt(self, messages: list[dict], env: SkillEnv) -> str:
        skill_context = ""
        if env.skill_content:
            skill_context = (
                f"\n\n[SUPERIMPORTANT: You must follow this skill guide]\n"
                f"{env.skill_content}\n"
                f"[/SUPERIMPORTANT: Follow the skill guide above]\n"
            )

        completion_check = (
            "[!] Before calling task_complete, verify:\n"
            "- Does the solution meet the task requirements? [TODO/DONE]\n"
            "- Have you tested the solution? [TODO/DONE]\n"
            "- Are all edge cases handled? [TODO/DONE]\n"
        )

        return (
            f"Task: {self._original_instruction}\n{skill_context}\n{completion_check}"
        )

    async def _call_llm(
        self, messages: list[dict], current_prompt: str
    ) -> dict[str, Any]:
        full_messages = messages + [{"role": "user", "content": current_prompt}]

        try:
            response = await asyncio.wait_for(
                asyncio.to_thread(
                    _get_client().chat.completions.create,
                    model=self.model,
                    messages=full_messages,
                    temperature=self.temperature,
                    tools=TOOLS,
                    timeout=900,
                ),
                timeout=BLOCK_TIMEOUT_SEC,
            )
        except asyncio.TimeoutError:
            return {"content": "ERROR: LLM call timed out.", "tool_calls": []}
        except Exception as e:
            return {"content": f"ERROR: {e}", "tool_calls": []}

        choice = response.choices[0]
        message = choice.message

        return {
            "content": message.content or "",
            "tool_calls": [
                {
                    "id": tc.id,
                    "type": "function",
                    "function": {
                        "name": tc.function.name,
                        "arguments": tc.function.arguments,
                    },
                }
                for tc in (message.tool_calls or [])
            ],
            "usage": response.model_dump().get("usage", {}),
        }

    def _parse_tool_calls(
        self, tool_calls: list[dict]
    ) -> tuple[list[Command], bool, str, str, str, ImageReadRequest | None]:
        commands = []
        is_complete = False
        feedback = ""
        analysis = ""
        plan = ""
        image_req = None

        if not tool_calls:
            return commands, False, "No tool calls.", analysis, plan, image_req

        for tc in tool_calls:
            fn = tc.get("function", {})
            name = fn.get("name", "")
            raw_args = fn.get("arguments", "{}")

            if isinstance(raw_args, str):
                try:
                    args = json.loads(raw_args)
                except json.JSONDecodeError:
                    continue
            else:
                args = raw_args

            if name == "execute_commands":
                analysis = args.get("analysis", "")
                plan = args.get("plan", "")
                cmds = args.get("commands", [])
                if isinstance(cmds, str):
                    try:
                        cmds = json.loads(cmds)
                    except json.JSONDecodeError:
                        cmds = []
                for cmd in cmds:
                    keystrokes = cmd.get("keystrokes", "")
                    duration = min(float(cmd.get("duration", 1.0)), 60.0)
                    commands.append(
                        Command(keystrokes=keystrokes, duration_sec=duration)
                    )

            elif name == "task_complete":
                is_complete = True
                if not self._pending_completion:
                    self._pending_completion = True

            elif name == "image_read":
                fp = args.get("file_path", "")
                inst = args.get("image_read_instruction", "")
                if fp and inst:
                    image_req = ImageReadRequest(
                        file_path=fp, image_read_instruction=inst
                    )

        return commands, is_complete, feedback, analysis, plan, image_req

    async def _execute_commands(self, commands: list[Command], env: SkillEnv) -> str:
        if self._session is None:
            return "ERROR: No tmux session"

        output_parts = []
        for cmd in commands:
            self._marker_seq += 1
            marker = f"{MARKER_PREFIX}{self._marker_seq}__"
            start = time.monotonic()

            self._session.send_keys(cmd.keystrokes)
            self._session.send_keys(f"echo '{marker}'")

            await asyncio.sleep(min(0.3, cmd.duration_sec))
            while time.monotonic() - start < cmd.duration_sec:
                pane = self._session.capture_pane()
                if marker in pane:
                    break
                await asyncio.sleep(0.5)

            saved = cmd.duration_sec - (time.monotonic() - start)
            if saved > 0.1:
                self._total_time_saved += saved

            pane = self._session.capture_pane()
            lines = pane.split("\n")
            clean_lines = [
                line
                for line in lines
                if not any(
                    m in line
                    for m in [
                        f"{MARKER_PREFIX}{i}__" for i in range(1, self._marker_seq + 1)
                    ]
                )
            ]
            output_parts.append("\n".join(clean_lines))

        result = "\n".join(output_parts)
        return result[:30000]

    async def _execute_image_read(self, req: ImageReadRequest, env: SkillEnv) -> str:
        if self._session is None:
            return "ERROR: No tmux session"

        result = await asyncio.wait_for(
            asyncio.to_thread(self._session.exec, f"base64 {req.file_path}", 15),
            timeout=20,
        )
        b64 = result.stdout.replace("\n", "")

        ext = Path(req.file_path).suffix.lower()
        mime_map = {
            ".png": "image/png",
            ".jpg": "image/jpeg",
            ".jpeg": "image/jpeg",
            ".gif": "image/gif",
            ".webp": "image/webp",
        }
        mime = mime_map.get(ext, "image/png")

        multimodal = [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": req.image_read_instruction},
                    {
                        "type": "image_url",
                        "image_url": {"url": f"data:{mime};base64,{b64}"},
                    },
                ],
            }
        ]

        try:
            resp = await asyncio.wait_for(
                asyncio.to_thread(
                    _get_client().chat.completions.create,
                    model=self.model,
                    messages=multimodal,
                    temperature=self.temperature,
                ),
                timeout=900,
            )
        except Exception as e:
            return f"ERROR: {e}"

        text = resp.choices[0].message.content or ""
        return f"Image analysis for '{req.file_path}':\n{text}"


class SkillEnv:
    """Sandbox environment for skill compliance testing."""

    def __init__(
        self,
        session_name: str,
        skill_content: str | None = None,
        setup_commands: list[str] | None = None,
        working_dir: str = "/app",
    ):
        self.session_name = session_name
        self.skill_content = skill_content
        self.setup_commands = setup_commands or []
        self.working_dir = working_dir

    async def setup(self) -> None:
        session = TmuxSession(self.session_name)
        session.start()

        session.send_keys(f"mkdir -p {self.working_dir} && cd {self.working_dir}")
        await asyncio.sleep(0.5)

        for cmd in self.setup_commands:
            session.send_keys(cmd)
            await asyncio.sleep(1.0)

        session.kill()
