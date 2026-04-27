"""
Coordinator Worker — LLM-powered sub-agent that executes a single role.

Each worker gets instructions, calls switchAILocal, and writes structured
output to the shared scratchpad. Workers are stateless — all context flows
through the scratchpad and the instructions they receive.
"""

import logging
import os
import time
from enum import Enum
from typing import Any, Dict, Optional

import httpx

from core.coordinator.scratchpad import Scratchpad

log = logging.getLogger("harvey.coordinator.worker")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
SWITCH_AI_URL = os.environ.get("SWITCH_AI_URL", "http://localhost:18080/v1")
LLM_MODEL = os.environ.get("LLM_MODEL", "auto")


class WorkerRole(Enum):
    """Defined roles a worker can assume in the swarm pipeline."""
    RESEARCHER = "researcher"
    SYNTHESIZER = "synthesizer"
    IMPLEMENTER = "implementer"
    VERIFIER = "verifier"


class Worker:
    """
    LLM-powered sub-agent bound to a specific role and task.

    Calls switchAILocal to execute its instructions, then writes
    the output to scratchpad/{role_name}.md.
    """

    def __init__(
        self,
        role: WorkerRole,
        task_id: str,
        instructions: str,
        scratchpad: Scratchpad,
        *,
        worker_id: str = "",
        model: str = "",
        timeout: float = 120.0,
    ):
        self.role = role
        self.task_id = task_id
        self.instructions = instructions
        self.scratchpad = scratchpad
        self.worker_id = worker_id or f"{role.value}-{task_id[:8]}"
        self.model = model or LLM_MODEL
        self.timeout = timeout

    def _build_messages(self) -> list:
        """Build the chat messages for the LLM call."""
        system = (
            f"You are a {self.role.value} agent in a multi-agent swarm. "
            f"Task ID: {self.task_id}. "
            f"Your role: {self.role.value.upper()}. "
            f"Follow the instructions precisely. Be thorough and structured. "
            f"Output markdown."
        )
        return [
            {"role": "system", "content": system},
            {"role": "user", "content": self.instructions},
        ]

    def _call_llm(self) -> str:
        """POST to switchAILocal /chat/completions and return the response text."""
        payload = {
            "model": self.model,
            "messages": self._build_messages(),
            "temperature": 0.3,
        }
        with httpx.Client(timeout=self.timeout) as client:
            resp = client.post(
                f"{SWITCH_AI_URL}/chat/completions",
                json=payload,
            )
            resp.raise_for_status()
            data = resp.json()

        # Extract assistant message content
        choices = data.get("choices", [])
        if not choices:
            raise ValueError(f"LLM returned no choices for worker {self.worker_id}")
        return choices[0]["message"]["content"]

    def execute(self) -> Dict[str, Any]:
        """
        Run the worker: call LLM, write output to scratchpad, return results.

        Returns:
            dict with keys: status, output_path, duration, worker_id, role
        """
        output_filename = f"{self.role.value}.md"
        if self.worker_id != f"{self.role.value}-{self.task_id[:8]}":
            # Disambiguate when multiple workers share a role
            output_filename = f"{self.worker_id}.md"

        start = time.time()
        try:
            log.info("Worker %s starting (model=%s)", self.worker_id, self.model)
            content = self._call_llm()
            self.scratchpad.write(output_filename, content)
            duration = round(time.time() - start, 2)
            log.info("Worker %s completed in %.1fs", self.worker_id, duration)
            return {
                "status": "completed",
                "output_path": output_filename,
                "duration": duration,
                "worker_id": self.worker_id,
                "role": self.role.value,
            }
        except Exception as e:
            duration = round(time.time() - start, 2)
            error_msg = f"Worker {self.worker_id} failed: {e}"
            log.error(error_msg)
            self.scratchpad.write(
                output_filename,
                f"# ERROR\n\n{error_msg}\n",
            )
            return {
                "status": "failed",
                "output_path": output_filename,
                "duration": duration,
                "worker_id": self.worker_id,
                "role": self.role.value,
                "error": str(e),
            }

    def __repr__(self) -> str:
        return f"Worker(role={self.role.value}, id={self.worker_id})"
