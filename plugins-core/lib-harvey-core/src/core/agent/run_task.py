"""
run_task — CLI entry point to resume a single task from the TaskStore.

Used by the cron resumer (core.agent.resumer) after it claims a stale task.
Can also be invoked manually:

    python -m core.agent.run_task --task-id <uuid>

Protocol:
  1. Load the task from TaskStore
  2. Refuse if terminal (already completed / failed / cancelled)
  3. Rebuild the message history from task_entries (pi-mono replay pattern)
  4. Find the last user message (the one that needs a response)
  5. Set state RUNNING
  6. Call HarveyAgent.process() with task_id + store, letting the agent
     checkpoint every turn / tool call / artifact
  7. On success: state=COMPLETED with the agent response
  8. On exception: state=FAILED with the error

Retry budget lives in task.metadata["resume_attempts"]. If >= MAX_ATTEMPTS
when we enter here, the task is immediately marked FAILED to prevent
infinite resume loops.
"""

from __future__ import annotations

import argparse
import logging
import os
import sys
from typing import Any, Dict, List, Optional

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
sys.path.insert(0, os.path.join(HARVEY_HOME, "harvey-os"))

log = logging.getLogger("harvey.resumer.run_task")

MAX_RESUME_ATTEMPTS = 3


def rehydrate_messages(entries) -> List[Dict[str, str]]:
    """Rebuild an OpenAI-format messages list from task_entries.

    Maps:
      - MESSAGE role=user → {role: user, content}
      - MESSAGE role=assistant → {role: assistant, content}
      - TOOL_CALL → skipped (tool calls are attached to the preceding
        assistant message in the OpenAI format, but for a resume we
        don't have the original tool_call structure — we rebuild from
        scratch and let the next LLM call decide what to do)
      - TOOL_RESULT → {role: tool, content, tool_call_id: ""} — included
        so the next LLM call has the context of prior tool outputs
      - Other types → skipped
    """
    from core.tasks import EntryType

    messages: List[Dict[str, str]] = []
    for entry in entries:
        if entry.entry_type == EntryType.MESSAGE:
            role = entry.role or "user"
            if entry.content:  # skip empty assistant turns (tool-only rounds)
                messages.append({"role": role, "content": entry.content})
        elif entry.entry_type == EntryType.TOOL_RESULT:
            # Fold tool results into the history as plain assistant
            # observations so the next LLM call sees what happened
            messages.append({
                "role": "assistant",
                "content": f"[tool {entry.tool_name} result]: {entry.content[:2000]}",
            })
    return messages


def find_last_user_message(entries) -> Optional[Any]:
    """Return the most recent user MESSAGE entry, or None."""
    from core.tasks import EntryType
    for entry in reversed(entries):
        if entry.entry_type == EntryType.MESSAGE and entry.role == "user":
            return entry
    return None


def run_task(task_id: str, store=None, agent=None) -> int:
    """Resume one task. Returns exit code (0 success, non-zero failure)."""
    from core.tasks import TaskState, TaskStore

    if store is None:
        store = TaskStore()

    task = store.get_task(task_id)
    if task is None:
        log.error(f"run_task: unknown task {task_id}")
        return 2

    if task.is_terminal():
        log.info(f"run_task: {task_id[:8]} already terminal ({task.state.value}) — nothing to do")
        return 0

    # Check retry budget
    attempts = int(task.metadata.get("resume_attempts", 0)) if task.metadata else 0
    if attempts >= MAX_RESUME_ATTEMPTS:
        log.warning(
            f"run_task: {task_id[:8]} exhausted retry budget "
            f"({attempts}/{MAX_RESUME_ATTEMPTS}) — marking FAILED"
        )
        store.set_state(
            task_id,
            TaskState.FAILED,
            error=f"resumer: exhausted {MAX_RESUME_ATTEMPTS} resume attempts",
        )
        return 3

    # Bump attempt counter via metadata update
    new_metadata = dict(task.metadata) if task.metadata else {}
    new_metadata["resume_attempts"] = attempts + 1
    with store._tx() as conn:
        import json
        conn.execute(
            "UPDATE tasks SET metadata = ? WHERE id = ?",
            (json.dumps(new_metadata), task_id),
        )

    entries = store.get_entries(task_id)
    last_user = find_last_user_message(entries)
    if last_user is None:
        log.error(f"run_task: {task_id[:8]} has no user message to respond to")
        store.set_state(
            task_id,
            TaskState.FAILED,
            error="resumer: no user message in task entries",
        )
        return 4

    messages = rehydrate_messages(entries)
    # Drop the trailing user message — we pass it as `message`, not history
    if messages and messages[-1]["role"] == "user" and messages[-1]["content"] == last_user.content:
        messages = messages[:-1]

    # Transition to RUNNING + bump heartbeat
    store.set_state(task_id, TaskState.RUNNING)

    # Agent
    if agent is None:
        from core.agent.harvey_agent import HarveyAgent
        from core.chat.config import load_config
        cfg = load_config().bridge
        agent = HarveyAgent(
            llm_url=cfg.switchai_url,
            llm_model=cfg.switchai_model,
            api_key=cfg.switchai_api_key,
            max_tokens=cfg.max_tokens,
        )

    # System prompt — use the same one the live bridge would use
    try:
        from core.chat.bridge import HARVEY_SYSTEM_PROMPT
        system_prompt = HARVEY_SYSTEM_PROMPT.format(channel=task.channel)
    except Exception:
        system_prompt = f"You are Harvey. Resuming task {task_id[:8]} on channel {task.channel}."

    log.info(
        f"run_task: resuming {task_id[:8]} "
        f"(attempt {attempts + 1}/{MAX_RESUME_ATTEMPTS}, "
        f"{len(messages)} prior messages)"
    )

    try:
        response = agent.process(
            message=last_user.content,
            history=messages,
            system_prompt=system_prompt,
            channel=task.channel,
            task_id=task_id,
            store=store,
        )
    except Exception as e:
        log.error(f"run_task: agent crashed for {task_id[:8]}: {e}", exc_info=True)
        store.set_state(
            task_id,
            TaskState.FAILED,
            error=f"resumer: agent crash: {type(e).__name__}: {e}"[:500],
        )
        return 5

    if response is None:
        store.set_state(task_id, TaskState.FAILED, error="resumer: agent returned None")
        return 6

    store.set_state(task_id, TaskState.COMPLETED, result=response)
    log.info(f"run_task: {task_id[:8]} completed ({len(response)} chars)")
    return 0


def _cli() -> int:
    parser = argparse.ArgumentParser(description="Resume a Harvey task")
    parser.add_argument("--task-id", required=True, help="Task UUID")
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s [%(name)s] %(levelname)s: %(message)s",
    )
    return run_task(args.task_id)


if __name__ == "__main__":
    sys.exit(_cli())
