"""
HarveyChat Gateway — Main service that orchestrates channels, bridge, and persistence.

This is the central nervous system of HarveyChat. It:
1. Receives messages from any channel (Telegram, etc.)
2. Loads conversation history from the store
3. Routes to Harvey's brain via the bridge
4. Stores the response
5. Logs significant interactions to Brain

NEW: Also manages persistent tasks with multi-turn refinement:
- Tracks active tasks per user
- Supports background job execution
- Enables asking for clarification and resuming work
- Sends progress updates during long operations

Usage:
    chat = HarveyChat()
    await chat.start()   # Blocks until stopped
"""

import asyncio
import logging
import os
import re
import signal
import sys
import time
from typing import Dict
from pathlib import Path

import requests

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))

from core.chat.config import ChatConfig, load_config
from core.chat.store import ChatStore
from core.chat.bridge import HarveyBridge
from core.chat.brain_sync import log_to_journal, log_session_summary
from core.chat.channels.telegram import TelegramChannel
from core.chat.task_queue import TaskQueue, TaskState
from core.chat.conversation import ConversationManager

log = logging.getLogger("harveychat.gateway")

# ── Smart routing: messages under this word count always go to bridge.send()
# regardless of keyword hits (prevents simple Q&A regression)
SHORT_MESSAGE_THRESHOLD = 8

# ── Intents that trigger background workflows instead of single-shot bridge
WORKFLOW_INTENTS = {"research", "image", "archive"}

# Feature flag: when set to "1", enable the cognitive core path
# (TaskStore + agent checkpointing + durable task tree). When "0" or
# unset, HarveyChat runs the legacy gateway path identical to before
# the cognitive core sprint. This gives Sebastian a single-env-var
# rollback if Phase 2 introduces a regression in production.
COGNITIVE_CORE_ENABLED = os.environ.get("HARVEY_COGNITIVE_CORE", "0") == "1"

# Strip XML tool-call fragments that MiniMax sometimes puts in content
TOOL_XML_PATTERN = re.compile(
    r'<invoke\s+name="[^"]+">.*?</invoke\s*>',
    re.DOTALL,
)


class HarveyChat:
    """Main gateway service — coordinates channels, LLM bridge, and persistence."""

    def __init__(self, config: ChatConfig = None):
        self.config = config or load_config()
        self.store = ChatStore(self.config.db_path)
        self.bridge = HarveyBridge(self.config.bridge)
        self.task_queue = TaskQueue()
        self.conv_manager = ConversationManager(self.task_queue)
        self.channels = []
        self._running = False
        self._start_time = 0

        # Smart routing: intent classification + workflow dispatch
        self._router = None
        self._workflow_engine = None
        self._dag_executor = None
        self._active_workflows: Dict[str, asyncio.Task] = {}  # key: "channel:user_id"
        try:
            from core.orchestration.intelligent_router import IntelligentRouter
            self._router = IntelligentRouter()
            log.info("[gateway] IntelligentRouter loaded — smart routing enabled")
        except Exception as e:
            log.warning(f"[gateway] IntelligentRouter unavailable, using bridge-only: {e}")

        if self._router:
            try:
                from core.workflow.engine import WorkflowEngine
                from core.workflow.async_dag_executor import AsyncDAGExecutor
                self._workflow_engine = WorkflowEngine()
                self._dag_executor = AsyncDAGExecutor()
                log.info("[gateway] WorkflowEngine + AsyncDAGExecutor loaded")
            except Exception as e:
                log.warning(f"[gateway] Workflow system unavailable: {e}")
                self._router = None  # Can't route without workflow engine

        # Health monitor state
        self._switchai_healthy = True
        self._switchai_fail_count = 0
        self._health_check_interval = 30  # seconds
        self._health_fail_threshold = 3

        # Polling watchdog state
        self._poll_stall_threshold = 90  # seconds
        self._poll_watchdog_interval = 30  # seconds

        # Cognitive core: durable task tree, checkpoint writes, resumable
        # work. Gated by HARVEY_COGNITIVE_CORE env var so the legacy path
        # is the zero-config default while Phase 2 rolls out.
        # HARVEY_COGNITIVE_TASKS_DB allows an explicit path override, used
        # by tests to isolate from the production DB.
        self.task_store = None
        if COGNITIVE_CORE_ENABLED:
            try:
                from core.tasks import TaskStore
                override_path = os.environ.get("HARVEY_COGNITIVE_TASKS_DB")
                if override_path:
                    self.task_store = TaskStore(db_path=override_path)
                else:
                    self.task_store = TaskStore()
                log.info(
                    f"[gateway] cognitive core ENABLED — TaskStore at {self.task_store.db_path}"
                )
            except Exception as e:
                log.error(
                    f"[gateway] cognitive core requested but TaskStore init failed: {e} "
                    f"— falling back to legacy path",
                    exc_info=True,
                )
                self.task_store = None
        else:
            log.info("[gateway] cognitive core disabled (HARVEY_COGNITIVE_CORE=0)")

        # Event bus — lazy-loaded, best-effort. Used for task lifecycle
        # publishes that the StreamingProgressBridge (and auto_memory_router
        # once it's wired) consume. Same singleton as the subagent swarm.
        self._event_bus = None
        if COGNITIVE_CORE_ENABLED:
            try:
                from core.orchestration.persistent_event_bus import get_default_bus
                self._event_bus = get_default_bus()
            except Exception as e:
                log.warning(f"[gateway] event bus unavailable: {e}")
                self._event_bus = None

        # Initialize configured channels
        if self.config.telegram.bot_token:
            self.channels.append(TelegramChannel(self.config.telegram))

    def _publish_event(self, topic: str, **data) -> None:
        """Best-effort event publish. Never raises."""
        if self._event_bus is None:
            return
        try:
            self._event_bus.publish(topic, source="harvey-gateway", **data)
        except Exception as e:
            log.warning(f"[gateway] publish({topic}) failed: {e}")

    async def handle_message(
        self, channel: str, user_id: str, username: str, text: str
    ) -> str:
        """
        Central message handler — called by all channels.

        New flow:
        1. Check if user has an active task awaiting input
        2. If yes, feed message to that task for refinement
        3. If no, check message type and route appropriately
        4. Handle async tasks: send progress updates, final results, file attachments

        Handles special commands, routes to bridge, persists everything.
        """
        # Handle special commands
        if text == "/status":
            return self._get_status(channel, user_id)

        if text == "/clear":
            # Clear active task if any
            conv = self.conv_manager.get_or_create(channel, user_id)
            task = conv.get_active_task()
            if task:
                self.task_queue.set_failed(task, "User cleared context")
                log.info(f"Cleared active task {task.id}")
            return "Context cleared. Starting fresh."

        if text == "/cancel":
            wf_key = f"{channel}:{user_id}"
            wf_task = self._active_workflows.pop(wf_key, None)
            if wf_task and not wf_task.done():
                wf_task.cancel()
                log.info(f"Cancelled workflow for {wf_key}")
                return "Workflow cancelled."
            return "No active workflow to cancel."

        if text.startswith("/"):
            return f"Unknown command: {text.split()[0]}. Just talk to me normally."

        # Store user message
        self.store.add_message(channel, user_id, "user", text, {"username": username})

        # Get or create conversation state
        conv = self.conv_manager.get_or_create(channel, user_id)

        # Check if user is responding to a pending task question
        active_task = conv.get_active_task()
        if active_task and active_task.awaiting_input_prompt:
            # User is providing feedback/clarification to existing task
            log.info(f"Resuming task {active_task.id} with user feedback")
            self.task_queue.add_message(active_task, "user", text)

            # Resume task with new input
            response = await self._resume_task(active_task, text, channel, user_id, username)
        else:
            # New conversation/task
            response = await self._handle_new_message(text, channel, user_id, username)

        # Sanitize — strip any XML tool-call fragments MiniMax might leak into content
        response = self._sanitize_response(response)

        # Store assistant response
        self.store.add_message(channel, user_id, "assistant", response)

        # Log to Brain if configured
        if self.config.log_to_brain and _is_significant(text, response):
            log_to_journal(channel, f"@{username}: {text[:80]}", response[:200])

        return response

    async def _handle_new_message(
        self, text: str, channel: str, user_id: str, username: str
    ) -> str:
        """Handle a fresh message (not responding to pending task).

        Smart routing (when available):
        - Short messages (<15 words) → always bridge.send() (fast path)
        - unknown/minimal intent → bridge.send()
        - research/image/archive intent → background workflow
        """
        # ── Smart routing: classify intent if router is available ──
        if self._router and self._workflow_engine and self._dag_executor:
            word_count = len(text.split())
            if word_count >= SHORT_MESSAGE_THRESHOLD:
                classification = self._router.classify(text)
                if (
                    classification.intent in WORKFLOW_INTENTS
                    and classification.is_confident(0.3)
                ):
                    # Check for already-running workflow
                    wf_key = f"{channel}:{user_id}"
                    existing = self._active_workflows.get(wf_key)
                    if existing and not existing.done():
                        return (
                            "I'm still working on your previous request. "
                            "Send /cancel to stop it, or wait for it to finish."
                        )

                    log.info(
                        f"[routing] intent={classification.intent} "
                        f"confidence={classification.confidence} "
                        f"keywords={classification.keywords_hit} → workflow"
                    )
                    return await self._handle_workflow(
                        text, classification, channel, user_id
                    )

        # ── Fast path: simple Q&A via bridge.send() ──
        # Get conversation history
        history = self.store.get_history(
            channel, user_id, limit=self.config.bridge.max_history_messages
        )

        # Cognitive core: create/resume a durable root task, pass task_id
        # + store through the bridge so every LLM turn + tool call is
        # checkpointed. Guarded by the feature flag — the legacy path
        # below is bit-for-bit what Sebastian had before Phase 2.
        cognitive_task_id = None
        cognitive_task_resumed = False
        if self.task_store is not None:
            try:
                from core.tasks import TaskState, TaskEntry
                # Active root task for this user (if any)
                active = self.task_store.active_root_for_user(channel, user_id)
                if active is not None:
                    cognitive_task_id = active.id
                    cognitive_task_resumed = True
                    # Record the new user message as a turn on the active task
                    self.task_store.append_entry(
                        TaskEntry.message(active.id, "user", text)
                    )
                    log.info(
                        f"[gateway] resuming task {active.id[:8]} for {channel}:{user_id}"
                    )
                    self._publish_event(
                        "task.resumed",
                        task_id=active.id,
                        channel=channel,
                        user_id=user_id,
                        goal=active.goal,
                        reason="user_message",
                    )
                else:
                    new_task = self.task_store.create_root_task(
                        channel=channel, user_id=user_id, goal=text
                    )
                    cognitive_task_id = new_task.id
                    self.task_store.set_state(new_task.id, TaskState.RUNNING)
                    self.task_store.append_entry(
                        TaskEntry.message(new_task.id, "user", text)
                    )
                    log.info(
                        f"[gateway] created root task {new_task.id[:8]} for {channel}:{user_id}"
                    )
                    self._publish_event(
                        "task.created",
                        task_id=new_task.id,
                        channel=channel,
                        user_id=user_id,
                        goal=text[:500],
                        kind="root",
                    )
            except Exception as e:
                log.error(
                    f"[gateway] task creation failed, falling back to legacy path: {e}",
                    exc_info=True,
                )
                cognitive_task_id = None

        # Route to Harvey's brain with progress feedback for slow responses.
        # If the bridge takes >5s, send a "Thinking..." nudge so the user
        # knows Olibia is alive. At 30s, send a second nudge.
        bridge_future = asyncio.get_event_loop().run_in_executor(
            None,
            lambda: self._bridge_send_with_file_hints(
                text, history, channel, task_id=cognitive_task_id
            ),
        )

        async def _send_thinking_feedback():
            await asyncio.sleep(5)
            if not bridge_future.done():
                for ch in self.channels:
                    if ch.name == channel:
                        try:
                            await ch.send(user_id, "Thinking...")
                        except Exception:
                            pass
            await asyncio.sleep(25)  # 30s total
            if not bridge_future.done():
                for ch in self.channels:
                    if ch.name == channel:
                        try:
                            await ch.send(user_id, "Still working on it...")
                        except Exception:
                            pass

        feedback_task = asyncio.ensure_future(_send_thinking_feedback())

        try:
            response, files_to_send = await bridge_future
        except Exception as bridge_exc:
            feedback_task.cancel()
            if self.task_store is not None and cognitive_task_id is not None:
                try:
                    from core.tasks import TaskState
                    self.task_store.set_state(
                        cognitive_task_id,
                        TaskState.FAILED,
                        error=f"bridge crash: {type(bridge_exc).__name__}: {bridge_exc}"[:500],
                    )
                except Exception as state_err:
                    log.warning(
                        f"[gateway] failed to mark task FAILED after bridge crash "
                        f"(original bridge error: {bridge_exc!r}; state update error: {state_err})"
                    )
                self._publish_event(
                    "task.failed",
                    task_id=cognitive_task_id,
                    channel=channel,
                    user_id=user_id,
                    error=f"{type(bridge_exc).__name__}: {bridge_exc}"[:500],
                    stage="bridge",
                )
            raise

        # Bridge call succeeded — stop the thinking feedback
        feedback_task.cancel()

        # Update task state after the agent returns
        if self.task_store is not None and cognitive_task_id is not None:
            try:
                from core.tasks import TaskState
                self.task_store.set_state(
                    cognitive_task_id, TaskState.COMPLETED, result=response
                )
            except Exception as e:
                log.warning(f"[gateway] task completion update failed: {e}")
            # Aggregate artifacts for the completion event
            artifacts = []
            try:
                arts = self.task_store.get_artifacts(cognitive_task_id)
                artifacts = [
                    {"kind": a.kind, "path": a.path, "size_bytes": a.size_bytes}
                    for a in arts
                ]
            except Exception:
                pass
            self._publish_event(
                "task.completed",
                task_id=cognitive_task_id,
                channel=channel,
                user_id=user_id,
                response_length=len(response or ""),
                artifact_count=len(artifacts),
                artifacts=artifacts,
            )

        # Send any files Harvey marked for sending
        for file_type, file_path in files_to_send:
            await self._send_file(channel, user_id, file_type, file_path)

        return response

    async def _resume_task(
        self, task, user_input: str, channel: str, user_id: str, username: str
    ) -> str:
        """Resume a task that was awaiting user input."""
        # Get full task history
        history = self.store.get_history(
            channel, user_id, limit=self.config.bridge.max_history_messages
        )

        # Mark task as running again (was awaiting_input)
        self.task_queue.set_running(task)

        # Send to bridge with task context
        response, files_to_send = await asyncio.get_event_loop().run_in_executor(
            None, lambda: self._bridge_send_with_file_hints(
                user_input, history, channel
            )
        )

        # Update task with assistant response
        self.task_queue.add_message(task, "assistant", response)

        # Send files if any
        for file_type, file_path in files_to_send:
            await self._send_file(channel, user_id, file_type, file_path)
            task.files_to_send.append((file_type, file_path))

        # Check if response includes another clarification request
        # (In full impl, Harvey would mark task as awaiting_input again)
        # For now, mark as complete
        if task.state == TaskState.RUNNING:
            self.task_queue.set_completed(task, response, task.files_to_send)

        return response

    async def _handle_workflow(
        self, text: str, classification, channel: str, user_id: str
    ) -> str:
        """Route complex requests to background workflow execution."""
        from core.chat.workflow_dispatcher import ChatWorkflowDispatcher

        team_roster = self._router.route(text)

        async def send_to_user(msg: str):
            for ch in self.channels:
                if ch.name == channel:
                    try:
                        await ch.send(user_id, msg)
                    except Exception as e:
                        log.warning(f"[workflow] send failed: {e}")

        async def send_typing():
            for ch in self.channels:
                if ch.name == channel:
                    try:
                        chat_id = ch._resolve_chat_id(user_id)
                        await ch._app.bot.send_chat_action(
                            chat_id=chat_id, action="typing"
                        )
                    except Exception:
                        pass

        dispatcher = ChatWorkflowDispatcher(
            engine=self._workflow_engine,
            executor=self._dag_executor,
            send_fn=send_to_user,
            send_typing_fn=send_typing,
        )

        task = await dispatcher.dispatch(
            text=text,
            intent=classification.intent,
            team_roster=team_roster,
            channel=channel,
            user_id=user_id,
        )

        if task:
            wf_key = f"{channel}:{user_id}"
            self._active_workflows[wf_key] = task

        # Return empty — dispatcher already sent acknowledgment
        # The actual results will arrive via background task
        return f"Working on it — {classification.intent} workflow started."

    def _bridge_send_with_file_hints(
        self, text: str, history: list, channel: str, task_id: str = None
    ) -> tuple[str, list]:
        """
        Call bridge.send() and check for file markers.
        Returns (response, [(type, path), ...]).
        Runs synchronously in executor.

        If task_id is provided AND the cognitive core is enabled, the
        bridge + agent will checkpoint every LLM turn and tool call to
        the TaskStore for the given task.
        """
        import re

        response = self.bridge.send(
            text,
            history,
            channel,
            task_id=task_id,
            store=self.task_store,
        )

        # Check for file markers
        FILE_PATTERN = re.compile(r"\[\[SEND_FILE:([^\]]+)\]\]")
        PHOTO_PATTERN = re.compile(r"\[\[SEND_PHOTO:([^\]]+)\]\]")

        files = []
        for m in FILE_PATTERN.finditer(response):
            path = os.path.expanduser(m.group(1).strip())
            if os.path.exists(path):
                files.append(("file", path))
            else:
                log.warning(f"SEND_FILE marker: file not found: {path}")
        for m in PHOTO_PATTERN.finditer(response):
            path = os.path.expanduser(m.group(1).strip())
            if os.path.exists(path):
                files.append(("photo", path))
            else:
                log.warning(f"SEND_PHOTO marker: file not found: {path}")

        # Remove markers from response
        response = FILE_PATTERN.sub("", response)
        response = PHOTO_PATTERN.sub("", response)
        response = response.strip()

        return response, files

    def _sanitize_response(self, response: str) -> str:
        """Strip XML tool-call fragments that models sometimes leak into content."""
        cleaned = TOOL_XML_PATTERN.sub("", response)
        # Also strip any stray XML-like fragments
        cleaned = re.sub(r"<[^>]{0,200}>", "", cleaned)
        cleaned = cleaned.strip()
        if not cleaned:
            return "(processed)"
        return cleaned

    async def _send_file(
        self, channel: str, user_id: str, file_type: str, file_path: str
    ):
        """Send a file via the appropriate channel."""
        for ch in self.channels:
            if ch.name == channel:
                if file_type == "file":
                    await ch.send_document(user_id, file_path)
                elif file_type == "photo":
                    await ch.send_photo(user_id, file_path)
                return

    def _get_status(self, channel: str, user_id: str) -> str:
        """Build status response."""
        uptime = time.time() - self._start_time if self._start_time else 0
        stats = self.store.get_stats()
        hours = int(uptime / 3600)
        minutes = int((uptime % 3600) / 60)

        channels_active = [ch.name for ch in self.channels if ch.is_configured()]

        return (
            f"Harvey Chat Gateway\n"
            f"Uptime: {hours}h {minutes}m\n"
            f"Messages: {stats['total_messages']}\n"
            f"Sessions: {stats['total_sessions']}\n"
            f"Channels: {', '.join(channels_active)}\n"
            f"switchAILocal: {'online' if self._switchai_healthy else 'OFFLINE'}"
            f"{'' if self._switchai_healthy else f' (fails: {self._switchai_fail_count})'}\n"
            f"Brain sync: {'on' if self.config.log_to_brain else 'off'}"
        )

    def _switchai_health_url(self) -> str:
        """Derive the health endpoint from the bridge URL.

        Handles: http://localhost:18080/v1, http://localhost:18080/v1/,
        http://localhost:18080, etc.
        """
        base = self.config.bridge.switchai_url.rstrip("/")
        # Strip known API path suffixes
        for suffix in ("/v1", "/v1/chat/completions", "/chat/completions"):
            if base.endswith(suffix):
                base = base[: -len(suffix)]
                break
        return f"{base}/health"

    async def _health_monitor(self):
        """Background task: check switchAILocal health every 30s."""
        health_url = self._switchai_health_url()
        while self._running:
            try:
                r = requests.get(health_url, timeout=5)
                healthy = r.status_code == 200
            except Exception:
                healthy = False

            if healthy:
                if not self._switchai_healthy:
                    log.info("[health] switchAILocal recovered")
                self._switchai_healthy = True
                self._switchai_fail_count = 0
            else:
                self._switchai_fail_count += 1
                if self._switchai_fail_count >= self._health_fail_threshold:
                    if self._switchai_healthy:
                        log.warning(
                            f"[health] switchAILocal DOWN — "
                            f"{self._switchai_fail_count} consecutive failures"
                        )
                    self._switchai_healthy = False

            await asyncio.sleep(self._health_check_interval)

    async def _poll_watchdog(self):
        """Background task: detect stalled Telegram polling and restart."""
        while self._running:
            await asyncio.sleep(self._poll_watchdog_interval)
            for ch in self.channels:
                if ch.name == "telegram" and hasattr(ch, "last_poll_time"):
                    elapsed = time.time() - ch.last_poll_time
                    if elapsed > self._poll_stall_threshold:
                        log.error(
                            f"[watchdog] Telegram polling stalled for {elapsed:.0f}s — restarting"
                        )
                        try:
                            await ch.stop()
                            await ch.start(self.handle_message)
                            log.info("[watchdog] Telegram polling restarted")
                        except Exception as e:
                            log.error(f"[watchdog] Failed to restart Telegram: {e}")

    async def start(self):
        """Start all channels and run until stopped."""
        self._running = True
        self._start_time = time.time()

        # Write PID file
        pid_path = Path(self.config.pid_file)
        pid_path.parent.mkdir(parents=True, exist_ok=True)
        pid_path.write_text(str(os.getpid()))

        active = []
        for ch in self.channels:
            if ch.is_configured():
                try:
                    await ch.start(self.handle_message)
                    active.append(ch.name)
                    log.info(f"Channel started: {ch.name}")
                except Exception as e:
                    log.error(f"Failed to start channel {ch.name}: {e}", exc_info=True)

        if not active:
            log.error("No channels started! Configure at least one channel.")
            log.error("Set TELEGRAM_BOT_TOKEN env var or edit data/chat/config.json")
            return

        log.info(f"HarveyChat gateway live — channels: {', '.join(active)}")

        if self.config.log_to_brain:
            log_to_journal(
                "system", f"HarveyChat gateway started — channels: {', '.join(active)}"
            )

        # Start background monitors
        health_task = asyncio.ensure_future(self._health_monitor())
        watchdog_task = asyncio.ensure_future(self._poll_watchdog())

        # Run until stopped
        try:
            while self._running:
                await asyncio.sleep(1)
        except (KeyboardInterrupt, SystemExit):
            pass
        finally:
            health_task.cancel()
            watchdog_task.cancel()
            await self.stop()

    async def stop(self):
        """Gracefully stop all channels."""
        self._running = False
        log.info("Shutting down HarveyChat gateway...")

        for ch in self.channels:
            try:
                await ch.stop()
            except Exception as e:
                log.error(f"Error stopping {ch.name}: {e}")

        self.store.close()
        self.task_queue.close()

        # Remove PID file
        pid_path = Path(self.config.pid_file)
        if pid_path.exists():
            pid_path.unlink()

        if self.config.log_to_brain:
            log_to_journal("system", "HarveyChat gateway stopped")

        log.info("HarveyChat gateway stopped.")


def _is_significant(user_msg: str, response: str) -> bool:
    """Decide if a message exchange is worth logging to Brain."""
    # Don't log trivial exchanges
    if len(user_msg) < 10 and len(response) < 50:
        return False
    # Don't log greetings
    trivial = {"hi", "hey", "hello", "yo", "thanks", "ok", "k", "bye", "gn"}
    if user_msg.strip().lower().rstrip("!.") in trivial:
        return False
    return True
